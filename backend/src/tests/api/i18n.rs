use axum::body::Body;
use axum::http::{Request, StatusCode, header};

use super::{Fixture, body_json, get, send};
use crate::api::i18n::{normalize_lang, parse_table};

#[test]
fn parses_flat_table_shape() {
    let map = parse_table(
        br#"{"-1000162003504662904":"Naughty Little Knight\t","42":"Pelica","7":{"text":"Wrapped"},"8":5}"#,
    )
    .unwrap();
    assert_eq!(map["-1000162003504662904"], "Naughty Little Knight\t");
    assert_eq!(map["42"], "Pelica");
    assert_eq!(map["7"], "Wrapped");
    assert!(!map.contains_key("8"));
    assert!(parse_table(b"[1,2]").is_err());
}

#[test]
fn language_codes() {
    assert_eq!(normalize_lang("en").unwrap(), "EN");
    assert!(normalize_lang("../x").is_err());
    assert!(normalize_lang("e").is_err());
}

#[tokio::test]
async fn i18n_endpoints() {
    let fx = Fixture::new();
    let app = fx.app();

    let v = body_json(get(&app, "/api/i18n/languages").await).await;
    assert_eq!(v["languages"], serde_json::json!(["EN", "JP"]));

    let v = body_json(get(&app, "/api/i18n/en/42").await).await;
    assert_eq!(v["lang"], "EN");
    assert_eq!(v["text"], "Pelica");

    let v = body_json(get(&app, "/api/i18n/JP/42").await).await;
    assert_eq!(v["text"], "ペリカ");

    assert_eq!(
        get(&app, "/api/i18n/EN/999").await.status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        get(&app, "/api/i18n/DE/42").await.status(),
        StatusCode::NOT_FOUND
    );

    let res = send(
        &app,
        Request::post("/api/i18n/EN/resolve")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(r#"{"keys":["42",-1000162003504662904,"nope"]}"#))
            .unwrap(),
    )
    .await;
    assert_eq!(res.status(), StatusCode::OK);
    let v = body_json(res).await;
    assert_eq!(
        v["texts"],
        serde_json::json!({"42": "Pelica", "-1000162003504662904": "Naughty Little Knight\t"})
    );
}
