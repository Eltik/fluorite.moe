use crate::extraction::gamedata::{
    TextKind, remove_legacy_gamedata, sniff_extension, text_asset_filename,
};
use std::fs;

#[test]
fn sniffs_common_formats() {
    assert_eq!(sniff_extension(b"glTF\x02\0\0\0"), Some("glb"));
    assert_eq!(sniff_extension(b"RIFF\0\0\0\0WEBPVP8 "), Some("webp"));
    assert_eq!(sniff_extension(b"RIFF\0\0\0\0WAVEfmt "), Some("wav"));
    assert_eq!(sniff_extension(b"\x89PNG\r\n\x1a\n...."), Some("png"));
    assert_eq!(sniff_extension(b"plain"), None);
}

#[test]
fn container_extension_wins_and_bytes_suffix_is_dropped() {
    let json = br#"{"a":1}"#;
    assert_eq!(
        text_asset_filename("dialog", Some("assets/x/dialog.json"), json),
        ("dialog.json".into(), TextKind::Json)
    );
    // Spine skeletons ship as `<name>.skel.bytes`.
    assert_eq!(
        text_asset_filename("hero.skel", Some("assets/x/hero.skel.bytes"), &[0, 1, 2]),
        ("hero.skel".into(), TextKind::Binary)
    );
    assert_eq!(
        text_asset_filename(
            "hero.atlas",
            Some("assets/x/hero.atlas.txt"),
            b"hero.png\nsize: 1,1"
        ),
        ("hero.atlas.txt".into(), TextKind::Text)
    );
}

#[test]
fn falls_back_to_sniffing_and_content_shape() {
    assert_eq!(
        text_asset_filename("model", Some("assets/x/model.bytes"), b"glTF\x02\0\0\0"),
        ("model.glb".into(), TextKind::Binary)
    );
    assert_eq!(
        text_asset_filename("cfg", None, b"[1, 2]"),
        ("cfg.json".into(), TextKind::Json)
    );
    assert_eq!(
        text_asset_filename("notes", None, b"hello"),
        ("notes.txt".into(), TextKind::Text)
    );
    assert_eq!(
        text_asset_filename("grid_-3_-9", None, &[0, 0, 0, 0]),
        ("grid_-3_-9.bytes".into(), TextKind::Binary)
    );
    assert_eq!(
        text_asset_filename("a/b:c", None, b"x"),
        ("a_b_c.txt".into(), TextKind::Text)
    );
}

#[test]
fn legacy_gamedata_removal_keeps_container_tree() {
    let dir = std::env::temp_dir().join(format!("fluorite-gamedata-{}", std::process::id()));
    fs::create_dir_all(dir.join("monobehaviour/script_x")).unwrap();
    fs::create_dir_all(dir.join("beyond/data")).unwrap();
    fs::write(dir.join("1001.json"), "{}").unwrap();
    fs::write(dir.join("monobehaviour/script_x/a.json"), "{}").unwrap();
    fs::write(dir.join("beyond/data/keep.json"), "{}").unwrap();

    assert_eq!(remove_legacy_gamedata(&dir).unwrap(), 2);
    assert!(!dir.join("1001.json").exists());
    assert!(!dir.join("monobehaviour").exists());
    assert!(dir.join("beyond/data/keep.json").exists());
    fs::remove_dir_all(dir).unwrap();
}
