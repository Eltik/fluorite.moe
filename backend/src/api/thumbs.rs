//! `GET /thumb/{*path}?size=128|256|512` — WebP thumbnails of `textures` PNGs.
//!
//! Thumbnails are lossless WebP (the `image` crate's encoder is lossless-only), keyed by
//! (path, size, source size, source mtime) so a changed texture gets a new file and ETag.
//! They are written atomically under `<cache>/thumbs/<size>/` and generated on blocking
//! threads behind a semaphore.

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
const THUMB_CACHE: &str = "public, max-age=604800, stale-while-revalidate=86400";

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

/// FNV-1a: a stable (cross-build) hash for cache file names.
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

/// Cache file for a thumbnail of `rel` at `size` given the source's (len, mtime nanos).
pub fn cache_path(thumbs_dir: &Path, rel: &str, size: u32, len: u64, mtime_ns: u128) -> PathBuf {
    let h = fnv1a(&[rel.as_bytes(), &len.to_le_bytes(), &mtime_ns.to_le_bytes()]);
    thumbs_dir
        .join(size.to_string())
        .join(format!("{h:016x}.webp"))
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

fn generate(src: &Path, dest: &Path, size: u32) -> ApiResult<()> {
    let img = image::ImageReader::open(src)
        .map_err(|e| ApiError::io(e, "texture"))?
        .with_guessed_format()
        .map_err(|e| ApiError::io(e, "texture"))?
        .decode()
        .map_err(|e| ApiError::internal(format!("decode failed: {e}")))?;
    let thumb = downscale(img, size);
    let bytes =
        encode_webp(&thumb).map_err(|e| ApiError::internal(format!("encode failed: {e}")))?;
    write_atomic(dest, &bytes).map_err(|e| ApiError::io(e, "thumbnail cache"))
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
    let src = resolve_async(Root::Textures.dir(&state.config.output), rel.clone()).await?;
    let meta = tokio::fs::metadata(&src)
        .await
        .map_err(|e| ApiError::io(e, "texture"))?;
    if !meta.is_file() {
        return Err(ApiError::not_found("texture not found"));
    }
    let mtime_ns = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_nanos());
    let dest = cache_path(&state.config.thumbs_dir(), &rel, size, meta.len(), mtime_ns);

    if tokio::fs::metadata(&dest).await.is_err() {
        let _permit = state
            .thumbs
            .permits
            .clone()
            .acquire_owned()
            .await
            .map_err(|e| ApiError::internal(e.to_string()))?;
        // Another request may have produced it while we waited.
        if tokio::fs::metadata(&dest).await.is_err() {
            let (s, d) = (src.clone(), dest.clone());
            tokio::task::spawn_blocking(move || generate(&s, &d, size))
                .await
                .map_err(|e| ApiError::internal(e.to_string()))??;
        }
    }
    serve_file(&dest, &headers, THUMB_CACHE).await
}
