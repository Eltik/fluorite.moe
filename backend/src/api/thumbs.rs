//! `GET /thumb/{*path}?size=128|256|512` — WebP thumbnails of `textures` PNGs.
//!
//! Thumbnails are lossless WebP (the `image` crate's encoder is lossless-only), keyed by
//! (path, source size, source tag) so a changed texture gets a new file and ETag. The
//! tag is the modification time of a local texture and the entity tag of one in the
//! bucket. They are written atomically under `<cache>/thumbs/<size>/` and generated on
//! blocking threads behind a semaphore; a bucket may hold pre-generated ones under the
//! same names, which are served before anything is generated.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::UNIX_EPOCH;

use axum::extract::{Path as UrlPath, Query, State};
use axum::http::HeaderMap;
use axum::response::Response;
use image::imageops::FilterType;
use image::{DynamicImage, ImageEncoder};
use serde::Deserialize;
use tokio::sync::Semaphore;

use super::SharedState;
use super::error::{ApiError, ApiResult};
use super::files::serve_file;
use super::paths::{Root, extension, resolve_async, sanitize};

pub const SIZES: [u32; 3] = [128, 256, 512];
const DEFAULT_SIZE: u32 = 256;
pub const THUMB_CACHE: &str = "public, max-age=604800, stale-while-revalidate=86400";

pub struct ThumbLimiter {
    permits: Arc<Semaphore>,
}

impl ThumbLimiter {
    pub fn new(concurrency: usize) -> Self {
        Self {
            permits: Arc::new(Semaphore::new(concurrency.max(1))),
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct ThumbQuery {
    pub size: Option<u32>,
}

/// FNV-1a: a stable (cross-build, cross-server) hash for cache file names.
fn fnv1a(parts: &[&[u8]]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for part in parts {
        for &b in *part {
            h ^= u64::from(b);
            h = h.wrapping_mul(0x0100_0000_01b3);
        }
        h ^= 0xff;
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h
}

/// The name of the cache file of a thumbnail of `rel`, from the source's (len, tag).
///
/// The name is the same in the local cache and in the bucket, so the sync of the
/// cache is a plain directory mirror and every server finds the thumbnails the
/// others generated.
pub fn cache_name(rel: &str, len: u64, tag: &str) -> String {
    let h = fnv1a(&[rel.as_bytes(), &len.to_le_bytes(), tag.as_bytes()]);
    format!("{h:016x}.webp")
}

/// Cache file for a thumbnail of `rel` at `size` given the source's (len, tag).
///
/// The tag is the source's modification time locally and its entity tag in the
/// bucket, so a changed texture always gets a new file.
pub fn cache_path(thumbs_dir: &Path, rel: &str, size: u32, len: u64, tag: &str) -> PathBuf {
    thumbs_dir
        .join(size.to_string())
        .join(cache_name(rel, len, tag))
}

/// Downscale so the longest side is at most `size`, preserving aspect ratio. Very large
/// sources get a cheap pre-shrink to 2× the target before the Lanczos pass.
pub fn downscale(img: DynamicImage, size: u32) -> DynamicImage {
    if img.width() <= size && img.height() <= size {
        return img;
    }
    let img = if img.width().max(img.height()) > size * 4 {
        img.thumbnail(size * 2, size * 2)
    } else {
        img
    };
    img.resize(size, size, FilterType::Lanczos3)
}

/// Encode as lossless WebP (RGBA8, or RGB8 when the source has no alpha).
pub fn encode_webp(img: &DynamicImage) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    let enc = image::codecs::webp::WebPEncoder::new_lossless(&mut out);
    if img.color().has_alpha() {
        let rgba = img.to_rgba8();
        enc.write_image(
            rgba.as_raw(),
            rgba.width(),
            rgba.height(),
            image::ExtendedColorType::Rgba8,
        )
    } else {
        let rgb = img.to_rgb8();
        enc.write_image(
            rgb.as_raw(),
            rgb.width(),
            rgb.height(),
            image::ExtendedColorType::Rgb8,
        )
    }
    .map_err(|e| e.to_string())?;
    Ok(out)
}

/// Write `bytes` to `dest` via a temp file + rename in the same directory.
fn write_atomic(dest: &Path, bytes: &[u8]) -> std::io::Result<()> {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let dir = dest.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(dir)?;
    let tmp = dir.join(format!(
        ".{}.{}.{}.tmp",
        dest.file_name().and_then(|n| n.to_str()).unwrap_or("thumb"),
        std::process::id(),
        SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    let result = (|| {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        std::fs::rename(&tmp, dest)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

/// Where the texture a thumbnail is made from comes from.
enum Source {
    /// A texture on the local disk.
    File(PathBuf),
    /// A texture read from the bucket.
    Bytes(Vec<u8>),
}

impl Source {
    fn decode(&self) -> ApiResult<DynamicImage> {
        let io = |e| ApiError::io(e, "texture");
        let decoded = match self {
            Source::File(path) => image::ImageReader::open(path)
                .map_err(io)?
                .with_guessed_format()
                .map_err(io)?
                .decode(),
            Source::Bytes(bytes) => image::ImageReader::new(std::io::Cursor::new(bytes))
                .with_guessed_format()
                .map_err(io)?
                .decode(),
        };
        decoded.map_err(|e| ApiError::internal(format!("decode failed: {e}")))
    }
}

/// Decode the source and write its thumbnail to `dest`. Blocking.
fn generate(src: &Source, dest: &Path, size: u32) -> ApiResult<()> {
    let thumb = downscale(src.decode()?, size);
    let bytes =
        encode_webp(&thumb).map_err(|e| ApiError::internal(format!("encode failed: {e}")))?;
    write_atomic(dest, &bytes).map_err(|e| ApiError::io(e, "thumbnail cache"))
}

/// The size and modification time of a local texture, used as its cache tag.
async fn local_source(output: &Path, rel: &str) -> ApiResult<(u64, String)> {
    let src = resolve_async(Root::Textures.dir(output), rel.to_string()).await?;
    let meta = tokio::fs::metadata(&src)
        .await
        .map_err(|e| ApiError::io(e, "texture"))?;
    if !meta.is_file() {
        return Err(ApiError::not_found("texture not found"));
    }
    let tag = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_nanos())
        .to_string();
    Ok((meta.len(), tag))
}

/// `GET /thumb/{*path}?size=`
pub async fn thumb(
    State(state): State<SharedState>,
    UrlPath(path): UrlPath<String>,
    Query(q): Query<ThumbQuery>,
    headers: HeaderMap,
) -> ApiResult<Response> {
    let size = q.size.unwrap_or(DEFAULT_SIZE);
    if !SIZES.contains(&size) {
        return Err(ApiError::bad_request("size must be 128, 256 or 512"));
    }
    let rel = sanitize(&path)?;
    if !extension(&rel).eq_ignore_ascii_case("png") {
        return Err(ApiError::bad_request(
            "thumbnails are only available for PNG textures",
        ));
    }

    // The identity of the source decides the name of the thumbnail: the same
    // texture always gets the same name, on this server and on the others.
    let remote = state.storage.is_remote(Root::Textures);
    let key = state.storage.key_for(Root::Textures, &rel);
    let (len, tag) = if remote {
        match state.storage.stat_key(&key).await {
            Ok(meta) => (meta.size, meta.etag),
            // Not in the bucket (yet): the local copy is the source.
            Err(_) => local_source(&state.config.output, &rel).await?,
        }
    } else {
        local_source(&state.config.output, &rel).await?
    };
    let dest = cache_path(&state.config.thumbs_dir(), &rel, size, len, &tag);

    // The local cache comes first: it is a local read, the bucket is a round trip.
    if tokio::fs::metadata(&dest).await.is_ok() {
        return serve_file(&dest, &headers, THUMB_CACHE).await;
    }

    // A thumbnail that was generated ahead of time is served straight from the bucket.
    if let Some(res) = state
        .storage
        .serve_thumb(size, &rel, len, &tag, &headers)
        .await?
    {
        return Ok(res);
    }

    let _permit = state
        .thumbs
        .permits
        .clone()
        .acquire_owned()
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;
    // Another request may have produced it while we waited.
    if tokio::fs::metadata(&dest).await.is_err() {
        let source = if remote {
            Source::Bytes(state.storage.read_key(&key).await?.bytes)
        } else {
            Source::File(
                resolve_async(Root::Textures.dir(&state.config.output), rel.clone()).await?,
            )
        };
        let d = dest.clone();
        tokio::task::spawn_blocking(move || generate(&source, &d, size))
            .await
            .map_err(|e| ApiError::internal(e.to_string()))??;
    }
    serve_file(&dest, &headers, THUMB_CACHE).await
}
