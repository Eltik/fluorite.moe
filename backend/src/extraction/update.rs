/// Persistent state and change detection for the incremental update command.
use crate::core::endfield::config::EndfieldGameConfig;
use crate::extraction::texture_layout::LAYOUT_VERSION;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::Write;
use std::path::Path;

/// Bump when gamedata/mesh output for a `.chk` changes and every chunk must be re-extracted.
/// 2: container-path gamedata layout, typed TextAssets, Mesh → glTF.
pub const EXTRACT_VERSION: u32 = 2;

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
        "version={};res_version={};packs={};resources={};texture_layout={};extract={}",
        config.version,
        config.res_version,
        packs.join(","),
        resources.join(","),
        LAYOUT_VERSION,
        EXTRACT_VERSION
    )
}

/// Format ZIP central-directory identity used for `.chk` and `.blc` entries.
pub fn zip_digest(crc32: u32, uncompressed_size: u64) -> String {
    format!("{:08x}-{}", crc32, uncompressed_size)
}

/// Digest recorded for a processed `.chk`: its ZIP identity tagged with the output versions.
pub fn chk_digest(base: &str, textures: bool, meshes: bool) -> String {
    let mut digest = format!("{base}+x{EXTRACT_VERSION}");
    if !meshes {
        digest.push_str("+nomesh");
    }
    if textures {
        digest.push_str(&format!("+tex{LAYOUT_VERSION}"));
    } else {
        // Tagged so a later run with textures sees these chks as dirty.
        digest.push_str("+notex");
    }
    digest
}

/// Whether PNGs left by a previous run of a `.chk` already match the current texture layout,
/// so re-extraction may keep them instead of decoding again.
pub fn textures_current(previous: Option<&str>) -> bool {
    previous.is_some_and(|digest| digest.ends_with(&format!("+tex{LAYOUT_VERSION}")))
}

/// Mark a chunk's on-disk PNGs as untrusted: neither `repair` nor `update` may keep them,
/// so the next update decodes every texture again. Used when a re-extraction starts on
/// `+tex1` PNGs (an interruption would leave a mix of orientations) and when `repair`
/// cannot tell which files belong to a chunk.
pub fn mark_textures_stale(digest: &str) -> String {
    let base = digest.split('+').next().unwrap_or(digest);
    format!("{base}+texstale")
}

/// Rewrite a `+tex1` digest to `+tex2` once that chunk's PNGs were flipped in place.
pub fn mark_textures_flipped(digest: &str) -> Option<String> {
    let base = digest.strip_suffix("+tex1")?;
    Some(format!("{base}+tex{LAYOUT_VERSION}"))
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
