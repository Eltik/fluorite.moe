//! Roots, request-path sanitization, hidden-entry rules, and symlink-safe resolution.

use std::path::{Path, PathBuf};

use super::error::{ApiError, ApiResult};

pub use crate::core::roots::Root;

/// The root with that name, or a 404: the name comes from the request path.
pub fn parse_root(name: &str) -> ApiResult<Root> {
    Root::parse(name).ok_or_else(|| ApiError::not_found(format!("unknown root {name:?}")))
}

/// Whether a single file or directory name is invisible to clients.
pub fn is_hidden(name: &str) -> bool {
    name.starts_with('.')
        || name.ends_with(".tmp")
        || name.ends_with(".mpk")
        || name == "blc_metadata.txt"
}

/// Normalize a client-supplied relative path.
///
/// Rejects (400) absolute paths, backslashes, NUL, and `.`/`..` segments; collapses
/// empty segments. Hidden segments are a 404 (they "do not exist"). Returns `""` for the
/// root itself.
pub fn sanitize(raw: &str) -> ApiResult<String> {
    if raw.contains('\\') || raw.contains('\0') {
        return Err(ApiError::bad_request("invalid character in path"));
    }
    if raw.starts_with('/') {
        return Err(ApiError::bad_request("absolute paths are not allowed"));
    }
    let mut out = String::with_capacity(raw.len());
    for seg in raw.split('/') {
        match seg {
            "" => continue,
            "." | ".." => return Err(ApiError::bad_request("path traversal is not allowed")),
            s if is_hidden(s) => return Err(ApiError::not_found("no such entry")),
            s => {
                if !out.is_empty() {
                    out.push('/');
                }
                out.push_str(s);
            }
        }
    }
    Ok(out)
}

/// Resolve a sanitized relative path under `root_dir`, canonicalizing so that symlinks
/// pointing outside the root are rejected (404, indistinguishable from missing).
pub fn resolve(root_dir: &Path, rel: &str) -> ApiResult<PathBuf> {
    let base = root_dir
        .canonicalize()
        .map_err(|e| ApiError::io(e, "root"))?;
    let joined = if rel.is_empty() {
        base.clone()
    } else {
        base.join(rel)
    };
    let real = joined
        .canonicalize()
        .map_err(|e| ApiError::io(e, "entry"))?;
    if !real.starts_with(&base) {
        return Err(ApiError::not_found("entry not found"));
    }
    Ok(real)
}

/// [`resolve`] on the blocking pool (canonicalizing stats every path component).
pub async fn resolve_async(root_dir: PathBuf, rel: String) -> ApiResult<PathBuf> {
    tokio::task::spawn_blocking(move || resolve(&root_dir, &rel))
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?
}

/// Extension of a file name as written (`""` when none); compare case-insensitively.
pub fn extension(name: &str) -> &str {
    match name.rfind('.') {
        Some(i) if i > 0 => &name[i + 1..],
        _ => "",
    }
}
