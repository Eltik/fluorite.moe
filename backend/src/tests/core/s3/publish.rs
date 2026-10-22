//! The tests of the publisher: the local tree goes into the bucket, the second run has
//! nothing to do, and the local copy can go away afterwards.

use crate::core::s3::{PublishOptions, S3Settings};

use super::{env, mock_endpoint};

/// The settings of the mock endpoint, with the prefix and the roots of the deployment.
fn settings(endpoint: &str) -> S3Settings {
    S3Settings::from_env(env(&[
        ("S3_ENDPOINT", endpoint),
        ("S3_BUCKET", "assets"),
        ("S3_ACCESS_KEY_ID", "AKID"),
        ("S3_SECRET_ACCESS_KEY", "secret"),
        ("S3_PREFIX", "fluorite"),
        ("S3_ROOTS", "textures"),
    ]))
    .unwrap()
    .unwrap()
}

/// A throwaway output tree with a few textures in it.
struct Tree {
    dir: std::path::PathBuf,
}

impl Tree {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "fluorite-publish-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(dir.join("textures/ui")).unwrap();
        std::fs::write(dir.join("textures/ui/logo.png"), b"PNG-DATA").unwrap();
        std::fs::write(dir.join("textures/icon.png"), b"PNG-ICON").unwrap();
        Self { dir }
    }

    fn path(&self, rel: &str) -> std::path::PathBuf {
        self.dir.join(rel)
    }
}

impl Drop for Tree {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

#[tokio::test]
async fn the_local_tree_goes_into_the_bucket() {
    let (endpoint, store, _log) = mock_endpoint().await;
    let tree = Tree::new("upload");
    let report =
        crate::core::s3::publish(&settings(&endpoint), &tree.dir, &PublishOptions::default())
            .await
            .unwrap();

    assert_eq!(report.uploaded, 2);
    assert_eq!(report.skipped, 0);
    assert_eq!(report.failed, 0);
    assert_eq!(report.bytes, 8 + 8);

    // The keys carry the prefix and the name of the root, like the API expects them.
    let store = store.lock().unwrap();
    assert_eq!(
        store
            .get("fluorite/textures/ui/logo.png")
            .map(Vec::as_slice),
        Some(&b"PNG-DATA"[..])
    );
    assert_eq!(
        store.get("fluorite/textures/icon.png").map(Vec::as_slice),
        Some(&b"PNG-ICON"[..])
    );
}

#[tokio::test]
async fn a_second_run_has_nothing_to_upload() {
    let (endpoint, _store, _log) = mock_endpoint().await;
    let tree = Tree::new("twice");
    let options = PublishOptions::default();
    crate::core::s3::publish(&settings(&endpoint), &tree.dir, &options)
        .await
        .unwrap();
    let report = crate::core::s3::publish(&settings(&endpoint), &tree.dir, &options)
        .await
        .unwrap();

    assert_eq!(report.uploaded, 0);
    assert_eq!(report.skipped, 2, "the files are already in the bucket");
    assert_eq!(report.failed, 0);
}

#[tokio::test]
async fn a_changed_file_is_uploaded_again() {
    let (endpoint, store, _log) = mock_endpoint().await;
    let tree = Tree::new("changed");
    let options = PublishOptions::default();
    crate::core::s3::publish(&settings(&endpoint), &tree.dir, &options)
        .await
        .unwrap();

    // A file that changed has another size: the size catches it even when the clock
    // does not move.
    std::fs::write(tree.path("textures/icon.png"), b"PNG-ICON-CHANGED").unwrap();
    let report = crate::core::s3::publish(&settings(&endpoint), &tree.dir, &options)
        .await
        .unwrap();

    assert_eq!(report.uploaded, 1);
    assert_eq!(report.skipped, 1);
    assert_eq!(
        store
            .lock()
            .unwrap()
            .get("fluorite/textures/icon.png")
            .map(Vec::as_slice),
        Some(&b"PNG-ICON-CHANGED"[..])
    );
}

#[tokio::test]
async fn a_dry_run_uploads_nothing() {
    let (endpoint, store, _log) = mock_endpoint().await;
    let tree = Tree::new("dry");
    let report = crate::core::s3::publish(
        &settings(&endpoint),
        &tree.dir,
        &PublishOptions {
            dry_run: true,
            ..PublishOptions::default()
        },
    )
    .await
    .unwrap();

    assert_eq!(report.uploaded, 2, "the run counts what it would upload");
    assert!(
        store.lock().unwrap().is_empty(),
        "but nothing is in the bucket"
    );
    assert!(
        tree.path("textures/icon.png").exists(),
        "and the local files are still there"
    );
}

#[tokio::test]
async fn the_local_copy_can_go_away_after_the_upload() {
    let (endpoint, store, _log) = mock_endpoint().await;
    let tree = Tree::new("remove");
    let report = crate::core::s3::publish(
        &settings(&endpoint),
        &tree.dir,
        &PublishOptions {
            remove_local: true,
            ..PublishOptions::default()
        },
    )
    .await
    .unwrap();

    assert_eq!(report.uploaded, 2);
    assert_eq!(report.removed, 2);
    assert!(
        !tree.path("textures/icon.png").exists(),
        "the file is only in the bucket now"
    );
    assert!(
        !tree.path("textures/ui").exists(),
        "the directory it emptied is gone too"
    );
    assert_eq!(
        store.lock().unwrap().len(),
        2,
        "the bytes are in the bucket"
    );
}

#[tokio::test]
async fn an_empty_root_is_not_listed() {
    // A root without local files has nothing to upload: the run must not even ask the
    // bucket what it holds. That is the normal case once the local copies are gone.
    let (endpoint, _store, log) = mock_endpoint().await;
    let tree = Tree::new("empty");
    std::fs::remove_dir_all(tree.path("textures")).unwrap();
    std::fs::create_dir_all(tree.path("textures")).unwrap();

    let report =
        crate::core::s3::publish(&settings(&endpoint), &tree.dir, &PublishOptions::default())
            .await
            .unwrap();

    assert_eq!(report.uploaded, 0);
    assert!(
        !log.lock()
            .unwrap()
            .iter()
            .any(|(method, _)| method == "GET"),
        "the bucket was not listed"
    );
}

#[tokio::test]
async fn the_prune_removes_the_objects_that_are_gone() {
    let (endpoint, store, _log) = mock_endpoint().await;
    let tree = Tree::new("prune");
    let options = PublishOptions::default();
    crate::core::s3::publish(&settings(&endpoint), &tree.dir, &options)
        .await
        .unwrap();

    // A file that is gone from the disk and the prune is on: the object goes too.
    std::fs::remove_file(tree.path("textures/icon.png")).unwrap();
    let report = crate::core::s3::publish(
        &settings(&endpoint),
        &tree.dir,
        &PublishOptions {
            prune: true,
            ..PublishOptions::default()
        },
    )
    .await
    .unwrap();

    assert_eq!(report.pruned, 1);
    assert!(
        !store
            .lock()
            .unwrap()
            .contains_key("fluorite/textures/icon.png")
    );
}

#[tokio::test]
async fn the_prune_is_off_by_default() {
    let (endpoint, store, _log) = mock_endpoint().await;
    let tree = Tree::new("noprune");
    let options = PublishOptions::default();
    crate::core::s3::publish(&settings(&endpoint), &tree.dir, &options)
        .await
        .unwrap();

    std::fs::remove_file(tree.path("textures/icon.png")).unwrap();
    let report = crate::core::s3::publish(&settings(&endpoint), &tree.dir, &options)
        .await
        .unwrap();

    assert_eq!(report.pruned, 0);
    assert!(
        store
            .lock()
            .unwrap()
            .contains_key("fluorite/textures/icon.png"),
        "an object is not deleted without the prune"
    );
}
