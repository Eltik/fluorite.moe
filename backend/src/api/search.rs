//! In-memory file-path index and `GET /api/search`.
//!
//! Layout (the output tree holds ~2M files, mostly `gamedata`): every directory is
//! interned once, and each file is a 16-byte [`Entry`] pointing at its directory and at its
//! name in a per-segment `String` arena. A lowercased copy of a name or directory is stored
//! only when it differs from the original. File sizes are not indexed; they are `stat`ed
//! for the returned page only, which keeps the build to plain `readdir` calls.
//!
//! Queries are split on whitespace; every token must occur in the lowercased
//! root-relative path. Directory matches are computed once per query as a bitmask.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant, SystemTime};

use axum::Json;
use axum::extract::{Query, State};
use serde::{Deserialize, Serialize};
use tokio::sync::{Semaphore, watch};

use super::SharedState;
use super::error::{ApiError, ApiResult};
use super::paths::{Root, extension, is_hidden};

const DEFAULT_LIMIT: usize = 50;
const MAX_LIMIT: usize = 500;
/// Deeper pages are refused: every page re-ranks all matches up to `offset + limit`.
pub const MAX_OFFSET: usize = 10_000;
/// Searches running at once; more wait for a permit.
const MAX_CONCURRENT: usize = 4;
/// Tokens beyond this are ignored (matches are tracked in a `u32` bitmask).
const MAX_TOKENS: usize = 16;

/// A string in the arena plus its lowercase twin (same offset when identical).
#[derive(Debug, Clone, Copy)]
struct Span {
    off: u32,
    lower_off: u32,
}

#[derive(Debug, Clone, Copy)]
struct Dir {
    /// Root-relative directory path with a trailing `/` (empty for the root itself).
    span: Span,
    len: u32,
}

#[derive(Debug, Clone, Copy)]
struct Entry {
    dir: u32,
    name: Span,
    name_len: u16,
    root: u8,
}

/// Kept as the per-thread segments produced by the walk; they double as the chunks of
/// the parallel query scan.
#[derive(Debug, Default)]
pub struct SearchIndex {
    output: PathBuf,
    segments: Vec<Segment>,
    counts: [usize; Root::ALL.len()],
    pub built_at: Option<SystemTime>,
    pub build_ms: u128,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Hit {
    pub root: &'static str,
    pub path: String,
    pub name: String,
    pub size: u64,
}

/// Rank tiers, best first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Tier {
    ExactName,
    NamePrefix,
    NameContains,
    PathContains,
}

/// Lowercased whitespace-separated tokens of a query (at most [`MAX_TOKENS`]).
pub fn tokenize(q: &str) -> Vec<String> {
    q.split_whitespace()
        .map(|t| t.to_ascii_lowercase())
        .filter(|t| !t.is_empty())
        .take(MAX_TOKENS)
        .collect()
}

/// A tokenized query with precomputed bitmasks.
struct Prepared {
    tokens: Vec<String>,
    whole: String,
    full: u32,
    slash_mask: u32,
}

impl Prepared {
    fn new(tokens: Vec<String>) -> Self {
        let bits = |pred: &dyn Fn(&str) -> bool| {
            tokens
                .iter()
                .enumerate()
                .filter(|(_, t)| pred(t))
                .fold(0u32, |m, (i, _)| m | (1 << i))
        };
        Self {
            whole: tokens.join(" "),
            full: bits(&|_| true),
            slash_mask: bits(&|t| t.contains('/')),
            tokens,
        }
    }

    fn mask_of(&self, hay: &str) -> u32 {
        self.tokens
            .iter()
            .enumerate()
            .filter(|(_, t)| hay.contains(t.as_str()))
            .fold(0, |m, (i, _)| m | (1 << i))
    }
}

/// One walker thread's share of the tree: its own arena, directories and files.
#[derive(Debug, Default)]
struct Segment {
    arena: String,
    dirs: Vec<Dir>,
    entries: Vec<Entry>,
    counts: [usize; Root::ALL.len()],
}

impl Segment {
    /// Append `s` (and its lowercase form when different); `None` if offsets overflow.
    fn intern(&mut self, s: &str) -> Option<Span> {
        let off = u32::try_from(self.arena.len()).ok()?;
        self.arena.push_str(s);
        let lower_off = if s.bytes().any(|b| b.is_ascii_uppercase()) {
            let lo = u32::try_from(self.arena.len()).ok()?;
            self.arena.extend(s.chars().map(|c| c.to_ascii_lowercase()));
            lo
        } else {
            off
        };
        Some(Span { off, lower_off })
    }

    fn slice(&self, off: u32, len: usize) -> &str {
        &self.arena[off as usize..off as usize + len]
    }

    fn dir_of(&self, e: &Entry) -> &Dir {
        &self.dirs[e.dir as usize]
    }

    fn dir_str(&self, d: &Dir) -> &str {
        self.slice(d.span.off, d.len as usize)
    }

    fn dir_lower(&self, d: &Dir) -> &str {
        self.slice(d.span.lower_off, d.len as usize)
    }

    fn name_str(&self, e: &Entry) -> &str {
        self.slice(e.name.off, e.name_len as usize)
    }

    fn name_lower(&self, e: &Entry) -> &str {
        self.slice(e.name.lower_off, e.name_len as usize)
    }

    fn heap_bytes(&self) -> usize {
        self.arena.capacity()
            + self.dirs.capacity() * std::mem::size_of::<Dir>()
            + self.entries.capacity() * std::mem::size_of::<Entry>()
    }

    /// Classify one entry, or `None` when some token is missing from its path.
    fn rank(&self, e: &Entry, q: &Prepared, dir_mask: u32, joined: &mut String) -> Option<Tier> {
        let name = self.name_lower(e);
        let name_mask = q.mask_of(name);
        let mut mask = dir_mask | name_mask;
        if q.slash_mask & !mask != 0 {
            // Tokens containing '/' may straddle the directory/name boundary.
            joined.clear();
            joined.push_str(self.dir_lower(self.dir_of(e)));
            joined.push_str(name);
            mask |= q.mask_of(joined) & q.slash_mask;
        }
        if mask != q.full {
            return None;
        }
        let stem = name.rsplit_once('.').map_or(name, |(s, _)| s);
        Some(if name == q.whole || stem == q.whole {
            Tier::ExactName
        } else if name.starts_with(q.tokens[0].as_str()) {
            Tier::NamePrefix
        } else if name_mask == q.full {
            Tier::NameContains
        } else {
            Tier::PathContains
        })
    }

    /// Matching entries as `(tier, path length, entry index)`.
    fn scan(&self, q: &Prepared, root: Option<Root>, ext: Option<&str>) -> Vec<(Tier, u32, u32)> {
        let dir_masks: Vec<u32> = self
            .dirs
            .iter()
            .map(|d| q.mask_of(self.dir_lower(d)))
            .collect();
        let mut out = Vec::new();
        let mut joined = String::new();
        for (i, e) in self.entries.iter().enumerate() {
            if root.is_some_and(|r| r as u8 != e.root)
                || ext.is_some_and(|x| !extension(self.name_lower(e)).eq_ignore_ascii_case(x))
            {
                continue;
            }
            if let Some(tier) = self.rank(e, q, dir_masks[e.dir as usize], &mut joined) {
                let len = self.dir_of(e).len + u32::from(e.name_len);
                out.push((tier, len, i as u32));
            }
        }
        out
    }
}

type WorkItem = (Root, PathBuf, String);

/// Shared directory queue; `pending` counts queued plus in-progress items.
struct WorkQueue {
    items: Mutex<Vec<WorkItem>>,
    pending: AtomicUsize,
}

impl WorkQueue {
    fn push(&self, item: WorkItem) {
        self.pending.fetch_add(1, AtomicOrdering::SeqCst);
        self.items
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .push(item);
    }

    fn pop(&self) -> Option<WorkItem> {
        self.items.lock().unwrap_or_else(|p| p.into_inner()).pop()
    }
}

/// List one directory into `seg`, queueing its visible subdirectories. Never follows
/// symlinks.
fn scan_dir(queue: &WorkQueue, seg: &mut Segment, root: Root, abs: &Path, rel: &str) {
    let Ok(rd) = std::fs::read_dir(abs) else {
        return;
    };
    let (Some(span), Ok(dir), Ok(len)) = (
        seg.intern(rel),
        u32::try_from(seg.dirs.len()),
        u32::try_from(rel.len()),
    ) else {
        return;
    };
    seg.dirs.push(Dir { span, len });
    for entry in rd.flatten() {
        let Ok(name) = entry.file_name().into_string() else {
            continue;
        };
        if is_hidden(&name) {
            continue;
        }
        let Ok(ft) = entry.file_type() else {
            continue;
        };
        if ft.is_dir() {
            queue.push((root, entry.path(), format!("{rel}{name}/")));
        } else if ft.is_file() {
            let (Ok(name_len), Some(name_span)) = (u16::try_from(name.len()), seg.intern(&name))
            else {
                continue;
            };
            seg.entries.push(Entry {
                dir,
                name: name_span,
                name_len,
                root: root as u8,
            });
            seg.counts[root.index()] += 1;
        }
    }
}

impl SearchIndex {
    pub fn len(&self) -> usize {
        self.segments.iter().map(|s| s.entries.len()).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn count(&self, root: Root) -> usize {
        self.counts[root.index()]
    }

    /// Approximate heap bytes held by the index.
    pub fn heap_bytes(&self) -> usize {
        self.segments.iter().map(Segment::heap_bytes).sum()
    }

    /// Walk every root under `output` with a pool of threads sharing a directory queue.
    /// Each thread's output becomes one segment as-is (no merge copy). Blocking.
    pub fn build(output: &Path) -> SearchIndex {
        let started = Instant::now();
        let queue = WorkQueue {
            items: Mutex::new(Vec::new()),
            pending: AtomicUsize::new(0),
        };
        for root in Root::ALL {
            let dir = root.dir(output);
            if dir.is_dir() {
                queue.push((root, dir, String::new()));
            }
        }
        let threads = std::thread::available_parallelism().map_or(4, |n| n.get().min(16));
        let segments: Vec<Segment> = std::thread::scope(|s| {
            let handles: Vec<_> = (0..threads)
                .map(|_| {
                    s.spawn(|| {
                        let mut seg = Segment::default();
                        loop {
                            match queue.pop() {
                                Some((root, abs, rel)) => {
                                    scan_dir(&queue, &mut seg, root, &abs, &rel);
                                    queue.pending.fetch_sub(1, AtomicOrdering::SeqCst);
                                }
                                None if queue.pending.load(AtomicOrdering::SeqCst) == 0 => {
                                    break;
                                }
                                None => std::thread::sleep(Duration::from_micros(200)),
                            }
                        }
                        seg.arena.shrink_to_fit();
                        seg.dirs.shrink_to_fit();
                        seg.entries.shrink_to_fit();
                        seg
                    })
                })
                .collect();
            handles
                .into_iter()
                .filter_map(|h| h.join().ok())
                .filter(|seg| !seg.entries.is_empty())
                .collect()
        });

        let mut counts = [0; Root::ALL.len()];
        for seg in &segments {
            for (total, n) in counts.iter_mut().zip(seg.counts) {
                *total += n;
            }
        }
        SearchIndex {
            output: output.to_path_buf(),
            segments,
            counts,
            built_at: Some(SystemTime::now()),
            build_ms: started.elapsed().as_millis(),
        }
    }

    fn hit(&self, seg: u32, idx: u32) -> Hit {
        let seg = &self.segments[seg as usize];
        let e = &seg.entries[idx as usize];
        let root = Root::ALL[e.root as usize];
        let name = seg.name_str(e);
        let path = format!("{}{}", seg.dir_str(seg.dir_of(e)), name);
        let size =
            std::fs::symlink_metadata(root.dir(&self.output).join(&path)).map_or(0, |m| m.len());
        Hit {
            root: root.name(),
            name: name.to_string(),
            path,
            size,
        }
    }

    /// Multi-token AND search, scanning the segments one after another on the calling
    /// thread. Returns `(total matches, requested page)`. The endpoint uses
    /// [`SearchIndex::search_parallel`].
    pub fn search(
        &self,
        query: &str,
        root: Option<Root>,
        ext: Option<&str>,
        offset: usize,
        limit: usize,
    ) -> (usize, Vec<Hit>) {
        let tokens = tokenize(query);
        if tokens.is_empty() {
            return (0, Vec::new());
        }
        let q = Prepared::new(tokens);
        let matches = (0..self.segments.len())
            .flat_map(|si| self.scan_segment(si, &q, root, ext))
            .collect();
        self.page(matches, offset, limit)
    }

    /// [`SearchIndex::search`] with one task per segment on Tokio's blocking pool (reused
    /// threads, no per-request spawning).
    pub async fn search_parallel(
        self: Arc<Self>,
        query: &str,
        root: Option<Root>,
        ext: Option<String>,
        offset: usize,
        limit: usize,
    ) -> Result<(usize, Vec<Hit>), tokio::task::JoinError> {
        let tokens = tokenize(query);
        if tokens.is_empty() {
            return Ok((0, Vec::new()));
        }
        let q = Arc::new(Prepared::new(tokens));
        let ext = ext.map(Arc::<str>::from);
        let tasks: Vec<_> = (0..self.segments.len())
            .map(|si| {
                let (index, q, ext) = (self.clone(), q.clone(), ext.clone());
                tokio::task::spawn_blocking(move || {
                    index.scan_segment(si, &q, root, ext.as_deref())
                })
            })
            .collect();
        let mut matches = Vec::new();
        for t in tasks {
            matches.extend(t.await?);
        }
        tokio::task::spawn_blocking(move || self.page(matches, offset, limit)).await
    }

    /// Matches of one segment as `(tier, path length, segment, entry)`.
    fn scan_segment(
        &self,
        si: usize,
        q: &Prepared,
        root: Option<Root>,
        ext: Option<&str>,
    ) -> Vec<(Tier, u32, u32, u32)> {
        self.segments[si]
            .scan(q, root, ext)
            .into_iter()
            .map(|(t, len, i)| (t, len, si as u32, i))
            .collect()
    }

    /// Rank `matches` and materialize `offset..offset + limit`.
    fn page(
        &self,
        mut matches: Vec<(Tier, u32, u32, u32)>,
        offset: usize,
        limit: usize,
    ) -> (usize, Vec<Hit>) {
        let total = matches.len();
        let end = offset.saturating_add(limit).min(total);
        if offset >= end {
            return (total, Vec::new());
        }
        let key = |m: &(Tier, u32, u32, u32)| {
            let seg = &self.segments[m.2 as usize];
            let e = &seg.entries[m.3 as usize];
            (m.0, m.1, seg.dir_str(seg.dir_of(e)), seg.name_str(e))
        };
        if end < total {
            matches.select_nth_unstable_by(end, |a, b| key(a).cmp(&key(b)));
            matches.truncate(end);
        }
        matches.sort_unstable_by(|a, b| key(a).cmp(&key(b)));
        let page = matches[offset..end]
            .iter()
            .map(|m| self.hit(m.2, m.3))
            .collect();
        (total, page)
    }
}

/// Current index plus a readiness signal for the first build.
pub struct IndexHandle {
    current: RwLock<Option<Arc<SearchIndex>>>,
    ready: watch::Sender<bool>,
    /// Concurrent searches; each one fans out over every core.
    permits: Arc<Semaphore>,
}

impl Default for IndexHandle {
    fn default() -> Self {
        Self::new()
    }
}

impl IndexHandle {
    pub fn new() -> Self {
        Self {
            current: RwLock::new(None),
            ready: watch::channel(false).0,
            permits: Arc::new(Semaphore::new(MAX_CONCURRENT)),
        }
    }

    pub fn get(&self) -> Option<Arc<SearchIndex>> {
        self.current
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }

    pub fn set(&self, index: SearchIndex) {
        *self.current.write().unwrap_or_else(|p| p.into_inner()) = Some(Arc::new(index));
        self.ready.send_replace(true);
    }

    /// Wait for the first build to finish.
    pub async fn wait(&self) -> Arc<SearchIndex> {
        let mut rx = self.ready.subscribe();
        loop {
            if let Some(index) = self.get() {
                return index;
            }
            if rx.changed().await.is_err() {
                // Sender lives as long as self; unreachable in practice.
                return Arc::new(SearchIndex::default());
            }
        }
    }
}

fn mtime(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

async fn rebuild(state: &SharedState, reason: &str) {
    let output = state.config.output.clone();
    match tokio::task::spawn_blocking(move || SearchIndex::build(&output)).await {
        Ok(index) => {
            tracing::info!(
                "search index built ({reason}): {} files, {:.1} MiB, {} ms",
                index.len(),
                index.heap_bytes() as f64 / (1024.0 * 1024.0),
                index.build_ms
            );
            state.index.set(index);
        }
        Err(e) => tracing::error!("search index build failed: {e}"),
    }
}

/// Build the index now, then rebuild whenever `update-log.jsonl` or `models/index.json`
/// changes (polled; the model exporter writes the latter without logging an update).
pub fn spawn_maintenance(state: SharedState) {
    tokio::spawn(async move {
        let watched = [
            state.config.update_log(),
            Root::Models.dir(&state.config.output).join("index.json"),
        ];
        let stamp = || {
            let w = watched.clone();
            async move {
                tokio::task::spawn_blocking(move || w.map(|p| mtime(&p)))
                    .await
                    .unwrap_or_default()
            }
        };
        let mut seen = stamp().await;
        rebuild(&state, "startup").await;
        let mut tick = tokio::time::interval(state.config.index_poll);
        tick.tick().await;
        loop {
            tick.tick().await;
            let now = stamp().await;
            if now != seen {
                let reason = if now[0] != seen[0] {
                    "update-log changed"
                } else {
                    "models index changed"
                };
                seen = now;
                rebuild(&state, reason).await;
            }
        }
    });
}

#[derive(Debug, Deserialize)]
pub struct SearchQuery {
    #[serde(default)]
    pub q: String,
    pub root: Option<String>,
    pub ext: Option<String>,
    #[serde(default)]
    pub offset: usize,
    pub limit: Option<usize>,
}

#[derive(Debug, Serialize)]
pub struct SearchResponse {
    pub query: String,
    pub total: usize,
    pub items: Vec<Hit>,
}

/// `GET /api/search?q=&root=&ext=&limit=&offset=`
pub async fn search(
    State(state): State<SharedState>,
    Query(q): Query<SearchQuery>,
) -> ApiResult<Json<SearchResponse>> {
    if tokenize(&q.q).is_empty() {
        return Err(ApiError::bad_request("query parameter q is required"));
    }
    let root = match q.root.as_deref().filter(|r| !r.is_empty()) {
        Some(r) => Some(Root::parse(r).map_err(|_| ApiError::bad_request("unknown root"))?),
        None => None,
    };
    let ext = q
        .ext
        .filter(|e| !e.is_empty())
        .map(|e| e.trim_start_matches('.').to_string());
    let limit = q.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
    if q.offset > MAX_OFFSET {
        return Err(ApiError::bad_request(format!(
            "offset must be at most {MAX_OFFSET}; narrow the query instead"
        )));
    }
    let index = state.index.wait().await;
    let permit = state
        .index
        .permits
        .clone()
        .acquire_owned()
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;
    // Detached so a client that disconnects keeps its permit until the scan ends.
    let query = q.q.clone();
    let offset = q.offset;
    let (total, items) = tokio::spawn(async move {
        let _permit = permit;
        index
            .search_parallel(&query, root, ext, offset, limit)
            .await
    })
    .await
    .map_err(|e| ApiError::internal(e.to_string()))?
    .map_err(|e| ApiError::internal(e.to_string()))?;
    Ok(Json(SearchResponse {
        query: q.q,
        total,
        items,
    }))
}
