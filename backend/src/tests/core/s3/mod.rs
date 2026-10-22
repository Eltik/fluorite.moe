//! The tests of the object storage layer: the client, the settings, the providers and
//! the publisher.
//!
//! The mock service is the shared part: it stores what is uploaded, serves it back and
//! lists it, so the tests of the client and of the publisher speak the same protocol.

mod client;
mod provider;
mod publish;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use axum::Router;
use axum::body::Body;
use axum::extract::State;
use axum::http::{Method, Request, StatusCode};
use axum::response::{IntoResponse, Response};

/// The environment of a test: a map of variables.
pub fn env(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
    let map: HashMap<&str, &str> = pairs.iter().copied().collect();
    move |name: &str| map.get(name).map(|v| v.to_string())
}

/// A complete S3 configuration, with the variables that are not given left out.
pub fn base(pairs: &[(&'static str, &'static str)]) -> Vec<(&'static str, &'static str)> {
    let mut all = vec![
        ("S3_ENDPOINT", "https://example.r2.cloudflarestorage.com"),
        ("S3_BUCKET", "fluorite"),
        ("S3_ACCESS_KEY_ID", "AKID"),
        ("S3_SECRET_ACCESS_KEY", "secret"),
    ];
    all.extend_from_slice(pairs);
    all
}

/// An empty response with `status`.
fn empty(status: StatusCode) -> Response {
    Response::builder()
        .status(status)
        .body(Body::empty())
        .unwrap()
}

/// The `ETag` header of an object, quoted the way the service writes it.
fn etag_header(bytes: &[u8]) -> String {
    format!("\"{}\"", etag(bytes))
}

/// The objects the mock service holds, keyed by the object key.
pub type Store = Arc<Mutex<HashMap<String, Vec<u8>>>>;

/// The requests the mock service received: the method and the headers.
pub type Log = Arc<Mutex<Vec<(String, HashMap<String, String>)>>>;

/// A mock S3 endpoint: it stores what is uploaded and serves it back.
///
/// The bucket is the first path segment, like in a path-style endpoint
/// (`/<bucket>/<key>`), and the listing answers `ListObjectsV2`.
pub async fn mock_endpoint() -> (String, Store, Log) {
    let store: Store = Arc::new(Mutex::new(HashMap::new()));
    let log: Log = Arc::new(Mutex::new(Vec::new()));
    let app = Router::new()
        .fallback(handle)
        .with_state((store.clone(), log.clone()));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    (format!("http://{addr}"), store, log)
}

/// The key of the request: the path below the bucket.
fn key_of(req: &Request<Body>) -> String {
    let path = req.uri().path();
    path.trim_start_matches('/')
        .split_once('/')
        .map(|(_, key)| key.to_string())
        .unwrap_or_default()
}

/// The mock service: it stores, serves, lists and deletes.
async fn handle(State((store, log)): State<(Store, Log)>, req: Request<Body>) -> Response {
    let key = key_of(&req);
    let query = req.uri().query().unwrap_or_default().to_string();
    log.lock().unwrap().push((
        req.method().to_string(),
        req.headers()
            .iter()
            .map(|(name, value)| {
                (
                    name.as_str().to_string(),
                    value.to_str().unwrap_or_default().to_string(),
                )
            })
            .collect(),
    ));
    if query.contains("list-type=2") {
        return list(&store, &query);
    }
    match *req.method() {
        Method::PUT => {
            let bytes = axum::body::to_bytes(req.into_body(), usize::MAX)
                .await
                .unwrap_or_default();
            store.lock().unwrap().insert(key, bytes.to_vec());
            Response::builder()
                .status(StatusCode::OK)
                .header(axum::http::header::ETAG, etag_header(&bytes))
                .body(Body::empty())
                .unwrap()
        }
        Method::DELETE => {
            store.lock().unwrap().remove(&key);
            empty(StatusCode::NO_CONTENT)
        }
        Method::HEAD => match store.lock().unwrap().get(&key) {
            Some(bytes) => Response::builder()
                .status(StatusCode::OK)
                .header(axum::http::header::CONTENT_LENGTH, bytes.len())
                .header(axum::http::header::ETAG, etag_header(bytes))
                .header(
                    axum::http::header::LAST_MODIFIED,
                    chrono::Utc::now()
                        .format("%a, %d %b %Y %H:%M:%S GMT")
                        .to_string(),
                )
                .body(Body::empty())
                .unwrap(),
            None => empty(StatusCode::NOT_FOUND),
        },
        Method::GET => match store.lock().unwrap().get(&key) {
            Some(bytes) => {
                let range = req
                    .headers()
                    .get(axum::http::header::RANGE)
                    .and_then(|v| v.to_str().ok())
                    .map(str::to_string);
                match range {
                    Some(value) => {
                        let spec = value.trim().strip_prefix("bytes=").unwrap_or_default();
                        let (start, end) = spec.split_once('-').unwrap();
                        let start: usize = start.parse().unwrap();
                        let end: usize = end.parse().unwrap();
                        let slice = &bytes[start..=end.min(bytes.len() - 1)];
                        Response::builder()
                            .status(StatusCode::PARTIAL_CONTENT)
                            .header(axum::http::header::CONTENT_LENGTH, slice.len())
                            .header(
                                axum::http::header::CONTENT_RANGE,
                                format!("bytes {start}-{end}/{}", bytes.len()),
                            )
                            .header(axum::http::header::ETAG, etag_header(bytes))
                            .body(Body::from(slice.to_vec()))
                            .unwrap()
                    }
                    None => Response::builder()
                        .status(StatusCode::OK)
                        .header(axum::http::header::CONTENT_LENGTH, bytes.len())
                        .header(axum::http::header::ETAG, etag_header(bytes))
                        .body(Body::from(bytes.clone()))
                        .unwrap(),
                }
            }
            None => empty(StatusCode::NOT_FOUND),
        },
        _ => StatusCode::METHOD_NOT_ALLOWED.into_response(),
    }
}

/// The listing of the mock service: `ListObjectsV2`, with or without a delimiter.
fn list(store: &Store, query: &str) -> Response {
    let params: HashMap<String, String> = url::Url::parse(&format!("http://x/?{query}"))
        .unwrap()
        .query_pairs()
        .into_owned()
        .collect();
    let prefix = params.get("prefix").cloned().unwrap_or_default();
    let delimiter = params.get("delimiter").cloned();

    let objects = store.lock().unwrap();
    let mut contents = String::new();
    let mut prefixes: Vec<String> = Vec::new();
    for (key, bytes) in objects.iter() {
        if !key.starts_with(&prefix) {
            continue;
        }
        let rest = &key[prefix.len()..];
        match &delimiter {
            Some(delimiter) if rest.contains(delimiter) => {
                let common = format!("{prefix}{}", rest.split(delimiter).next().unwrap());
                let common = format!("{common}{delimiter}");
                if !prefixes.contains(&common) {
                    prefixes.push(common);
                }
            }
            _ => {
                contents.push_str(&format!(
                    "<Contents><Key>{key}</Key><LastModified>{}</LastModified>\
                     <ETag>&quot;{}&quot;</ETag><Size>{}</Size><StorageClass>STANDARD</StorageClass></Contents>",
                    now(),
                    etag(bytes),
                    bytes.len()
                ));
            }
        }
    }
    let common: String = prefixes
        .iter()
        .map(|p| format!("<CommonPrefixes><Prefix>{p}</Prefix></CommonPrefixes>"))
        .collect();
    let xml = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?><ListBucketResult>\
         <Name>fluorite</Name><Prefix>{prefix}</Prefix><KeyCount>{}</KeyCount>\
         <MaxKeys>1000</MaxKeys><IsTruncated>false</IsTruncated>{contents}{common}\
         </ListBucketResult>",
        contents.len()
    );
    Response::builder()
        .status(StatusCode::OK)
        .header(axum::http::header::CONTENT_TYPE, "application/xml")
        .body(Body::from(xml))
        .unwrap()
}

/// The entity tag of an object: the mock does not compute an MD5, the tests only need
/// something stable.
fn etag(bytes: &[u8]) -> String {
    format!("{:032x}", bytes.len())
}

/// The time an object was stored, as the bucket writes it.
fn now() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}
