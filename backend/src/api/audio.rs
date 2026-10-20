//! Audio: Wwise packages indexed and served as browser-playable clips.
//!
//! - `GET /api/audio`: packages, languages, counts, mapping coverage, tool availability.
//! - `GET /api/audio/entries?pack=&lang=&q=&kind=&offset=&limit=`: paged entries.
//! - `GET /audio/{pack}/{id}.{ogg|m4a|wav}`: on-demand transcode, cached under
//!   `<cache>/audio/`, served with Range/ETag (see [`transcode`]).
//! - `GET /api/voice/{charId}?lang=`: an operator's voice lines with per-language audio.
//!
//! Packages come from `output/decrypted/*Audio*/**/*.pck`, any `FLUORITE_AUDIO_DIRS`, and
//! the voice-language packages (`AudioChinese`, `AudioEnglish`, ...) that only exist on the
//! game CDN: [`remote`] discovers those and reads their entries with HTTP ranges.
//!
//! The index ([`crate::extraction::audio::AudioIndex`]) takes ~10 s to build, so it is
//! cached on disk keyed by a fingerprint of its inputs and rebuilt when they change.

pub mod remote;
pub mod transcode;

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use axum::Json;
use axum::extract::{Path as UrlPath, Query, State};
use axum::http::StatusCode;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::SharedState;
use super::catalog::tables::{CharacterTable, Table};
use super::catalog::text::Rich;
use super::catalog::{LangQuery, tables_dir};
use super::error::{ApiError, ApiResult};
use crate::extraction::audio::{
    AudioIndex, DialogInput, EntryKind, IndexBuilder, IndexEntry, IndexLookups, PackInput, PckFile,
    PckTable, dialog_key, parse_table, scan_names,
};

/// Bumped when the index format or build logic changes.
const INDEX_VERSION: u32 = 2;
/// How often a request may re-check the package fingerprint.
const RECHECK: Duration = Duration::from_secs(30);

// ── Languages ────────────────────────────────────────────────────────────

/// Package language name ↔ API code.
const LANGS: [(&str, &str); 5] = [
    ("chinese", "CN"),
    ("english", "EN"),
    ("japanese", "JP"),
    ("korean", "KR"),
    ("sfx", "SFX"),
];

pub fn lang_code(name: &str) -> String {
    LANGS
        .iter()
        .find(|(n, _)| n.eq_ignore_ascii_case(name))
        .map_or_else(|| name.to_ascii_uppercase(), |(_, c)| (*c).to_string())
}

/// Accepts `EN`, `en`, `english`; returns the package language name.
pub fn lang_name(code: &str) -> String {
    LANGS
        .iter()
        .find(|(n, c)| c.eq_ignore_ascii_case(code) || n.eq_ignore_ascii_case(code))
        .map_or_else(|| code.to_ascii_lowercase(), |(n, _)| (*n).to_string())
}

// ── Sources ──────────────────────────────────────────────────────────────

/// Where a package's entries are read from.
#[derive(Debug, Clone)]
pub enum Source {
    Local(Arc<PckFile>),
    /// Entries fetched with HTTP ranges at `base + entry.offset`.
    Remote {
        url: String,
        base: u64,
    },
}

/// The index plus what is needed to read entries.
pub struct Loaded {
    pub index: AudioIndex,
    pub lookups: IndexLookups,
    /// Aligned with `index.packs`.
    pub sources: Vec<Source>,
    pub fingerprint: String,
    pub built_ms: u128,
    pub from_cache: bool,
}

impl Loaded {
    pub fn entry(&self, pack: &str, id: u64) -> Option<(u32, &IndexEntry)> {
        let (p, _) = self.index.pack(pack)?;
        let i = *self.lookups.by_id.get(&(p, id))?;
        Some((p, &self.index.entries[i as usize]))
    }
}

/// A package discovered on disk.
struct LocalPack {
    group: String,
    path: PathBuf,
    size: u64,
    mtime: u128,
}

fn mtime_ns(meta: &std::fs::Metadata) -> u128 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_nanos())
}

fn find_pcks(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            find_pcks(&p, out);
        } else if p.extension().is_some_and(|x| x.eq_ignore_ascii_case("pck")) {
            out.push(p);
        }
    }
}

/// `<output>/decrypted/*Audio*/**/*.pck` and `FLUORITE_AUDIO_DIRS`, the group being the
/// top-level directory name.
fn local_packs(output: &Path, extra: &[PathBuf]) -> Vec<LocalPack> {
    let mut roots: Vec<(String, PathBuf)> = Vec::new();
    if let Ok(rd) = std::fs::read_dir(output.join("decrypted")) {
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if name.to_ascii_lowercase().contains("audio") && e.path().is_dir() {
                roots.push((name, e.path()));
            }
        }
    }
    for d in extra {
        let name = d
            .file_name()
            .map_or_else(|| "extra".into(), |n| n.to_string_lossy().into_owned());
        roots.push((name, d.clone()));
    }
    let mut packs = Vec::new();
    for (group, root) in roots {
        let mut files = Vec::new();
        find_pcks(&root, &mut files);
        files.sort();
        for path in files {
            if let Ok(meta) = std::fs::metadata(&path) {
                packs.push(LocalPack {
                    group: group.clone(),
                    size: meta.len(),
                    mtime: mtime_ns(&meta),
                    path,
                });
            }
        }
    }
    packs
}

fn pack_name(path: &Path) -> String {
    path.file_stem()
        .map_or_else(String::new, |s| s.to_string_lossy().into_owned())
}

// ── State ────────────────────────────────────────────────────────────────

pub struct AudioState {
    current: std::sync::RwLock<Option<Arc<Loaded>>>,
    checked: std::sync::Mutex<Option<Instant>>,
    build: tokio::sync::Mutex<()>,
    pub remote: remote::RemoteState,
    pub transcoder: transcode::Transcoder,
}

impl Default for AudioState {
    fn default() -> Self {
        Self::new()
    }
}

impl AudioState {
    pub fn new() -> Self {
        Self {
            current: std::sync::RwLock::new(None),
            checked: std::sync::Mutex::new(None),
            build: tokio::sync::Mutex::new(()),
            remote: remote::RemoteState::new(),
            transcoder: transcode::Transcoder::new(),
        }
    }

    /// The loaded index if one is ready, without building or re-checking.
    pub fn current(&self) -> Option<Arc<Loaded>> {
        self.current
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// Make the next request re-check the inputs instead of waiting out [`RECHECK`].
    pub fn invalidate(&self) {
        *self.checked.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }

    fn set(&self, loaded: Arc<Loaded>) {
        *self.current.write().unwrap_or_else(|e| e.into_inner()) = Some(loaded);
    }
}

pub fn audio_cache(state: &SharedState) -> PathBuf {
    state.config.cache_dir.join("audio")
}

/// Inputs of an index build and their fingerprint.
struct Plan {
    locals: Vec<LocalPack>,
    remotes: Vec<remote::RemotePack>,
    fingerprint: String,
}

fn plan(state: &SharedState) -> Plan {
    let output = &state.config.output;
    let locals = local_packs(output, &state.config.audio_dirs);
    let local_names: std::collections::HashSet<String> =
        locals.iter().map(|p| pack_name(&p.path)).collect();
    let remotes: Vec<remote::RemotePack> = state
        .audio
        .remote
        .packs()
        .into_iter()
        .filter(|r| !local_names.contains(&r.name))
        .collect();
    let dialog = tables_dir(state).join("AudioDialog.json");
    let dialog_stamp = std::fs::metadata(&dialog)
        .map(|m| (m.len(), mtime_ns(&m)))
        .unwrap_or((0, 0));
    let mut h = Fnv::new();
    h.add(&INDEX_VERSION.to_le_bytes());
    for p in &locals {
        h.add(p.path.to_string_lossy().as_bytes());
        h.add(&p.size.to_le_bytes());
        h.add(&p.mtime.to_le_bytes());
    }
    for r in &remotes {
        h.add(r.url.as_bytes());
        h.add(&r.base.to_le_bytes());
        h.add(&r.size.to_le_bytes());
    }
    h.add(&dialog_stamp.0.to_le_bytes());
    h.add(&dialog_stamp.1.to_le_bytes());
    Plan {
        locals,
        remotes,
        fingerprint: format!("{:016x}", h.0),
    }
}

/// FNV-1a, for cache keys.
pub struct Fnv(pub u64);

impl Default for Fnv {
    fn default() -> Self {
        Self::new()
    }
}

impl Fnv {
    pub fn new() -> Self {
        Self(0xcbf2_9ce4_8422_2325)
    }
    pub fn add(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.0 ^= u64::from(b);
            self.0 = self.0.wrapping_mul(0x0100_0000_01b3);
        }
        self.0 ^= 0xff;
        self.0 = self.0.wrapping_mul(0x0100_0000_01b3);
    }
}

/// The current index, (re)building it when its inputs changed.
pub async fn loaded(state: &SharedState) -> ApiResult<Arc<Loaded>> {
    let audio = &state.audio;
    if let Some(cur) = audio.current() {
        let mut checked = audio.checked.lock().unwrap_or_else(|e| e.into_inner());
        if checked.is_some_and(|t| t.elapsed() < RECHECK) {
            return Ok(cur);
        }
        *checked = Some(Instant::now());
    }
    let s = state.clone();
    let plan = blocking(move || plan(&s)).await?;
    if let Some(cur) = audio.current()
        && cur.fingerprint == plan.fingerprint
    {
        return Ok(cur);
    }
    let _guard = audio.build.lock().await;
    if let Some(cur) = audio.current()
        && cur.fingerprint == plan.fingerprint
    {
        return Ok(cur);
    }
    let s = state.clone();
    let loaded = Arc::new(blocking(move || build(&s, plan)).await??);
    tracing::info!(
        "audio index: {} packs, {} entries ({} in {} ms)",
        loaded.index.packs.len(),
        loaded.index.entries.len(),
        if loaded.from_cache { "cached" } else { "built" },
        loaded.built_ms
    );
    audio.set(loaded.clone());
    *audio.checked.lock().unwrap_or_else(|e| e.into_inner()) = Some(Instant::now());
    Ok(loaded)
}

async fn blocking<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> ApiResult<T> {
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| ApiError::internal(e.to_string()))
}

#[derive(Debug, Default, Deserialize)]
#[serde(transparent)]
pub struct AudioDialogTable(pub HashMap<String, DialogRow>);

impl Table for AudioDialogTable {
    const NAME: &'static str = "AudioDialog";
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct DialogRow {
    pub path: String,
    pub vo_type: String,
    pub speaker_channel: String,
    pub codec: i64,
    pub wav_duration: f32,
    #[serde(rename = "wavDurationEN")]
    pub wav_duration_en: f32,
    #[serde(rename = "wavDurationJP")]
    pub wav_duration_jp: f32,
    #[serde(rename = "wavDurationKR")]
    pub wav_duration_kr: f32,
}

fn dialog_inputs(tables: &Path) -> Vec<DialogInput> {
    let Ok(bytes) = std::fs::read(tables.join("AudioDialog.json")) else {
        return Vec::new();
    };
    let Ok(table) = serde_json::from_slice::<AudioDialogTable>(&bytes) else {
        tracing::warn!("AudioDialog.json: unexpected shape");
        return Vec::new();
    };
    let mut rows: Vec<DialogInput> = table
        .0
        .into_iter()
        .filter_map(|(k, r)| {
            Some(DialogInput {
                key: k.parse().ok()?,
                durations: vec![
                    ("chinese".into(), r.wav_duration),
                    ("english".into(), r.wav_duration_en),
                    ("japanese".into(), r.wav_duration_jp),
                    ("korean".into(), r.wav_duration_kr),
                ],
                path: r.path,
                vo_type: r.vo_type,
                speaker: r.speaker_channel,
                codec: r.codec,
            })
        })
        .collect();
    rows.sort_by_key(|r| r.key);
    rows
}

/// Texts whose identifiers name events and banks: non-i18n tables, Lua, JsonData.
fn scan_name_sources(
    output: &Path,
    wanted: &std::collections::HashSet<u32>,
) -> HashMap<u32, String> {
    let mut names = HashMap::new();
    let dec = output.join("decrypted");
    let mut stack = vec![
        dec.join("Table/TableCfg"),
        dec.join("Lua"),
        dec.join("JsonData"),
    ];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else {
            continue;
        };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
                continue;
            }
            let n = e.file_name();
            let n = n.to_string_lossy();
            if n.starts_with("I18n") || n.ends_with(".bytes") || n.ends_with(".mpk") {
                continue;
            }
            if let Ok(bytes) = std::fs::read(&p) {
                scan_names(&bytes, wanted, &mut names);
            }
        }
    }
    names
}

fn build(state: &SharedState, plan: Plan) -> ApiResult<Loaded> {
    let started = Instant::now();
    let dir = audio_cache(state);
    let cache_file = dir.join(format!("index-{}.json", plan.fingerprint));

    // Sources aligned with pack order: locals, then remotes.
    let mut sources = Vec::new();
    // (name, group, file, remote)
    let mut opened: Vec<(String, String, Arc<PckFile>, bool)> = Vec::new();
    for p in &plan.locals {
        match PckFile::open(&p.path) {
            Ok(f) => {
                let f = Arc::new(f);
                sources.push(Source::Local(f.clone()));
                opened.push((pack_name(&p.path), p.group.clone(), f, false));
            }
            Err(e) => tracing::warn!("audio: {}: {e}", p.path.display()),
        }
    }
    let mut remote_tables: Vec<(remote::RemotePack, PckTable)> = Vec::new();
    for r in &plan.remotes {
        if let Some(copy) = &r.local_copy {
            match PckFile::open(copy) {
                Ok(f) => {
                    let f = Arc::new(f);
                    sources.push(Source::Local(f.clone()));
                    opened.push((r.name.clone(), r.group.clone(), f, true));
                }
                Err(e) => tracing::warn!("audio: {}: {e}", copy.display()),
            }
        } else if let Some(hdr) = &r.header {
            match std::fs::read(hdr)
                .map_err(|e| e.to_string())
                .and_then(|b| parse_table(&b))
            {
                Ok(t) => remote_tables.push((r.clone(), t)),
                Err(e) => tracing::warn!("audio: {}: {e}", hdr.display()),
            }
        }
    }
    for (r, _) in &remote_tables {
        sources.push(Source::Remote {
            url: r.url.clone(),
            base: r.base,
        });
    }

    if let Ok(bytes) = std::fs::read(&cache_file)
        && let Ok(index) = serde_json::from_slice::<AudioIndex>(&bytes)
        && index.packs.len() == sources.len()
    {
        let lookups = index.lookups();
        return Ok(Loaded {
            index,
            lookups,
            sources,
            fingerprint: plan.fingerprint,
            built_ms: started.elapsed().as_millis(),
            from_cache: true,
        });
    }

    let mut b = IndexBuilder::new();
    for (name, group, f, remote) in &opened {
        b.add_pack(PackInput {
            name: name.clone(),
            group: group.clone(),
            size: f.len,
            table: &f.table,
            local: Some(f),
            remote: *remote,
        });
    }
    for (r, t) in &remote_tables {
        b.add_pack(PackInput {
            name: r.name.clone(),
            group: r.group.clone(),
            size: r.size,
            table: t,
            local: None,
            remote: true,
        });
    }
    b.add_dialog(dialog_inputs(&tables_dir(state)));
    let wanted = b.wanted_names();
    let names = scan_name_sources(&state.config.output, &wanted);
    let index = b.finish(&names);

    if let Ok(bytes) = serde_json::to_vec(&index) {
        let _ = std::fs::create_dir_all(&dir);
        // Old fingerprints are dead weight (tens of MB each).
        if let Ok(rd) = std::fs::read_dir(&dir) {
            for e in rd.flatten() {
                let n = e.file_name().to_string_lossy().into_owned();
                if n.starts_with("index-") && n.ends_with(".json") {
                    let _ = std::fs::remove_file(e.path());
                }
            }
        }
        if let Err(e) = transcode::write_atomic(&cache_file, &bytes) {
            tracing::warn!("audio index cache: {e}");
        }
    }
    let lookups = index.lookups();
    Ok(Loaded {
        index,
        lookups,
        sources,
        fingerprint: plan.fingerprint,
        built_ms: started.elapsed().as_millis(),
        from_cache: false,
    })
}

/// Discover remote packages (unless disabled) and warm the index, in the background.
pub fn spawn_maintenance(state: SharedState) {
    tokio::spawn(async move {
        // Serve from the last known remote list right away; the CDN check takes a while.
        if state.config.audio_remote {
            state.audio.remote.load_saved(&audio_cache(&state));
        }
        if let Err(e) = loaded(&state).await {
            tracing::warn!("audio index: {}", e.message);
        }
        if !state.config.audio_remote {
            return;
        }
        loop {
            remote::refresh(&state).await;
            state.audio.invalidate();
            if let Err(e) = loaded(&state).await {
                tracing::warn!("audio index: {}", e.message);
            }
            tokio::time::sleep(Duration::from_secs(6 * 3600)).await;
        }
    });
}

// ── JSON shapes ──────────────────────────────────────────────────────────

/// `{ogg, m4a}` URLs of a playable entry.
pub fn audio_urls(pack: &str, id: u64) -> Value {
    json!({
        "ogg": format!("/audio/{pack}/{id}.ogg"),
        "m4a": format!("/audio/{pack}/{id}.m4a"),
    })
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct EventOut<'a> {
    id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<&'a str>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct EntryOut<'a> {
    pack: &'a str,
    id: String,
    kind: &'static str,
    lang: String,
    size: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    codec: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    channels: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    duration: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    vo_type: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    speaker: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    path: Option<&'a str>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    events: Vec<EventOut<'a>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    audio: Option<Value>,
}

fn entry_out<'a>(l: &'a Loaded, e: &'a IndexEntry) -> EntryOut<'a> {
    let pack = l.index.packs[e.pack as usize].name.as_str();
    let dialog = e.dialog.map(|d| &l.index.dialog[d as usize]);
    EntryOut {
        pack,
        id: e.id.to_string(),
        kind: e.kind.as_str(),
        lang: lang_code(&e.lang),
        size: e.size,
        codec: e.codec.as_deref(),
        channels: e.channels,
        duration: e.duration.map(round3),
        name: e.name.as_deref(),
        vo_type: dialog.map(|d| d.vo_type.as_str()),
        speaker: dialog.map(|d| d.speaker.as_str()).filter(|s| !s.is_empty()),
        path: dialog.map(|d| d.path.as_str()),
        events: e
            .events
            .iter()
            .take(8)
            .map(|id| EventOut {
                id: id.to_string(),
                name: l.index.event_names.get(id).map(String::as_str),
            })
            .collect(),
        audio: (e.kind != EntryKind::Bank).then(|| audio_urls(pack, e.id)),
    }
}

// ── Handlers ─────────────────────────────────────────────────────────────

/// `GET /api/audio`
pub async fn summary(State(state): State<SharedState>) -> ApiResult<Json<Value>> {
    let l = loaded(&state).await?;
    let mut langs: BTreeMap<String, usize> = BTreeMap::new();
    for e in &l.index.entries {
        *langs.entry(e.lang.clone()).or_default() += 1;
    }
    let tools = state.audio.transcoder.tools();
    Ok(Json(json!({
        "packs": l.index.packs.iter().map(|p| json!({
            "name": p.name,
            "group": p.group,
            "size": p.size,
            "languages": p.languages.iter().map(|n| lang_code(n)).collect::<Vec<_>>(),
            "counts": { "bank": p.banks, "stream": p.streams, "external": p.externals,
                        "total": p.banks + p.streams + p.externals },
            "remote": p.remote,
        })).collect::<Vec<_>>(),
        "languages": langs.iter().map(|(n, c)| json!({
            "code": lang_code(n), "name": n, "entries": c,
        })).collect::<Vec<_>>(),
        "kinds": {
            "bank": l.index.entries.iter().filter(|e| e.kind == EntryKind::Bank).count(),
            "stream": l.index.entries.iter().filter(|e| e.kind == EntryKind::Stream).count(),
            "external": l.index.entries.iter().filter(|e| e.kind == EntryKind::External).count(),
        },
        "total": l.index.entries.len(),
        "coverage": l.index.coverage,
        "tools": { "vgmstream": tools.vgmstream.is_some(), "ffmpeg": tools.ffmpeg.is_some() },
        "remote": state.audio.remote.status(),
        "formats": ["ogg", "m4a", "wav"],
    })))
}

#[derive(Debug, Deserialize, Default)]
pub struct EntriesQuery {
    pub pack: Option<String>,
    pub lang: Option<String>,
    pub q: Option<String>,
    pub kind: Option<String>,
    pub offset: Option<usize>,
    pub limit: Option<usize>,
}

pub const DEFAULT_LIMIT: usize = 100;
pub const MAX_LIMIT: usize = 1000;

/// `GET /api/audio/entries`
pub async fn entries(
    State(state): State<SharedState>,
    Query(q): Query<EntriesQuery>,
) -> ApiResult<Json<Value>> {
    let l = loaded(&state).await?;
    let packs: Option<Vec<u32>> = match q.pack.as_deref().filter(|s| !s.is_empty()) {
        None => None,
        Some(list) => {
            let mut ids = Vec::new();
            for name in list.split(',') {
                let (i, _) = l
                    .index
                    .pack(name.trim())
                    .ok_or_else(|| ApiError::not_found(format!("no audio pack {name:?}")))?;
                ids.push(i);
            }
            Some(ids)
        }
    };
    let lang = q.lang.as_deref().filter(|s| !s.is_empty()).map(lang_name);
    let kind = match q.kind.as_deref().filter(|s| !s.is_empty()) {
        None => None,
        Some(k) => Some(
            EntryKind::parse(k)
                .ok_or_else(|| ApiError::bad_request("kind must be bank, stream or external"))?,
        ),
    };
    let needle =
        q.q.as_deref()
            .map(|s| s.trim().to_ascii_lowercase())
            .filter(|s| !s.is_empty());
    let limit = q.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
    let offset = q.offset.unwrap_or(0);

    let s = l.clone();
    let (total, page) = blocking(move || {
        let matches = |e: &IndexEntry| -> bool {
            if packs.as_ref().is_some_and(|p| !p.contains(&e.pack))
                || lang.as_ref().is_some_and(|x| *x != e.lang)
                || kind.is_some_and(|k| k != e.kind)
            {
                return false;
            }
            let Some(n) = &needle else {
                return true;
            };
            let id = e.id.to_string();
            if id == *n || format!("{:x}", e.id) == n.trim_start_matches("0x") {
                return true;
            }
            if e.name.as_ref().is_some_and(|x| x.contains(n.as_str())) {
                return true;
            }
            if let Some(d) = e.dialog
                && s.index.dialog[d as usize]
                    .path
                    .to_ascii_lowercase()
                    .contains(n.as_str())
            {
                return true;
            }
            e.events.iter().any(|ev| {
                s.index
                    .event_names
                    .get(ev)
                    .is_some_and(|x| x.contains(n.as_str()))
            })
        };
        let mut total = 0usize;
        let mut page = Vec::new();
        for (i, e) in s.index.entries.iter().enumerate() {
            if matches(e) {
                if total >= offset && page.len() < limit {
                    page.push(i);
                }
                total += 1;
            }
        }
        (total, page)
    })
    .await?;
    let items: Vec<EntryOut> = page
        .iter()
        .map(|&i| entry_out(&l, &l.index.entries[i]))
        .collect();
    Ok(Json(json!({
        "total": total,
        "offset": offset,
        "limit": limit,
        "entries": items,
    })))
}

/// Per-language audio of an `AudioDialog` key: `{CN: {ogg, m4a}, ...}`.
pub fn dialog_audio(l: &Loaded, key: i32) -> BTreeMap<String, Value> {
    let mut out = BTreeMap::new();
    for &i in l
        .lookups
        .by_dialog
        .get(&key)
        .map(Vec::as_slice)
        .unwrap_or(&[])
    {
        let e = &l.index.entries[i as usize];
        let pack = &l.index.packs[e.pack as usize].name;
        out.entry(lang_code(&e.lang))
            .or_insert_with(|| audio_urls(pack, e.id));
    }
    out
}

/// Attach `audio` to an operator detail's voice lines when the index is ready.
pub fn annotate_operator(state: &SharedState, detail: &mut Value) {
    let Some(l) = state.audio.current() else {
        return;
    };
    super::catalog::operators::attach_voice_audio(detail, |vo_id| {
        let a = dialog_audio(&l, dialog_key(vo_id));
        (!a.is_empty()).then(|| json!(a))
    });
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct VoiceLineOut {
    id: String,
    title: String,
    text: Rich,
    vo_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    vo_type: Option<String>,
    durations: BTreeMap<String, f64>,
    audio: BTreeMap<String, Value>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ExtraLineOut {
    name: String,
    vo_type: String,
    path: String,
    durations: BTreeMap<String, f64>,
    audio: BTreeMap<String, Value>,
}

/// Seconds rounded to milliseconds (f32 durations otherwise print as 3.79699993).
fn round3(v: f32) -> f64 {
    (f64::from(v) * 1000.0).round() / 1000.0
}

fn durations_of(l: &Loaded, key: i32) -> (Option<String>, BTreeMap<String, f64>) {
    let Some(&i) = l.lookups.by_dialog.get(&key).and_then(|v| v.first()) else {
        return (None, BTreeMap::new());
    };
    let Some(d) = l.index.entries[i as usize]
        .dialog
        .map(|d| &l.index.dialog[d as usize])
    else {
        return (None, BTreeMap::new());
    };
    let durations = d
        .durations
        .iter()
        .filter(|(_, v)| **v > 0.0)
        .map(|(k, v)| (lang_code(k), round3(*v)))
        .collect();
    (Some(d.vo_type.clone()), durations)
}

/// `GET /api/voice/{charId}?lang=&gender=`
pub async fn voice(
    State(state): State<SharedState>,
    UrlPath(char_id): UrlPath<String>,
    Query(q): Query<LangQuery>,
) -> ApiResult<Json<Value>> {
    let (lang, gender) = q.parse()?;
    let chars = state
        .catalog
        .tables
        .get::<CharacterTable>(&tables_dir(&state))
        .await?;
    let Some(c) = chars.get(&char_id) else {
        return Err(ApiError::not_found(format!("no operator {char_id:?}")));
    };
    let texts = super::catalog::texts(&state, &lang, gender).await?;
    let l = loaded(&state).await?;

    let mut profile_keys = std::collections::HashSet::new();
    let lines: Vec<VoiceLineOut> = c
        .profile_voice
        .iter()
        .map(|v| {
            let key = dialog_key(&v.vo_id);
            profile_keys.insert(key);
            let (vo_type, durations) = durations_of(&l, key);
            VoiceLineOut {
                id: v.id.clone(),
                title: texts.plain(v.voice_title),
                text: texts.rich(v.voice_desc, None),
                vo_id: v.vo_id.clone(),
                vo_type,
                durations,
                audio: dialog_audio(&l, key),
            }
        })
        .collect();

    // Other lines recorded for this operator (combat barks, interactions, ...).
    let needle = format!("/{}/", char_id.to_ascii_lowercase());
    let mut extra: Vec<ExtraLineOut> = l
        .index
        .dialog
        .iter()
        .filter(|d| !profile_keys.contains(&d.key) && d.path.to_ascii_lowercase().contains(&needle))
        .map(|d| ExtraLineOut {
            name: d.name.clone(),
            vo_type: d.vo_type.clone(),
            path: d.path.clone(),
            durations: durations_of(&l, d.key).1,
            audio: dialog_audio(&l, d.key),
        })
        .collect();
    extra.sort_by(|a, b| a.name.cmp(&b.name));

    let mut languages: Vec<String> = lines
        .iter()
        .flat_map(|x| x.audio.keys().cloned())
        .chain(extra.iter().flat_map(|x| x.audio.keys().cloned()))
        .collect();
    languages.sort();
    languages.dedup();
    let with_audio = lines.iter().filter(|x| !x.audio.is_empty()).count();
    let name = texts.plain(c.name);
    Ok(Json(json!({
        "charId": char_id,
        "name": if name.is_empty() { c.eng_name.clone() } else { name },
        "lang": lang,
        "languages": languages,
        "coverage": { "lines": lines.len(), "withAudio": with_audio },
        "lines": lines,
        "extra": extra,
    })))
}

/// `GET /audio/{pack}/{file}`: `{id}.{ogg|m4a|wav}`.
pub async fn clip(
    State(state): State<SharedState>,
    UrlPath((pack, file)): UrlPath<(String, String)>,
    headers: axum::http::HeaderMap,
) -> ApiResult<axum::response::Response> {
    let (id, ext) = file
        .rsplit_once('.')
        .ok_or_else(|| ApiError::bad_request("expected {id}.{ogg|m4a|wav}"))?;
    let format = transcode::Format::parse(ext)
        .ok_or_else(|| ApiError::bad_request("format must be ogg, m4a or wav"))?;
    let id: u64 = id
        .parse()
        .map_err(|_| ApiError::bad_request("id must be a decimal number"))?;
    let l = loaded(&state).await?;
    let (p, entry) = l
        .entry(&pack, id)
        .ok_or_else(|| ApiError::not_found(format!("no entry {id} in audio pack {pack:?}")))?;
    if entry.kind == EntryKind::Bank {
        return Err(ApiError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "unsupported",
            "soundbanks are not playable; their media are listed as separate entries",
        ));
    }
    let entry = entry.clone();
    let source = l.sources[p as usize].clone();
    transcode::serve(&state, &pack, &entry, source, format, &headers).await
}

/// Seconds since the epoch, for status payloads.
pub fn unix_secs(t: SystemTime) -> u64 {
    t.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}
