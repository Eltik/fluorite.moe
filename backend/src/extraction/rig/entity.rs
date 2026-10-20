//! Finding entity prefabs in the store index and naming their exports.
use super::store::CabRecord;
use serde_json::Value;
use std::collections::{BTreeMap, HashMap, HashSet};

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
///
/// NPC folders are checked first: `postmodels/npc/chr_0023_antal_postmodel` is the NPC
/// variant of an operator and must not land on (and overwrite) the operator's file.
#[must_use]
pub fn kind_of(container: &str) -> &'static str {
    let c = container.to_ascii_lowercase();
    let stem = id_of(&c);
    if c.contains("/postmodels/npc/") || c.contains("/npc/") {
        "npc"
    } else if c.contains("/postmodels/characters/")
        || c.contains("/entity/actor/")
        || stem.starts_with("chr_")
    {
        "operator"
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
    } else {
        "other"
    }
}

/// Claim the output path `<kind>/<id>.glb` for a container. When another container of this
/// run already holds it, a short hash of the container is appended
/// (`<kind>/<id>__<crc32>.glb`), so no export overwrites another.
pub fn claim_output(
    claimed: &mut HashMap<String, String>,
    kind: &str,
    id: &str,
    container: &str,
) -> String {
    let base = format!("{kind}/{}", file_stem(id));
    let mut rel = format!("{base}.glb");
    if claimed.get(&rel).is_some_and(|c| c != container) {
        rel = format!(
            "{base}__{:08x}.glb",
            super::anim::crc32(container.as_bytes())
        );
    }
    claimed.insert(rel.clone(), container.to_string());
    rel
}

/// Merge this run's index entries into the previous `index.json`: fresh entries replace old
/// ones for the same container or the same GLB (whose file this run just rewrote), and no two
/// entries ever point at one file. Sorted by kind, then id.
#[must_use]
pub fn merge_index(old: Option<&Value>, fresh: Vec<Value>) -> Vec<Value> {
    let key = |v: &Value, k: &str| v.get(k).and_then(Value::as_str).map(str::to_string);
    let fresh_containers: HashSet<String> =
        fresh.iter().filter_map(|e| key(e, "container")).collect();
    let mut taken: HashSet<String> = fresh.iter().filter_map(|e| key(e, "glb")).collect();
    let mut out = fresh;
    for e in old
        .and_then(|o| o.get("entries"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        if key(e, "container").is_none_or(|c| fresh_containers.contains(&c)) {
            continue;
        }
        if let Some(glb) = key(e, "glb")
            && !taken.insert(glb)
        {
            continue;
        }
        out.push(e.clone());
    }
    out.sort_by_key(|v| {
        (
            key(v, "kind").unwrap_or_default(),
            key(v, "id").unwrap_or_default(),
            key(v, "container").unwrap_or_default(),
        )
    });
    out
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
