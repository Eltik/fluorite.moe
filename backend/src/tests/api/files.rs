use axum::body::Body;
use axum::http::{Request, StatusCode, header};

use super::{Fixture, body_bytes, body_json, get, send};
use crate::api::files::{RangeSpec, content_type, parse_range};

#[test]
fn range_parsing() {
    assert_eq!(parse_range("bytes=0-99", 1000), RangeSpec::Partial(0, 99));
    assert_eq!(
        parse_range("bytes=900-", 1000),
        RangeSpec::Partial(900, 999)
    );
    assert_eq!(
        parse_range("bytes=-100", 1000),
        RangeSpec::Partial(900, 999)
    );
    assert_eq!(parse_range("bytes=-5000", 1000), RangeSpec::Partial(0, 999));
    assert_eq!(
        parse_range("bytes=990-2000", 1000),
        RangeSpec::Partial(990, 999)
    );
    assert_eq!(parse_range("bytes=1000-", 1000), RangeSpec::Unsatisfiable);
    assert_eq!(parse_range("bytes=-0", 1000), RangeSpec::Unsatisfiable);
    assert_eq!(parse_range("bytes=5-1", 1000), RangeSpec::Full);
    assert_eq!(parse_range("bytes=0-1,5-6", 1000), RangeSpec::Full);
    assert_eq!(parse_range("items=0-1", 1000), RangeSpec::Full);
    assert_eq!(parse_range("bytes=abc", 1000), RangeSpec::Full);
}

#[test]
fn content_types() {
    assert_eq!(content_type("a.PNG"), "image/png");
    assert_eq!(content_type("a.glb"), "model/gltf-binary");
    assert_eq!(content_type("a.json"), "application/json");
    assert_eq!(content_type("a.lua"), "text/plain; charset=utf-8");
    assert_eq!(content_type("a.bytes"), "application/octet-stream");
    assert_eq!(content_type("a.wem"), "audio/x-wem");
    assert_eq!(content_type("noext"), "application/octet-stream");
}

#[tokio::test]
async fn full_file_and_validators() {
    let fx = Fixture::new();
    let app = fx.app();
    let res = get(&app, "/files/meshes/chars/pelica.glb").await;
    assert_eq!(res.status(), StatusCode::OK);
    let h = res.headers().clone();
    assert_eq!(h[header::CONTENT_TYPE], "model/gltf-binary");
    assert_eq!(h[header::CONTENT_LENGTH], "1000");
    assert_eq!(h[header::ACCEPT_RANGES], "bytes");
    assert_eq!(h[header::CACHE_CONTROL], "public, max-age=3600");
    assert_eq!(h[header::ACCESS_CONTROL_ALLOW_ORIGIN.as_str()], "*");
    assert!(
        h.get(header::CONTENT_ENCODING).is_none(),
        "files are not compressed"
    );
    let etag = h[header::ETAG].to_str().unwrap().to_string();
    let last_modified = h[header::LAST_MODIFIED].to_str().unwrap().to_string();
    assert_eq!(body_bytes(res).await.len(), 1000);

    let req = |name: header::HeaderName, value: &str| {
        Request::get("/files/meshes/chars/pelica.glb")
            .header(name, value)
            .body(Body::empty())
            .unwrap()
    };
    let res = send(&app, req(header::IF_NONE_MATCH, &etag)).await;
    assert_eq!(res.status(), StatusCode::NOT_MODIFIED);
    assert_eq!(res.headers()[header::ETAG], etag.as_str());
    assert!(body_bytes(res).await.is_empty());

    let res = send(
        &app,
        req(header::IF_NONE_MATCH, &format!("\"x\", W/{etag}")),
    )
    .await;
    assert_eq!(res.status(), StatusCode::NOT_MODIFIED);

    let res = send(&app, req(header::IF_NONE_MATCH, "\"other\"")).await;
    assert_eq!(res.status(), StatusCode::OK);

    let res = send(&app, req(header::IF_MODIFIED_SINCE, &last_modified)).await;
    assert_eq!(res.status(), StatusCode::NOT_MODIFIED);

    let res = send(
        &app,
        req(header::IF_MODIFIED_SINCE, "Thu, 01 Jan 1970 00:00:00 GMT"),
    )
    .await;
    assert_eq!(res.status(), StatusCode::OK);
}

#[tokio::test]
async fn ranges() {
    let fx = Fixture::new();
    let app = fx.app();
    let req = |range: &str| {
        Request::get("/files/meshes/chars/pelica.glb")
            .header(header::RANGE, range)
            .header(header::ACCEPT_ENCODING, "gzip")
            .body(Body::empty())
            .unwrap()
    };
    let res = send(&app, req("bytes=10-19")).await;
    assert_eq!(res.status(), StatusCode::PARTIAL_CONTENT);
    assert_eq!(res.headers()[header::CONTENT_RANGE], "bytes 10-19/1000");
    assert_eq!(res.headers()[header::CONTENT_LENGTH], "10");
    let body = body_bytes(res).await;
    let expected: Vec<u8> = (10..20u32).map(|i| (i % 251) as u8).collect();
    assert_eq!(body, expected);

    let res = send(&app, req("bytes=-3")).await;
    assert_eq!(res.headers()[header::CONTENT_RANGE], "bytes 997-999/1000");
    assert_eq!(body_bytes(res).await.len(), 3);

    let res = send(&app, req("bytes=5000-")).await;
    assert_eq!(res.status(), StatusCode::RANGE_NOT_SATISFIABLE);
    assert_eq!(res.headers()[header::CONTENT_RANGE], "bytes */1000");

    // A stale If-Range validator downgrades to the full body.
    let res = send(
        &app,
        Request::get("/files/meshes/chars/pelica.glb")
            .header(header::RANGE, "bytes=0-9")
            .header(header::IF_RANGE, "\"stale\"")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(body_bytes(res).await.len(), 1000);
}

#[tokio::test]
async fn tables_endpoints() {
    let fx = Fixture::new();
    let app = fx.app();
    let v = body_json(get(&app, "/api/tables").await).await;
    let names: Vec<_> = v["tables"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, vec!["Foo", "I18nTextTable_EN", "I18nTextTable_JP"]);

    let res = get(&app, "/api/tables/Foo").await;
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(res.headers()[header::CONTENT_TYPE], "application/json");
    assert_eq!(body_json(res).await["a"], 1);
    assert_eq!(
        get(&app, "/api/tables/Foo.json").await.status(),
        StatusCode::OK
    );
    assert_eq!(
        get(&app, "/api/tables/Missing").await.status(),
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn thumbnails_are_generated_and_cached() {
    let fx = Fixture::new();
    let app = fx.app();
    let res = get(&app, "/thumb/ui/Pelica.png?size=128").await;
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(res.headers()[header::CONTENT_TYPE], "image/webp");
    let etag = res.headers()[header::ETAG].to_str().unwrap().to_string();
    let bytes = body_bytes(res).await;
    let img = image::load_from_memory(&bytes).unwrap();
    assert_eq!((img.width(), img.height()), (128, 64));

    let cached: Vec<_> = std::fs::read_dir(fx.dir.join("cache/thumbs/128"))
        .unwrap()
        .flatten()
        .collect();
    assert_eq!(cached.len(), 1, "exactly one cache file, no temp leftovers");

    let res = send(
        &app,
        Request::get("/thumb/ui/Pelica.png?size=128")
            .header(header::IF_NONE_MATCH, &etag)
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(res.status(), StatusCode::NOT_MODIFIED);

    assert_eq!(
        get(&app, "/thumb/ui/Pelica.png?size=100").await.status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        get(&app, "/thumb/ui/missing.png").await.status(),
        StatusCode::NOT_FOUND
    );
    assert!(!super::exists(&fx.output().join(".cache")));
}

#[tokio::test]
async fn status_and_catalog_stub() {
    let fx = Fixture::new();
    let app = fx.app();
    let v = body_json(get(&app, "/api/status").await).await;
    assert_eq!(v["version"], "1.6.4");
    assert_eq!(v["resVersion"], "r1");
    assert_eq!(v["counts"]["textures"], 6);
    assert_eq!(v["counts"]["lua"], 1);
    assert_eq!(v["index"]["ready"], true);

    let res = get(&app, "/api/catalog/operators").await;
    assert_eq!(res.status(), StatusCode::NOT_IMPLEMENTED);
    assert_eq!(body_json(res).await["error"]["code"], "not_implemented");

    let res = get(&app, "/api/nope").await;
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
}
