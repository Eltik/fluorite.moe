//! Streaming file responses with ETag/Last-Modified validation and single-range support.

use std::io::SeekFrom;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use axum::body::Body;
use axum::extract::{Path as UrlPath, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use tokio::io::{AsyncReadExt, AsyncSeekExt};
use tokio_util::io::ReaderStream;

use super::SharedState;
use super::error::{ApiError, ApiResult};
use super::paths::{Root, extension, parse_root, sanitize};

pub const DEFAULT_CACHE: &str = "public, max-age=3600";
/// Exported models (GLBs, clip and charinfo JSON) are rewritten in place under the same URL
/// by re-exports: browsers must revalidate them every time (the ETag makes that a 304).
pub const REVALIDATE_CACHE: &str = "no-cache";

/// `Cache-Control` of a file under `root`.
#[must_use]
pub fn cache_control(root: Root) -> &'static str {
    match root {
        Root::Models => REVALIDATE_CACHE,
        _ => DEFAULT_CACHE,
    }
}
const STREAM_CHUNK: usize = 64 * 1024;

/// `GET /files/{root}/{*path}`
pub async fn file(
    State(state): State<SharedState>,
    UrlPath((root, path)): UrlPath<(String, String)>,
    headers: HeaderMap,
) -> ApiResult<Response> {
    let root = parse_root(&root)?;
    let rel = sanitize(&path)?;
    if rel.is_empty() {
        return Err(ApiError::bad_request("missing file path"));
    }
    // The root decides where the bytes come from: the bucket when it is configured for
    // it, the local output directory otherwise.
    state
        .storage
        .serve(root, &rel, &headers, cache_control(root))
        .await
}

/// Content type for a file name, by extension.
pub fn content_type(name: &str) -> &'static str {
    match extension(name).to_ascii_lowercase().as_str() {
        "png" => "image/png",
        "webp" => "image/webp",
        "jpg" | "jpeg" => "image/jpeg",
        "glb" => "model/gltf-binary",
        "gltf" => "model/gltf+json",
        "json" => "application/json",
        "jsonl" => "application/x-ndjson",
        "lua" | "txt" | "log" | "csv" => "text/plain; charset=utf-8",
        "wem" => "audio/x-wem",
        "bnk" => "application/x-wwise-bnk",
        "pck" => "application/x-wwise-pck",
        "usm" => "video/x-usm",
        "mp4" => "video/mp4",
        "ogg" => "audio/ogg",
        "wav" => "audio/wav",
        "m4a" => "audio/mp4",
        "opus" => "audio/ogg",
        _ => "application/octet-stream",
    }
}

/// Strong validator derived from size and mtime (nanoseconds): a re-export, written through a
/// temp file and rename, always gets a new one.
pub fn etag_for(size: u64, mtime: SystemTime) -> String {
    let nanos = mtime.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    format!("\"{size:x}-{nanos:x}\"")
}

fn etag_matches(list: &str, etag: &str) -> bool {
    let bare = etag.trim_start_matches("W/");
    list.split(',')
        .map(str::trim)
        .any(|candidate| candidate == "*" || candidate.trim_start_matches("W/") == bare)
}

/// Whether the request's conditional headers say the client copy is current.
pub fn not_modified(headers: &HeaderMap, etag: &str, mtime: SystemTime) -> bool {
    if let Some(inm) = headers.get(header::IF_NONE_MATCH) {
        // If-None-Match takes precedence; If-Modified-Since is then ignored.
        return inm.to_str().is_ok_and(|v| etag_matches(v, etag));
    }
    headers
        .get(header::IF_MODIFIED_SINCE)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| httpdate::parse_http_date(v).ok())
        .is_some_and(|since| {
            let secs = |t: SystemTime| t.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
            secs(mtime) <= secs(since)
        })
}

/// Outcome of parsing a `Range` header against a file of `size` bytes.
#[derive(Debug, PartialEq, Eq)]
pub enum RangeSpec {
    /// No usable range: send the whole file.
    Full,
    /// Inclusive byte range.
    Partial(u64, u64),
    /// Syntactically valid but outside the file: 416.
    Unsatisfiable,
}

/// Parse a single `bytes=` range. Multi-range and malformed headers fall back to `Full`.
pub fn parse_range(value: &str, size: u64) -> RangeSpec {
    let Some(spec) = value.trim().strip_prefix("bytes=") else {
        return RangeSpec::Full;
    };
    if spec.contains(',') {
        return RangeSpec::Full;
    }
    let Some((start, end)) = spec.trim().split_once('-') else {
        return RangeSpec::Full;
    };
    let (start, end) = (start.trim(), end.trim());
    let parse = |s: &str| s.parse::<u64>().ok();
    match (start.is_empty(), end.is_empty()) {
        // bytes=-N : last N bytes
        (true, false) => match parse(end) {
            Some(0) => RangeSpec::Unsatisfiable,
            Some(n) if size > 0 => RangeSpec::Partial(size.saturating_sub(n), size - 1),
            Some(_) => RangeSpec::Unsatisfiable,
            None => RangeSpec::Full,
        },
        // bytes=N-
        (false, true) => match parse(start) {
            Some(s) if s < size => RangeSpec::Partial(s, size - 1),
            Some(_) => RangeSpec::Unsatisfiable,
            None => RangeSpec::Full,
        },
        // bytes=N-M
        (false, false) => match (parse(start), parse(end)) {
            (Some(s), Some(e)) if s <= e => {
                if s >= size {
                    RangeSpec::Unsatisfiable
                } else {
                    RangeSpec::Partial(s, e.min(size - 1))
                }
            }
            _ => RangeSpec::Full,
        },
        (true, true) => RangeSpec::Full,
    }
}

/// Serve `path` (already resolved and validated) with caching validators and Range support.
pub async fn serve_file(
    path: &Path,
    headers: &HeaderMap,
    cache_control: &str,
) -> ApiResult<Response> {
    let mut file = tokio::fs::File::open(path)
        .await
        .map_err(|e| ApiError::io(e, "file"))?;
    let meta = file.metadata().await.map_err(|e| ApiError::io(e, "file"))?;
    if !meta.is_file() {
        return Err(ApiError::not_found("not a file"));
    }
    let size = meta.len();
    let mtime = meta.modified().unwrap_or(UNIX_EPOCH);
    let etag = etag_for(size, mtime);
    let last_modified = httpdate::fmt_http_date(mtime);
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");

    let mut builder = Response::builder()
        .header(header::ETAG, &etag)
        .header(header::LAST_MODIFIED, &last_modified)
        .header(header::CACHE_CONTROL, cache_control)
        .header(header::ACCEPT_RANGES, "bytes");

    if not_modified(headers, &etag, mtime) {
        return builder
            .status(StatusCode::NOT_MODIFIED)
            .body(Body::empty())
            .map_err(|e| ApiError::internal(e.to_string()));
    }

    builder = builder.header(header::CONTENT_TYPE, content_type(name));

    // If-Range: only honour Range when the validator still matches.
    let range_allowed = headers
        .get(header::IF_RANGE)
        .is_none_or(|v| v.to_str().is_ok_and(|v| v == etag || v == last_modified));
    let range = match headers.get(header::RANGE).and_then(|v| v.to_str().ok()) {
        Some(v) if range_allowed => parse_range(v, size),
        _ => RangeSpec::Full,
    };

    let response = match range {
        RangeSpec::Full => {
            let stream = ReaderStream::with_capacity(file, STREAM_CHUNK);
            builder
                .status(StatusCode::OK)
                .header(header::CONTENT_LENGTH, size)
                .body(Body::from_stream(stream))
        }
        RangeSpec::Partial(start, end) => {
            file.seek(SeekFrom::Start(start))
                .await
                .map_err(|e| ApiError::io(e, "file"))?;
            let len = end - start + 1;
            let stream = ReaderStream::with_capacity(file.take(len), STREAM_CHUNK);
            builder
                .status(StatusCode::PARTIAL_CONTENT)
                .header(header::CONTENT_LENGTH, len)
                .header(header::CONTENT_RANGE, format!("bytes {start}-{end}/{size}"))
                .body(Body::from_stream(stream))
        }
        RangeSpec::Unsatisfiable => {
            return Ok((
                StatusCode::RANGE_NOT_SATISFIABLE,
                [(
                    header::CONTENT_RANGE,
                    HeaderValue::from_str(&format!("bytes */{size}"))
                        .map_err(|e| ApiError::internal(e.to_string()))?,
                )],
            )
                .into_response());
        }
    };
    response.map_err(|e| ApiError::internal(e.to_string()))
}
