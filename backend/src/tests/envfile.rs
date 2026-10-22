//! The tests of the `.env` loader.

use std::fs;

use crate::envfile;

/// The file the loader reads, with a variable nobody else uses.
const MARKER: &str = "FLUORITE_TEST_ENV_MARKER";

/// The same, for the test that puts the variable in the environment first.
const MARKER_SET: &str = "FLUORITE_TEST_ENV_MARKER_SET";

#[test]
fn the_file_is_read_into_the_environment() {
    let dir = temp_dir("read");
    let path = dir.join(".env");
    fs::write(&path, format!("{MARKER}=from_the_file\n")).unwrap();

    assert!(envfile::load_path(&path));
    assert_eq!(std::env::var(MARKER).unwrap(), "from_the_file");

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn the_environment_wins_over_the_file() {
    let dir = temp_dir("wins");
    let path = dir.join(".env");
    fs::write(&path, format!("{MARKER_SET}=from_the_file\n")).unwrap();

    // The variable is already set: the file must not overwrite it.
    unsafe { std::env::set_var(MARKER_SET, "from_the_environment") };
    assert!(envfile::load_path(&path));
    assert_eq!(std::env::var(MARKER_SET).unwrap(), "from_the_environment");

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn a_file_that_is_not_there_is_not_an_error() {
    let dir = temp_dir("missing");
    assert!(!envfile::load_path(&dir.join(".env")));
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn the_crate_root_is_one_of_the_candidates() {
    // The binary finds the file of its own checkout, whatever the directory it
    // is started from.
    let candidates = envfile::candidates();
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(".env");
    assert!(candidates.contains(&root));
}

/// A directory of its own, so the tests do not fight over the same file.
fn temp_dir(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("fluorite-envfile-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}
