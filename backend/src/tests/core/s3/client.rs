//! The tests of the settings, the key layout and the client's object operations.

use crate::api::config::{S3Config, S3Mode};
use crate::api::paths::Root;
use crate::api::storage::Storage;
use crate::api::thumbs::cache_name;
use crate::core::s3::{Credentials, S3Client, S3Settings, encode_path};

use super::{base, env, mock_endpoint};

#[test]
fn no_variables_leaves_the_storage_local() {
    assert!(S3Config::from_env(env(&[])).unwrap().is_none());
}

#[test]
fn the_defaults_serve_the_big_roots_through_the_api() {
    let cfg = S3Config::from_env(env(&base(&[]))).unwrap().unwrap();
    assert_eq!(
        cfg.settings.endpoint,
        "https://example.r2.cloudflarestorage.com"
    );
    assert_eq!(cfg.settings.bucket, "fluorite");
    assert_eq!(cfg.settings.region, "auto");
    assert_eq!(
        cfg.settings.prefix, "fluorite",
        "everything lives under the one prefix"
    );
    assert_eq!(cfg.mode, S3Mode::Proxy);
    assert!(cfg.thumbs);
    assert!(cfg.settings.path_style);
    assert!(cfg.settings.public_base_url.is_none());
    assert!(cfg.settings.keep_local, "the local copy is kept by default");
    assert_eq!(
        cfg.settings.roots,
        vec![Root::Textures, Root::Meshes, Root::Models],
        "the heavy roots are the ones that leave the disk"
    );
}

#[test]
fn every_setting_can_be_overridden() {
    let cfg = S3Config::from_env(env(&base(&[
        ("S3_REGION", "us-west-004"),
        ("S3_PREFIX", "/fluorite/"),
        ("S3_PUBLIC_BASE_URL", "https://assets.fluorite.moe"),
        ("S3_ROOTS", "textures, meshes"),
        ("S3_MODE", "redirect"),
        ("S3_THUMBS", "0"),
        ("S3_PATH_STYLE", "false"),
        ("S3_SESSION_TOKEN", "token"),
        ("S3_KEEP_LOCAL", "0"),
    ])))
    .unwrap()
    .unwrap();
    assert_eq!(cfg.settings.region, "us-west-004");
    assert_eq!(
        cfg.settings.prefix, "fluorite",
        "the slashes around the prefix are not part of it"
    );
    assert_eq!(
        cfg.settings.public_base_url.as_deref(),
        Some("https://assets.fluorite.moe")
    );
    assert_eq!(cfg.settings.roots, vec![Root::Textures, Root::Meshes]);
    assert_eq!(cfg.mode, S3Mode::Redirect);
    assert!(!cfg.thumbs);
    assert!(!cfg.settings.path_style);
    assert_eq!(
        cfg.settings.credentials.session_token.as_deref(),
        Some("token")
    );
    assert!(
        !cfg.settings.keep_local,
        "the local copy may be dropped after the upload"
    );
}

#[test]
fn a_half_filled_configuration_is_an_error() {
    // A typo must not silently put the files back on the server's disk.
    let err = S3Config::from_env(env(&[
        ("S3_ENDPOINT", "https://example.r2.cloudflarestorage.com"),
        ("S3_BUCKET", "fluorite"),
        ("S3_ACCESS_KEY_ID", "AKID"),
    ]))
    .unwrap_err();
    assert!(
        err.contains("S3_SECRET_ACCESS_KEY"),
        "the error names the missing variable: {err}"
    );
}

#[test]
fn an_unknown_root_or_mode_is_an_error() {
    let err = S3Config::from_env(env(&base(&[("S3_ROOTS", "textures,nope")]))).unwrap_err();
    assert!(err.contains("nope"), "{err}");

    let err = S3Config::from_env(env(&base(&[("S3_MODE", "sideways")]))).unwrap_err();
    assert!(err.contains("S3_MODE"), "{err}");
}

#[test]
fn the_bucket_mirrors_the_local_layout() {
    let cfg = S3Config::from_env(env(&base(&[]))).unwrap().unwrap();
    let storage = Storage::new(std::env::temp_dir(), Some(&cfg)).unwrap();
    // The key of a file is its path below the output directory, under the prefix,
    // so the sync is a plain directory mirror.
    assert_eq!(
        storage.key_for(Root::Textures, "ui/logo.png"),
        "fluorite/textures/ui/logo.png"
    );
    assert_eq!(
        storage.key_for(Root::Tables, "Foo.json"),
        "fluorite/decrypted/Table/TableCfg/Foo.json"
    );
    // A thumbnail is keyed by the identity of its source, so the cache of one
    // server is the bucket key of the others.
    assert_eq!(
        storage.thumb_key(128, "ui/logo.png", 4096, "etag"),
        format!(
            "fluorite/thumbs/128/{}",
            cache_name("ui/logo.png", 4096, "etag")
        )
    );
    assert_eq!(
        storage.thumb_key(128, "ui/logo.png", 4096, "etag"),
        storage.thumb_key(128, "ui/logo.png", 4096, "etag")
    );
    assert_ne!(
        storage.thumb_key(128, "ui/logo.png", 4096, "one"),
        storage.thumb_key(128, "ui/logo.png", 4096, "two"),
        "a changed source gets a new thumbnail"
    );
}

#[test]
fn a_prefix_moves_the_whole_bucket_layout() {
    let cfg = S3Config::from_env(env(&base(&[("S3_PREFIX", "elsewhere")])))
        .unwrap()
        .unwrap();
    let storage = Storage::new(std::env::temp_dir(), Some(&cfg)).unwrap();
    assert_eq!(
        storage.key_for(Root::Textures, "ui/logo.png"),
        "elsewhere/textures/ui/logo.png"
    );
    assert_eq!(
        storage.thumb_key(256, "ui/logo.png", 4096, "etag"),
        format!(
            "elsewhere/thumbs/256/{}",
            cache_name("ui/logo.png", 4096, "etag")
        )
    );
}

#[test]
fn only_the_configured_roots_leave_the_disk() {
    let cfg = S3Config::from_env(env(&base(&[("S3_ROOTS", "textures")])))
        .unwrap()
        .unwrap();
    let storage = Storage::new(std::env::temp_dir(), Some(&cfg)).unwrap();
    assert!(storage.is_remote(Root::Textures));
    assert!(!storage.is_remote(Root::Meshes));
    assert!(!storage.is_remote(Root::Gamedata));
}

#[test]
fn without_a_bucket_every_root_is_local() {
    let storage = Storage::new(std::env::temp_dir(), None).unwrap();
    for root in Root::ALL {
        assert!(!storage.is_remote(root), "{} stays local", root.name());
    }
    assert!(!storage.remote_thumbs());
}

#[test]
fn object_keys_are_encoded_for_public_urls() {
    assert_eq!(
        encode_path("textures/ui/a b&c.png"),
        "textures/ui/a%20b%26c.png"
    );
    assert_eq!(encode_path("textures/ui/logo.png"), "textures/ui/logo.png");
}

/// A client for the mock endpoint.
fn client(endpoint: &str) -> S3Client {
    S3Client::new(
        endpoint,
        "us-east-1",
        "fluorite",
        Credentials {
            access_key_id: "AKID".to_string(),
            secret_access_key: "secret".to_string(),
            session_token: None,
        },
        true,
    )
    .unwrap()
}

/// The settings of the mock endpoint, with the prefix the deployment uses.
fn settings(endpoint: &str) -> S3Settings {
    S3Settings::from_env(env(&[
        ("S3_ENDPOINT", endpoint),
        ("S3_BUCKET", "fluorite"),
        ("S3_ACCESS_KEY_ID", "AKID"),
        ("S3_SECRET_ACCESS_KEY", "secret"),
        ("S3_PREFIX", "fluorite"),
        ("S3_ROOTS", "textures"),
        ("S3_CACHE_CONTROL", "public, max-age=3600"),
    ]))
    .unwrap()
    .unwrap()
}

#[test]
fn the_keys_carry_the_prefix_and_the_root() {
    // The publisher and the API must agree on the key of a file, so both read the
    // same mapping.
    let settings = settings("http://127.0.0.1:9000");
    assert_eq!(settings.root_prefix(Root::Textures), "fluorite/textures/");
    assert_eq!(
        settings.key_for(Root::Textures, "ui/logo.png"),
        "fluorite/textures/ui/logo.png"
    );
    assert_eq!(
        settings.key_for(Root::Tables, "Foo.json"),
        "fluorite/decrypted/Table/TableCfg/Foo.json"
    );
}

#[tokio::test]
async fn the_size_comes_from_the_header_of_a_head() {
    // A `HEAD` response has no body: the size of the object is only in the header.
    let (endpoint, store, _log) = mock_endpoint().await;
    store
        .lock()
        .unwrap()
        .insert("textures/ui/logo.png".to_string(), b"PNG-DATA".to_vec());
    let meta = client(&endpoint)
        .head("textures/ui/logo.png")
        .await
        .unwrap();
    assert_eq!(meta.size, 8);
    assert!(meta.last_modified.is_some());
}

#[tokio::test]
async fn a_ranged_read_reaches_the_service() {
    let (endpoint, store, _log) = mock_endpoint().await;
    store
        .lock()
        .unwrap()
        .insert("textures/ui/logo.png".to_string(), b"PNG-DATA".to_vec());
    let object = client(&endpoint)
        .get("textures/ui/logo.png", Some((0, 3)))
        .await
        .unwrap();
    assert_eq!(object.status, 206);
    assert_eq!(object.content_length, Some(4));
    assert_eq!(object.content_range.as_deref(), Some("bytes 0-3/8"));
    let body = object.body.bytes().await.unwrap();
    assert_eq!(&body[..], b"PNG-");
}

#[tokio::test]
async fn a_missing_object_is_a_not_found() {
    let (endpoint, _store, _log) = mock_endpoint().await;
    let err = client(&endpoint)
        .head("textures/ui/nope.png")
        .await
        .unwrap_err();
    assert_eq!(err.status, 404);
}

#[tokio::test]
async fn an_upload_is_stored_and_read_back() {
    let (endpoint, store, log) = mock_endpoint().await;
    let dir = std::env::temp_dir().join(format!("fluorite-upload-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("logo.png");
    std::fs::write(&file, b"PNG-DATA").unwrap();

    let meta = client(&endpoint)
        .put_file(
            "fluorite/textures/ui/logo.png",
            &file,
            Some("image/png"),
            Some("public, max-age=3600"),
        )
        .await
        .unwrap();
    assert_eq!(meta.size, 8);

    // The bytes are in the bucket, under the key of the file.
    let stored = store.lock().unwrap();
    assert_eq!(
        stored
            .get("fluorite/textures/ui/logo.png")
            .map(Vec::as_slice),
        Some(&b"PNG-DATA"[..])
    );
    drop(stored);

    // The upload is signed with the hash of its body and carries the headers the
    // object is stored with.
    let log = log.lock().unwrap();
    let (method, headers) = &log[0];
    assert_eq!(method, "PUT");
    assert_eq!(
        headers.get("x-amz-content-sha256").map(String::as_str),
        Some("a1331ee43337bf71f6b9c42d2f9a15c2cf37625c82deb3f9065482daf1426499")
    );
    assert_eq!(
        headers.get("content-type").map(String::as_str),
        Some("image/png")
    );
    assert_eq!(
        headers.get("cache-control").map(String::as_str),
        Some("public, max-age=3600")
    );
    assert_eq!(headers.get("content-length").map(String::as_str), Some("8"));

    std::fs::remove_dir_all(&dir).unwrap();
}

#[tokio::test]
async fn an_uploaded_object_can_be_deleted() {
    let (endpoint, store, _log) = mock_endpoint().await;
    store.lock().unwrap().insert(
        "fluorite/textures/ui/logo.png".to_string(),
        b"PNG-DATA".to_vec(),
    );
    client(&endpoint)
        .delete("fluorite/textures/ui/logo.png")
        .await
        .unwrap();
    assert!(
        !store
            .lock()
            .unwrap()
            .contains_key("fluorite/textures/ui/logo.png")
    );
}

#[tokio::test]
async fn deleting_a_missing_object_is_not_an_error() {
    // The bucket ends up in the wanted state either way.
    let (endpoint, _store, _log) = mock_endpoint().await;
    client(&endpoint)
        .delete("fluorite/textures/ui/nope.png")
        .await
        .unwrap();
}
