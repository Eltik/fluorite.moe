//! Shared utility functions for the extraction pipeline.
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

/// Format a byte count as a human-readable string (e.g., "1.23 MB").
pub fn format_bytes(bytes: u64) -> String {
    const UNITS: &[&str] = &["B", "KB", "MB", "GB", "TB"];
    if bytes == 0 {
        return "0 B".into();
    }
    let mut size = bytes as f64;
    let mut i = 0;
    while size >= 1024.0 && i < UNITS.len() - 1 {
        size /= 1024.0;
        i += 1;
    }
    format!("{:.2} {}", size, UNITS[i])
}

/// Render up to `max_bytes` of `data` as hex rows of 16 bytes, each row prefixed by `indent`
/// and its address (`base_offset` + row offset, zero-padded to `address_width` digits).
fn format_hex_dump(
    data: &[u8],
    base_offset: usize,
    max_bytes: usize,
    indent: &str,
    address_width: usize,
) -> String {
    let mut out = String::new();
    let dump_len = data.len().min(max_bytes);
    for off in (0..dump_len).step_by(16) {
        let row = &data[off..dump_len.min(off + 16)];
        out.push_str(&format!("{indent}{:0address_width$x}: ", base_offset + off));
        for i in 0..16 {
            match row.get(i) {
                Some(b) => out.push_str(&format!("{b:02x} ")),
                None => out.push_str("   "),
            }
        }
        out.push_str(" |");
        for &b in row {
            out.push(if b.is_ascii_graphic() || b == b' ' {
                b as char
            } else {
                '.'
            });
        }
        out.push_str("|\n");
    }
    out
}

/// Print a hex dump of `data` to stdout (up to `max_bytes`).
pub fn hex_dump(data: &[u8], max_bytes: usize) {
    print!("{}", hex_dump_string(data, max_bytes));
}

/// Return a hex dump as a String (up to `max_bytes`).
pub fn hex_dump_string(data: &[u8], max_bytes: usize) -> String {
    format_hex_dump(data, 0, max_bytes, "  ", 4)
}

/// Print a hex dump with a base address offset (8-digit hex addresses).
pub fn hex_dump_offset(data: &[u8], base_offset: usize, max_bytes: usize) {
    print!(
        "{}",
        format_hex_dump(data, base_offset, max_bytes, "    ", 8)
    );
}

/// Lowercase hex encoding of `bytes`.
pub fn to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Sanitize a string for use as a filename: replace invalid characters.
pub fn sanitize_filename(name: &str) -> String {
    name.replace(['\\', ':', '*', '?', '"', '<', '>', '|'], "_")
}

/// Like [`sanitize_filename`], but also flattens path separators so the name stays a single
/// path component.
pub fn flat_filename(name: &str) -> String {
    sanitize_filename(name).replace(['/', '\\'], "_")
}

/// Longest file name written, leaving room for a `.tmp` suffix under the usual 255-byte limit.
pub const MAX_FILENAME_BYTES: usize = 200;

/// Shorten an over-long file name, keeping its extension and appending `__<path_id>` so the
/// result stays unique and stable (e.g. Unity's `Asset(Clone)(Clone)...` names).
pub fn fit_filename(file_name: &str, path_id: i64) -> String {
    if file_name.len() <= MAX_FILENAME_BYTES {
        return file_name.to_string();
    }
    let (stem, ext) = match file_name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() && ext.len() <= 16 => (stem, format!(".{ext}")),
        _ => (file_name, String::new()),
    };
    let suffix = format!("__{path_id}{ext}");
    let mut end = MAX_FILENAME_BYTES
        .saturating_sub(suffix.len())
        .min(stem.len());
    while !stem.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}{suffix}", &stem[..end])
}

/// Split a trailing `.ext` (1-8 alphanumeric chars) off a file name.
pub(crate) fn split_extension(name: &str) -> Option<(&str, &str)> {
    let (stem, ext) = name.rsplit_once('.')?;
    (!stem.is_empty()
        && (1..=8).contains(&ext.len())
        && ext.chars().all(|c| c.is_ascii_alphanumeric()))
    .then_some((stem, ext))
}

/// Insert `__<path_id>` before a file name's extension (`foo.json` → `foo__7.json`).
pub(crate) fn with_path_id(file_name: &str, path_id: i64) -> String {
    match split_extension(file_name) {
        Some((stem, ext)) => format!("{stem}__{path_id}.{ext}"),
        None => format!("{file_name}__{path_id}"),
    }
}

/// Pick a unique path for `file_name` in `dir`, suffixing the path ID on collision.
///
/// `seen` is scoped to one SerializedFile, so the result is deterministic per object.
pub(crate) fn claim_path(
    dir: PathBuf,
    file_name: &str,
    path_id: i64,
    seen: &mut HashSet<PathBuf>,
) -> Option<PathBuf> {
    let file_name = &fit_filename(file_name, path_id);
    let path = dir.join(file_name);
    if seen.insert(path.clone()) {
        return Some(path);
    }
    let path = dir.join(with_path_id(file_name, path_id));
    seen.insert(path.clone()).then_some(path)
}

/// `path` with `suffix` appended to its final component (`state.json` → `state.json.tmp`).
pub(crate) fn append_to_path(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(suffix);
    PathBuf::from(name)
}

/// Write a file atomically (temporary file + rename) so a crash never leaves it truncated.
pub(crate) fn write_file(path: &Path, bytes: &[u8]) -> bool {
    if let Some(parent) = path.parent()
        && fs::create_dir_all(parent).is_err()
    {
        return false;
    }
    let tmp = append_to_path(path, ".tmp");
    fs::write(&tmp, bytes)
        .and_then(|()| fs::rename(&tmp, path))
        .is_ok()
}

/// `fs::remove_dir_all`, retried when the tree is repopulated mid-removal.
///
/// On macOS a sweep over a large tree can fail with `ENOTEMPTY` when an entry appears during
/// the walk (e.g. Finder's `.DS_Store`); a fresh pass removes the stragglers.
pub fn remove_dir_all_retry(path: &Path) -> std::io::Result<()> {
    let mut attempt = 0;
    loop {
        match fs::remove_dir_all(path) {
            Err(error) if error.kind() == std::io::ErrorKind::DirectoryNotEmpty && attempt < 3 => {
                attempt += 1;
            }
            result => return result,
        }
    }
}
