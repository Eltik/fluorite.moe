//! Where a request's bytes come from: the local output directory (the default) or an
//! S3-compatible bucket for the roots listed in `S3_ROOTS`.
//!
//! The API surface does not change with the backend. `Storage::serve` answers a file
//! request from either side, so moving a root to a bucket is a configuration change
//! rather than a code change. Roots that are not configured for S3 keep reading the
//! local disk, and so does everything when no bucket is configured at all.
//!
//! A remote root is served one of two ways:
//!
//! - **proxy** (the default): the API asks the bucket for the object and streams it
//!   back. The bucket can be private, and the response headers (entity tag, last
//!   modification, cache policy) are the ones the local file would have produced.
//! - **redirect**: the API answers `302` with the object's public URL, so the browser
//!   fetches the bytes from the bucket (or its CDN) and they never travel through the
//!   server. Set `S3_PUBLIC_BASE_URL` to enable it.
//!
//! When a remote read fails, the local file is used instead, so a bucket that is
//! unreachable (or not yet synced) degrades to the old behaviour instead of erroring.
//!
//! The endpoint tells the service apart (Cloudflare R2, Backblaze B2, a MinIO on the
//! local disk, ...); the provider supplies the settings that are not given, and a
//! service that is not known still works as a plain S3 endpoint.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use axum::body::Body;
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use chrono::{DateTime, Utc};

use super::config::{S3Config, S3Mode};
use super::error::{ApiError, ApiResult};
use super::files::{RangeSpec, content_type, not_modified, parse_range, serve_file};
use super::paths::{Root, is_hidden, resolve_async};
use super::thumbs::{THUMB_CACHE, cache_name};
use super::tree::{DirEntry, entry_order};
use crate::core::s3::{ObjectMeta, S3Client, S3Error, S3Settings, encode_path};

/// How long a remote listing stays cached.
const LISTING_TTL: Duration = Duration::from_secs(30);

/// A whole object read into memory (thumbnails need the bytes, not a stream).
pub struct Blob {
    pub bytes: Vec<u8>,
    pub etag: String,
    pub last_modified: Option<DateTime<Utc>>,
}

/// A file in the bucket: the path relative to the root of its tree and its size.
#[derive(Debug, Clone)]
pub struct RemoteFile {
    /// The path below the root, with `/` between the directories.
    pub path: String,
    /// The size of the object, in bytes.
    pub size: u64,
}

/// A cached directory listing and the moment it was read.
type CachedListing = (Instant, Arc<Vec<DirEntry>>);

/// The remote half of the storage layer.
pub struct Remote {
    client: Arc<S3Client>,
    /// The bucket, the prefix and the roots: the settings the API and the unpacker
    /// share.
    settings: S3Settings,
    mode: S3Mode,
    /// Serve pre-generated thumbnails from the bucket.
    thumbs: bool,
    /// Cached directory listings, keyed by root and path.
    listings: Mutex<HashMap<(Root, String), CachedListing>>,
}

impl Remote {
    /// The public URL of an object, for redirect mode.
    fn public_url(&self, key: &str) -> Option<String> {
        let base = self.settings.public_base_url.as_ref()?;
        let trimmed = base.trim_end_matches('/');
        Some(format!("{trimmed}/{}", encode_path(key)))
    }

    /// The metadata of an object in the bucket.
    async fn head(&self, key: &str) -> ApiResult<ObjectMeta> {
        self.client.head(key).await.map_err(s3_error)
    }
}

/// The storage layer: the output directory plus an optional bucket.
pub struct Storage {
    output: PathBuf,
    remote: Option<Remote>,
}

impl Storage {
    /// Build the storage layer. Without an S3 configuration this is the local output
    /// directory and nothing else.
    pub fn new(output: PathBuf, s3: Option<&S3Config>) -> Result<Self, String> {
        let remote = match s3 {
            Some(cfg) => Some(Remote {
                client: Arc::new(cfg.settings.client()?),
                settings: cfg.settings.clone(),
                mode: cfg.mode,
                thumbs: cfg.thumbs,
                listings: Mutex::new(HashMap::new()),
            }),
            None => None,
        };
        Ok(Self { output, remote })
    }

    /// The bucket, or the error a bucket operation answers with when there is none.
    fn remote(&self) -> ApiResult<&Remote> {
        self.remote
            .as_ref()
            .ok_or_else(|| ApiError::internal("no bucket configured"))
    }

    /// Whether `root` is served from the bucket.
    pub fn is_remote(&self, root: Root) -> bool {
        self.remote
            .as_ref()
            .is_some_and(|r| r.settings.roots.contains(&root))
    }

    /// Whether pre-generated thumbnails are served from the bucket.
    pub fn remote_thumbs(&self) -> bool {
        self.remote.as_ref().is_some_and(|r| r.thumbs)
    }

    /// The key of a file inside the bucket (`textures/ui/logo.png`).
    pub fn key_for(&self, root: Root, rel: &str) -> String {
        match &self.remote {
            Some(remote) => remote.settings.key_for(root, rel),
            None => root.rel_dir().to_string(),
        }
    }

    /// The key of a pre-generated thumbnail inside the bucket.
    ///
    /// It is the same path the local cache uses, so the sync of the cache is a
    /// plain directory mirror and the thumbnail another server generated is found.
    pub fn thumb_key(&self, size: u32, rel: &str, len: u64, tag: &str) -> String {
        let path = format!("thumbs/{size}/{}", cache_name(rel, len, tag));
        match &self.remote {
            Some(remote) => remote.settings.prefixed(&path),
            None => path,
        }
    }

    /// Serve a file of `root`, from the bucket when it is configured for it and from
    /// the local disk otherwise.
    pub async fn serve(
        &self,
        root: Root,
        rel: &str,
        headers: &HeaderMap,
        cache_control: &str,
    ) -> ApiResult<Response> {
        if self.is_remote(root) {
            let key = self.key_for(root, rel);
            match self.serve_object(&key, headers, cache_control, None).await {
                Ok(res) => return Ok(res),
                Err(err) => {
                    // The bucket may be empty (not synced yet) or briefly unavailable:
                    // the local copy is the fallback.
                    let provider = self
                        .remote
                        .as_ref()
                        .map_or("bucket", |r| r.settings.provider.name());
                    tracing::warn!(
                        "{provider} read failed for {key}: {err:?}; using the local copy"
                    );
                }
            }
        }
        let abs = resolve_async(root.dir(&self.output), rel.to_string()).await?;
        serve_file(&abs, headers, cache_control).await
    }

    /// Serve a pre-generated thumbnail from the bucket, if one is there.
    ///
    /// `len` and `tag` identify the source of the thumbnail (its size and its
    /// entity tag in the bucket), so the key is the one every server computes.
    /// `Ok(None)` means the bucket has no thumbnail for it: the caller generates one.
    pub async fn serve_thumb(
        &self,
        size: u32,
        rel: &str,
        len: u64,
        tag: &str,
        headers: &HeaderMap,
    ) -> ApiResult<Option<Response>> {
        if !self.remote_thumbs() {
            return Ok(None);
        }
        let key = self.thumb_key(size, rel, len, tag);
        let meta = match self.stat_key(&key).await {
            Ok(meta) => meta,
            Err(err) if err.status == StatusCode::NOT_FOUND.as_u16() => return Ok(None),
            Err(err) => return Err(err),
        };
        let res = self
            .serve_object(&key, headers, THUMB_CACHE, Some(meta))
            .await?;
        Ok(Some(res))
    }

    /// Serve one object, optionally with its metadata already known.
    async fn serve_object(
        &self,
        key: &str,
        headers: &HeaderMap,
        cache_control: &str,
        known: Option<ObjectMeta>,
    ) -> ApiResult<Response> {
        let remote = self.remote()?;

        if remote.mode == S3Mode::Redirect
            && let Some(url) = remote.public_url(key)
        {
            return redirect(&url, cache_control);
        }

        // The validators are needed before the body is sent (a conditional request) or
        // before the range can be clamped, so they are fetched separately then.
        let range_header = headers.get(header::RANGE).and_then(|v| v.to_str().ok());
        let needs_meta = known.is_none()
            && (range_header.is_some()
                || headers.contains_key(header::IF_NONE_MATCH)
                || headers.contains_key(header::IF_MODIFIED_SINCE)
                || headers.contains_key(header::IF_RANGE));
        let meta = match known {
            Some(meta) => Some(meta),
            None if needs_meta => Some(remote.head(key).await?),
            None => None,
        };
        let size = meta.as_ref().map_or(0, |m| m.size);

        let etag = meta.as_ref().map(|m| quoted(&m.etag));
        let last_modified = meta.as_ref().and_then(|m| m.last_modified);

        if let (Some(etag), Some(meta)) = (&etag, &meta)
            && not_modified(headers, etag, system_time(meta.last_modified))
        {
            return Ok(not_modified_response(
                etag.clone(),
                meta.last_modified,
                cache_control,
            ));
        }

        // A range is only honoured when `If-Range` still matches the current validator.
        let range = match (&meta, range_header) {
            (Some(meta), Some(value)) => {
                let allowed = headers.get(header::IF_RANGE).is_none_or(|v| {
                    v.to_str().is_ok_and(|v| {
                        v == etag.as_deref().unwrap_or_default()
                            || last_modified.is_some_and(|lm| v == http_date(lm))
                    })
                });
                if allowed {
                    parse_range(value, meta.size)
                } else {
                    RangeSpec::Full
                }
            }
            _ => RangeSpec::Full,
        };

        let requested = match range {
            RangeSpec::Partial(start, end) => Some((start, end)),
            RangeSpec::Full => None,
            // A range outside the object is a 416, not the whole file.
            RangeSpec::Unsatisfiable => return Ok(range_not_satisfiable(size)),
        };

        let object = match remote.client.get(key, requested).await {
            Ok(object) => object,
            Err(err) if err.status == 416 => return Ok(range_not_satisfiable(size)),
            Err(err) => return Err(s3_error(err)),
        };

        let etag = etag.unwrap_or_else(|| quoted(&object.etag));
        let last_modified = last_modified.or(object.last_modified);
        // The name of the key decides the content type: a thumbnail ends in `.webp`,
        // a texture in `.png`.
        let name = key.rsplit('/').next().unwrap_or(key);

        let mut builder = Response::builder()
            .header(header::ETAG, &etag)
            .header(header::CACHE_CONTROL, cache_control)
            .header(header::ACCEPT_RANGES, "bytes")
            .header(header::CONTENT_TYPE, content_type(name));
        if let Some(lm) = last_modified {
            builder = builder.header(header::LAST_MODIFIED, http_date(lm));
        }
        if let Some(len) = object.content_length {
            builder = builder.header(header::CONTENT_LENGTH, len);
        }
        let status = if object.status == 206 {
            if let Some(range) = &object.content_range {
                builder = builder.header(header::CONTENT_RANGE, range);
            }
            StatusCode::PARTIAL_CONTENT
        } else {
            StatusCode::OK
        };
        let body = Body::from_stream(object.body.bytes_stream());
        builder
            .status(status)
            .body(body)
            .map_err(|e| ApiError::internal(e.to_string()))
    }

    /// The metadata of an object in the bucket.
    pub async fn stat_key(&self, key: &str) -> ApiResult<ObjectMeta> {
        self.remote()?.head(key).await
    }

    /// Read a whole object of the bucket.
    pub async fn read_key(&self, key: &str) -> ApiResult<Blob> {
        let remote = self.remote()?;
        let object = remote.client.get(key, None).await.map_err(s3_error)?;
        let bytes = object
            .body
            .bytes()
            .await
            .map_err(|e| ApiError::internal(format!("bucket read failed: {e}")))?;
        Ok(Blob {
            bytes: bytes.to_vec(),
            etag: quoted(&object.etag),
            last_modified: object.last_modified,
        })
    }

    /// List a directory of the bucket.
    ///
    /// The listing is cached for [`LISTING_TTL`]: the file explorer walks the same
    /// directories repeatedly, and a listing is a billable request.
    pub async fn list_remote(&self, root: Root, rel: &str) -> ApiResult<Arc<Vec<DirEntry>>> {
        let remote = self.remote()?;
        let cache_key = (root, rel.to_string());
        let now = Instant::now();
        {
            let cache = remote.listings.lock().unwrap_or_else(|p| p.into_inner());
            if let Some((loaded, entries)) = cache.get(&cache_key)
                && now.duration_since(*loaded) < LISTING_TTL
            {
                return Ok(entries.clone());
            }
        }

        let mut prefix = remote.settings.root_prefix(root);
        if !rel.is_empty() {
            prefix.push_str(rel);
            prefix.push('/');
        }

        // With the delimiter the bucket answers with the objects directly below the
        // prefix and the deeper directories as common prefixes.
        let listing = remote
            .client
            .list_all(&prefix, Some("/"))
            .await
            .map_err(s3_error)?;
        let mut entries: Vec<DirEntry> = Vec::new();
        for object in &listing.objects {
            let name = object.key.strip_prefix(&prefix).unwrap_or(&object.key);
            if name.is_empty() || name.contains('/') || is_hidden(name) {
                continue;
            }
            entries.push(DirEntry {
                name: name.to_string(),
                is_dir: false,
                size: object.size,
            });
        }
        for common in &listing.prefixes {
            let name = common
                .strip_prefix(&prefix)
                .unwrap_or(common)
                .trim_end_matches('/');
            if name.is_empty() || name.contains('/') || is_hidden(name) {
                continue;
            }
            entries.push(DirEntry {
                name: name.to_string(),
                is_dir: true,
                size: 0,
            });
        }

        entries.sort_by(entry_order);
        let entries = Arc::new(entries);
        let mut cache = remote.listings.lock().unwrap_or_else(|p| p.into_inner());
        cache.insert(cache_key, (now, entries.clone()));
        Ok(entries)
    }

    /// Every file of a root in the bucket, listed in one pass.
    ///
    /// The listing is not cached: it is the whole tree of a root, and it is read once
    /// per build of the search index.
    async fn list_root(&self, root: Root) -> ApiResult<Vec<RemoteFile>> {
        let remote = self.remote()?;
        let prefix = remote.settings.root_prefix(root);
        // Without a delimiter the bucket answers with every object below the prefix.
        let listing = remote
            .client
            .list_all(&prefix, None)
            .await
            .map_err(s3_error)?;
        let files = listing
            .objects
            .iter()
            .filter_map(|object| {
                let path = object.key.strip_prefix(&prefix).unwrap_or(&object.key);
                if path.is_empty() || path.ends_with('/') || has_hidden_part(path) {
                    return None;
                }
                Some(RemoteFile {
                    path: path.to_string(),
                    size: object.size,
                })
            })
            .collect();
        Ok(files)
    }

    /// The files of every root that is served from the bucket.
    ///
    /// A root whose listing fails is left out: the index then knows nothing about it,
    /// while the files themselves are still served (the bucket is asked per request).
    pub async fn list_remote_roots(&self) -> Vec<(Root, Vec<RemoteFile>)> {
        let Some(remote) = self.remote.as_ref() else {
            return Vec::new();
        };
        let mut out: Vec<(Root, Vec<RemoteFile>)> = Vec::new();
        for root in &remote.settings.roots {
            match self.list_root(*root).await {
                Ok(files) => {
                    tracing::info!(
                        "search index: {root:?} lists {} files in the bucket",
                        files.len()
                    );
                    out.push((*root, files));
                }
                Err(e) => tracing::warn!("search index: listing {root:?} failed: {e:?}"),
            }
        }
        out
    }
}

/// Whether a path holds a hidden file or directory.
fn has_hidden_part(path: &str) -> bool {
    path.split('/').any(is_hidden)
}

/// The entity tag of an object, quoted the way HTTP wants it.
fn quoted(etag: &str) -> String {
    if etag.starts_with('"') {
        etag.to_string()
    } else {
        format!("\"{etag}\"")
    }
}

/// The modification time of an object as a `SystemTime`; the epoch when unknown.
fn system_time(time: Option<DateTime<Utc>>) -> SystemTime {
    time.map(SystemTime::from).unwrap_or(UNIX_EPOCH)
}

/// A timestamp in the form the HTTP date headers take.
fn http_date(time: DateTime<Utc>) -> String {
    httpdate::fmt_http_date(SystemTime::from(time))
}

/// The S3 failure `err` reported as the API error the client sees.
fn s3_error(err: S3Error) -> ApiError {
    ApiError::new(status_for(err.status), "s3_error", err.message)
}

/// A `302 Found` to the object's public URL.
fn redirect(url: &str, cache_control: &str) -> ApiResult<Response> {
    let mut res = Response::builder()
        .status(StatusCode::FOUND)
        .header(header::LOCATION, url)
        .header(header::CACHE_CONTROL, cache_control)
        .body(Body::empty())
        .map_err(|e| ApiError::internal(e.to_string()))?;
    // A redirect is only a pointer; the object itself carries the validators.
    res.headers_mut().remove(header::ETAG);
    Ok(res)
}

/// A `304 Not Modified` with the validators the client needs.
fn not_modified_response(
    etag: String,
    last_modified: Option<DateTime<Utc>>,
    cache_control: &str,
) -> Response {
    let mut builder = Response::builder()
        .status(StatusCode::NOT_MODIFIED)
        .header(header::ETAG, etag)
        .header(header::CACHE_CONTROL, cache_control)
        .header(header::ACCEPT_RANGES, "bytes");
    if let Some(lm) = last_modified {
        builder = builder.header(header::LAST_MODIFIED, http_date(lm));
    }
    builder
        .body(Body::empty())
        .unwrap_or_else(|_| StatusCode::NOT_MODIFIED.into_response())
}

/// A `416 Range Not Satisfiable`, naming the real size of the object.
fn range_not_satisfiable(size: u64) -> Response {
    let value = HeaderValue::from_str(&format!("bytes */{size}"))
        .unwrap_or_else(|_| HeaderValue::from_static("bytes */0"));
    (
        StatusCode::RANGE_NOT_SATISFIABLE,
        [(header::CONTENT_RANGE, value)],
    )
        .into_response()
}

/// The HTTP status an S3 failure is reported as.
fn status_for(status: u16) -> StatusCode {
    match status {
        404 => StatusCode::NOT_FOUND,
        403 => StatusCode::FORBIDDEN,
        416 => StatusCode::RANGE_NOT_SATISFIABLE,
        0 => StatusCode::BAD_GATEWAY,
        400..=499 => StatusCode::BAD_REQUEST,
        _ => StatusCode::BAD_GATEWAY,
    }
}
