use crate::extraction::texture_layout::Placement;
use crate::extraction::texture_sort::{SortCandidate, sort_flat_textures};
use std::fs;
use std::path::{Path, PathBuf};

fn temp_dir(name: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "backend-texture-sort-{name}-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&path);
    fs::create_dir_all(&path).unwrap();
    path
}

fn png(path: &Path, width: u32, height: u32) {
    image::save_buffer(
        path,
        &vec![0; (width * height * 4) as usize],
        width,
        height,
        image::ColorType::Rgba8,
    )
    .unwrap();
}

fn candidate(name: &str, relative: &str, width: u32, height: u32) -> SortCandidate {
    SortCandidate {
        chk_name: "a.chk".to_string(),
        relative: PathBuf::from(relative),
        name: name.to_string(),
        placement: Some(Placement::Direct("assets/x/file".to_string())),
        width,
        height,
        format: "RGBA32",
    }
}

#[test]
fn moves_single_candidate() {
    let dir = temp_dir("single");
    png(&dir.join("one.png"), 1, 1);
    let report = sort_flat_textures(&dir, &[candidate("one", "a/one.png", 1, 1)]).unwrap();
    assert_eq!(report.moved, 1);
    assert!(dir.join("a/one.png").is_file());
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn disambiguates_by_dimensions() {
    let dir = temp_dir("dimensions");
    png(&dir.join("same.png"), 2, 3);
    let report = sort_flat_textures(
        &dir,
        &[
            candidate("same", "a/same.png", 1, 1),
            candidate("same", "b/same.png", 2, 3),
        ],
    )
    .unwrap();
    assert_eq!(report.moved, 1);
    assert!(dir.join("b/same.png").is_file());
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn same_size_is_ambiguous() {
    let dir = temp_dir("ambiguous");
    png(&dir.join("same.png"), 2, 2);
    let report = sort_flat_textures(
        &dir,
        &[
            candidate("same", "a/same.png", 2, 2),
            candidate("same", "b/same.png", 2, 2),
        ],
    )
    .unwrap();
    assert_eq!(report.ambiguous, 1);
    assert!(dir.join("_ambiguous/same.png").is_file());
    assert!(
        fs::read_to_string(dir.join("_ambiguous/candidates.jsonl"))
            .unwrap()
            .contains("a/same.png")
    );
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn duplicate_relative_paths_are_deduped() {
    let dir = temp_dir("dedupe");
    png(&dir.join("same.png"), 1, 1);
    let report = sort_flat_textures(
        &dir,
        &[
            candidate("same", "a/same.png", 1, 1),
            candidate("same", "a/same.png", 1, 1),
        ],
    )
    .unwrap();
    assert_eq!(report.moved, 1);
    assert_eq!(report.missing, 0);
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn existing_target_deletes_flat_file() {
    let dir = temp_dir("existing");
    png(&dir.join("same.png"), 1, 1);
    fs::create_dir_all(dir.join("a")).unwrap();
    png(&dir.join("a/same.png"), 1, 1);
    let report = sort_flat_textures(&dir, &[candidate("same", "a/same.png", 1, 1)]).unwrap();
    assert_eq!(report.already_sorted, 1);
    assert!(!dir.join("same.png").exists());
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn unmatched_is_left_in_place() {
    let dir = temp_dir("unmatched");
    png(&dir.join("old.png"), 1, 1);
    let report = sort_flat_textures(&dir, &[]).unwrap();
    assert_eq!(report.unmatched, 1);
    assert!(dir.join("old.png").is_file());
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn missing_is_counted() {
    let dir = temp_dir("missing");
    let report = sort_flat_textures(&dir, &[candidate("gone", "a/gone.png", 1, 1)]).unwrap();
    assert_eq!(report.missing, 1);
    let _ = fs::remove_dir_all(dir);
}
