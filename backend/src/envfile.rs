//! Loading the `.env` file into the environment.
//!
//! The project keeps its configuration in one file, `backend/.env`: the API, the
//! unpacker and the shell scripts all read it, so a deployment is that file plus the
//! checkout. The shell scripts read it with `load_env` (see `scripts/env.sh`); the
//! Rust binaries load it here, before they read any variable.
//!
//! A variable that is already in the environment wins over the file, so a systemd
//! `Environment=`, a `FOO=1 ./endfield` or a `docker run -e` overrides the file
//! without anyone having to edit it.

use std::path::{Path, PathBuf};

/// The name of the file.
const FILE: &str = ".env";

/// The variable that points at the file, when the default locations do not fit.
pub const FILE_VARIABLE: &str = "FLUORITE_ENV_FILE";

/// A variable of the environment, when it is set and not blank.
///
/// A blank value counts as unset: the template of the `.env` leaves the optional
/// variables empty, and an empty value must not switch anything on.
pub fn var(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.trim().is_empty())
}

/// Load the `.env` file into the environment, without overriding what is already set.
///
/// The candidates, in order:
///
/// 1. the file named by `FLUORITE_ENV_FILE`,
/// 2. `.env` in the current directory,
/// 3. `.env` in the crate root — the directory the binary was built from, so a
///    binary started from anywhere finds the file of its own checkout.
///
/// The first one that exists is the one that is read. A file that cannot be read
/// is ignored: the variables that are already in the environment still work, and
/// the server says what it is missing.
pub fn load() {
    // An empty variable is the same as no variable: the template of the `.env`
    // has it empty, and it must not stop the file from being read.
    if let Some(path) = std::env::var_os(FILE_VARIABLE).filter(|p| !p.is_empty()) {
        read(Path::new(&path));
        return;
    }
    for path in candidates() {
        if path.is_file() {
            read(&path);
            return;
        }
    }
}

/// Read one file, and say so when it is malformed: the parser stops at the first bad
/// line, so every variable after it would otherwise go missing without a word.
fn read(path: &Path) {
    if let Err(error) = dotenvy::from_path(path) {
        eprintln!("{}: {error} (the lines after it were not read)", path.display());
    }
}

/// Read one file into the environment, without overriding what is already set.
///
/// Returns whether the file was there and could be read.
pub fn load_path(path: &Path) -> bool {
    path.is_file() && dotenvy::from_path(path).is_ok()
}

/// The default locations of the file, in the order they are tried.
///
/// The first one is the `.env` of the current directory, the second the one of
/// the crate the binary was built from.
pub fn candidates() -> Vec<PathBuf> {
    let mut paths = vec![PathBuf::from(FILE)];
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR")).join(FILE);
    if manifest != paths[0] {
        paths.push(manifest);
    }
    paths
}
