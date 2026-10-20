use axum::http::StatusCode;

use super::{Fixture, body_json, get};
use crate::api::tree::read_listing;

#[test]
fn listing_sorts_dirs_first_and_hides() {
    let fx = Fixture::new();
    let names: Vec<_> = read_listing(&fx.output().join("textures/ui"))
        .unwrap()
        .into_iter()
        .map(|e| e.name)
        .collect();
    // Symlinks (escape.png) and hidden entries are dropped; names compare case-insensitively.
    assert_eq!(
        names,
        vec![
            "sub",
            "Alpha.png",
            "Pelica.png",
            "pelica_icon_big.png",
            "zeta.png"
        ]
    );
}

#[tokio::test]
async fn tree_endpoint() {
    let fx = Fixture::new();
    let app = fx.app();

    let v = body_json(get(&app, "/api/tree/textures").await).await;
    assert_eq!(v["root"], "textures");
    assert_eq!(v["path"], "");
    let entries = v["entries"].as_array().unwrap();
    let names: Vec<_> = entries
        .iter()
        .map(|e| e["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, vec!["chars", "ui"], ".index must be hidden");
    assert_eq!(entries[1]["kind"], "dir");
    assert_eq!(entries[1]["children"], 5);

    let v = body_json(get(&app, "/api/tree/textures?path=ui&offset=1&limit=2").await).await;
    assert_eq!(v["total"], 5);
    assert_eq!(v["offset"], 1);
    assert_eq!(v["limit"], 2);
    let e = &v["entries"][0];
    assert_eq!(e["name"], "Alpha.png");
    assert_eq!(e["path"], "ui/Alpha.png");
    assert_eq!(e["kind"], "file");
    assert!(e["size"].as_u64().unwrap() > 0);
    assert!(e.get("children").is_none());

    assert_eq!(
        get(&app, "/api/tree/textures?path=nope").await.status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        get(&app, "/api/tree/textures?path=ui/zeta.png")
            .await
            .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        get(&app, "/api/tree/textures?path=../meshes")
            .await
            .status(),
        StatusCode::BAD_REQUEST
    );
    let res = get(&app, "/api/tree/bogus").await;
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
    assert_eq!(body_json(res).await["error"]["code"], "not_found");
}

#[tokio::test]
async fn tree_listing_refreshes_when_directory_changes() {
    let fx = Fixture::new();
    let app = fx.app();
    let v = body_json(get(&app, "/api/tree/lua").await).await;
    assert_eq!(v["total"], 1);
    // Ensure the directory mtime differs even on coarse-timestamp filesystems.
    std::thread::sleep(std::time::Duration::from_millis(20));
    std::fs::write(fx.output().join("decrypted/Lua/LuaScripts/new.lua"), b"x").unwrap();
    let v = body_json(get(&app, "/api/tree/lua").await).await;
    assert_eq!(v["total"], 2);
}
