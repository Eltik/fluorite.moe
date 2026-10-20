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
