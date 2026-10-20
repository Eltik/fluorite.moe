//! `GET /api/tables` and `GET /api/tables/{name}` over the SparkBuffer JSON tables.

use axum::Json;
use axum::extract::{Path as UrlPath, State};
use axum::http::HeaderMap;
use axum::response::Response;
use serde::Serialize;

use super::SharedState;
use super::error::{ApiError, ApiResult};
use super::files::{DEFAULT_CACHE, serve_file};
use super::paths::{Root, resolve, sanitize};

#[derive(Debug, Serialize)]
pub struct TableInfo {
    pub name: String,
    pub size: u64,
}

#[derive(Debug, Serialize)]
pub struct TablesResponse {
    pub tables: Vec<TableInfo>,
}

/// `GET /api/tables` — every visible `*.json` table, by stem, sorted.
pub async fn list(State(state): State<SharedState>) -> ApiResult<Json<TablesResponse>> {
    let st = state.clone();
    tokio::task::spawn_blocking(move || {
        let dir = resolve(&Root::Tables.dir(&st.config.output), "")?;
        let listing = st
            .listings
            .get(&dir)
            .map_err(|e| ApiError::io(e, "tables"))?;
        let tables = listing
            .iter()
            .filter(|e| !e.is_dir)
            .filter_map(|e| {
                e.name.strip_suffix(".json").map(|stem| TableInfo {
                    name: stem.to_string(),
                    size: e.size,
                })
            })
            .collect();
        Ok(Json(TablesResponse { tables }))
    })
    .await
    .map_err(|e| ApiError::internal(e.to_string()))?
}

/// `GET /api/tables/{name}` — the table JSON, streamed (`name` with or without `.json`).
pub async fn table(
    State(state): State<SharedState>,
    UrlPath(name): UrlPath<String>,
    headers: HeaderMap,
) -> ApiResult<Response> {
    let name = sanitize(&name)?;
    if name.is_empty() || name.contains('/') {
        return Err(ApiError::bad_request("invalid table name"));
    }
    let file = if name.ends_with(".json") {
        name
    } else {
        format!("{name}.json")
    };
    let abs = resolve(&Root::Tables.dir(&state.config.output), &file)
        .map_err(|_| ApiError::not_found(format!("table {file:?} not found")))?;
    serve_file(&abs, &headers, DEFAULT_CACHE).await
}
