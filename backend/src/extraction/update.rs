/// Persistent state and change detection for the incremental update command.
use crate::core::endfield::config::EndfieldGameConfig;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::Write;
use std::path::Path;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UpdateState {
    pub fingerprint: String,
    pub version: String,
    pub res_version: String,
    #[serde(default)]
    pub updated_at: String,
    #[serde(default)]
    pub chk: BTreeMap<String, String>,
    #[serde(default)]
    pub blc_dirs: BTreeMap<String, String>,
}

/// Produce a stable launcher-only identity for a game configuration.
pub fn fingerprint(config: &EndfieldGameConfig) -> String {
    let mut resources: Vec<_> = config.resources.iter().collect();
    resources.sort_by(|a, b| a.name.cmp(&b.name));
    let packs = config
        .packs
        .iter()
        .map(|p| p.md5.as_str())
        .collect::<Vec<_>>();
    let resources = resources
        .iter()
        .map(|r| format!("{}:{}", r.name, r.version))
        .collect::<Vec<_>>();
    format!(
        "version={};res_version={};packs={};resources={};texture_layout={}",
        config.version,
        config.res_version,
        packs.join(","),
        resources.join(","),
        crate::extraction::texture_layout::LAYOUT_VERSION
    )
}

/// Format ZIP central-directory identity used for `.chk` and `.blc` entries.
pub fn zip_digest(crc32: u32, uncompressed_size: u64) -> String {
    format!("{:08x}-{}", crc32, uncompressed_size)
}

/// Whether an entry needs work, independent of I/O and suitable for tests.
pub fn is_dirty(
    previous: &BTreeMap<String, String>,
    name: &str,
    digest: &str,
    force: bool,
) -> bool {
    force || previous.get(name).is_none_or(|old| old != digest)
}

pub fn load_state(path: &Path) -> Result<UpdateState, Box<dyn std::error::Error>> {
    if !path.exists() {
        return Ok(UpdateState::default());
    }
    Ok(serde_json::from_slice(&fs::read(path)?)?)
}

/// Atomically save state, syncing the temporary file before its replacement.
pub fn save_state(path: &Path, state: &UpdateState) -> Result<(), Box<dyn std::error::Error>> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent)?;
    }
    let mut tmp_name = path.as_os_str().to_os_string();
    tmp_name.push(".tmp");
    let tmp = std::path::PathBuf::from(tmp_name);
    let bytes = serde_json::to_vec_pretty(state)?;
    let mut file = File::create(&tmp)?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    fs::rename(&tmp, path)?;
    Ok(())
}
