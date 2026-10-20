//! Voice-language packages read straight from the game CDN.
//!
//! `AudioChinese`, `AudioEnglish`, `AudioJapanese` and `AudioKorean` are VFS groups that
//! are not in the base game archive: the launcher's patch index lists their chunks, and
//! each group's `.blc` catalog sits at `<main resource>/VFS/<hash>/<hash>.blc`. Their
//! `.pck` files are stored unencrypted at offset 0 of a chunk, so a package entry is one
//! HTTP range request away (then the usual per-entry decipher).
//!
//! Once `endfield update` has extracted a group into `output/decrypted/<group>/`, its hash is
//! skipped here and the local packages are served instead (see `local_group_hashes`).
//!
//! Discovery keeps small packages (the `*_banks.pck`, ~2 MB) as local copies and only the
//! header of the 1.6 GB stream packages, all under `<cache>/audio/remote/`. The result is
//! saved to `<cache>/audio/remote.json` so a restart works before the CDN answers. Copies
//! and headers of chunks that are no longer listed are deleted after a complete refresh.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::super::SharedState;
use super::{audio_cache, transcode::write_atomic, unix_secs};
use crate::core::endfield::config::{fetch_patch_index, resolve_game_config};
use crate::extraction::audio::{PckEntry, decipher_entry, header_len};
use crate::extraction::blc::parse_blc;
use crate::extraction::pipeline::{
    cdn_blc_url, cdn_vfs_url, default_resolve_params, main_resource_path, patch_directories,
};

/// Packages up to this size are copied whole; larger ones are read by range.
const COPY_LIMIT: u64 = 64 << 20;

/// A package on the CDN.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RemotePack {
    /// File stem, e.g. `default_english_stream_0`.
    pub name: String,
    /// VFS group, e.g. `AudioEnglish`.
    pub group: String,
    /// Chunk URL.
    pub url: String,
    /// Offset of the package inside the chunk.
    pub base: u64,
    pub size: u64,
    /// Whole package copied locally (small packages).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local_copy: Option<PathBuf>,
    /// Cached header bytes (large packages).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub header: Option<PathBuf>,
}

#[derive(Debug, Default, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct Status {
    refreshed_at: Option<u64>,
    error: Option<String>,
    groups: Vec<String>,
}

pub struct RemoteState {
    packs: std::sync::RwLock<Vec<RemotePack>>,
    status: std::sync::Mutex<Status>,
    pub http: reqwest::Client,
}

impl Default for RemoteState {
    fn default() -> Self {
        Self::new()
    }
}

impl RemoteState {
    pub fn new() -> Self {
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(15))
            .timeout(Duration::from_secs(120))
            .build()
            .unwrap_or_default();
        Self {
            packs: std::sync::RwLock::new(Vec::new()),
            status: std::sync::Mutex::new(Status::default()),
            http,
        }
    }

    pub fn packs(&self) -> Vec<RemotePack> {
        self.packs.read().unwrap_or_else(|e| e.into_inner()).clone()
    }

    fn set(&self, packs: Vec<RemotePack>) {
        *self.packs.write().unwrap_or_else(|e| e.into_inner()) = packs;
    }

    pub fn status(&self) -> Value {
        let s = self
            .status
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        json!({
            "packs": self.packs.read().unwrap_or_else(|e| e.into_inner()).len(),
            "groups": s.groups,
            "refreshedAt": s.refreshed_at,
            "error": s.error,
        })
    }

    /// Load the list saved by the last refresh (only packages whose files still exist).
    pub fn load_saved(&self, cache: &Path) {
        let Ok(bytes) = std::fs::read(cache.join("remote.json")) else {
            return;
        };
        let Ok(packs) = serde_json::from_slice::<Vec<RemotePack>>(&bytes) else {
            return;
        };
        let packs: Vec<RemotePack> = packs
            .into_iter()
            .filter(|p| {
                p.local_copy
                    .as_ref()
                    .or(p.header.as_ref())
                    .is_some_and(|f| f.is_file())
            })
            .collect();
        let mut groups: Vec<String> = packs.iter().map(|p| p.group.clone()).collect();
        groups.sort();
        groups.dedup();
        self.status.lock().unwrap_or_else(|e| e.into_inner()).groups = groups;
        self.set(packs);
    }
}

/// GET `len` bytes of `url` from `start`. A server that ignores the range answers 200 with
/// the whole chunk (1.6 GB for stream packages): that is only accepted from offset 0, and
/// the body is read no further than `len` bytes, then the connection is dropped.
pub async fn fetch_range(
    http: &reqwest::Client,
    url: &str,
    start: u64,
    len: u64,
) -> Result<Vec<u8>, String> {
    if len == 0 {
        return Ok(Vec::new());
    }
    let mut resp = http
        .get(url)
        .header(
            reqwest::header::RANGE,
            format!("bytes={start}-{}", start + len - 1),
        )
        .send()
        .await
        .map_err(|e| format!("{url}: {e}"))?;
    let status = resp.status();
    if !(status == reqwest::StatusCode::PARTIAL_CONTENT
        || (status == reqwest::StatusCode::OK && start == 0))
    {
        return Err(format!("{url}: HTTP {status}"));
    }
    let mut buf = Vec::with_capacity(len.min(COPY_LIMIT) as usize);
    while (buf.len() as u64) < len {
        match resp.chunk().await.map_err(|e| format!("{url}: {e}"))? {
            Some(chunk) => buf.extend_from_slice(&chunk),
            None => break,
        }
    }
    if (buf.len() as u64) < len {
        return Err(format!("{url}: short read ({} of {len} bytes)", buf.len()));
    }
    buf.truncate(len as usize);
    Ok(buf)
}

/// A CDN-supplied chunk md5, checked before it goes into a URL or file name.
pub fn is_md5(s: &str) -> bool {
    s.len() == 32 && s.bytes().all(|b| b.is_ascii_hexdigit())
}

/// A CDN-supplied directory or package name usable as one URL / file-name component.
pub fn is_safe_component(s: &str) -> bool {
    !s.is_empty() && !s.contains(['/', '\\']) && !s.contains("..")
}

/// Delete the package copies and headers in `dir` that are not named in `keep`. Blocking;
/// returns how many were removed.
pub fn prune_cached(dir: &Path, keep: &HashSet<String>) -> usize {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return 0;
    };
    let mut removed = 0;
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let cached = name.ends_with(".pck") || name.ends_with(".hdr");
        if cached
            && !name.starts_with('.')
            && !keep.contains(&name)
            && std::fs::remove_file(e.path()).is_ok()
        {
            removed += 1;
        }
    }
    removed
}

/// [`write_atomic`] on the blocking pool.
async fn write_blocking(dest: PathBuf, bytes: Vec<u8>) -> Result<(), String> {
    tokio::task::spawn_blocking(move || write_atomic(&dest, &bytes))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())
}

async fn is_file(p: &Path) -> bool {
    tokio::fs::metadata(p).await.is_ok_and(|m| m.is_file())
}

/// Fetch and decipher one entry of a remote package.
pub async fn read_entry(
    http: &reqwest::Client,
    url: &str,
    base: u64,
    entry: &PckEntry,
) -> Result<Vec<u8>, String> {
    let mut data = fetch_range(http, url, base + entry.offset, u64::from(entry.size)).await?;
    decipher_entry(&mut data, entry);
    Ok(data)
}

/// Hashes of the VFS groups already extracted locally (from `blc_metadata.txt`).
fn local_group_hashes(output: &Path) -> HashSet<String> {
    let mut out = HashSet::new();
    if let Ok(rd) = std::fs::read_dir(output.join("decrypted")) {
        for e in rd.flatten() {
            if let Ok(meta) = std::fs::read_to_string(e.path().join("blc_metadata.txt"))
                && let Some(first) = meta.lines().next()
                && let Some((_, rest)) = first.rsplit_once('(')
            {
                out.insert(rest.trim_end_matches(')').trim().to_ascii_uppercase());
            }
        }
    }
    out
}

async fn discover(state: &SharedState) -> Result<Vec<RemotePack>, String> {
    let http = &state.audio.remote.http;
    let config = resolve_game_config(http, &default_resolve_params())
        .await
        .map_err(|e| format!("launcher: {e}"))?;
    let main = main_resource_path(&config)
        .ok_or("launcher lists no main resource")?
        .to_string();
    let patch = fetch_patch_index(http, &main)
        .await
        .map_err(|e| format!("patch index: {e}"))?;
    let output = state.config.output.clone();
    let local = tokio::task::spawn_blocking(move || local_group_hashes(&output))
        .await
        .map_err(|e| e.to_string())?;
    let dirs: Vec<String> = patch_directories(&patch)
        .into_iter()
        .filter(|d| !local.contains(d))
        .filter(|d| {
            let ok = is_safe_component(d);
            if !ok {
                tracing::warn!("audio remote: invalid directory {d:?}, skipped");
            }
            ok
        })
        .collect();

    let cache = audio_cache(state).join("remote");
    tokio::fs::create_dir_all(&cache)
        .await
        .map_err(|e| e.to_string())?;
    let mut packs = Vec::new();
    // File names still in use; pruning waits for a refresh that saw every catalog.
    let mut keep = HashSet::new();
    let mut complete = true;
    for dir in dirs {
        let blc_url = cdn_blc_url(&main, &dir);
        let raw = match http.get(&blc_url).send().await {
            Ok(r) if r.status().is_success() => r.bytes().await.map_err(|e| e.to_string())?,
            _ => {
                complete = false;
                continue;
            }
        };
        let Ok(blc) = parse_blc(&raw) else {
            continue;
        };
        if !blc.group_cfg_name.starts_with("Audio") {
            continue;
        }
        for f in &blc.files {
            if !f.file_name.to_ascii_lowercase().ends_with(".pck") {
                continue;
            }
            if f.use_encrypt || f.offset < 0 || f.len <= 0 {
                tracing::warn!("audio remote: {} is encrypted, skipped", f.file_name);
                continue;
            }
            let name = Path::new(&f.file_name.replace('\\', "/"))
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default();
            if !is_md5(&f.chunk_md5) || !is_safe_component(&name) {
                tracing::warn!(
                    "audio remote: invalid entry {:?} (chunk {:?}), skipped",
                    f.file_name,
                    f.chunk_md5
                );
                continue;
            }
            let url = cdn_vfs_url(
                &main,
                &format!("{dir}/{}.chk", f.chunk_md5.to_ascii_uppercase()),
            );
            let (base, size) = (f.offset as u64, f.len as u64);
            let tag = format!("{name}-{}", f.chunk_md5.to_ascii_lowercase());
            let mut pack = RemotePack {
                name,
                group: blc.group_cfg_name.clone(),
                url: url.clone(),
                base,
                size,
                local_copy: None,
                header: None,
            };
            if size <= COPY_LIMIT {
                let dest = cache.join(format!("{tag}.pck"));
                if !is_file(&dest).await {
                    let bytes = fetch_range(http, &url, base, size).await?;
                    write_blocking(dest.clone(), bytes).await?;
                }
                keep.insert(format!("{tag}.pck"));
                pack.local_copy = Some(dest);
            } else {
                let dest = cache.join(format!("{tag}.hdr"));
                if !is_file(&dest).await {
                    let first = fetch_range(http, &url, base, 8).await?;
                    let len = header_len(&first)?;
                    let head = fetch_range(http, &url, base, len).await?;
                    write_blocking(dest.clone(), head).await?;
                }
                keep.insert(format!("{tag}.hdr"));
                pack.header = Some(dest);
            }
            packs.push(pack);
        }
    }
    if complete {
        let removed = tokio::task::spawn_blocking(move || prune_cached(&cache, &keep))
            .await
            .unwrap_or(0);
        if removed > 0 {
            tracing::info!("audio remote: removed {removed} superseded package files");
        }
    }
    Ok(packs)
}

/// Re-discover the remote packages; keeps the previous list when the CDN fails.
pub async fn refresh(state: &SharedState) {
    let remote = &state.audio.remote;
    match discover(state).await {
        Ok(packs) => {
            let mut groups: Vec<String> = packs.iter().map(|p| p.group.clone()).collect();
            groups.sort();
            groups.dedup();
            tracing::info!(
                "audio remote: {} packages in {}",
                packs.len(),
                groups.join(", ")
            );
            if let Ok(bytes) = serde_json::to_vec_pretty(&packs) {
                let _ = write_blocking(audio_cache(state).join("remote.json"), bytes).await;
            }
            let mut s = remote.status.lock().unwrap_or_else(|e| e.into_inner());
            s.refreshed_at = Some(unix_secs(SystemTime::now()));
            s.error = None;
            s.groups = groups;
            drop(s);
            remote.set(packs);
        }
        Err(e) => {
            tracing::warn!("audio remote discovery failed: {e}");
            remote
                .status
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .error = Some(e);
        }
    }
}
