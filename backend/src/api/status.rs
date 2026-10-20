//! `GET /api/status` — data version, schema version and per-root file counts.

use std::path::Path;

use axum::Json;
use axum::extract::State;
use serde::{Deserialize, Serialize};

use super::SharedState;
use super::error::{ApiError, ApiResult};
use super::paths::Root;

#[derive(Debug, Default, Deserialize)]
struct UpdateState {
    #[serde(default)]
    version: String,
    #[serde(default)]
    res_version: String,
    #[serde(default)]
    updated_at: String,
}

#[derive(Debug, Default, Deserialize)]
struct LogLine {
    #[serde(default)]
    at: String,
    #[serde(default)]
    version: String,
    #[serde(default)]
    res_version: String,
}

#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Versions {
    pub version: Option<String>,
    pub res_version: Option<String>,
    pub updated_at: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct Counts {
    pub textures: usize,
    pub meshes: usize,
    pub tables: usize,
    pub jsondata: usize,
    pub lua: usize,
    pub gamedata: usize,
    pub audio: usize,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IndexInfo {
    pub ready: bool,
    pub files: usize,
    pub built_at: Option<String>,
    pub build_ms: Option<u128>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StatusResponse {
    #[serde(flatten)]
    pub versions: Versions,
    pub schema_version: Option<String>,
    /// `null` until the first index build finishes.
    pub counts: Option<Counts>,
    pub index: IndexInfo,
}

fn non_empty(s: String) -> Option<String> {
    Some(s).filter(|s| !s.trim().is_empty())
}

/// Versions from `update-state.json`, falling back field-by-field to the last line of
/// `update-log.jsonl` (the state file leaves them empty until a run completes).
pub fn read_versions(state_file: &Path, update_log: &Path) -> Versions {
    let st: UpdateState = std::fs::read(state_file)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default();
    let last: LogLine = std::fs::read_to_string(update_log)
        .ok()
        .and_then(|s| {
            s.lines()
                .rev()
                .find(|l| !l.trim().is_empty())
                .and_then(|l| serde_json::from_str(l).ok())
        })
        .unwrap_or_default();
    Versions {
        version: non_empty(st.version).or_else(|| non_empty(last.version)),
        res_version: non_empty(st.res_version).or_else(|| non_empty(last.res_version)),
        updated_at: non_empty(st.updated_at).or_else(|| non_empty(last.at)),
    }
}

pub async fn status(State(state): State<SharedState>) -> ApiResult<Json<StatusResponse>> {
    let (state_file, log) = (state.config.state_file.clone(), state.config.update_log());
    let versions = tokio::task::spawn_blocking(move || read_versions(&state_file, &log))
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;
    let schema_version = backend_schema_version();
    let index = state.index.get();
    let counts = index.as_ref().map(|i| Counts {
        textures: i.count(Root::Textures),
        meshes: i.count(Root::Meshes),
        tables: i.count(Root::Tables),
        jsondata: i.count(Root::Jsondata),
        lua: i.count(Root::Lua),
        gamedata: i.count(Root::Gamedata),
        audio: i.count(Root::Audio),
    });
    let info = IndexInfo {
        ready: index.is_some(),
        files: index.as_ref().map_or(0, |i| i.len()),
        built_at: index
            .as_ref()
            .and_then(|i| i.built_at)
            .map(|t| chrono::DateTime::<chrono::Utc>::from(t).to_rfc3339()),
        build_ms: index.as_ref().map(|i| i.build_ms),
    };
    Ok(Json(StatusResponse {
        versions,
        schema_version,
        counts,
        index: info,
    }))
}

fn backend_schema_version() -> Option<String> {
    crate::extraction::memorypack::Schema::embedded()
        .ok()
        .and_then(|s| s.game_version())
        .map(str::to_string)
}
