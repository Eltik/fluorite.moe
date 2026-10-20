//! Output layout derived from Unity AssetBundle container metadata.
use crate::extraction::unity::SerializedFile;
use crate::extraction::utils::{
    claim_path, fit_filename, flat_filename, sanitize_filename, with_path_id,
};
use serde::Serialize;
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

/// Bump when existing PNGs on disk become stale. 2: rows flipped to top-down orientation.
pub const LAYOUT_VERSION: u32 = 2;

#[derive(Debug, Clone, PartialEq)]
pub enum Placement {
    Direct(String),
    Dependency(String),
}

/// A texture and the location where it belongs in the output layout.
pub struct PlannedTexture {
    pub path_id: i64,
    pub name: String,
    pub relative: PathBuf,
    pub placement: Option<Placement>,
}

/// Plan texture paths for one SerializedFile.
///
/// This is shared by extraction and legacy flat-layout migration so both use
/// exactly the same placement and duplicate rules.
pub fn plan_textures(sf: &SerializedFile) -> Vec<PlannedTexture> {
    let containers = container_map(sf);
    let mut planned = Vec::new();
    let mut paths = HashSet::new();

    for obj in sf.objects_of_type(28) {
        let Some(name) = sf.peek_name(obj).ok().flatten() else {
            continue;
        };
        if name.is_empty() {
            continue;
        }
        let placement = containers.get(&obj.path_id).cloned();
        let mut relative = relative_path(placement.as_ref(), &name, obj.path_id);
        if !paths.insert(relative.clone()) {
            let stem = relative
                .file_stem()
                .and_then(|stem| stem.to_str())
                .unwrap_or("texture");
            relative.set_file_name(format!("{stem}__{}.png", obj.path_id));
            paths.insert(relative.clone());
        }
        planned.push(PlannedTexture {
            path_id: obj.path_id,
            name,
            relative,
            placement,
        });
    }
    planned
}

/// Map Texture2D path IDs to their AssetBundle container placement.
pub fn container_map(sf: &SerializedFile) -> HashMap<i64, Placement> {
    let mut map = HashMap::new();
    let mut bundles = Vec::new();

    for obj in sf.objects_of_type(142) {
        // Pairs stay `[path, info]` arrays: `m_Container` repeats paths across assets.
        let Ok(value) = sf.read_object_keep_pairs(obj) else {
            continue;
        };
        bundles.push(value);
    }

    for bundle in &bundles {
        for (path, asset) in container_entries(bundle) {
            if let Some(path_id) = local_path_id(asset) {
                map.entry(path_id)
                    .or_insert_with(|| Placement::Direct(path));
            }
        }
    }

    for bundle in &bundles {
        let Some(table) = bundle.get("m_PreloadTable").and_then(Value::as_array) else {
            continue;
        };
        for (path, entry) in container_entries(bundle) {
            let Some(entry) = entry.as_object() else {
                continue;
            };
            let start = entry
                .get("preloadIndex")
                .and_then(Value::as_u64)
                .unwrap_or(0) as usize;
            let size = entry
                .get("preloadSize")
                .and_then(Value::as_u64)
                .unwrap_or(0) as usize;
            let end = start.saturating_add(size).min(table.len());
            for preload in table.get(start..end).unwrap_or_default() {
                if let Some(path_id) = local_path_id(preload) {
                    map.entry(path_id)
                        .or_insert_with(|| Placement::Dependency(path.clone()));
                }
            }
        }
    }

    map
}

fn container_entries(bundle: &Value) -> Vec<(String, &Value)> {
    bundle
        .get("m_Container")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|entry| {
            let pair = entry.as_array()?;
            Some((pair.first()?.as_str()?.to_string(), pair.get(1)?))
        })
        .collect()
}

fn local_path_id(value: &Value) -> Option<i64> {
    let value = value.get("asset").unwrap_or(value);
    (value.get("m_FileID")?.as_i64()? == 0)
        .then(|| value.get("m_PathID")?.as_i64())
        .flatten()
}

/// Build a texture path relative to the textures output directory.
pub fn relative_path(placement: Option<&Placement>, name: &str, path_id: i64) -> PathBuf {
    let mut stem = flat_filename(name);
    if stem.is_empty() {
        stem = format!("obj_{path_id}");
    }

    let Some(mut result) = placement_dir(placement) else {
        return PathBuf::from("_unsorted")
            .join(fit_filename(&format!("{stem}__{path_id}.png"), path_id));
    };
    result.push(fit_filename(&format!("{stem}.png"), path_id));
    result
}

/// Output directory for an object with this placement, or `None` when it has no container.
///
/// Direct assets sit beside their container path; dependencies get a folder named after the
/// container they were loaded with (e.g. `prefab_name/`).
pub fn placement_dir(placement: Option<&Placement>) -> Option<PathBuf> {
    let placement = placement?;
    let container = match placement {
        Placement::Direct(path) | Placement::Dependency(path) => {
            path.strip_prefix("assets/").unwrap_or(path)
        }
    };
    let directory = match placement {
        Placement::Direct(_) => container.rsplit_once('/').map_or("", |(parent, _)| parent),
        Placement::Dependency(_) => without_extension(container),
    };
    let components: Vec<_> = directory
        .split('/')
        .filter(|component| !component.is_empty() && *component != "." && *component != "..")
        .map(sanitize_filename)
        .filter(|component| !component.is_empty())
        .collect();
    Some(if components.is_empty() {
        PathBuf::from("_root")
    } else {
        components.iter().collect()
    })
}

/// Claim the output path for an object under `out_dir`, laid out by its placement.
///
/// Objects without a container go to `unsorted_dir`, where unrelated chunks share one
/// directory; suffixing the path ID keeps each object's output path stable across runs.
pub(crate) fn claim_object_path(
    out_dir: &Path,
    placement: Option<&Placement>,
    unsorted_dir: PathBuf,
    file_name: &str,
    path_id: i64,
    seen: &mut HashSet<PathBuf>,
) -> Option<PathBuf> {
    let (dir, file_name) = match placement_dir(placement) {
        Some(dir) => (dir, file_name.to_string()),
        None => (unsorted_dir, with_path_id(file_name, path_id)),
    };
    claim_path(out_dir.join(dir), &file_name, path_id, seen)
}

fn without_extension(path: &str) -> &str {
    let file_start = path.rfind('/').map_or(0, |index| index + 1);
    match path[file_start..].rfind('.') {
        Some(index) => &path[..file_start + index],
        None => path,
    }
}

#[derive(Serialize)]
pub struct IndexEntry {
    pub path: String,
    pub name: String,
    pub container: Option<String>,
    pub placement: &'static str,
    pub width: u32,
    pub height: u32,
    pub format: &'static str,
}

impl IndexEntry {
    pub fn new(
        relative: &Path,
        name: &str,
        placement: Option<&Placement>,
        width: u32,
        height: u32,
        format: &'static str,
    ) -> Self {
        let (container, placement) = match placement {
            Some(Placement::Direct(path)) => (Some(path.clone()), "direct"),
            Some(Placement::Dependency(path)) => (Some(path.clone()), "dependency"),
            None => (None, "unsorted"),
        };
        Self {
            path: slash_path(relative),
            name: name.to_string(),
            container,
            placement,
            width,
            height,
            format,
        }
    }
}

/// A relative output path with `/` separators on every platform.
pub fn slash_path(relative: &Path) -> String {
    relative.to_string_lossy().replace('\\', "/")
}

/// Write one chunk's texture index atomically.
pub fn write_chk_index(tex_dir: &Path, chk_name: &str, entries: &[IndexEntry]) -> io::Result<()> {
    fs::create_dir_all(tex_dir.join(".index"))?;
    let path = chk_index_path(tex_dir, chk_name);
    let tmp = path.with_extension("jsonl.tmp");
    let mut file = fs::File::create(&tmp)?;
    for entry in entries {
        serde_json::to_writer(&mut file, entry).map_err(io::Error::other)?;
        writeln!(file)?;
    }
    fs::rename(tmp, path)
}

/// Remove per-chunk indexes for chunks no longer present upstream.
pub fn remove_chk_indexes_except(
    tex_dir: &Path,
    keep_chk_names: &HashSet<String>,
) -> io::Result<usize> {
    let index_dir = tex_dir.join(".index");
    let Ok(entries) = fs::read_dir(index_dir) else {
        return Ok(0);
    };
    let keep: HashSet<_> = keep_chk_names
        .iter()
        .map(|name| index_filename(name))
        .collect();
    let mut removed = 0;
    for entry in entries {
        let entry = entry?;
        let file_name = entry.file_name();
        let is_jsonl = Path::new(&file_name)
            .extension()
            .is_some_and(|extension| extension == "jsonl");
        if entry.file_type()?.is_file()
            && is_jsonl
            && !keep.contains(file_name.to_string_lossy().as_ref())
        {
            fs::remove_file(entry.path())?;
            removed += 1;
        }
    }
    Ok(removed)
}

/// Rebuild the combined texture index from the chunk indexes.
pub fn rebuild_index(tex_dir: &Path) -> io::Result<usize> {
    fs::create_dir_all(tex_dir)?;
    let mut files = match fs::read_dir(tex_dir.join(".index")) {
        Ok(entries) => entries.collect::<Result<Vec<_>, _>>()?,
        Err(error) if error.kind() == io::ErrorKind::NotFound => Vec::new(),
        Err(error) => return Err(error),
    };
    files.retain(|entry| {
        entry.file_type().is_ok_and(|kind| kind.is_file())
            && entry
                .path()
                .extension()
                .is_some_and(|extension| extension == "jsonl")
    });
    files.sort_by_key(|entry| entry.file_name());
    let path = tex_dir.join("index.jsonl");
    let tmp = tex_dir.join("index.jsonl.tmp");
    let mut output = fs::File::create(&tmp)?;
    let mut count = 0;
    for file in files {
        let contents = fs::read_to_string(file.path())?;
        for line in contents.lines() {
            writeln!(output, "{line}")?;
            count += 1;
        }
    }
    fs::rename(tmp, path)?;
    Ok(count)
}

/// Remove PNGs from the legacy flat texture output layout.
pub fn remove_legacy_flat_textures(tex_dir: &Path) -> io::Result<usize> {
    let Ok(entries) = fs::read_dir(tex_dir) else {
        return Ok(0);
    };
    let mut removed = 0;
    for entry in entries {
        let entry = entry?;
        if is_png_file(&entry)? {
            fs::remove_file(entry.path())?;
            removed += 1;
        }
    }
    Ok(removed)
}

/// Whether a directory entry is a regular file with a `.png` extension (any case).
pub(crate) fn is_png_file(entry: &fs::DirEntry) -> io::Result<bool> {
    Ok(entry.file_type()?.is_file()
        && entry
            .path()
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("png")))
}

/// Path of one chunk's texture index (`<tex_dir>/.index/<chk>.jsonl`).
pub fn chk_index_path(tex_dir: &Path, chk_name: &str) -> PathBuf {
    tex_dir.join(".index").join(index_filename(chk_name))
}

/// Why a PNG could not be flipped.
#[derive(Debug)]
pub enum FlipError {
    /// The file exists but is not a readable image (e.g. truncated by a crash).
    Corrupt(image::ImageError),
    Io(io::Error),
}

impl std::fmt::Display for FlipError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FlipError::Corrupt(error) => write!(f, "unreadable image: {error}"),
            FlipError::Io(error) => write!(f, "{error}"),
        }
    }
}

/// Write an upside-down copy of a PNG beside it and return the copy's path.
///
/// The caller renames it over the original once the flip is recorded, so a crash can never
/// leave a file flipped without a record of it.
pub fn write_flipped_png(path: &Path) -> Result<PathBuf, FlipError> {
    let image = image::open(path).map_err(|error| match error {
        // A truncated file surfaces as an I/O end-of-file while decoding.
        image::ImageError::IoError(io)
            if !matches!(
                io.kind(),
                io::ErrorKind::UnexpectedEof | io::ErrorKind::InvalidData
            ) =>
        {
            FlipError::Io(io)
        }
        error => FlipError::Corrupt(error),
    })?;
    let tmp = path.with_extension("png.tmp");
    image
        .flipv()
        .save_with_format(&tmp, image::ImageFormat::Png)
        .map_err(|error| FlipError::Io(io::Error::other(error)))?;
    Ok(tmp)
}

/// Flip a PNG upside down in place, replacing it atomically.
pub fn flip_png_in_place(path: &Path) -> Result<(), FlipError> {
    let tmp = write_flipped_png(path)?;
    fs::rename(tmp, path).map_err(FlipError::Io)
}

fn index_filename(chk_name: &str) -> String {
    let name = chk_name.rsplit('/').next().unwrap_or(chk_name);
    format!("{}.jsonl", name.strip_suffix(".chk").unwrap_or(name))
}
