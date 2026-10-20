//! An on-disk store of the small parts of every chunk a rig export needs.
//!
//! Chunks are ~1 GB each and mostly texture pixels, while a prefab, its meshes and materials
//! usually live in different bundles of different chunks. Scanning a chunk once keeps every
//! SerializedFile (a few MB per chunk) and the resource streams of bundles holding meshes,
//! so exports can resolve cross-bundle references without re-downloading anything.
//!
//! Layout (also accepted as `--source-dir`):
//! - `sf/<CAB>`: SerializedFile bytes
//! - `res/<CAB>.resS`: resource streams of bundles with Mesh objects
//! - `index/<chunk>.jsonl`: one [`CabRecord`] per SerializedFile of a scanned chunk
use super::chunk::{ChunkFiles, NamedFile, containers};
use crate::extraction::unity::SerializedFile;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

const CLASS_MESH: i32 = 43;

/// Asset classes a rig export resolves through PPtrs (meshes, materials, textures, shaders,
/// skinned renderers, avatars, controllers).
pub const ASSET_CLASSES: &[i32] = &[21, 28, 43, 48, 90, 91, 137];

/// Container path fragments of entity bundles (prefabs, their meshes and animations).
pub const ENTITY_HINTS: &[&str] = &["postmodels", "prefabs/weapons", "/entity/", "/actors/"];

/// Container prefixes of bundles no entity prefab draws from (scenes, UI, dialog timelines).
pub const SKIPPED_PREFIXES: &[&str] = &[
    "assets/beyond/arts/environment/",
    "assets/beyond/arts/ui/",
    "assets/beyond/scenes/",
    "assets/beyond/dynamicassets/scenes/",
    "assets/beyond/dynamicassets/gameplay/dialog/",
    "assets/beyond/dynamicassets/gameplay/ui/",
];

fn is_entity_bundle(containers: &[(String, i64, i64)]) -> bool {
    containers.iter().any(|(p, _, _)| {
        ENTITY_HINTS
            .iter()
            .any(|h| p.to_ascii_lowercase().contains(h))
    })
}

/// Whether a SerializedFile is worth keeping: an entity bundle, or one holding shared assets
/// (shaders, materials, textures, meshes) that is not purely a scene/UI/dialog bundle.
///
/// Everything else (MonoBehaviour-only data, cutscenes, level sequences — gigabytes per run) is
/// indexed but not stored.
#[must_use]
pub fn is_rig_relevant(classes: &BTreeMap<i32, usize>, containers: &[(String, i64, i64)]) -> bool {
    if is_entity_bundle(containers) {
        return true;
    }
    let scene_only = !containers.is_empty()
        && containers.iter().all(|(p, _, _)| {
            SKIPPED_PREFIXES
                .iter()
                .any(|s| p.to_ascii_lowercase().starts_with(s))
        });
    !scene_only && classes.keys().any(|c| ASSET_CLASSES.contains(c))
}

/// Whether a bundle's resource streams are kept: entity bundles (or anonymous ones) with meshes.
#[must_use]
pub fn keeps_resources(classes: &BTreeMap<i32, usize>, containers: &[(String, i64, i64)]) -> bool {
    classes.contains_key(&CLASS_MESH) && (containers.is_empty() || is_entity_bundle(containers))
}

/// Index entry for one SerializedFile of a scanned chunk.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CabRecord {
    pub cab: String,
    pub chunk: String,
    pub sf_size: usize,
    /// Resource streams of this bundle that were stored.
    #[serde(default)]
    pub resources: Vec<String>,
    /// Object counts by Unity class ID.
    pub classes: BTreeMap<i32, usize>,
    /// `m_Container` entries: `[path, fileID, pathID]`.
    pub containers: Vec<(String, i64, i64)>,
    /// CAB names of `m_Externals`, in order (`m_FileID` n is `externals[n-1]`).
    pub externals: Vec<String>,
}

/// Result of storing one chunk.
#[derive(Debug, Default)]
pub struct StoreStats {
    pub files: usize,
    pub sf_bytes: usize,
    pub res_bytes: usize,
}

fn write_atomic(path: &Path, data: &[u8]) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, data)?;
    fs::rename(&tmp, path)
}

fn record(chunk: &str, file: &NamedFile, resources: Vec<String>) -> CabRecord {
    let mut classes = BTreeMap::new();
    for obj in &file.sf.objects {
        *classes.entry(obj.class_id).or_default() += 1;
    }
    CabRecord {
        cab: file.cab.clone(),
        chunk: chunk.to_string(),
        sf_size: file.sf.data.len(),
        resources,
        classes,
        containers: containers(&file.sf)
            .into_iter()
            .map(|c| (c.path, c.file_id, c.path_id))
            .collect(),
        externals: file
            .sf
            .externals
            .iter()
            .map(|e| e.cab_name().to_string())
            .collect(),
    }
}

/// Path of a chunk's index in the store.
#[must_use]
pub fn chunk_index_path(store: &Path, chunk: &str) -> PathBuf {
    store.join("index").join(format!("{chunk}.jsonl"))
}

/// Whether a chunk has already been scanned into the store.
#[must_use]
pub fn is_scanned(store: &Path, chunk: &str) -> bool {
    chunk_index_path(store, chunk).is_file()
}

/// Write a chunk's SerializedFiles, mesh resource streams and index into the store.
///
/// The index is written last, so an interrupted scan is redone rather than half-trusted.
pub fn store_chunk(store: &Path, chunk: &str, files: &ChunkFiles) -> io::Result<StoreStats> {
    let mut stats = StoreStats::default();
    let mut lines = Vec::new();
    for file in &files.files {
        let mut rec = record(chunk, file, Vec::new());
        if !is_rig_relevant(&rec.classes, &rec.containers) {
            lines.push(serde_json::to_string(&rec).map_err(io::Error::other)?);
            continue;
        }
        write_atomic(&store.join("sf").join(&file.cab), &file.sf.data)?;
        stats.files += 1;
        stats.sf_bytes += file.sf.data.len();
        let mut stored = Vec::new();
        if keeps_resources(&rec.classes, &rec.containers) {
            for (name, data) in &files.resources {
                if name.starts_with(&file.cab) {
                    write_atomic(&store.join("res").join(name), data)?;
                    stats.res_bytes += data.len();
                    stored.push(name.clone());
                }
            }
        }
        rec.resources = stored;
        lines.push(serde_json::to_string(&rec).map_err(io::Error::other)?);
    }
    let mut body = lines.join("\n");
    body.push('\n');
    write_atomic(&chunk_index_path(store, chunk), body.as_bytes())?;
    Ok(stats)
}

/// Read every chunk index of the store.
pub fn read_index(store: &Path) -> io::Result<Vec<CabRecord>> {
    let mut out = Vec::new();
    let dir = store.join("index");
    let Ok(entries) = fs::read_dir(&dir) else {
        return Ok(out);
    };
    let mut paths: Vec<_> = entries.filter_map(Result::ok).map(|e| e.path()).collect();
    paths.sort();
    for path in paths {
        if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
            continue;
        }
        for line in fs::read_to_string(&path)?.lines() {
            if line.trim().is_empty() {
                continue;
            }
            match serde_json::from_str::<CabRecord>(line) {
                Ok(rec) => out.push(rec),
                Err(e) => eprintln!("  bad index line in {}: {e}", path.display()),
            }
        }
    }
    Ok(out)
}

/// Lazily loaded SerializedFiles by CAB name, backed by a store and/or in-memory chunks.
pub struct CabSource {
    store: Option<PathBuf>,
    memory: ChunkFiles,
    /// CAB name (lowercase) → index into `loaded.files`, `None` when known missing.
    loaded: HashMap<String, Option<usize>>,
    pub files: ChunkFiles,
}

impl CabSource {
    #[must_use]
    pub fn new(store: Option<PathBuf>, memory: ChunkFiles) -> Self {
        Self {
            store,
            memory,
            loaded: HashMap::new(),
            files: ChunkFiles::default(),
        }
    }

    /// Index of a CAB in `self.files`, loading it (and its resource streams) on first use.
    pub fn get(&mut self, cab: &str) -> Option<usize> {
        let key = cab.to_ascii_lowercase();
        if let Some(found) = self.loaded.get(&key) {
            return *found;
        }
        let found = self.load(cab);
        self.loaded.insert(key, found);
        found
    }

    fn load(&mut self, cab: &str) -> Option<usize> {
        if let Some(i) = self.memory.file_index(cab) {
            let file = self.memory.files.swap_remove(i);
            let names: Vec<String> = self
                .memory
                .resources
                .keys()
                .filter(|k| k.starts_with(&file.cab))
                .cloned()
                .collect();
            for name in names {
                if let Some(data) = self.memory.resources.remove(&name) {
                    self.files.resources.insert(name, data);
                }
            }
            self.files.files.push(file);
            return Some(self.files.files.len() - 1);
        }
        let store = self.store.as_ref()?;
        let data = fs::read(store.join("sf").join(cab)).ok()?;
        let sf = SerializedFile::parse(data).ok()?;
        for suffix in [".resS", ".resource"] {
            let name = format!("{cab}{suffix}");
            if let Ok(data) = fs::read(store.join("res").join(&name)) {
                self.files.resources.insert(name, data);
            }
        }
        self.files.files.push(NamedFile {
            cab: cab.to_string(),
            sf,
        });
        Some(self.files.files.len() - 1)
    }

    /// Load every in-memory file (used when exporting straight from downloaded chunks).
    pub fn load_all_memory(&mut self) {
        let cabs: Vec<String> = self.memory.files.iter().map(|f| f.cab.clone()).collect();
        for cab in cabs {
            self.get(&cab);
        }
    }

    /// Drop loaded files to bound memory between entities.
    pub fn clear(&mut self) {
        if self.store.is_some() {
            self.files = ChunkFiles::default();
            self.loaded.clear();
        }
    }
}

/// Delete stored files that the current relevance rules would not keep. Returns bytes freed.
pub fn prune(store: &Path) -> io::Result<u64> {
    let mut freed = 0u64;
    for rec in read_index(store)? {
        let mut doomed = Vec::new();
        if !is_rig_relevant(&rec.classes, &rec.containers) {
            doomed.push(store.join("sf").join(&rec.cab));
        }
        if !keeps_resources(&rec.classes, &rec.containers)
            || !is_rig_relevant(&rec.classes, &rec.containers)
        {
            doomed.extend(rec.resources.iter().map(|r| store.join("res").join(r)));
        }
        for path in doomed {
            if let Ok(meta) = fs::metadata(&path) {
                fs::remove_file(&path)?;
                freed += meta.len();
            }
        }
    }
    Ok(freed)
}
