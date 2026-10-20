//! Persistent state and change detection for the incremental update command.
use crate::core::endfield::config::{EndfieldGameConfig, PatchIndex};
use crate::extraction::pipeline::patch_files;
use crate::extraction::texture_layout::LAYOUT_VERSION;
use crate::extraction::utils::append_to_path;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::Write;
use std::path::Path;

/// Bump when gamedata/mesh output for a `.chk` changes and every chunk must be re-extracted.
/// 2: container-path gamedata layout, typed TextAssets, Mesh → glTF.
/// 3: meshes flagged compressed but stored plainly, and objects with >1M-element arrays.
pub const EXTRACT_VERSION: u32 = 3;

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
    /// ZIP digest of the `GameAssembly.dll` the newest MemoryPack schema was generated from;
    /// empty until a schema has been generated, so the first run generates one.
    #[serde(default)]
    pub memorypack_client: String,
    /// Digest of a `GameAssembly.dll` whose schema regeneration was refused or failed, so
    /// later runs do not download the same client again; cleared by a successful run.
    #[serde(default)]
    pub memorypack_attempted: String,
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

/// The ZIP identity a `.chk` digest starts with, before any `+tag`.
pub fn digest_base(digest: &str) -> &str {
    digest.split('+').next().unwrap_or(digest)
}

/// Digest for a chunk whose PNGs were sorted out of the legacy flat layout: they come from the
/// old decoder (upside down, `+tex1`) and its gamedata and meshes are not extracted yet.
pub fn sorted_legacy_digest(digest: &str) -> String {
    format!("{}+tex1", digest_base(digest))
}

/// Whether a `.chk` digest was produced by the current gamedata/mesh extraction.
pub fn extract_current(digest: &str) -> bool {
    digest.contains(&format!("+x{EXTRACT_VERSION}"))
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
    format!("{}+texstale", digest_base(digest))
}

/// Texture layout that flipping `+tex1` PNGs in place produces.
// Pinned rather than `LAYOUT_VERSION`: a later layout bump must not mark flipped files current.
const FLIPPED_LAYOUT_VERSION: u32 = 2;

/// Rewrite a `+tex1` digest to `+tex2` once that chunk's PNGs were flipped in place.
pub fn mark_textures_flipped(digest: &str) -> Option<String> {
    let base = digest.strip_suffix("+tex1")?;
    Some(format!("{base}+tex{FLIPPED_LAYOUT_VERSION}"))
}

/// Marks the part of a BLC digest that identifies a catalog read from the patch CDN.
pub const CDN_CATALOG_TAG: &str = "cdn-blc";

/// Stands in for [`catalog_digest`] when the CDN has no catalog for a hot-updated directory
/// and the archive catalog was used instead.
pub const CDN_CATALOG_MISSING: &str = "missing";

/// Identity of a raw catalog read from the patch CDN: FNV-1a 64 of its bytes plus its length.
pub fn catalog_digest(bytes: &[u8]) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in bytes {
        hash ^= u64::from(b);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    format!("{hash:016x}-{}", bytes.len())
}

/// Digest of a BLC directory: the archive catalog's ZIP identity (`None` when only the patch
/// index lists the directory), the CDN catalog's [`catalog_digest`] when it is read from
/// the CDN (see `blc::uses_cdn_catalog`), and every patch file under it as `name:md5`, sorted.
///
/// The CDN catalog covers chunks the patch index leaves out; tagging it also makes
/// directories extracted from the archive catalog alone re-extract once.
pub fn blc_digest(
    archive_catalog: Option<String>,
    cdn_catalog: Option<String>,
    patch: Option<&PatchIndex>,
    dir: &str,
) -> String {
    let mut patched: Vec<String> = patch
        .map(|index| {
            patch_files(index, dir)
                .map(|f| format!("{}:{}", f.name, f.md5))
                .collect()
        })
        .unwrap_or_default();
    patched.sort();
    let mut parts: Vec<String> = archive_catalog.into_iter().collect();
    parts.extend(cdn_catalog.map(|d| format!("{CDN_CATALOG_TAG}={d}")));
    parts.extend(patched);
    parts.join(";")
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
    let tmp = append_to_path(path, ".tmp");
    let bytes = serde_json::to_vec_pretty(state)?;
    let mut file = File::create(&tmp)?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    fs::rename(&tmp, path)?;
    Ok(())
}
