//! On-demand WEM → browser audio: `vgmstream-cli` decodes to WAV, `ffmpeg` encodes Opus in
//! Ogg (default, 64 kb/s, source channel layout) or AAC in MP4 (`m4a`, for Safari).
//!
//! Results are cached under `<cache>/audio/<pack>/<id>-<key>.<ext>` (the key covers the
//! entry's offset and size and the identity of the package it is read from, so an update
//! that changes the media changes the file), written atomically, produced behind a
//! semaphore, and deduplicated: the first request starts a detached task and every
//! concurrent request for the same file awaits its result, so a client that disconnects
//! mid-transcode neither aborts the work nor leaks its temp files. [`sweep_clips`] keeps
//! the cache under `FLUORITE_AUDIO_CACHE_MB`.
//!
//! Tools come from `FLUORITE_VGMSTREAM` / `FLUORITE_FFMPEG` or the usual install
//! locations; when one is missing the endpoint answers 501. Payloads vgmstream cannot
//! decode answer 422.

use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use axum::http::{HeaderMap, StatusCode};
use axum::response::Response;
use tokio::process::Command;
use tokio::sync::{Semaphore, watch};

use super::super::SharedState;
use super::super::error::{ApiError, ApiResult};
use super::super::files::serve_file;
use super::{Fnv, Source, audio_cache, remote};
use crate::extraction::audio::{IndexEntry, PckEntry};

const CLIP_CACHE: &str = "public, max-age=86400";
const TOOL_TIMEOUT: Duration = Duration::from_secs(120);
/// Temp files this old are left over from a crash and swept.
const STALE_TEMP: Duration = Duration::from_secs(3600);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Ogg,
    M4a,
    Wav,
}

impl Format {
    pub fn parse(ext: &str) -> Option<Self> {
        match ext.to_ascii_lowercase().as_str() {
            "ogg" | "opus" => Some(Format::Ogg),
            "m4a" | "aac" => Some(Format::M4a),
            "wav" => Some(Format::Wav),
            _ => None,
        }
    }

    pub fn ext(self) -> &'static str {
        match self {
            Format::Ogg => "ogg",
            Format::M4a => "m4a",
            Format::Wav => "wav",
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct Tools {
    pub vgmstream: Option<PathBuf>,
    pub ffmpeg: Option<PathBuf>,
}

fn find_tool(env: &str, names: &[&str]) -> Option<PathBuf> {
    if let Some(p) = std::env::var_os(env).map(PathBuf::from) {
        return p.is_file().then_some(p);
    }
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).collect())
        .unwrap_or_default();
    for d in ["/opt/homebrew/bin", "/usr/local/bin", "/usr/bin"] {
        dirs.push(PathBuf::from(d));
    }
    for d in dirs {
        for n in names {
            let p = d.join(n);
            if p.is_file() {
                return Some(p);
            }
        }
    }
    None
}

/// Result of a transcode job, `None` until it finishes.
type Outcome = watch::Receiver<Option<ApiResult<()>>>;

pub struct Transcoder {
    permits: Arc<Semaphore>,
    /// Running jobs by destination file.
    inflight: std::sync::Mutex<HashMap<PathBuf, Outcome>>,
    tools: OnceLock<Tools>,
}

impl Default for Transcoder {
    fn default() -> Self {
        Self::new()
    }
}

impl Transcoder {
    pub fn new() -> Self {
        let n = std::thread::available_parallelism().map_or(4, |n| n.get());
        Self {
            permits: Arc::new(Semaphore::new((n / 2).clamp(2, 8))),
            inflight: std::sync::Mutex::new(HashMap::new()),
            tools: OnceLock::new(),
        }
    }

    /// Tools, located once.
    pub fn tools(&self) -> &Tools {
        self.tools.get_or_init(|| Tools {
            vgmstream: find_tool("FLUORITE_VGMSTREAM", &["vgmstream-cli", "vgmstream_cli"]),
            ffmpeg: find_tool("FLUORITE_FFMPEG", &["ffmpeg"]),
        })
    }

    /// Number of jobs still running.
    pub fn inflight(&self) -> usize {
        self.inflight
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .len()
    }
}

/// Removes a job from `inflight` when its task ends, panics included.
struct InflightEntry {
    state: SharedState,
    dest: PathBuf,
}

impl Drop for InflightEntry {
    fn drop(&mut self) {
        self.state
            .audio
            .transcoder
            .inflight
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&self.dest);
    }
}

/// Temp files of one transcode, deleted (on the blocking pool when there is a runtime)
/// when dropped, so errors, timeouts and panics clean up too.
pub struct TempFiles(pub Vec<PathBuf>);

impl Drop for TempFiles {
    fn drop(&mut self) {
        let paths = std::mem::take(&mut self.0);
        let remove = move || {
            for p in &paths {
                let _ = std::fs::remove_file(p);
            }
        };
        match tokio::runtime::Handle::try_current() {
            Ok(rt) => drop(rt.spawn_blocking(remove)),
            Err(_) => remove(),
        }
    }
}

/// Write `bytes` to `dest` via a temp file + rename in the same directory.
pub fn write_atomic(dest: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let tmp = temp_beside(dest);
    let result = (|| {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        std::fs::rename(&tmp, dest)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

fn temp_beside(dest: &Path) -> PathBuf {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let dir = dest.parent().unwrap_or(Path::new("."));
    let _ = std::fs::create_dir_all(dir);
    dir.join(format!(
        ".{}.{}.{}.tmp",
        dest.file_name().and_then(|n| n.to_str()).unwrap_or("audio"),
        std::process::id(),
        SEQ.fetch_add(1, Ordering::Relaxed)
    ))
}

/// Identity of the package an entry is read from: size and mtime of a local file, the
/// chunk URL (named by the chunk's md5) and offset of a remote one.
pub async fn source_stamp(source: &Source) -> u64 {
    let mut h = Fnv::new();
    match source {
        Source::Local(f) => {
            let mtime = tokio::fs::metadata(&f.path)
                .await
                .ok()
                .and_then(|m| m.modified().ok())
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map_or(0, |d| d.as_nanos());
            h.add(&f.len.to_le_bytes());
            h.add(&mtime.to_le_bytes());
        }
        Source::Remote { url, base } => {
            h.add(url.as_bytes());
            h.add(&base.to_le_bytes());
        }
    }
    h.0
}

/// Cache file of an entry in `format`; `stamp` is the package's [`source_stamp`].
pub fn cache_path(
    cache: &Path,
    pack: &str,
    entry: &IndexEntry,
    stamp: u64,
    format: Format,
) -> PathBuf {
    let mut h = Fnv::new();
    h.add(&entry.offset.to_le_bytes());
    h.add(&entry.size.to_le_bytes());
    h.add(&stamp.to_le_bytes());
    cache
        .join(pack)
        .join(format!("{}-{:08x}.{}", entry.id, h.0 as u32, format.ext()))
}

async fn run(tool: &Path, args: &[&std::ffi::OsStr]) -> Result<(), String> {
    let mut cmd = Command::new(tool);
    cmd.args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    let child = cmd
        .spawn()
        .map_err(|e| format!("{}: {e}", tool.display()))?;
    let out = tokio::time::timeout(TOOL_TIMEOUT, child.wait_with_output())
        .await
        .map_err(|_| format!("{} timed out", tool.display()))?
        .map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(())
    } else {
        let err = String::from_utf8_lossy(&out.stderr);
        let tail: String = err.lines().rev().take(3).collect::<Vec<_>>().join(" | ");
        Err(format!(
            "{} exited with {}: {tail}",
            tool.file_name()
                .map_or_else(String::new, |n| n.to_string_lossy().into_owned()),
            out.status
        ))
    }
}

async fn read_source(state: &SharedState, source: &Source, e: &IndexEntry) -> ApiResult<Vec<u8>> {
    let pe = PckEntry {
        id: e.id,
        kind: e.kind,
        lang_id: 0,
        offset: e.offset,
        size: e.size,
    };
    match source {
        Source::Local(f) => {
            let f = f.clone();
            tokio::task::spawn_blocking(move || f.read_entry(&pe, None))
                .await
                .map_err(|e| ApiError::internal(e.to_string()))?
                .map_err(|e| ApiError::io(e, "audio package"))
        }
        Source::Remote { url, base } => {
            remote::read_entry(&state.audio.remote.http, url, *base, &pe)
                .await
                .map_err(|e| {
                    tracing::warn!("audio remote read: {e}");
                    ApiError::new(
                        StatusCode::BAD_GATEWAY,
                        "upstream",
                        "could not fetch audio from the game CDN",
                    )
                })
        }
    }
}

/// Produce `dest` from the entry (WEM → WAV → target format).
async fn produce(
    state: &SharedState,
    entry: &IndexEntry,
    source: &Source,
    format: Format,
    dest: &Path,
    tools: &Tools,
) -> ApiResult<()> {
    let vgm = tools
        .vgmstream
        .as_deref()
        .ok_or_else(|| ApiError::not_implemented("vgmstream-cli is not installed on the server"))?;
    let ffmpeg =
        match format {
            Format::Wav => None,
            _ => Some(tools.ffmpeg.as_deref().ok_or_else(|| {
                ApiError::not_implemented("ffmpeg is not installed on the server")
            })?),
        };
    let started = Instant::now();
    let data = read_source(state, source, entry).await?;
    let fetched = started.elapsed();
    let wem = temp_beside(dest).with_extension("wem");
    let wav = temp_beside(dest).with_extension("wav");
    let out = temp_beside(dest).with_extension(format.ext());
    let _temps = TempFiles(vec![wem.clone(), wav.clone(), out.clone()]);
    let result = async {
        tokio::fs::write(&wem, &data)
            .await
            .map_err(|e| ApiError::io(e, "audio temp file"))?;
        run(vgm, &["-o".as_ref(), wav.as_os_str(), wem.as_os_str()])
            .await
            .map_err(|e| {
                ApiError::new(
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "unsupported_codec",
                    format!(
                        "cannot decode {} media: {e}",
                        entry.codec.as_deref().unwrap_or("unknown")
                    ),
                )
            })?;
        let produced = match (format, ffmpeg) {
            (Format::Wav, _) | (_, None) => wav.clone(),
            (f, Some(ff)) => {
                let codec: &[&str] = if f == Format::Ogg {
                    &["-c:a", "libopus", "-b:a", "64k", "-vbr", "on", "-f", "ogg"]
                } else {
                    &[
                        "-c:a",
                        "aac",
                        "-b:a",
                        "128k",
                        "-movflags",
                        "+faststart",
                        "-f",
                        "ipod",
                    ]
                };
                let mut args: Vec<&std::ffi::OsStr> =
                    ["-hide_banner", "-loglevel", "error", "-y", "-i"]
                        .iter()
                        .map(|s| s.as_ref())
                        .collect();
                args.push(wav.as_os_str());
                args.push("-vn".as_ref());
                args.extend(codec.iter().map(|s| -> &std::ffi::OsStr { s.as_ref() }));
                args.push(out.as_os_str());
                run(ff, &args)
                    .await
                    .map_err(|e| ApiError::internal(format!("encode failed: {e}")))?;
                out.clone()
            }
        };
        tokio::fs::rename(&produced, dest)
            .await
            .map_err(|e| ApiError::io(e, "audio cache"))
    }
    .await;
    if result.is_ok() {
        tracing::info!(
            "transcoded {} ({} bytes {}) to {} in {} ms (read {} ms)",
            entry.id,
            entry.size,
            entry.codec.as_deref().unwrap_or("?"),
            format.ext(),
            started.elapsed().as_millis(),
            fetched.as_millis()
        );
    }
    result
}

/// Produce `dest` unless it appeared while waiting for a permit.
async fn job(
    state: &SharedState,
    entry: &IndexEntry,
    source: &Source,
    format: Format,
    dest: &Path,
) -> ApiResult<()> {
    let t = &state.audio.transcoder;
    let _permit = t
        .permits
        .acquire()
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;
    if tokio::fs::metadata(dest).await.is_ok() {
        return Ok(());
    }
    let tools = t.tools().clone();
    produce(state, entry, source, format, dest, &tools).await
}

/// Make sure `dest` exists: join the running job for it or start one. The job runs in its
/// own task, so it finishes (and is cached) even when every waiting client goes away.
pub async fn ensure(
    state: &SharedState,
    entry: &IndexEntry,
    source: Source,
    format: Format,
    dest: &Path,
) -> ApiResult<()> {
    let mut outcome = {
        let mut map = state
            .audio
            .transcoder
            .inflight
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        match map.get(dest) {
            Some(rx) => rx.clone(),
            None => {
                let (tx, rx) = watch::channel(None);
                map.insert(dest.to_path_buf(), rx.clone());
                let entry_guard = InflightEntry {
                    state: state.clone(),
                    dest: dest.to_path_buf(),
                };
                let entry = entry.clone();
                tokio::spawn(async move {
                    let g = entry_guard;
                    let result = job(&g.state, &entry, &source, format, &g.dest).await;
                    tx.send_replace(Some(result));
                });
                rx
            }
        }
    };
    let done = outcome
        .wait_for(Option::is_some)
        .await
        .map_err(|_| ApiError::internal("transcode task failed"))?;
    done.clone().unwrap_or(Ok(()))
}

/// Serve an entry in `format`, transcoding it first unless cached.
pub async fn serve(
    state: &SharedState,
    pack: &str,
    entry: &IndexEntry,
    source: Source,
    format: Format,
    headers: &HeaderMap,
) -> ApiResult<Response> {
    let stamp = source_stamp(&source).await;
    let dest = cache_path(&audio_cache(state), pack, entry, stamp, format);
    if tokio::fs::metadata(&dest).await.is_err() {
        ensure(state, entry, source, format, &dest).await?;
    }
    serve_file(&dest, headers, CLIP_CACHE).await
}

/// Delete the least recently used clips (later of atime and mtime) in the per-pack folders
/// of `audio_cache` until they total at most `max_bytes`, plus temp files older than
/// [`STALE_TEMP`]. `remote/` and the index files are left alone. Blocking; returns
/// `(files removed, bytes freed)`.
pub fn sweep_clips(audio_cache: &Path, max_bytes: u64) -> (usize, u64) {
    let now = SystemTime::now();
    let mut clips: Vec<(SystemTime, u64, PathBuf)> = Vec::new();
    let mut total = 0u64;
    let (mut removed, mut freed) = (0, 0);
    let Ok(packs) = std::fs::read_dir(audio_cache) else {
        return (0, 0);
    };
    for pack in packs.flatten() {
        if pack.file_name() == "remote" || !pack.file_type().is_ok_and(|t| t.is_dir()) {
            continue;
        }
        let Ok(files) = std::fs::read_dir(pack.path()) else {
            continue;
        };
        for f in files.flatten() {
            let Ok(meta) = f.metadata() else { continue };
            if !meta.is_file() {
                continue;
            }
            let mtime = meta.modified().unwrap_or(UNIX_EPOCH);
            let used = meta.accessed().map_or(mtime, |a| a.max(mtime));
            if f.file_name().to_string_lossy().starts_with('.') {
                // In-progress temp files; only crash leftovers are removed.
                if now.duration_since(mtime).is_ok_and(|age| age > STALE_TEMP)
                    && std::fs::remove_file(f.path()).is_ok()
                {
                    removed += 1;
                    freed += meta.len();
                }
                continue;
            }
            total += meta.len();
            clips.push((used, meta.len(), f.path()));
        }
    }
    if total > max_bytes {
        clips.sort_unstable();
        for (_, len, path) in clips {
            if total <= max_bytes {
                break;
            }
            if std::fs::remove_file(&path).is_ok() {
                total -= len;
                removed += 1;
                freed += len;
            }
        }
    }
    (removed, freed)
}
