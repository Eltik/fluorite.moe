//! Localized text lookup from `tables/I18nTextTable_<LANG>.json`.
//!
//! The converted SparkBuffer table is a flat JSON object `{ "<i64 key>": "<text>" }`.
//! Each language is parsed on first use (blocking thread) and kept in a small cache that is
//! invalidated when the file's size or mtime changes.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Instant, SystemTime};

use axum::Json;
use axum::extract::{Path as UrlPath, State};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::Mutex;

use super::SharedState;
use super::error::{ApiError, ApiResult};
use super::paths::Root;

const PREFIX: &str = "I18nTextTable_";
const SUFFIX: &str = ".json";
const MAX_RESOLVE_KEYS: usize = 10_000;

pub type TextMap = HashMap<String, String>;

/// Parse a converted I18n table. Values are strings; objects carrying a `text`/`value`
/// string (other converter shapes) are accepted too; anything else is skipped.
pub fn parse_table(bytes: &[u8]) -> Result<TextMap, String> {
    let root: serde_json::Map<String, Value> =
        serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
    let mut map = HashMap::with_capacity(root.len());
    for (key, value) in root {
        let text = match value {
            Value::String(s) => s,
            Value::Object(mut o) => match o.remove("text").or_else(|| o.remove("value")) {
                Some(Value::String(s)) => s,
                _ => continue,
            },
            _ => continue,
        };
        map.insert(key, text);
    }
    Ok(map)
}

/// Normalize a language code (`en` → `EN`); 400 for anything but 2–5 ASCII letters.
pub fn normalize_lang(lang: &str) -> ApiResult<String> {
    if (2..=5).contains(&lang.len()) && lang.bytes().all(|b| b.is_ascii_alphabetic()) {
        Ok(lang.to_ascii_uppercase())
    } else {
        Err(ApiError::bad_request("invalid language code"))
    }
}

/// Languages with an `I18nTextTable_<LANG>.json` in `tables_dir`, sorted.
pub fn available_languages(tables_dir: &Path) -> std::io::Result<Vec<String>> {
    let mut langs: Vec<String> = std::fs::read_dir(tables_dir)?
        .flatten()
        .filter_map(|e| e.file_name().into_string().ok())
        .filter_map(|n| {
            n.strip_prefix(PREFIX)?
                .strip_suffix(SUFFIX)
                .filter(|l| normalize_lang(l).is_ok())
                .map(str::to_string)
        })
        .collect();
    langs.sort();
    Ok(langs)
}

struct Loaded {
    stamp: (u64, SystemTime),
    used: Instant,
    texts: Arc<TextMap>,
}

/// Per-language parsed tables, at most `capacity` resident (LRU).
pub struct I18nCache {
    capacity: usize,
    map: Mutex<HashMap<String, Loaded>>,
}

impl I18nCache {
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity: capacity.max(1),
            map: Mutex::new(HashMap::new()),
        }
    }

    /// Table for `lang` (normalized), loading it if missing or stale.
    pub async fn get(&self, tables_dir: &Path, lang: &str) -> ApiResult<Arc<TextMap>> {
        let path: PathBuf = tables_dir.join(format!("{PREFIX}{lang}{SUFFIX}"));
        let meta = tokio::fs::metadata(&path)
            .await
            .map_err(|_| ApiError::not_found(format!("language {lang:?} not available")))?;
        let stamp = (
            meta.len(),
            meta.modified().unwrap_or(SystemTime::UNIX_EPOCH),
        );

        // Held across the load so concurrent requests for one language parse it once.
        let mut map = self.map.lock().await;
        if let Some(l) = map.get_mut(lang)
            && l.stamp == stamp
        {
            l.used = Instant::now();
            return Ok(l.texts.clone());
        }
        let started = Instant::now();
        let texts = tokio::task::spawn_blocking(move || {
            let bytes = std::fs::read(&path).map_err(|e| ApiError::io(e, "i18n table"))?;
            parse_table(&bytes).map_err(|e| ApiError::internal(format!("i18n table: {e}")))
        })
        .await
        .map_err(|e| ApiError::internal(e.to_string()))??;
        let texts = Arc::new(texts);
        tracing::info!(
            "loaded i18n {lang}: {} keys in {} ms",
            texts.len(),
            started.elapsed().as_millis()
        );
        if map.len() >= self.capacity
            && !map.contains_key(lang)
            && let Some(oldest) = map
                .iter()
                .min_by_key(|(_, l)| l.used)
                .map(|(k, _)| k.clone())
        {
            map.remove(&oldest);
        }
        map.insert(
            lang.to_string(),
            Loaded {
                stamp,
                used: Instant::now(),
                texts: texts.clone(),
            },
        );
        Ok(texts)
    }
}

#[derive(Debug, Serialize)]
pub struct LanguagesResponse {
    pub languages: Vec<String>,
}

/// `GET /api/i18n/languages`
pub async fn languages(State(state): State<SharedState>) -> ApiResult<Json<LanguagesResponse>> {
    let dir = Root::Tables.dir(&state.config.output);
    let languages = tokio::task::spawn_blocking(move || available_languages(&dir))
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?
        .map_err(|e| ApiError::io(e, "tables directory"))?;
    Ok(Json(LanguagesResponse { languages }))
}

#[derive(Debug, Serialize)]
pub struct TextResponse {
    pub lang: String,
    pub key: String,
    pub text: String,
}

/// `GET /api/i18n/{lang}/{key}`
pub async fn text(
    State(state): State<SharedState>,
    UrlPath((lang, key)): UrlPath<(String, String)>,
) -> ApiResult<Json<TextResponse>> {
    let lang = normalize_lang(&lang)?;
    let texts = state
        .i18n
        .get(&Root::Tables.dir(&state.config.output), &lang)
        .await?;
    let text = texts
        .get(&key)
        .cloned()
        .ok_or_else(|| ApiError::not_found(format!("no {lang} text for key {key:?}")))?;
    Ok(Json(TextResponse { lang, key, text }))
}

#[derive(Debug, Deserialize)]
pub struct ResolveRequest {
    pub keys: Vec<Value>,
}

#[derive(Debug, Serialize)]
pub struct ResolveResponse {
    pub texts: HashMap<String, String>,
}

/// `POST /api/i18n/{lang}/resolve` with `{ keys: [...] }` (strings or integers). Keys with
/// no text are omitted from `texts`.
pub async fn resolve(
    State(state): State<SharedState>,
    UrlPath(lang): UrlPath<String>,
    Json(req): Json<ResolveRequest>,
) -> ApiResult<Json<ResolveResponse>> {
    let lang = normalize_lang(&lang)?;
    if req.keys.len() > MAX_RESOLVE_KEYS {
        return Err(ApiError::bad_request(format!(
            "at most {MAX_RESOLVE_KEYS} keys per request"
        )));
    }
    let table = state
        .i18n
        .get(&Root::Tables.dir(&state.config.output), &lang)
        .await?;
    let texts = req
        .keys
        .iter()
        .filter_map(|k| match k {
            Value::String(s) => Some(s.clone()),
            Value::Number(n) => Some(n.to_string()),
            _ => None,
        })
        .filter_map(|k| table.get(&k).map(|t| (k, t.clone())))
        .collect();
    Ok(Json(ResolveResponse { texts }))
}
