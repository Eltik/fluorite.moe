use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use serde_json::json;

use std::collections::HashSet;
use std::fs::FileTimes;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use super::catalog::catalog_fixture;
use super::{Fixture, body_bytes, body_json, get, send};
use crate::api::audio::remote::{fetch_range, is_md5, is_safe_component, prune_cached};
use crate::api::audio::transcode::{
    Format, TempFiles, Transcoder, cache_path, source_stamp, sweep_clips,
};
use crate::api::audio::{Source, lang_code, lang_name, loaded};
use crate::extraction::audio::{
    EntryKind, PackageFile, PckFile, build_package, dialog_key, external_id,
};
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

    // `playable=true` drops soundbank refs (no media) and keeps the rest.
    let all = body_json(get(&app, "/api/audio/entries").await).await;
    let banks = v["total"].as_u64().unwrap();
    assert!(banks > 0);
    let v = body_json(get(&app, "/api/audio/entries?playable=true").await).await;
    assert_eq!(
        v["total"].as_u64().unwrap(),
        all["total"].as_u64().unwrap() - banks
    );
    assert!(
        v["entries"]
            .as_array()
            .unwrap()
            .iter()
            .all(|e| e["kind"] != "bank" && e.get("audio").is_some())
    );

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

#[test]
fn temp_files_are_removed_on_drop() {
    let dir = std::env::temp_dir().join(format!("fluorite-temps-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let (a, b) = (dir.join(".a.wem"), dir.join(".a.wav"));
    std::fs::write(&a, b"x").unwrap();
    std::fs::write(&b, b"x").unwrap();
    drop(TempFiles(vec![
        a.clone(),
        b.clone(),
        dir.join("never-created.ogg"),
    ]));
    assert!(!a.exists() && !b.exists());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn clip_sweep_drops_least_recently_used_first() {
    let fx = Fixture::new();
    let cache = fx.dir.join("cache/audio");
    let now = SystemTime::now();
    let put = |rel: &str, len: usize, age_secs: u64| {
        let p = cache.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, vec![0u8; len]).unwrap();
        let t = now - Duration::from_secs(age_secs);
        std::fs::File::options()
            .write(true)
            .open(&p)
            .unwrap()
            .set_times(FileTimes::new().set_accessed(t).set_modified(t))
            .unwrap();
        p
    };
    let oldest = put("pack_a/1-00000000.ogg", 100, 300);
    let middle = put("pack_b/2-00000000.ogg", 100, 200);
    let newest = put("pack_a/3-00000000.ogg", 100, 100);
    let stale_temp = put("pack_a/.4.ogg.1.2.wav", 10, 7200);
    let fresh_temp = put("pack_a/.5.ogg.1.3.wav", 10, 1);
    let remote = put("remote/default_english_stream_0-abc.hdr", 1000, 9000);
    let index = put("index-0123.json", 1000, 9000);

    // Under the cap: only the crash leftover goes.
    assert_eq!(sweep_clips(&cache, 1000), (1, 10));
    assert!(!stale_temp.exists() && fresh_temp.exists());

    assert_eq!(sweep_clips(&cache, 250), (1, 100));
    assert!(!oldest.exists() && middle.exists() && newest.exists());
    assert_eq!(sweep_clips(&cache, 0), (2, 200));
    assert!(fresh_temp.exists() && remote.exists() && index.exists());
}

#[tokio::test]
async fn clip_cache_key_tracks_the_package() {
    let fx = audio_fixture();
    let pck = fx
        .output()
        .join("decrypted/InitAudio/Audio/PCK/Windows/Initial/test_stream.pck");
    let source = Source::Local(Arc::new(PckFile::open(&pck).unwrap()));
    let before = source_stamp(&source).await;
    std::fs::File::options()
        .append(true)
        .open(&pck)
        .unwrap()
        .set_modified(SystemTime::now() + Duration::from_secs(5))
        .unwrap();
    let after = source_stamp(&source).await;
    assert_ne!(before, after);

    let remote = |url: &str| Source::Remote {
        url: url.into(),
        base: 0,
    };
    assert_ne!(
        source_stamp(&remote("https://cdn/VFS/X/AAAA.chk")).await,
        source_stamp(&remote("https://cdn/VFS/X/BBBB.chk")).await
    );

    let l = loaded(&fx_state(&fx)).await.unwrap();
    let (_, entry) = l.entry("test_stream", 1001).unwrap();
    let cache = fx.dir.join("cache/audio");
    assert_ne!(
        cache_path(&cache, "test_stream", entry, before, Format::Ogg),
        cache_path(&cache, "test_stream", entry, after, Format::Ogg)
    );
}

fn fx_state(fx: &Fixture) -> crate::api::SharedState {
    crate::api::AppState::new(fx.config())
}

#[tokio::test]
async fn abandoned_clip_request_still_finishes() {
    let fx = audio_fixture();
    let state = fx_state(&fx);
    let app = crate::api::router(state.clone());
    // Hang up as soon as the transcode has started; the job keeps running in its own task.
    let started = async {
        while state.audio.transcoder.inflight() == 0 {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    };
    tokio::select! {
        _ = get(&app, "/audio/test_stream/1001.wav") => {
            // Answered before the job was seen (tools missing, or too fast to observe).
            return;
        }
        () = started => {}
    }
    let dir = fx.dir.join("cache/audio/test_stream");
    for _ in 0..500 {
        if state.audio.transcoder.inflight() == 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(state.audio.transcoder.inflight(), 0);
    // Temp files go on the blocking pool; give it a moment.
    tokio::time::sleep(Duration::from_millis(50)).await;
    let names: Vec<String> = std::fs::read_dir(&dir)
        .map(|rd| {
            rd.flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    assert!(names.iter().all(|n| !n.starts_with('.')), "{names:?}");
    if Transcoder::new().tools().vgmstream.is_some() {
        assert_eq!(names.len(), 1, "{names:?}");
        assert!(names[0].ends_with(".wav"));
    }
}

#[test]
fn remote_names_are_validated() {
    assert!(is_md5("0123456789abcdefABCDEF0123456789"));
    assert!(!is_md5("0123456789abcdef0123456789abcde"));
    assert!(!is_md5("../3456789abcdef0123456789abcdef"));
    assert!(!is_md5("g123456789abcdef0123456789abcdef"));
    assert!(is_safe_component("default_english_stream_0"));
    assert!(is_safe_component("8F3B2C"));
    for bad in ["", "a/b", "a\\b", "..", "x..y"] {
        assert!(!is_safe_component(bad), "{bad:?}");
    }
}

#[test]
fn superseded_remote_files_are_pruned() {
    let fx = Fixture::new();
    let dir = fx.dir.join("cache/audio/remote");
    std::fs::create_dir_all(&dir).unwrap();
    for n in [
        "banks-aaaa.pck",
        "banks-bbbb.pck",
        "stream_0-cccc.hdr",
        "stream_0-dddd.hdr",
        ".banks-eeee.pck.1.2.tmp",
        "notes.txt",
    ] {
        std::fs::write(dir.join(n), b"x").unwrap();
    }
    let keep: HashSet<String> = ["banks-bbbb.pck", "stream_0-dddd.hdr"]
        .into_iter()
        .map(String::from)
        .collect();
    assert_eq!(prune_cached(&dir, &keep), 2);
    let mut left: Vec<String> = std::fs::read_dir(&dir)
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    left.sort();
    assert_eq!(
        left,
        vec![
            ".banks-eeee.pck.1.2.tmp",
            "banks-bbbb.pck",
            "notes.txt",
            "stream_0-dddd.hdr"
        ]
    );
}

/// One connection answering 200 with a body far longer than any range asked for.
async fn ignores_ranges(body_len: usize) -> String {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/chunk", listener.local_addr().unwrap());
    tokio::spawn(async move {
        while let Ok((mut socket, _)) = listener.accept().await {
            tokio::spawn(async move {
                let mut buf = vec![0u8; 4096];
                let _ = socket.read(&mut buf).await;
                let head = format!("HTTP/1.1 200 OK\r\nContent-Length: {body_len}\r\n\r\n");
                let _ = socket.write_all(head.as_bytes()).await;
                let block = vec![7u8; 64 * 1024];
                let mut sent = 0;
                while sent < body_len {
                    let n = block.len().min(body_len - sent);
                    if socket.write_all(&block[..n]).await.is_err() {
                        return;
                    }
                    sent += n;
                }
            });
        }
    });
    url
}

#[tokio::test]
async fn fetch_range_bounds_a_200_response() {
    let url = ignores_ranges(64 << 20).await;
    let http = reqwest::Client::new();
    let bytes = fetch_range(&http, &url, 0, 10).await.unwrap();
    assert_eq!(bytes, vec![7u8; 10]);
    // A whole-body answer to a range that does not start at 0 is unusable.
    assert!(fetch_range(&http, &url, 5, 10).await.is_err());
}
