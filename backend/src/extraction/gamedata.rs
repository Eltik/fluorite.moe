/// TextAsset and MonoBehaviour export, laid out by AssetBundle container path.
use crate::extraction::texture_layout::{self, Placement};
use crate::extraction::unity_extract::{get_script_type_name, parse_text_asset};
use crate::extraction::utils::{fit_filename, sanitize_filename};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use unity_asset_binary::asset::SerializedFile;
use unity_asset_binary::object::ObjectHandle;

/// How a TextAsset's bytes were classified when saved.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextKind {
    Json,
    Text,
    Binary,
}

#[derive(Default)]
pub struct GamedataStats {
    pub text_assets: u64,
    pub json: u64,
    pub text: u64,
    pub binary: u64,
    pub mono: u64,
    pub mono_type_counts: HashMap<String, u64>,
    /// Files that could not be written; the chunk must not be treated as extracted.
    pub write_errors: u64,
}

/// Container extensions whose payload Unity stores verbatim as text.
const TEXT_EXTENSIONS: &[&str] = &[
    "txt", "json", "jsonl", "csv", "tsv", "xml", "yaml", "yml", "lua", "html", "htm", "md",
    "atlas", "fnt", "shader", "cginc", "hlsl",
];

/// Guess a file extension from leading magic bytes.
pub fn sniff_extension(content: &[u8]) -> Option<&'static str> {
    let magic: &[(&[u8], &str)] = &[
        (b"glTF", "glb"),
        (b"\x89PNG\r\n\x1a\n", "png"),
        (b"\xff\xd8\xff", "jpg"),
        (b"GIF8", "gif"),
        (b"OggS", "ogg"),
        (b"fLaC", "flac"),
        (b"ID3", "mp3"),
        (b"\x1f\x8b", "gz"),
        (b"PK\x03\x04", "zip"),
        (b"BKHD", "bnk"),
        (b"AKPK", "pck"),
        (b"CRID", "usm"),
        (b"UnityFS", "unity3d"),
        (b"%PDF", "pdf"),
        (b"\x00\x01\x00\x00\x00", "ttf"),
        (b"OTTO", "otf"),
        (b"wOFF", "woff"),
        (b"wOF2", "woff2"),
    ];
    if content.len() >= 12 && &content[..4] == b"RIFF" {
        return match &content[8..12] {
            b"WEBP" => Some("webp"),
            b"WAVE" => Some("wav"),
            _ => Some("riff"),
        };
    }
    magic
        .iter()
        .find(|(prefix, _)| content.starts_with(prefix))
        .map(|(_, ext)| *ext)
}

/// Split a trailing `.ext` (1-8 alphanumeric chars) off a file name.
fn split_extension(name: &str) -> Option<(&str, &str)> {
    let (stem, ext) = name.rsplit_once('.')?;
    (!stem.is_empty()
        && (1..=8).contains(&ext.len())
        && ext.chars().all(|c| c.is_ascii_alphanumeric()))
    .then_some((stem, ext))
}

fn classify(content: &[u8]) -> TextKind {
    match std::str::from_utf8(content) {
        Ok(text) if serde_json::from_str::<serde_json::Value>(text).is_ok() => TextKind::Json,
        Ok(text) if !text.trim().is_empty() && !text.contains('\0') => TextKind::Text,
        _ => TextKind::Binary,
    }
}

/// Choose the output file name for a TextAsset.
///
/// The extension comes from, in order: the container path (dropping Unity's mandatory
/// `.bytes` suffix when the asset name carries its own extension, e.g. `hero.skel.bytes`),
/// the asset name itself, magic-byte sniffing, then the content's text/JSON shape.
pub fn text_asset_filename(
    name: &str,
    container: Option<&str>,
    content: &[u8],
) -> (String, TextKind) {
    let kind = classify(content);
    let safe = sanitize_filename(name).replace(['/', '\\'], "_");
    let container_ext = container
        .and_then(|path| path.rsplit('/').next())
        .and_then(split_extension)
        .map(|(_, ext)| ext.to_ascii_lowercase());
    let own =
        split_extension(&safe).map(|(stem, ext)| (stem.to_string(), ext.to_ascii_lowercase()));

    let ext = match (&container_ext, &own) {
        (Some(ext), Some(_)) if ext == "bytes" => None,
        (Some(ext), _) if ext != "bytes" => Some(ext.clone()),
        _ => None,
    };
    let (stem, ext) = match (ext, own) {
        // Container already names the type; avoid `foo.json.json`.
        (Some(ext), Some((stem, own_ext))) if own_ext == ext => (stem, ext),
        (Some(ext), _) => (safe.clone(), ext),
        (None, Some((stem, own_ext))) => (stem, own_ext),
        (None, None) => {
            let ext = sniff_extension(content).unwrap_or(match kind {
                TextKind::Json => "json",
                TextKind::Text => "txt",
                TextKind::Binary => "bytes",
            });
            (safe.clone(), ext.to_string())
        }
    };
    let kind = match kind {
        TextKind::Json if ext == "json" => TextKind::Json,
        TextKind::Json | TextKind::Text if TEXT_EXTENSIONS.contains(&ext.as_str()) => {
            TextKind::Text
        }
        _ => TextKind::Binary,
    };
    (format!("{stem}.{ext}"), kind)
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
    let renamed = match split_extension(file_name) {
        Some((stem, ext)) => format!("{stem}__{path_id}.{ext}"),
        None => format!("{file_name}__{path_id}"),
    };
    let path = dir.join(renamed);
    seen.insert(path.clone()).then_some(path)
}

/// Write a file atomically (temporary file + rename) so a crash never leaves it truncated.
pub(crate) fn write_file(path: &Path, bytes: &[u8]) -> bool {
    if let Some(parent) = path.parent()
        && fs::create_dir_all(parent).is_err()
    {
        return false;
    }
    let mut tmp = path.as_os_str().to_os_string();
    tmp.push(".tmp");
    fs::write(&tmp, bytes)
        .and_then(|()| fs::rename(&tmp, path))
        .is_ok()
}

/// Make a name unique for `_unsorted/`, where unrelated chunks share one directory.
///
/// Suffixing the path ID keeps each object's output path stable across incremental runs.
fn unsorted_name(file_name: &str, path_id: i64) -> String {
    match split_extension(file_name) {
        Some((stem, ext)) => format!("{stem}__{path_id}.{ext}"),
        None => format!("{file_name}__{path_id}"),
    }
}

fn container_path(placement: Option<&Placement>) -> Option<&str> {
    match placement? {
        Placement::Direct(path) => Some(path),
        Placement::Dependency(_) => None,
    }
}

/// Export every TextAsset (class 49) in `sf` under `out_dir`.
pub fn extract_text_assets(
    sf: &SerializedFile,
    containers: &HashMap<i64, Placement>,
    out_dir: &Path,
    seen: &mut HashSet<PathBuf>,
    stats: &mut GamedataStats,
) {
    for obj in sf.objects_of_type(49) {
        let Ok(bytes) = sf.object_bytes(obj) else {
            continue;
        };
        let Some((name, content)) = parse_text_asset(bytes) else {
            continue;
        };
        if content.is_empty() {
            continue;
        }
        let placement = containers.get(&obj.path_id);
        let (mut file_name, kind) = text_asset_filename(&name, container_path(placement), &content);
        let dir = texture_layout::placement_dir(placement).unwrap_or_else(|| {
            file_name = unsorted_name(&file_name, obj.path_id);
            PathBuf::from("_unsorted")
        });
        let Some(path) = claim_path(out_dir.join(dir), &file_name, obj.path_id, seen) else {
            continue;
        };
        let pretty = (kind == TextKind::Json)
            .then(|| {
                let value: serde_json::Value = serde_json::from_slice(&content).ok()?;
                serde_json::to_vec_pretty(&value).ok()
            })
            .flatten();
        if write_file(&path, pretty.as_deref().unwrap_or(&content)) {
            stats.text_assets += 1;
            match kind {
                TextKind::Json => stats.json += 1,
                TextKind::Text => stats.text += 1,
                TextKind::Binary => stats.binary += 1,
            }
        } else {
            stats.write_errors += 1;
        }
    }
}

/// Export every MonoBehaviour (class 114) in `sf` as JSON under `out_dir`.
///
/// Objects without a container go to `_unsorted/<script type>/`.
pub fn extract_monobehaviours(
    sf: &SerializedFile,
    containers: &HashMap<i64, Placement>,
    out_dir: &Path,
    seen: &mut HashSet<PathBuf>,
    stats: &mut GamedataStats,
) {
    if !sf.enable_type_tree {
        return;
    }
    for obj in sf.objects_of_type(114) {
        let type_name = sanitize_filename(&get_script_type_name(sf, obj));
        let handle = ObjectHandle::new(sf, obj);
        let placement = containers.get(&obj.path_id);
        let name = handle
            .peek_name()
            .ok()
            .flatten()
            .map(|n| sanitize_filename(&n).replace(['/', '\\'], "_"))
            .filter(|n| !n.is_empty())
            .or_else(|| {
                // Unnamed assets take their container's file name.
                let file = container_path(placement)?.rsplit('/').next()?;
                Some(
                    split_extension(file)
                        .map_or(file, |(stem, _)| stem)
                        .to_string(),
                )
            })
            .unwrap_or_else(|| format!("{type_name}_{}", obj.path_id));
        let mut file_name = format!("{name}.json");
        let dir = texture_layout::placement_dir(placement).unwrap_or_else(|| {
            file_name = unsorted_name(&file_name, obj.path_id);
            Path::new("_unsorted").join(&type_name)
        });
        let Some(path) = claim_path(out_dir.join(dir), &file_name, obj.path_id, seen) else {
            continue;
        };
        let Ok(unity_obj) = handle.read() else {
            continue;
        };
        let Ok(json) = serde_json::to_vec_pretty(unity_obj.class.properties()) else {
            continue;
        };
        if write_file(&path, &json) {
            stats.mono += 1;
            *stats.mono_type_counts.entry(type_name).or_insert(0) += 1;
        } else {
            stats.write_errors += 1;
        }
    }
}

/// Remove the legacy flat gamedata layout: top-level files and the `monobehaviour/` tree.
///
/// The container layout never writes files directly into `gamedata/`.
pub fn remove_legacy_gamedata(gamedata_dir: &Path) -> std::io::Result<usize> {
    let Ok(entries) = fs::read_dir(gamedata_dir) else {
        return Ok(0);
    };
    let mut removed = 0;
    for entry in entries {
        let entry = entry?;
        if entry.file_type()?.is_file() {
            fs::remove_file(entry.path())?;
            removed += 1;
        }
    }
    let mono = gamedata_dir.join("monobehaviour");
    if mono.is_dir() {
        fs::remove_dir_all(mono)?;
        removed += 1;
    }
    Ok(removed)
}
