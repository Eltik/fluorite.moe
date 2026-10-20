//! Migration of legacy flat texture output into the container-based layout.
use crate::extraction::texture;
use crate::extraction::texture_layout::{self, IndexEntry, Placement};
use crate::extraction::unity_extract::ChkContents;
use crate::extraction::utils::sanitize_filename;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use unity_asset_binary::asset::SerializedFileParser;
use unity_asset_binary::object::ObjectHandle;

pub struct SortCandidate {
    pub chk_name: String,
    pub relative: PathBuf,
    pub name: String,
    pub placement: Option<Placement>,
    pub width: u32,
    pub height: u32,
    pub format: &'static str,
}

pub fn collect_candidates(chk_name: &str, chk: &ChkContents) -> Vec<SortCandidate> {
    let mut candidates = Vec::new();
    for bytes in &chk.sf_data {
        let Ok(sf) = SerializedFileParser::from_bytes(bytes.clone()) else {
            continue;
        };
        let objects: HashMap<_, _> = sf
            .objects_of_type(28)
            .into_iter()
            .map(|obj| (obj.path_id, obj))
            .collect();
        for planned in texture_layout::plan_textures(&sf) {
            let Some(&obj) = objects.get(&planned.path_id) else {
                continue;
            };
            let Ok(value) = ObjectHandle::new(&sf, obj).read() else {
                continue;
            };
            let width = value.get("m_Width").and_then(|v| v.as_i64()).unwrap_or(0) as u32;
            let height = value.get("m_Height").and_then(|v| v.as_i64()).unwrap_or(0) as u32;
            if width == 0 || height == 0 {
                continue;
            }
            let format = value
                .get("m_TextureFormat")
                .and_then(|v| v.as_i64())
                .unwrap_or(0);
            candidates.push(SortCandidate {
                chk_name: chk_name.to_string(),
                relative: planned.relative,
                name: planned.name,
                placement: planned.placement,
                width,
                height,
                format: texture::texture_format_name(format),
            });
        }
    }
    candidates
}

#[derive(Default)]
pub struct SortReport {
    pub moved: usize,
    pub already_sorted: usize,
    pub ambiguous: usize,
    pub unmatched: usize,
    pub missing: usize,
    pub moved_by_chk: BTreeMap<String, Vec<IndexEntry>>,
}

pub fn sort_flat_textures(tex_dir: &Path, candidates: &[SortCandidate]) -> io::Result<SortReport> {
    let ambiguous_dir = tex_dir.join("_ambiguous");
    fs::create_dir_all(&ambiguous_dir)?;
    let mut ambiguous_file = fs::File::create(ambiguous_dir.join("candidates.jsonl"))?;
    let mut groups: BTreeMap<String, Vec<&SortCandidate>> = BTreeMap::new();
    let mut relative_paths = BTreeSet::new();
    for candidate in candidates {
        relative_paths.insert(candidate.relative.clone());
        let group = groups
            .entry(format!("{}.png", sanitize_filename(&candidate.name)))
            .or_default();
        if !group
            .iter()
            .any(|other| other.relative == candidate.relative)
        {
            group.push(candidate);
        }
    }

    let mut report = SortReport::default();
    for (key, group) in &groups {
        let flat = tex_dir.join(key);
        if !flat.is_file() {
            continue;
        }
        // Unreadable files get (0, 0) and fall through to `_ambiguous/`.
        let (width, height) = image::image_dimensions(&flat).unwrap_or((0, 0));
        // Dimensions are checked even for a lone candidate: a same-named texture from a
        // re-extracted chunk may be the one that won the flat-name collision.
        let matches: Vec<_> = group
            .iter()
            .copied()
            .filter(|candidate| candidate.width == width && candidate.height == height)
            .collect();
        if matches.len() != 1 {
            let destination = ambiguous_dir.join(key);
            fs::rename(&flat, &destination)?;
            let paths: Vec<String> = group
                .iter()
                .map(|candidate| candidate.relative.to_string_lossy().replace('\\', "/"))
                .collect();
            serde_json::to_writer(
                &mut ambiguous_file,
                &serde_json::json!({
                    "file": format!("_ambiguous/{key}"),
                    "width": width,
                    "height": height,
                    "candidates": paths,
                }),
            )
            .map_err(io::Error::other)?;
            writeln!(ambiguous_file)?;
            report.ambiguous += 1;
            continue;
        }
        let candidate = matches[0];
        let target = tex_dir.join(&candidate.relative);
        if target.exists() {
            fs::remove_file(&flat)?;
            report.already_sorted += 1;
        } else {
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::rename(&flat, target)?;
            report.moved += 1;
        }
        let (container, placement) = placement_fields(candidate.placement.as_ref());
        report
            .moved_by_chk
            .entry(candidate.chk_name.clone())
            .or_default()
            .push(IndexEntry {
                path: candidate.relative.to_string_lossy().replace('\\', "/"),
                name: candidate.name.clone(),
                container,
                placement,
                width: candidate.width,
                height: candidate.height,
                format: candidate.format,
            });
    }

    report.missing = relative_paths
        .iter()
        .filter(|relative| !tex_dir.join(relative).is_file())
        .count();
    let group_names: HashSet<_> = groups.keys().collect();
    for entry in fs::read_dir(tex_dir)? {
        let entry = entry?;
        if entry.file_type()?.is_file()
            && entry
                .path()
                .extension()
                .is_some_and(|extension| extension.eq_ignore_ascii_case("png"))
            && !group_names.contains(&entry.file_name().to_string_lossy().to_string())
        {
            report.unmatched += 1;
        }
    }
    Ok(report)
}

fn placement_fields(placement: Option<&Placement>) -> (Option<String>, &'static str) {
    match placement {
        Some(Placement::Direct(path)) => (Some(path.clone()), "direct"),
        Some(Placement::Dependency(path)) => (Some(path.clone()), "dependency"),
        None => (None, "unsorted"),
    }
}
