//! Migration of legacy flat texture output into the container-based layout.
use crate::extraction::texture;
use crate::extraction::texture_layout::{self, IndexEntry, Placement, slash_path};
use crate::extraction::unity::SerializedFile;
use crate::extraction::unity_extract::ChkContents;
use crate::extraction::utils::sanitize_filename;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

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
        let Ok(sf) = SerializedFile::parse(bytes.clone()) else {
            continue;
        };
        let objects: HashMap<_, _> = sf
            .objects_of_type(28)
            .map(|obj| (obj.path_id, obj))
            .collect();
        for planned in texture_layout::plan_textures(&sf) {
            let Some(&obj) = objects.get(&planned.path_id) else {
                continue;
            };
            let Ok(value) = sf.read_object(obj) else {
                continue;
            };
            let width = texture::get_int(&value, "m_Width") as u32;
            let height = texture::get_int(&value, "m_Height") as u32;
            if width == 0 || height == 0 {
                continue;
            }
            let format = texture::get_int(&value, "m_TextureFormat");
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
                .map(|candidate| slash_path(&candidate.relative))
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
        report
            .moved_by_chk
            .entry(candidate.chk_name.clone())
            .or_default()
            .push(IndexEntry::new(
                &candidate.relative,
                &candidate.name,
                candidate.placement.as_ref(),
                candidate.width,
                candidate.height,
                candidate.format,
            ));
    }

    report.missing = relative_paths
        .iter()
        .filter(|relative| !tex_dir.join(relative).is_file())
        .count();
    let group_names: HashSet<_> = groups.keys().collect();
    for entry in fs::read_dir(tex_dir)? {
        let entry = entry?;
        if texture_layout::is_png_file(&entry)?
            && !group_names.contains(&entry.file_name().to_string_lossy().to_string())
        {
            report.unmatched += 1;
        }
    }
    Ok(report)
}
