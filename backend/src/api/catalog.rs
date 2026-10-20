//! `/api/catalog/*` — placeholder until the data-mapping step defines the catalogs.
//!
//! Routes are registered so the frontend gets a stable 501 instead of a 404; replace the
//! handlers (keeping [`router`]'s signature) to implement them.

use axum::Router;
use axum::extract::Path;
use axum::routing::get;

use super::SharedState;
use super::error::ApiError;

pub fn router() -> Router<SharedState> {
    Router::new()
        .route("/{kind}", get(list))
        .route("/{kind}/{id}", get(detail))
}

async fn list(Path(kind): Path<String>) -> ApiError {
    ApiError::not_implemented(format!("catalog {kind:?} is not available yet"))
}

async fn detail(Path((kind, id)): Path<(String, String)>) -> ApiError {
    ApiError::not_implemented(format!("catalog {kind:?}/{id:?} is not available yet"))
}
