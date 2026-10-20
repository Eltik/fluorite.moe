use crate::extraction::texture_layout::{
    self, IndexEntry, Placement, rebuild_index, remove_chk_indexes_except,
    remove_legacy_flat_textures, write_chk_index,
};
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

fn temp_dir(label: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path =
        std::env::temp_dir().join(format!("fluorite-{label}-{}-{nonce}", std::process::id()));
    fs::create_dir_all(&path).unwrap();
    path
}

fn index_entry(path: &str) -> IndexEntry {
    IndexEntry {
        path: path.into(),
        name: "Texture".into(),
        container: None,
        placement: "unsorted",
        width: 1,
        height: 1,
        format: "RGBA32",
    }
}

#[test]
fn relative_path_uses_container_placement() {
    assert_eq!(
        texture_layout::relative_path(
            Some(&Placement::Direct(
                "assets/beyond/initialassets/ui/sprites/login/login_btn_start.png".into()
            )),
            "Start",
            1,
        ),
        Path::new("beyond/initialassets/ui/sprites/login/Start.png")
    );
    assert_eq!(
        texture_layout::relative_path(
            Some(&Placement::Dependency(
                "assets/beyond/ui/login/loginmockuidpanel.prefab".into()
            )),
            "Panel",
            2,
        ),
        Path::new("beyond/ui/login/loginmockuidpanel/Panel.png")
    );
    assert_eq!(
        texture_layout::relative_path(
            Some(&Placement::Direct("packages/com.game/ui/icon.png".into())),
            "Icon",
            3,
        ),
        Path::new("packages/com.game/ui/Icon.png")
    );
}

#[test]
fn relative_path_sanitizes_components_and_falls_back() {
    assert_eq!(
        texture_layout::relative_path(
            Some(&Placement::Direct("assets//beyond/../ui/./icon.png".into())),
            "A/B",
            4,
        ),
        Path::new("beyond/ui/A_B.png")
    );
    assert_eq!(
        texture_layout::relative_path(None, "", 42),
        Path::new("_unsorted/obj_42__42.png")
    );
}

#[test]
fn legacy_flat_texture_removal_is_not_recursive() {
    let dir = temp_dir("legacy-textures");
    fs::write(dir.join("one.png"), []).unwrap();
    fs::write(dir.join("two.PNG"), []).unwrap();
    fs::write(dir.join("keep.txt"), []).unwrap();
    fs::create_dir(dir.join("nested")).unwrap();
    fs::write(dir.join("nested/keep.png"), []).unwrap();

    assert_eq!(remove_legacy_flat_textures(&dir).unwrap(), 2);
    assert!(!dir.join("one.png").exists());
    assert!(!dir.join("two.PNG").exists());
    assert!(dir.join("keep.txt").exists());
    assert!(dir.join("nested/keep.png").exists());
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn chunk_indexes_are_pruned_and_rebuilt() {
    let dir = temp_dir("texture-index");
    write_chk_index(&dir, "dir/one.chk", &[index_entry("one.png")]).unwrap();
    write_chk_index(&dir, "two.chk", &[index_entry("two.png")]).unwrap();
    assert_eq!(
        remove_chk_indexes_except(&dir, &HashSet::from(["dir/one.chk".into()])).unwrap(),
        1
    );
    assert_eq!(rebuild_index(&dir).unwrap(), 1);
    let index = fs::read_to_string(dir.join("index.jsonl")).unwrap();
    assert!(index.contains("one.png"));
    assert!(!index.contains("two.png"));
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn placement_dir_matches_texture_layout() {
    assert_eq!(
        texture_layout::placement_dir(Some(&Placement::Direct("assets/beyond/ui/a.json".into()))),
        Some(PathBuf::from("beyond/ui"))
    );
    assert_eq!(
        texture_layout::placement_dir(Some(&Placement::Dependency(
            "assets/beyond/fx/hit.prefab".into()
        ))),
        Some(PathBuf::from("beyond/fx/hit"))
    );
    assert_eq!(
        texture_layout::placement_dir(Some(&Placement::Direct("a.json".into()))),
        Some(PathBuf::from("_root"))
    );
    assert_eq!(texture_layout::placement_dir(None), None);
}

#[test]
fn flip_png_in_place_reverses_rows() {
    let dir = temp_dir("flip");
    let path = dir.join("t.png");
    image::save_buffer(
        &path,
        &[255, 0, 0, 255, 0, 0, 255, 255],
        1,
        2,
        image::ColorType::Rgba8,
    )
    .unwrap();
    texture_layout::flip_png_in_place(&path).unwrap();
    let flipped = image::open(&path).unwrap().to_rgba8();
    assert_eq!(flipped.as_raw(), &[0, 0, 255, 255, 255, 0, 0, 255]);
    assert!(!path.with_extension("png.tmp").exists());
    fs::remove_dir_all(dir).unwrap();
}
