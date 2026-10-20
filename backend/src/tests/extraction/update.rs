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
