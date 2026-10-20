use axum::http::StatusCode;

use super::{Fixture, body_json, get};
use crate::api::paths::Root;
use crate::api::search::{MAX_OFFSET, SearchIndex, tokenize};

#[test]
fn tokenize_lowercases_and_splits() {
    assert_eq!(tokenize("  Pelica   ICON "), vec!["pelica", "icon"]);
    assert!(tokenize("   ").is_empty());
}

#[test]
fn index_hides_and_counts() {
    let fx = Fixture::new();
    let index = SearchIndex::build(&fx.output());
    // ui: Pelica, pelica_icon_big, zeta, Alpha, sub/one; chars/pelica/body (escape is a symlink).
    assert_eq!(index.count(Root::Textures), 6);
    assert_eq!(index.count(Root::Jsondata), 1, ".mpk must be hidden");
    let (total, _) = index.search("tmp", None, None, 0, 10);
    assert_eq!(total, 0);
    let (total, _) = index.search("stuff", None, None, 0, 10);
    assert_eq!(total, 0, "dot-directories are skipped");
}

#[test]
fn ranking_tiers() {
    let fx = Fixture::new();
    let index = SearchIndex::build(&fx.output());
    let (total, hits) = index.search("pelica", None, None, 0, 10);
    let paths: Vec<_> = hits
        .iter()
        .map(|h| format!("{}/{}", h.root, h.path))
        .collect();
    assert_eq!(total, 4, "{paths:?}");
    // Exact name (stem) first (shorter path wins the tie), then prefix, then path-only.
    assert_eq!(
        paths,
        vec![
            "textures/ui/Pelica.png",
            "meshes/chars/pelica.glb",
            "textures/ui/pelica_icon_big.png",
            "textures/chars/pelica/body.png",
        ]
    );
    assert_eq!(hits[0].name, "Pelica.png");
}

#[test]
fn multi_token_and_filters() {
    let fx = Fixture::new();
    let index = SearchIndex::build(&fx.output());
    let (total, hits) = index.search("PELICA big", None, None, 0, 10);
    assert_eq!(total, 1);
    assert_eq!(hits[0].path, "ui/pelica_icon_big.png");

    let (total, hits) = index.search("pelica", Some(Root::Meshes), None, 0, 10);
    assert_eq!((total, hits.len()), (1, 1));
    let (total, _) = index.search("pelica", None, Some("glb"), 0, 10);
    assert_eq!(total, 1);
    let (total, _) = index.search("pelica nothing", None, None, 0, 10);
    assert_eq!(total, 0);

    // Tokens with '/' may span the directory/name boundary.
    let (total, hits) = index.search("ui/pel", None, None, 0, 10);
    assert_eq!(total, 2);
    assert_eq!(hits[0].path, "ui/Pelica.png");
    assert!(hits[0].size > 0, "sizes are stat'ed for returned hits");
    let (total, _) = index.search("chars/pelica/bo", None, None, 0, 10);
    assert_eq!(total, 1);
}

#[test]
fn pagination() {
    let fx = Fixture::new();
    let index = SearchIndex::build(&fx.output());
    let (_, all) = index.search("png", None, None, 0, 100);
    let (total, page) = index.search("png", None, None, 2, 2);
    assert_eq!(total, all.len());
    assert_eq!(page, all[2..4].to_vec());
    let (_, beyond) = index.search("png", None, None, 100, 2);
    assert!(beyond.is_empty());
}

#[tokio::test]
async fn search_endpoint() {
    let fx = Fixture::new();
    let app = fx.app();
    let res = get(&app, "/api/search?q=pelica&root=textures&limit=1").await;
    assert_eq!(res.status(), StatusCode::OK);
    let v = body_json(res).await;
    assert_eq!(v["query"], "pelica");
    assert_eq!(v["total"], 3);
    assert_eq!(v["items"].as_array().unwrap().len(), 1);
    assert_eq!(v["items"][0]["path"], "ui/Pelica.png");

    assert_eq!(
        get(&app, "/api/search?q=").await.status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        get(&app, "/api/search?q=a&root=bogus").await.status(),
        StatusCode::BAD_REQUEST
    );
}

#[tokio::test]
async fn index_rebuilds_when_update_log_changes() {
    let fx = Fixture::new();
    let mut config = fx.config();
    config.index_poll = std::time::Duration::from_millis(50);
    let state = crate::api::AppState::new(config);
    crate::api::search::spawn_maintenance(state.clone());
    assert_eq!(state.index.wait().await.count(Root::Lua), 1);

    std::fs::write(fx.output().join("decrypted/Lua/LuaScripts/added.lua"), b"x").unwrap();
    let log = std::fs::File::options()
        .append(true)
        .open(fx.output().join("update-log.jsonl"))
        .unwrap();
    log.set_modified(std::time::SystemTime::now() + std::time::Duration::from_secs(5))
        .unwrap();

    for _ in 0..100 {
        if state.index.get().unwrap().count(Root::Lua) == 2 {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    panic!("index was not rebuilt after update-log.jsonl changed");
}

#[tokio::test]
async fn parallel_search_matches_sequential() {
    let fx = Fixture::new();
    let index = std::sync::Arc::new(SearchIndex::build(&fx.output()));
    for (q, root, ext) in [
        ("png", None, None),
        ("pelica", Some(Root::Meshes), None),
        ("ui/pel", None, Some("png")),
        ("nothing at all", None, None),
    ] {
        let seq = index.search(q, root, ext, 1, 3);
        let par = index
            .clone()
            .search_parallel(q, root, ext.map(String::from), 1, 3)
            .await
            .unwrap();
        assert_eq!(seq, par, "{q}");
    }
}

#[tokio::test]
async fn search_offset_is_capped() {
    let fx = Fixture::new();
    let app = fx.app();
    let ok = format!("/api/search?q=png&offset={MAX_OFFSET}");
    assert_eq!(get(&app, &ok).await.status(), StatusCode::OK);
    let deep = format!("/api/search?q=png&offset={}", MAX_OFFSET + 1);
    let res = get(&app, &deep).await;
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    assert_eq!(body_json(res).await["error"]["code"], "bad_request");
}

#[tokio::test]
async fn index_rebuilds_when_models_index_changes() {
    let fx = Fixture::new();
    let mut config = fx.config();
    config.index_poll = std::time::Duration::from_millis(50);
    let state = crate::api::AppState::new(config);
    crate::api::search::spawn_maintenance(state.clone());
    assert_eq!(state.index.wait().await.count(Root::Models), 0);

    let models = fx.output().join("models");
    std::fs::create_dir_all(models.join("operators")).unwrap();
    std::fs::write(models.join("operators/chr_0004_pelica.glb"), b"glTF").unwrap();
    std::fs::write(models.join("index.json"), b"{}").unwrap();

    for _ in 0..100 {
        if state.index.get().unwrap().count(Root::Models) == 2 {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    panic!("index was not rebuilt after models/index.json changed");
}
