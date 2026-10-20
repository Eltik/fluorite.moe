use axum::http::StatusCode;

use super::{Fixture, get};
use crate::api::paths::{Root, is_hidden, sanitize};

#[test]
fn sanitize_normalizes_and_rejects() {
    assert_eq!(sanitize("").unwrap(), "");
    assert_eq!(sanitize("a/b/c.png").unwrap(), "a/b/c.png");
    assert_eq!(sanitize("a//b/").unwrap(), "a/b");

    for bad in [
        "../x",
        "a/../../b",
        "a/./b",
        "/etc/passwd",
        "a\\b",
        "a\0b",
        "..",
    ] {
        let err = sanitize(bad).unwrap_err();
        assert_eq!(err.status, StatusCode::BAD_REQUEST, "{bad:?}");
    }
    for hidden in [
        ".index/x",
        "a/.hidden.png",
        "a/b.tmp",
        "x.mpk",
        "blc_metadata.txt",
    ] {
        let err = sanitize(hidden).unwrap_err();
        assert_eq!(err.status, StatusCode::NOT_FOUND, "{hidden:?}");
    }
}

#[test]
fn hidden_rules() {
    assert!(is_hidden(".staging"));
    assert!(is_hidden("foo.png.tmp"));
    assert!(is_hidden("Table.mpk"));
    assert!(is_hidden("blc_metadata.txt"));
    assert!(!is_hidden("metadata.txt"));
    assert!(!is_hidden("a.png"));
}

#[test]
fn roots_parse() {
    for r in Root::ALL {
        assert_eq!(Root::parse(r.name()).unwrap(), r);
    }
    assert!(Root::parse("etc").is_err());
}

#[tokio::test]
async fn files_reject_traversal_hidden_and_symlink_escape() {
    let fx = Fixture::new();
    let app = fx.app();
    assert_eq!(
        get(&app, "/files/textures/ui/%2e%2e/%2e%2e/update-log.jsonl")
            .await
            .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        get(&app, "/files/textures/ui%5c..%5cx").await.status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        get(&app, "/files/textures/ui/.hidden.png").await.status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        get(&app, "/files/textures/.index/stuff.png").await.status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        get(&app, "/files/jsondata/x.mpk").await.status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        get(&app, "/files/nope/x").await.status(),
        StatusCode::NOT_FOUND
    );
    #[cfg(unix)]
    assert_eq!(
        get(&app, "/files/textures/ui/escape.png").await.status(),
        StatusCode::NOT_FOUND
    );
}
