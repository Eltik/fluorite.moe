use crate::core::endfield::config::EndfieldGameConfig;
use crate::core::hypergryph::launcher::{LauncherLatestGamePack, LauncherLatestGameResource};
use crate::extraction::update::{self, UpdateState};
use std::collections::BTreeMap;

fn config(resources: Vec<LauncherLatestGameResource>) -> EndfieldGameConfig {
    EndfieldGameConfig {
        version: "1.2.3".into(),
        game_version: "1.2".into(),
        rand_str: "x".into(),
        packs: vec![LauncherLatestGamePack {
            url: "u".into(),
            md5: "pack".into(),
            package_size: "1".into(),
        }],
        resources,
        domain: String::new(),
        res_version: "r1".into(),
        patch_index_path: String::new(),
        configs: String::new(),
    }
}

#[test]
fn fingerprint_is_independent_of_resource_order() {
    let a = LauncherLatestGameResource {
        name: "a".into(),
        version: "1".into(),
        path: String::new(),
    };
    let b = LauncherLatestGameResource {
        name: "b".into(),
        version: "2".into(),
        path: String::new(),
    };
    assert_eq!(
        update::fingerprint(&config(vec![a.clone(), b.clone()])),
        update::fingerprint(&config(vec![b, a]))
    );
}

#[test]
fn state_missing_save_and_load_round_trip() {
    let path = std::env::temp_dir()
        .join(format!("fluorite-update-test-{}", std::process::id()))
        .join("state.json");
    assert!(update::load_state(&path).unwrap().fingerprint.is_empty());
    let state = UpdateState {
        fingerprint: "abc".into(),
        chk: BTreeMap::from([("x".into(), "d".into())]),
        ..Default::default()
    };
    update::save_state(&path, &state).unwrap();
    assert_eq!(update::load_state(&path).unwrap().fingerprint, "abc");
    std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
}

#[test]
fn dirty_detection_checks_digest_and_force() {
    let old = BTreeMap::from([("x".into(), "one".into())]);
    assert!(!update::is_dirty(&old, "x", "one", false));
    assert!(update::is_dirty(&old, "x", "two", false));
    assert!(update::is_dirty(&old, "missing", "one", false));
    assert!(update::is_dirty(&old, "x", "one", true));
}

#[test]
fn chk_digest_tags_output_versions() {
    let tex = format!("+tex{}", crate::extraction::texture_layout::LAYOUT_VERSION);
    let x = format!("+x{}", update::EXTRACT_VERSION);
    assert_eq!(
        update::chk_digest("ab-1", true, true),
        format!("ab-1{x}{tex}")
    );
    assert_eq!(
        update::chk_digest("ab-1", false, false),
        format!("ab-1{x}+nomesh+notex")
    );
}

#[test]
fn flipped_textures_become_current_but_stay_dirty() {
    assert!(!update::textures_current(Some("ab-1+tex1")));
    assert!(!update::textures_current(None));
    let marked = update::mark_textures_flipped("ab-1+tex1").unwrap();
    assert!(update::textures_current(Some(&marked)));
    // Flipping yields layout 2 specifically, whatever the current layout is.
    assert_eq!(marked, "ab-1+tex2");
    // Gamedata/meshes still need the new extraction pass.
    assert_ne!(marked, update::chk_digest("ab-1", true, true));
    assert!(update::mark_textures_flipped(&marked).is_none());
    assert!(update::mark_textures_flipped("ab-1+notex").is_none());
}

#[test]
fn stale_textures_are_neither_kept_nor_flipped() {
    let stale = update::mark_textures_stale("ab-1+tex1");
    assert_eq!(stale, "ab-1+texstale");
    assert!(!update::textures_current(Some(&stale)));
    assert!(update::mark_textures_flipped(&stale).is_none());
    assert_eq!(update::mark_textures_stale("ab-1+x2+tex2"), "ab-1+texstale");
}

#[test]
fn old_state_files_load_without_a_schema_client() {
    let old = r#"{"fingerprint": "f", "version": "1.6.4", "res_version": "r"}"#;
    let state: UpdateState = serde_json::from_str(old).unwrap();
    assert_eq!(state.memorypack_client, "");
    let mut state = state;
    state.memorypack_client = update::zip_digest(0xabc, 42);
    let text = serde_json::to_string(&state).unwrap();
    let back: UpdateState = serde_json::from_str(&text).unwrap();
    assert_eq!(back.memorypack_client, "00000abc-42");
}

fn patch_index(files: &[(&str, &str)]) -> crate::core::endfield::config::PatchIndex {
    crate::core::endfield::config::PatchIndex {
        version: "1".into(),
        files: files
            .iter()
            .map(|(name, md5)| crate::core::endfield::config::PatchFile {
                name: (*name).into(),
                md5: (*md5).into(),
                size: 1,
                diff_type: 0,
                patch: Vec::new(),
            })
            .collect(),
    }
}

#[test]
fn blc_digest_of_archive_only_directory_is_its_zip_identity() {
    let index = patch_index(&[("A31457D0/X.chk", "m")]);
    assert_eq!(
        update::blc_digest(Some("abc-1".into()), None, Some(&index), "42A8FCA6"),
        "abc-1"
    );
    assert_eq!(
        update::blc_digest(Some("abc-1".into()), None, None, "42A8FCA6"),
        "abc-1"
    );
}

#[test]
fn blc_digest_of_patched_directory_tracks_catalog_and_patch_md5s_in_order() {
    let a = patch_index(&[("42A8FCA6/B.chk", "2"), ("42A8FCA6/A.chk", "1")]);
    let b = patch_index(&[("42A8FCA6/A.chk", "1"), ("42A8FCA6/B.chk", "2")]);
    let digest = update::blc_digest(
        Some("abc-1".into()),
        Some("c1".into()),
        Some(&a),
        "42A8FCA6",
    );
    assert_eq!(digest, "abc-1;cdn-blc=c1;42A8FCA6/A.chk:1;42A8FCA6/B.chk:2");
    assert_eq!(
        digest,
        update::blc_digest(
            Some("abc-1".into()),
            Some("c1".into()),
            Some(&b),
            "42A8FCA6"
        )
    );
    // An archive-catalog digest of the same directory (older runs) differs, so it re-extracts.
    assert_ne!(digest, "abc-1;42A8FCA6/A.chk:1;42A8FCA6/B.chk:2");
}

#[test]
fn blc_digest_of_patch_only_directory_changes_with_its_chunks_or_catalog() {
    let before = patch_index(&[("A31457D0/DA3F.chk", "1"), ("E1E7D7CE/10E5.chk", "x")]);
    let after = patch_index(&[("A31457D0/DA3F.chk", "2"), ("E1E7D7CE/10E5.chk", "x")]);
    let old = update::blc_digest(None, Some("c1".into()), Some(&before), "A31457D0");
    assert_eq!(old, "cdn-blc=c1;A31457D0/DA3F.chk:1");
    assert_ne!(
        old,
        update::blc_digest(None, Some("c1".into()), Some(&after), "A31457D0")
    );
    // A chunk only the catalog names changes the catalog, not the patch index.
    assert_ne!(
        old,
        update::blc_digest(None, Some("c2".into()), Some(&before), "A31457D0")
    );
    // Another directory's chunk does not affect it.
    assert_eq!(
        update::blc_digest(None, Some("c".into()), Some(&before), "E1E7D7CE"),
        update::blc_digest(None, Some("c".into()), Some(&after), "E1E7D7CE")
    );
}

#[test]
fn catalog_digest_depends_on_bytes_and_length() {
    assert_eq!(
        update::catalog_digest(b"abc"),
        update::catalog_digest(b"abc")
    );
    assert_ne!(
        update::catalog_digest(b"abc"),
        update::catalog_digest(b"abd")
    );
    assert!(update::catalog_digest(b"").ends_with("-0"));
}
