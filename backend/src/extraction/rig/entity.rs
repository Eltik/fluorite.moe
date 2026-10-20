//! Finding entity prefabs in the store index and naming their exports.
use super::store::CabRecord;
use std::collections::BTreeMap;

/// A prefab container entry that may be an exportable entity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    pub container: String,
    pub kind: &'static str,
    pub id: String,
    /// `(cab, path_id)` of every bundle listing this container with a local asset.
    pub sources: Vec<(String, i64)>,
    pub chunks: Vec<String>,
}

/// Entity kind from a container path.
#[must_use]
pub fn kind_of(container: &str) -> &'static str {
    let c = container.to_ascii_lowercase();
    let stem = id_of(&c);
    if c.contains("/postmodels/characters/")
        || c.contains("/entity/actor/")
        || stem.starts_with("chr_")
    {
        if stem.starts_with("chr_") || c.contains("/entity/actor/") {
            "operator"
        } else {
            "npc"
        }
    } else if c.contains("/postmodels/enemies/")
        || c.contains("/entity/monster/")
        || stem.starts_with("eny_")
    {
        "enemy"
    } else if c.contains("/prefabs/weapons/")
        || c.contains("/entity/wpn/")
        || stem.starts_with("wpn_")
    {
        "weapon"
    } else if c.contains("/npc") {
        "npc"
    } else {
        "other"
    }
}

/// Entity id guess: the prefab file stem without a `_postmodel` suffix.
#[must_use]
pub fn id_of(container: &str) -> String {
    let file = container.rsplit('/').next().unwrap_or(container);
    let stem = file.strip_suffix(".prefab").unwrap_or(file);
    stem.strip_suffix("_postmodel").unwrap_or(stem).to_string()
}

/// Prefab containers matching `filter`, grouped by container path.
///
/// Bundles that hold the asset locally (`m_FileID == 0`) and contain GameObjects come first.
#[must_use]
pub fn candidates(records: &[CabRecord], filter: &str) -> Vec<Candidate> {
    let filter = filter.to_ascii_lowercase();
    let mut by_path: BTreeMap<String, Candidate> = BTreeMap::new();
    for rec in records {
        let has_objects = rec.classes.contains_key(&1);
        for (path, file_id, path_id) in &rec.containers {
            let lower = path.to_ascii_lowercase();
            if !lower.ends_with(".prefab")
                || !lower.contains(&filter)
                || *file_id != 0
                || !has_objects
            {
                continue;
            }
            let entry = by_path.entry(lower.clone()).or_insert_with(|| Candidate {
                container: path.clone(),
                kind: kind_of(path),
                id: id_of(&lower),
                sources: Vec::new(),
                chunks: Vec::new(),
            });
            if !entry
                .sources
                .iter()
                .any(|(c, p)| c == &rec.cab && p == path_id)
            {
                entry.sources.push((rec.cab.clone(), *path_id));
            }
            if !entry.chunks.contains(&rec.chunk) {
                entry.chunks.push(rec.chunk.clone());
            }
        }
    }
    by_path.into_values().collect()
}

/// A file-system-safe file stem for an entity id.
#[must_use]
pub fn file_stem(id: &str) -> String {
    id.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.') {
                c
            } else {
                '_'
            }
        })
        .collect()
}
