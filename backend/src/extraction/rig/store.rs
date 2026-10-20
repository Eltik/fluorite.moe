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
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

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
    /// Byte size of each stored resource stream, to detect damaged files.
    #[serde(default)]
    pub resource_sizes: BTreeMap<String, u64>,
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

/// A temp path next to `path`, unique per process and call, so concurrent writers (and
/// `X.resS` vs `X.resource`) never share one.
#[must_use]
pub fn temp_path(path: &Path) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("file");
    path.with_file_name(format!(".{name}.{}.{n}.tmp", std::process::id()))
}

/// Write through a unique temp file and rename, so readers never see partial files.
pub fn write_atomic(path: &Path, data: &[u8]) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp = temp_path(path);
    if let Err(e) = fs::write(&tmp, data).and_then(|()| fs::rename(&tmp, path)) {
        let _ = fs::remove_file(&tmp);
        return Err(e);
    }
    Ok(())
}

/// An exclusive lock file created with `create_new`, removed on drop.
#[derive(Debug)]
pub struct FileLock {
    path: PathBuf,
}

/// Locks older than this are assumed to belong to a crashed process.
const STALE_LOCK: Duration = Duration::from_secs(6 * 3600);

impl FileLock {
    /// Take the lock if it is free (or stale).
    pub fn try_acquire(path: &Path) -> io::Result<Option<Self>> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        for _ in 0..2 {
            match fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(path)
            {
                Ok(mut f) => {
                    use std::io::Write;
                    let _ = writeln!(f, "{}", std::process::id());
                    return Ok(Some(Self {
                        path: path.to_path_buf(),
                    }));
                }
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                    let stale = fs::metadata(path)
                        .and_then(|m| m.modified())
                        .ok()
                        .and_then(|t| t.elapsed().ok())
                        .is_some_and(|age| age > STALE_LOCK);
                    if !stale {
                        return Ok(None);
                    }
                    let _ = fs::remove_file(path);
                }
                Err(e) => return Err(e),
            }
        }
        Ok(None)
    }

    /// Wait for the lock, polling, for at most `timeout`.
    pub fn acquire(path: &Path, timeout: Duration) -> io::Result<Self> {
        let start = Instant::now();
        loop {
            if let Some(lock) = Self::try_acquire(path)? {
                return Ok(lock);
            }
            if start.elapsed() > timeout {
                return Err(io::Error::new(
                    io::ErrorKind::WouldBlock,
                    format!("lock {} is held", path.display()),
                ));
            }
            std::thread::sleep(Duration::from_millis(200));
        }
    }
}

impl Drop for FileLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

/// The store-wide lock held while writing files, pruning or repairing.
pub fn lock_store(store: &Path) -> io::Result<FileLock> {
    FileLock::acquire(&store.join(".lock"), Duration::from_secs(600))
}

/// The per-chunk lock held from download to index write, so two scans never fetch the same
/// chunk. `None` when another process holds it.
pub fn try_lock_chunk(store: &Path, chunk: &str) -> io::Result<Option<FileLock>> {
    FileLock::try_acquire(&store.join("index").join(format!("{chunk}.lock")))
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
        resource_sizes: BTreeMap::new(),
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
    let _lock = lock_store(store)?;
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
                    rec.resource_sizes.insert(name.clone(), data.len() as u64);
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

/// Delete stored files that the current relevance rules no longer keep. Records are grouped
/// by CAB (a CAB can appear in several chunks): a file is kept if any record keeps it.
/// Returns bytes freed.
pub fn prune(store: &Path) -> io::Result<u64> {
    let _lock = lock_store(store)?;
    let (sf, res) = prune_plan(&read_index(store)?);
    let mut freed = 0u64;
    let paths = sf
        .iter()
        .map(|cab| store.join("sf").join(cab))
        .chain(res.iter().map(|r| store.join("res").join(r)));
    for path in paths {
        if let Ok(meta) = fs::metadata(&path) {
            fs::remove_file(&path)?;
            freed += meta.len();
        }
    }
    Ok(freed)
}

/// SerializedFiles and resource streams no record keeps: `(sf cabs, res names)`.
#[must_use]
pub fn prune_plan(records: &[CabRecord]) -> (BTreeSet<String>, BTreeSet<String>) {
    let mut keep_sf: HashMap<&str, bool> = HashMap::new();
    let mut keep_res: HashMap<&str, bool> = HashMap::new();
    for rec in records {
        let relevant = is_rig_relevant(&rec.classes, &rec.containers);
        *keep_sf.entry(&rec.cab).or_default() |= relevant;
        let resources = relevant && keeps_resources(&rec.classes, &rec.containers);
        for r in &rec.resources {
            *keep_res.entry(r).or_default() |= resources;
        }
    }
    let unkept = |m: HashMap<&str, bool>| -> BTreeSet<String> {
        m.into_iter()
            .filter(|(_, keep)| !keep)
            .map(|(k, _)| k.to_string())
            .collect()
    };
    (unkept(keep_sf), unkept(keep_res))
}

/// Outcome of [`repair`].
#[derive(Debug, Default)]
pub struct RepairReport {
    pub checked: usize,
    /// CABs whose stored files were damaged and removed.
    pub removed: Vec<String>,
    /// Chunks whose index was removed so the next scan re-stores them.
    pub requeued: BTreeSet<String>,
}

fn sf_intact(path: &Path) -> bool {
    fs::read(path)
        .ok()
        .and_then(|d| SerializedFile::parse(d).ok())
        .is_some_and(|sf| sf.objects.iter().all(|o| sf.object_bytes(o).is_ok()))
}

/// Check stored files against the index: sizes must match a record of the CAB, and CABs
/// listed by several chunks (possibly written concurrently) must parse with every object in
/// bounds. Damaged files are removed and one chunk holding the CAB is re-queued (its index
/// is deleted, so the next `scan` downloads and stores it again).
pub fn repair(store: &Path) -> io::Result<RepairReport> {
    let _lock = lock_store(store)?;
    let mut by_cab: BTreeMap<String, Vec<CabRecord>> = BTreeMap::new();
    for rec in read_index(store)? {
        by_cab.entry(rec.cab.clone()).or_default().push(rec);
    }
    let mut report = RepairReport::default();
    for (cab, recs) in &by_cab {
        let sf_path = store.join("sf").join(cab);
        let Ok(meta) = fs::metadata(&sf_path) else {
            continue;
        };
        report.checked += 1;
        let size_ok = recs.iter().any(|r| r.sf_size as u64 == meta.len());
        let shared = recs.iter().map(|r| &r.chunk).collect::<BTreeSet<_>>().len() > 1;
        let parses = !shared || sf_intact(&sf_path);
        let res_ok = recs.iter().flat_map(|r| r.resources.iter()).all(|name| {
            let sizes: Vec<u64> = recs
                .iter()
                .filter_map(|r| r.resource_sizes.get(name).copied())
                .collect();
            sizes.is_empty()
                || fs::metadata(store.join("res").join(name))
                    .is_ok_and(|m| sizes.contains(&m.len()))
        });
        if size_ok && parses && res_ok {
            continue;
        }
        let _ = fs::remove_file(&sf_path);
        for name in recs.iter().flat_map(|r| r.resources.iter()) {
            let _ = fs::remove_file(store.join("res").join(name));
        }
        if let Some(chunk) = recs.iter().map(|r| r.chunk.clone()).min() {
            let _ = fs::remove_file(chunk_index_path(store, &chunk));
            report.requeued.insert(chunk);
        }
        report.removed.push(cab.clone());
    }
    Ok(report)
}
