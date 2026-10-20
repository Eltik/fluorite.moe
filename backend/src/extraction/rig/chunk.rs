//! Loading a `.chk` (or a directory of pre-extracted files) as named SerializedFiles.
use crate::extraction::unity::SerializedFile;
use crate::extraction::{unityfs_header, vfs_decrypt};
use std::collections::HashMap;
use std::path::Path;

/// One parsed SerializedFile and the CAB name other files reference it by.
pub struct NamedFile {
    /// Bare CAB name (`CAB-<hash>`), matching `FileIdentifier::cab_name` of referrers.
    pub cab: String,
    pub sf: SerializedFile,
}

/// Every SerializedFile and resource stream of one chunk (or several merged chunks).
#[derive(Default)]
pub struct ChunkFiles {
    pub files: Vec<NamedFile>,
    /// Resource streams keyed by entry name (`CAB-<hash>.resS`).
    pub resources: HashMap<String, Vec<u8>>,
    /// SerializedFiles that failed to parse.
    pub parse_failures: usize,
}

impl ChunkFiles {
    /// Index of the file with this CAB name.
    #[must_use]
    pub fn file_index(&self, cab: &str) -> Option<usize> {
        self.files
            .iter()
            .position(|f| f.cab.eq_ignore_ascii_case(cab))
    }

    /// Merge another chunk's files into this one (later duplicates are dropped).
    pub fn merge(&mut self, other: ChunkFiles) {
        for file in other.files {
            if self.file_index(&file.cab).is_none() {
                self.files.push(file);
            }
        }
        for (name, data) in other.resources {
            self.resources.entry(name).or_insert(data);
        }
        self.parse_failures += other.parse_failures;
    }
}

fn is_resource_stream(name: &str) -> bool {
    name.ends_with(".resS") || name.ends_with(".resource")
}

fn bare_name(path: &str) -> String {
    path.rsplit('/').next().unwrap_or(path).to_string()
}

/// Decrypt and unpack a downloaded `.chk`, keeping each SerializedFile's CAB name.
pub fn load_chunk(data: &[u8]) -> Result<ChunkFiles, String> {
    if !vfs_decrypt::is_valid_header(data) {
        return Err("not a VFS container".into());
    }
    let assets = unityfs_header::process_chk_file(data).map_err(|e| e.to_string())?;
    let mut out = ChunkFiles::default();
    let push_sf =
        |out: &mut ChunkFiles, cab: String, bytes: Vec<u8>| match SerializedFile::parse(bytes) {
            Ok(sf) => out.files.push(NamedFile { cab, sf }),
            Err(_) => out.parse_failures += 1,
        };
    for asset in assets {
        if asset.is_unityfs {
            for entry in &asset.bundle_entries {
                let Ok(start) = usize::try_from(entry.offset) else {
                    continue;
                };
                if start >= asset.data.len() {
                    continue;
                }
                let end = start
                    .saturating_add(usize::try_from(entry.size).unwrap_or(0))
                    .min(asset.data.len());
                let bytes = asset.data[start..end].to_vec();
                if is_resource_stream(&entry.path) {
                    out.resources.insert(bare_name(&entry.path), bytes);
                } else {
                    push_sf(&mut out, bare_name(&entry.path), bytes);
                }
            }
        } else if is_resource_stream(&asset.vfs_name) {
            out.resources.insert(bare_name(&asset.vfs_name), asset.data);
        } else {
            push_sf(&mut out, bare_name(&asset.vfs_name), asset.data);
        }
    }
    Ok(out)
}

/// Load a directory of pre-extracted files: `CAB-*` (or `*.bin`) SerializedFiles and `*.resS`.
///
/// A file's CAB name is its file name without a `.bin`/`.assets` extension.
pub fn load_source_dir(dir: &Path) -> Result<ChunkFiles, String> {
    let mut out = ChunkFiles::default();
    let mut entries: Vec<_> = std::fs::read_dir(dir)
        .map_err(|e| format!("{}: {e}", dir.display()))?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.is_file())
        .collect();
    entries.sort();
    for path in entries {
        let Some(name) = path
            .file_name()
            .and_then(|n| n.to_str())
            .map(str::to_string)
        else {
            continue;
        };
        let data = std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        if is_resource_stream(&name) {
            out.resources.insert(name, data);
            continue;
        }
        if name.ends_with(".json") || name.ends_with(".txt") || name.starts_with('.') {
            continue;
        }
        let cab = name
            .strip_suffix(".bin")
            .or_else(|| name.strip_suffix(".assets"))
            .unwrap_or(&name)
            .to_string();
        match SerializedFile::parse(data) {
            Ok(sf) => out.files.push(NamedFile { cab, sf }),
            Err(_) => out.parse_failures += 1,
        }
    }
    Ok(out)
}

/// One `m_Container` entry of an AssetBundle: a container path and the asset it names.
#[derive(Debug, Clone)]
pub struct ContainerEntry {
    pub path: String,
    pub file_id: i64,
    pub path_id: i64,
}

/// Every `m_Container` entry of the AssetBundle objects (class 142) in a SerializedFile.
#[must_use]
pub fn containers(sf: &SerializedFile) -> Vec<ContainerEntry> {
    let mut out = Vec::new();
    for obj in sf.objects_of_type(142) {
        let Ok(bundle) = sf.read_object_keep_pairs(obj) else {
            continue;
        };
        let Some(items) = bundle
            .get("m_Container")
            .and_then(serde_json::Value::as_array)
        else {
            continue;
        };
        for item in items {
            let Some(pair) = item.as_array() else {
                continue;
            };
            let (Some(path), Some(info)) = (pair.first().and_then(|v| v.as_str()), pair.get(1))
            else {
                continue;
            };
            let asset = info.get("asset").unwrap_or(info);
            let (Some(file_id), Some(path_id)) = (
                asset.get("m_FileID").and_then(serde_json::Value::as_i64),
                asset.get("m_PathID").and_then(serde_json::Value::as_i64),
            ) else {
                continue;
            };
            out.push(ContainerEntry {
                path: path.to_string(),
                file_id,
                path_id,
            });
        }
    }
    out
}
