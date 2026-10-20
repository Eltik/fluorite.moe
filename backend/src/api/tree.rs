//! Directory listings (`GET /api/tree/{root}`) with a small mtime-validated cache.

use std::cmp::Ordering;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use axum::Json;
use axum::extract::{Path as UrlPath, Query, State};
use serde::{Deserialize, Serialize};

use super::SharedState;
use super::error::{ApiError, ApiResult};
use super::paths::{Root, is_hidden, resolve, sanitize};

const DEFAULT_LIMIT: usize = 200;
const MAX_LIMIT: usize = 1000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirEntry {
    pub name: String,
    pub is_dir: bool,
    pub size: u64,
}

/// Directory-first, then case-insensitive name order (ties broken by exact bytes).
pub fn entry_order(a: &DirEntry, b: &DirEntry) -> Ordering {
    b.is_dir
        .cmp(&a.is_dir)
        .then_with(|| {
            a.name
                .bytes()
                .map(|c| c.to_ascii_lowercase())
                .cmp(b.name.bytes().map(|c| c.to_ascii_lowercase()))
        })
        .then_with(|| a.name.cmp(&b.name))
}

/// Read `dir`, dropping hidden entries and symlinks, sorted with [`entry_order`].
pub fn read_listing(dir: &Path) -> std::io::Result<Vec<DirEntry>> {
    let mut entries = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let Ok(name) = entry.file_name().into_string() else {
            continue;
        };
        if is_hidden(&name) {
            continue;
        }
        let ft = entry.file_type()?;
        if ft.is_dir() {
            entries.push(DirEntry {
                name,
                is_dir: true,
                size: 0,
            });
        } else if ft.is_file() {
            let size = entry.metadata().map_or(0, |m| m.len());
            entries.push(DirEntry {
                name,
                is_dir: false,
                size,
            });
        }
    }
    entries.sort_by(entry_order);
    Ok(entries)
}

struct Cached {
    mtime: SystemTime,
    loaded: Instant,
    used: Instant,
    entries: Arc<Vec<DirEntry>>,
}

/// Cache of sorted listings keyed by absolute path. An entry is reused while the
/// directory's mtime is unchanged and it is younger than `ttl` (sizes of rewritten files
/// do not bump the directory mtime).
pub struct ListingCache {
    capacity: usize,
    ttl: Duration,
    map: Mutex<HashMap<PathBuf, Cached>>,
}

impl ListingCache {
    pub fn new(capacity: usize, ttl: Duration) -> Self {
        Self {
            capacity,
            ttl,
            map: Mutex::new(HashMap::new()),
        }
    }

    /// Blocking: stats `dir` and reads it when the cached copy is stale.
    pub fn get(&self, dir: &Path) -> std::io::Result<Arc<Vec<DirEntry>>> {
        let mtime = std::fs::metadata(dir)?.modified()?;
        let now = Instant::now();
        {
            let mut map = self.map.lock().unwrap_or_else(|p| p.into_inner());
            if let Some(c) = map.get_mut(dir)
                && c.mtime == mtime
                && now.duration_since(c.loaded) < self.ttl
            {
                c.used = now;
                return Ok(c.entries.clone());
            }
        }
        let entries = Arc::new(read_listing(dir)?);
        let mut map = self.map.lock().unwrap_or_else(|p| p.into_inner());
        if map.len() >= self.capacity && !map.contains_key(dir) {
            // Evict the least recently used entry; O(n) is fine at this size.
            if let Some(oldest) = map
                .iter()
                .min_by_key(|(_, c)| c.used)
                .map(|(k, _)| k.clone())
            {
                map.remove(&oldest);
            }
        }
        map.insert(
            dir.to_path_buf(),
            Cached {
                mtime,
                loaded: now,
                used: now,
                entries: entries.clone(),
            },
        );
        Ok(entries)
    }
}

#[derive(Debug, Deserialize)]
pub struct TreeQuery {
    #[serde(default)]
    pub path: String,
    #[serde(default)]
    pub offset: usize,
    pub limit: Option<usize>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TreeEntry {
    pub name: String,
    pub path: String,
    pub kind: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub children: Option<usize>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TreeResponse {
    pub root: &'static str,
    pub path: String,
    pub entries: Vec<TreeEntry>,
    pub total: usize,
    pub offset: usize,
    pub limit: usize,
}

/// `GET /api/tree/{root}?path=&offset=&limit=`
pub async fn tree(
    State(state): State<SharedState>,
    UrlPath(root): UrlPath<String>,
    Query(q): Query<TreeQuery>,
) -> ApiResult<Json<TreeResponse>> {
    let root = Root::parse(&root)?;
    let rel = sanitize(&q.path)?;
    let limit = q.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
    let offset = q.offset;
    let st = state.clone();

    tokio::task::spawn_blocking(move || {
        let dir = resolve(&root.dir(&st.config.output), &rel)?;
        if !dir.is_dir() {
            return Err(ApiError::bad_request("not a directory"));
        }
        let listing = st
            .listings
            .get(&dir)
            .map_err(|e| ApiError::io(e, "directory"))?;
        let entries = listing
            .iter()
            .skip(offset)
            .take(limit)
            .map(|e| {
                let path = if rel.is_empty() {
                    e.name.clone()
                } else {
                    format!("{rel}/{}", e.name)
                };
                if e.is_dir {
                    let children = st.listings.get(&dir.join(&e.name)).map(|l| l.len()).ok();
                    TreeEntry {
                        name: e.name.clone(),
                        path,
                        kind: "dir",
                        size: None,
                        children,
                    }
                } else {
                    TreeEntry {
                        name: e.name.clone(),
                        path,
                        kind: "file",
                        size: Some(e.size),
                        children: None,
                    }
                }
            })
            .collect();
        Ok(Json(TreeResponse {
            root: root.name(),
            path: rel,
            entries,
            total: listing.len(),
            offset,
            limit,
        }))
    })
    .await
    .map_err(|e| ApiError::internal(e.to_string()))?
}
