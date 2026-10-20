use crate::core::endfield::config::{PatchFile, PatchIndex};
use crate::extraction::pipeline::{
    cdn_blc_url, cdn_vfs_url, merge_blc_directories, patch_directories, patch_files,
    resolve_dir_name,
};

fn patch(names: &[&str]) -> PatchIndex {
    PatchIndex {
        version: "1".into(),
        files: names
            .iter()
            .map(|name| PatchFile {
                name: (*name).into(),
                md5: format!("md5-{name}"),
                size: 1,
                diff_type: 0,
                patch: Vec::new(),
            })
            .collect(),
    }
}

#[test]
fn patch_directories_takes_hash_prefixes_only() {
    let index = patch(&[
        "a31457d0/DA3F.chk",
        "A31457D0/EAC5.chk",
        "42A8FCA6/0575.chk",
        "patch.json",
        "notahash/x.chk",
    ]);
    let dirs: Vec<String> = patch_directories(&index).into_iter().collect();
    assert_eq!(dirs, ["42A8FCA6", "A31457D0"]);
}

#[test]
fn merge_adds_patch_only_directories_without_bundles() {
    let archive = vec!["42A8FCA6".to_string(), "7064D8E2".to_string()];
    let index = patch(&[
        "42A8FCA6/0575.chk",
        "0CE8FA57/927D.chk",
        "E1E7D7CE/10E5.chk",
        "A31457D0/DA3F.chk",
    ]);
    assert_eq!(
        merge_blc_directories(&archive, Some(&index)),
        ["42A8FCA6", "A31457D0", "E1E7D7CE"]
    );
    assert_eq!(merge_blc_directories(&archive, None), ["42A8FCA6"]);
}

#[test]
fn patch_files_match_directory_case_insensitively() {
    let index = patch(&["A31457D0/DA3F.chk", "a31457d0/EAC5.chk", "A31457D01/X.chk"]);
    let names: Vec<&str> = patch_files(&index, "A31457D0")
        .map(|f| f.name.as_str())
        .collect();
    assert_eq!(names, ["A31457D0/DA3F.chk", "a31457d0/EAC5.chk"]);
}

#[test]
fn resolves_group_names_and_hashes() {
    assert_eq!(resolve_dir_name("AudioEnglish"), "A31457D0");
    assert_eq!(resolve_dir_name("audiokorean"), "E9D31017");
    assert_eq!(resolve_dir_name("video"), "55FC21C6");
    assert_eq!(resolve_dir_name(" 55fc21c6 "), "55FC21C6");
    assert_eq!(resolve_dir_name("DEADBEEF"), "DEADBEEF");
}

#[test]
fn cdn_urls_live_under_vfs() {
    assert_eq!(
        cdn_blc_url("https://cdn/main/files/", "A31457D0"),
        "https://cdn/main/files/VFS/A31457D0/A31457D0.blc"
    );
    assert_eq!(
        cdn_vfs_url("https://cdn/main/files", "A31457D0/DA3F.chk"),
        "https://cdn/main/files/VFS/A31457D0/DA3F.chk"
    );
}
