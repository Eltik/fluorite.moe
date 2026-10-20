use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use serde_json::json;

use super::catalog::catalog_fixture;
use super::{Fixture, body_bytes, body_json, get, send};
use crate::api::audio::transcode::Transcoder;
use crate::api::audio::{lang_code, lang_name};
use crate::extraction::audio::{EntryKind, PackageFile, build_package, dialog_key, external_id};
use crate::tests::extraction::audio::{bank_bytes, pcm_wav};

const CHEN_PATH: &str = "v1d0/Characters/chr_0005_chen/chr_0005_chen_chrbark_squad_01.wem";

/// Catalog fixture plus an SFX package, an English voice package and `AudioDialog`.
fn audio_fixture() -> Fixture {
    let fx = catalog_fixture();
    let out = fx.output();
    let write = |rel: &str, bytes: &[u8]| {
        let p = out.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, bytes).unwrap();
    };
    let wav = pcm_wav(1, 4800);
    let bank = bank_bytes(7, 5, 1001, 900);
    write(
        "decrypted/InitAudio/Audio/PCK/Windows/Initial/test_stream.pck",
        &build_package(
            &[(0, "sfx")],
            &[
                PackageFile {
                    kind: EntryKind::Bank,
                    id: 7,
                    lang_id: 0,
                    data: &bank,
                },
                PackageFile {
                    kind: EntryKind::Stream,
                    id: 1001,
                    lang_id: 0,
                    data: &wav,
                },
                PackageFile {
                    kind: EntryKind::Stream,
                    id: 1002,
                    lang_id: 0,
                    data: b"not audio at all",
                },
            ],
        ),
    );
    write(
        "decrypted/AudioEnglish/Audio/PCK/Windows/English/test_english_stream.pck",
        &build_package(
            &[(0, "sfx"), (1, "english")],
            &[PackageFile {
                kind: EntryKind::External,
                id: external_id("english", CHEN_PATH),
                lang_id: 0,
                data: &wav,
            }],
        ),
    );
    let key = dialog_key("chr_0005_chen_chrbark_squad_01").to_string();
    write(
        "decrypted/Table/TableCfg/AudioDialog.json",
        serde_json::to_string(&json!({
            key: { "path": CHEN_PATH, "voType": "Line", "speakerChannel": "chr_0005_chen",
                   "codec": 4, "wavDuration": 0.2, "wavDurationEN": 0.1 },
            "12345": { "path": "v1d0/Characters/chr_0005_chen/chr_0005_chen_combat_hit_01.wem",
                       "voType": "Short", "codec": 20 }
        }))
        .unwrap()
        .as_bytes(),
    );
    fx
}

#[test]
fn language_codes() {
    assert_eq!(lang_code("english"), "EN");
    assert_eq!(lang_code("sfx"), "SFX");
    assert_eq!(lang_name("jp"), "japanese");
    assert_eq!(lang_name("Korean"), "korean");
    assert_eq!(lang_name("xx"), "xx");
}

#[tokio::test]
async fn summary_lists_packs_and_coverage() {
    let fx = audio_fixture();
    let app = fx.app();
    let res = get(&app, "/api/audio").await;
    assert_eq!(res.status(), StatusCode::OK);
    let v = body_json(res).await;
    let names: Vec<&str> = v["packs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["test_english_stream", "test_stream"]);
    assert_eq!(
        v["packs"][1]["counts"],
        json!({"bank": 1, "stream": 2, "external": 0, "total": 3})
    );
    assert_eq!(v["packs"][0]["languages"], json!(["SFX", "EN"]));
    assert_eq!(v["total"], 4);
    assert_eq!(v["coverage"]["externalsWithDialog"], 1);
    assert_eq!(v["coverage"]["streamsWithEvent"], 1);
    assert_eq!(v["coverage"]["dialogLines"], 2);
    assert!(v["tools"]["vgmstream"].is_boolean());
}

#[tokio::test]
async fn entries_filter_and_page() {
    let fx = audio_fixture();
    let app = fx.app();
    let v = body_json(get(&app, "/api/audio/entries?kind=stream").await).await;
    assert_eq!(v["total"], 2);
    let e = &v["entries"][0];
    assert_eq!(e["id"], "1001");
    assert_eq!(e["codec"], "pcm");
    assert_eq!(e["duration"], 0.1);
    assert_eq!(e["events"][0]["id"], "900");
    assert_eq!(e["audio"]["ogg"], "/audio/test_stream/1001.ogg");

    let v = body_json(get(&app, "/api/audio/entries?lang=EN&q=squad").await).await;
    assert_eq!(v["total"], 1);
    let e = &v["entries"][0];
    assert_eq!(e["kind"], "external");
    assert_eq!(e["lang"], "EN");
    assert_eq!(e["name"], "chr_0005_chen_chrbark_squad_01");
    assert_eq!(e["voType"], "Line");
    assert_eq!(e["id"], external_id("english", CHEN_PATH).to_string());

    let v =
        body_json(get(&app, "/api/audio/entries?pack=test_stream&limit=1&offset=1").await).await;
    assert_eq!(
        (v["total"].as_u64(), v["entries"].as_array().unwrap().len()),
        (Some(3), 1)
    );
    let v = body_json(get(&app, "/api/audio/entries?kind=bank").await).await;
    assert!(v["entries"][0].get("audio").is_none());
    assert_eq!(v["entries"][0]["codec"], "bank");

    assert_eq!(
        get(&app, "/api/audio/entries?pack=nope").await.status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        get(&app, "/api/audio/entries?kind=zzz").await.status(),
        StatusCode::BAD_REQUEST
    );
}

#[tokio::test]
async fn voice_lines_resolve_audio() {
    let fx = audio_fixture();
    let app = fx.app();
    let res = get(&app, "/api/voice/chr_0005_chen?lang=EN").await;
    assert_eq!(res.status(), StatusCode::OK);
    let v = body_json(res).await;
    assert_eq!(v["languages"], json!(["EN"]));
    assert_eq!(v["coverage"], json!({"lines": 1, "withAudio": 1}));
    let line = &v["lines"][0];
    assert_eq!(line["title"], "Operation Prep 1");
    assert_eq!(line["voType"], "Line");
    assert_eq!(line["durations"], json!({"CN": 0.2, "EN": 0.1}));
    let id = external_id("english", CHEN_PATH);
    assert_eq!(
        line["audio"]["EN"]["m4a"],
        format!("/audio/test_english_stream/{id}.m4a")
    );
    // Other recorded lines of the operator, without audio here.
    assert_eq!(v["extra"][0]["name"], "chr_0005_chen_combat_hit_01");
    assert_eq!(v["extra"][0]["audio"], json!({}));

    // Once the index is loaded, operator details carry the same URLs.
    let d = body_json(get(&app, "/api/catalog/operators/chr_0005_chen").await).await;
    assert_eq!(
        d["voiceLines"][0]["audio"]["EN"]["ogg"],
        format!("/audio/test_english_stream/{id}.ogg")
    );

    assert_eq!(
        get(&app, "/api/voice/chr_9999_nobody").await.status(),
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn clip_errors() {
    let fx = audio_fixture();
    let app = fx.app();
    assert_eq!(
        get(&app, "/audio/test_stream/7.ogg").await.status(),
        StatusCode::UNPROCESSABLE_ENTITY
    );
    assert_eq!(
        get(&app, "/audio/test_stream/1001.mp3").await.status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        get(&app, "/audio/test_stream/abc.ogg").await.status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        get(&app, "/audio/test_stream/5.ogg").await.status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        get(&app, "/audio/nope/1001.ogg").await.status(),
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn clips_transcode_and_cache() {
    let tools = Transcoder::new().tools().clone();
    let fx = audio_fixture();
    let app = fx.app();
    let res = get(&app, "/audio/test_stream/1001.wav").await;
    if tools.vgmstream.is_none() {
        assert_eq!(res.status(), StatusCode::NOT_IMPLEMENTED);
        return;
    }
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(res.headers()[header::CONTENT_TYPE], "audio/wav");
    let etag = res.headers()[header::ETAG].clone();
    let wav = body_bytes(res).await;
    assert!(wav.starts_with(b"RIFF"));

    // Cached: same validator, ranges and conditional requests work.
    let req = Request::get("/audio/test_stream/1001.wav")
        .header(header::RANGE, "bytes=0-3")
        .body(Body::empty())
        .unwrap();
    let res = send(&app, req).await;
    assert_eq!(res.status(), StatusCode::PARTIAL_CONTENT);
    assert_eq!(res.headers()[header::ETAG], etag);
    assert_eq!(body_bytes(res).await, b"RIFF");
    let req = Request::get("/audio/test_stream/1001.wav")
        .header(header::IF_NONE_MATCH, etag)
        .body(Body::empty())
        .unwrap();
    assert_eq!(send(&app, req).await.status(), StatusCode::NOT_MODIFIED);
    let cached = std::fs::read_dir(fx.dir.join("cache/audio/test_stream"))
        .unwrap()
        .count();
    assert_eq!(cached, 1);

    // Undecodable media is a 422, not a 500.
    assert_eq!(
        get(&app, "/audio/test_stream/1002.wav").await.status(),
        StatusCode::UNPROCESSABLE_ENTITY
    );

    if tools.ffmpeg.is_some() {
        let res = get(&app, "/audio/test_stream/1001.ogg").await;
        assert_eq!(res.status(), StatusCode::OK);
        assert_eq!(res.headers()[header::CONTENT_TYPE], "audio/ogg");
        assert!(body_bytes(res).await.starts_with(b"OggS"));
        let res = get(&app, "/audio/test_stream/1001.m4a").await;
        assert_eq!(res.status(), StatusCode::OK);
        assert_eq!(res.headers()[header::CONTENT_TYPE], "audio/mp4");
    }
}
