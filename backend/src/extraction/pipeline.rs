//! VFS download + extract pipeline shared by multiple binaries.
use crate::core::endfield::{
    config::{
        EndfieldGameConfig, PatchFile, PatchIndex, ResolveParams, fetch_patch_index,
        resolve_game_config,
    },
    constants::{app_code, channel, sub_channel},
};
use crate::core::hypergryph::launcher::Platform;
use crate::extraction::zip_range::{SplitArchivePart, ZipCentralDirectory};
use reqwest::Client;
use std::collections::BTreeSet;
use std::sync::Arc;
use std::sync::atomic::AtomicU64;

/// Known VFS directories.
pub const VFS_DIRS: &[(&str, &str)] = &[("7064D8E2", "bundles/configs"), ("0CE8FA57", "textures")];

/// Archive path prefix under which every VFS directory lives.
pub const VFS_ROOT: &str = "Endfield_Data/StreamingAssets/VFS/";

/// Archive path of a VFS directory's `.blc` catalog.
pub fn blc_path(dir_hash: &str) -> String {
    format!("{VFS_ROOT}{dir_hash}/{dir_hash}.blc")
}

/// Default resolve params for OS Windows release.
pub fn default_resolve_params() -> ResolveParams<'static> {
    ResolveParams {
        app_code: app_code::game::OS_WIN_REL,
        launcher_app_code: app_code::launcher::OS_WIN_REL,
        channel: channel::OS_WIN_REL,
        sub_channel: sub_channel::OS_WIN_REL,
        launcher_sub_channel: sub_channel::OS_WIN_REL,
        platform: Platform::Windows,
    }
}

/// Resolved game configuration with ZIP central directory ready for extraction.
pub struct ResolvedPipeline {
    pub config: EndfieldGameConfig,
    /// Shared so concurrent download tasks can hold it across `tokio::spawn`.
    pub cd: Arc<ZipCentralDirectory>,
    pub patch_index: Option<PatchIndex>,
}

/// Resolve game config, parse ZIP central directory, and optionally fetch patch index.
pub async fn resolve_pipeline(
    client: &Client,
    fetch_patch: bool,
) -> Result<ResolvedPipeline, Box<dyn std::error::Error>> {
    let params = default_resolve_params();
    let config = resolve_game_config(client, &params).await?;

    resolve_pipeline_from_config(client, config, fetch_patch).await
}

/// Resolve ZIP and optional patch index from an already fetched game configuration.
pub async fn resolve_pipeline_from_config(
    client: &Client,
    config: EndfieldGameConfig,
    fetch_patch: bool,
) -> Result<ResolvedPipeline, Box<dyn std::error::Error>> {
    let parts: Vec<SplitArchivePart> = config
        .packs
        .iter()
        .map(|p| SplitArchivePart {
            url: p.url.clone(),
            size: p.package_size.parse::<u64>().expect("invalid package_size"),
        })
        .collect();
    let cd = ZipCentralDirectory::from_split_archive(client, parts).await?;

    let main_resource = config.resources.iter().find(|r| r.name == "main");
    let patch_index = match main_resource {
        Some(main_resource) if fetch_patch => {
            Some(fetch_patch_index(client, &main_resource.path).await?)
        }
        _ => None,
    };

    Ok(ResolvedPipeline {
        config,
        cd: Arc::new(cd),
        patch_index,
    })
}

/// Names and uncompressed sizes of the archive entries matching `filter`, in archive order.
fn entries_with_sizes(
    cd: &ZipCentralDirectory,
    filter: impl Fn(&str) -> bool,
) -> Vec<(String, u64)> {
    cd.filenames()
        .filter(|f| filter(f))
        .map(|f| {
            let size = cd.get(f).map_or(0, |e| e.uncompressed_size);
            (f.to_string(), size)
        })
        .collect()
}

/// List .chk file names and sizes for a VFS directory, sorted by size (ascending).
pub fn list_chk_files(cd: &ZipCentralDirectory, dir: &str) -> Vec<(String, u64)> {
    let mut candidates = entries_with_sizes(cd, |f| f.ends_with(".chk") && f.contains(dir));
    candidates.sort_by_key(|(_n, s)| *s);
    candidates
}

/// List ALL files in a VFS directory (including .blc catalogs), sorted by name.
pub fn list_vfs_dir_files(cd: &ZipCentralDirectory, dir: &str) -> Vec<(String, u64)> {
    let needle = format!("VFS/{dir}/");
    let mut files = entries_with_sizes(cd, |f| f.contains(&needle));
    files.sort_by(|a, b| a.0.cmp(&b.0));
    files
}

/// List ALL VFS directories found in the ZIP.
pub fn list_vfs_directories(cd: &ZipCentralDirectory) -> Vec<String> {
    let mut dirs = BTreeSet::new();
    for f in cd.filenames() {
        if let Some(rest) = f.strip_prefix(VFS_ROOT)
            && let Some(slash) = rest.find('/')
        {
            dirs.insert(rest[..slash].to_string());
        }
    }
    dirs.into_iter().collect()
}

/// VFS directories catalogued by a `.blc` and extracted file-by-file (everything but bundles):
/// those in the archive plus those only the patch index lists (the voice languages).
pub fn blc_directories(cd: &ZipCentralDirectory, patch: Option<&PatchIndex>) -> Vec<String> {
    merge_blc_directories(&list_vfs_directories(cd), patch)
}

/// Archive directories plus patch-index directories, minus the bundle directories; sorted
/// and deduplicated case-insensitively (uppercase hashes win).
pub fn merge_blc_directories(archive_dirs: &[String], patch: Option<&PatchIndex>) -> Vec<String> {
    let mut dirs: BTreeSet<String> = archive_dirs
        .iter()
        .map(|d| d.to_ascii_uppercase())
        .collect();
    if let Some(patch) = patch {
        dirs.extend(patch_directories(patch));
    }
    dirs.into_iter()
        .filter(|dir| {
            !VFS_DIRS
                .iter()
                .any(|(bundle, _)| bundle.eq_ignore_ascii_case(dir))
        })
        .collect()
}

/// Directories (uppercase hashes) the patch index lists files under (`<DIR>/<MD5>.chk`).
pub fn patch_directories(patch: &PatchIndex) -> BTreeSet<String> {
    patch
        .files
        .iter()
        .filter_map(|f| f.name.split_once('/').map(|(dir, _)| dir))
        .filter(|dir| dir.len() == 8 && dir.bytes().all(|b| b.is_ascii_hexdigit()))
        .map(str::to_ascii_uppercase)
        .collect()
}

/// Patch-index files under `dir` (case-insensitive).
pub fn patch_files<'a>(
    patch: &'a PatchIndex,
    dir: &'a str,
) -> impl Iterator<Item = &'a PatchFile> + 'a {
    patch.files.iter().filter(move |f| {
        f.name
            .split_once('/')
            .is_some_and(|(d, _)| d.eq_ignore_ascii_case(dir))
    })
}

/// Voice-language groups: patch-index only, one `.pck` stream package of ~1.5 GB each.
pub const VOICE_GROUPS: &[(&str, &str)] = &[
    ("E1E7D7CE", "AudioChinese"),
    ("A31457D0", "AudioEnglish"),
    ("F668D4EE", "AudioJapanese"),
    ("E9D31017", "AudioKorean"),
];

/// Known BLC groups by directory hash (game 1.6), so the CLI accepts group names.
pub const KNOWN_BLC_GROUPS: &[(&str, &str)] = &[
    ("42A8FCA6", "Table"),
    ("775A31D1", "JsonData"),
    ("19E3AE45", "Lua"),
    ("D6E622F7", "ExtendData"),
    ("3C9D9D2D", "InitialExtendData"),
    ("1CDDBF1F", "BundleManifest"),
    ("07A1BB91", "InitAudio"),
    ("24ED34CF", "Audio"),
    ("55FC21C6", "Video"),
    ("A63D7E6A", "IV"),
    ("F84BF5E6", "Terrain"),
    ("C3442D43", "Streaming"),
    ("23D53F5D", "DynamicStreaming"),
    ("DAFE52C9", "IFixPatchOut"),
    ("F151B649", "HotfixAudio"),
    ("E1E7D7CE", "AudioChinese"),
    ("A31457D0", "AudioEnglish"),
    ("F668D4EE", "AudioJapanese"),
    ("E9D31017", "AudioKorean"),
];

/// A directory hash or known group name (`AudioEnglish`, case-insensitive) as an uppercase hash.
pub fn resolve_dir_name(name: &str) -> String {
    let name = name.trim();
    KNOWN_BLC_GROUPS
        .iter()
        .find(|(_, group)| group.eq_ignore_ascii_case(name))
        .map_or_else(
            || name.to_ascii_uppercase(),
            |(hash, _)| (*hash).to_string(),
        )
}

/// Path of the launcher's `main` resource, the root of the patch CDN.
pub fn main_resource_path(config: &EndfieldGameConfig) -> Option<&str> {
    config
        .resources
        .iter()
        .find(|r| r.name == "main")
        .map(|r| r.path.as_str())
}

/// CDN URL of a VFS file under the main resource, e.g. `A31457D0/A31457D0.blc`.
pub fn cdn_vfs_url(main: &str, relative: &str) -> String {
    format!("{}/VFS/{relative}", main.trim_end_matches('/'))
}

/// CDN URL of a directory's hot-update `.blc` catalog.
pub fn cdn_blc_url(main: &str, dir: &str) -> String {
    cdn_vfs_url(main, &format!("{dir}/{dir}.blc"))
}

/// Download a .chk file from the ZIP by name.
pub async fn download_chk(
    client: &Client,
    cd: &ZipCentralDirectory,
    name: &str,
) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    cd.extract(client, name).await
}

/// Download a .chk file, adding received bytes to `downloaded` as they arrive.
pub async fn download_chk_counting(
    client: &Client,
    cd: &ZipCentralDirectory,
    name: &str,
    downloaded: &AtomicU64,
) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    cd.extract_counting(client, name, Some(downloaded)).await
}
