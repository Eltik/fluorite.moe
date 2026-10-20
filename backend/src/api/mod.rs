//! Read-only HTTP API over the extraction output directory (`fluorite-api`).
//!
//! The router is assembled in [`router`] from one module per endpoint group; [`AppState`]
//! holds everything handlers share (config, search index, listing/i18n caches, thumbnail
//! limiter). `catalog` is a stub until the data-mapping step lands.

pub mod catalog;
pub mod config;
pub mod error;
pub mod files;
pub mod i18n;
pub mod paths;
pub mod search;
pub mod status;
pub mod tables;
pub mod thumbs;
pub mod tree;

use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::http::{HeaderName, HeaderValue, Method, header};
use axum::routing::{get, post};
use tower_http::compression::CompressionLayer;
use tower_http::compression::predicate::{DefaultPredicate, Predicate};
use tower_http::cors::{Any, CorsLayer};
use tower_http::set_header::SetResponseHeaderLayer;
use tower_http::trace::{DefaultOnResponse, TraceLayer};
use tracing::Level;

pub use config::Config;
pub use error::ApiError;

/// Shared state handed to every handler.
pub struct AppState {
    pub config: Config,
    pub index: search::IndexHandle,
    pub listings: tree::ListingCache,
    pub i18n: i18n::I18nCache,
    pub thumbs: thumbs::ThumbLimiter,
}

pub type SharedState = Arc<AppState>;

impl AppState {
    /// Build state without starting any background work (the index starts empty).
    pub fn new(config: Config) -> SharedState {
        let threads = std::thread::available_parallelism().map_or(4, |n| n.get());
        Arc::new(Self {
            config,
            index: search::IndexHandle::new(),
            listings: tree::ListingCache::new(512, Duration::from_secs(30)),
            i18n: i18n::I18nCache::new(4),
            thumbs: thumbs::ThumbLimiter::new(threads),
        })
    }
}

/// The full application router (API + file/thumbnail serving) with middleware.
pub fn router(state: SharedState) -> Router {
    // JSON and other API bodies compress; ranged/partial responses never do.
    let compress =
        CompressionLayer::new()
            .gzip(true)
            .br(true)
            .compress_when(DefaultPredicate::new().and(
                |status: axum::http::StatusCode,
                 _: axum::http::Version,
                 headers: &axum::http::HeaderMap,
                 _: &axum::http::Extensions| {
                    status != axum::http::StatusCode::PARTIAL_CONTENT
                        && !headers.contains_key(header::CONTENT_RANGE)
                },
            ));

    let api = Router::new()
        .route("/status", get(status::status))
        .route("/tree/{root}", get(tree::tree))
        .route("/search", get(search::search))
        .route("/tables", get(tables::list))
        .route("/tables/{name}", get(tables::table))
        .route("/i18n/languages", get(i18n::languages))
        .route("/i18n/{lang}/resolve", post(i18n::resolve))
        .route("/i18n/{lang}/{key}", get(i18n::text))
        .nest("/catalog", catalog::router())
        .layer(compress);

    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods([Method::GET, Method::HEAD, Method::POST, Method::OPTIONS])
        .allow_headers([header::CONTENT_TYPE, header::RANGE, header::IF_NONE_MATCH])
        .expose_headers([
            header::CONTENT_RANGE,
            header::ACCEPT_RANGES,
            header::CONTENT_LENGTH,
            header::ETAG,
        ])
        .max_age(Duration::from_secs(86400));

    Router::new()
        .nest("/api", api)
        .route("/files/{root}/{*path}", get(files::file))
        .route("/thumb/{*path}", get(thumbs::thumb))
        .fallback(|| async { ApiError::not_found("no such route") })
        .with_state(state)
        .layer(SetResponseHeaderLayer::if_not_present(
            HeaderName::from_static("x-content-type-options"),
            HeaderValue::from_static("nosniff"),
        ))
        .layer(cors)
        .layer(
            TraceLayer::new_for_http()
                .make_span_with(|req: &axum::http::Request<_>| {
                    tracing::info_span!("http", method = %req.method(), uri = %req.uri())
                })
                .on_response(
                    DefaultOnResponse::new()
                        .level(Level::INFO)
                        .latency_unit(tower_http::LatencyUnit::Micros),
                ),
        )
}

/// Bind, build the index in the background, and serve until Ctrl-C.
pub async fn run(config: Config) -> std::io::Result<()> {
    let addr = config.addr;
    let state = AppState::new(config);
    search::spawn_maintenance(state.clone());

    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!(
        "listening on http://{} (output {}, cache {})",
        listener.local_addr()?,
        state.config.output.display(),
        state.config.cache_dir.display()
    );
    axum::serve(listener, router(state))
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
            tracing::info!("shutting down");
        })
        .await
}
