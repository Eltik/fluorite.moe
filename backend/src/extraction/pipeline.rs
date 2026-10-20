/// VFS download + extract pipeline shared by multiple binaries.
use crate::core::endfield::{
    config::{
        EndfieldGameConfig, PatchIndex, ResolveParams, fetch_patch_index, resolve_game_config,
    },
    constants::{app_code, channel, sub_channel},
};
use crate::core::hypergryph::launcher::Platform;
use crate::extraction::zip_range::{SplitArchivePart, ZipCentralDirectory};
use reqwest::Client;
use std::sync::Arc;

/// Known VFS directories.
pub const VFS_DIRS: &[(&str, &str)] = &[("7064D8E2", "bundles/configs"), ("0CE8FA57", "textures")];

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

    let patch_index = if fetch_patch {
        if let Some(main_resource) = config.resources.iter().find(|r| r.name == "main") {
            Some(fetch_patch_index(client, &main_resource.path).await?)
        } else {
            None
        }
    } else {
        None
    };

    Ok(ResolvedPipeline {
        config,
        cd: Arc::new(cd),
        patch_index,
    })
}

/// List .chk file names and sizes for a VFS directory, sorted by size (ascending).
pub fn list_chk_files(cd: &ZipCentralDirectory, dir: &str) -> Vec<(String, u64)> {
    let mut candidates: Vec<(String, u64)> = cd
        .filenames()
        .filter(|f| f.ends_with(".chk") && f.contains(dir))
        .map(|f| {
            let size = cd.get(f).map_or(0, |e| e.uncompressed_size);
            (f.to_string(), size)
        })
        .collect();
    candidates.sort_by_key(|(_n, s)| *s);
    candidates
}

/// List ALL files in a VFS directory (including .blc catalogs), sorted by name.
pub fn list_vfs_dir_files(cd: &ZipCentralDirectory, dir: &str) -> Vec<(String, u64)> {
    let mut files: Vec<(String, u64)> = cd
        .filenames()
        .filter(|f| f.contains(&format!("VFS/{dir}/")))
        .map(|f| {
            let size = cd.get(f).map_or(0, |e| e.uncompressed_size);
            (f.to_string(), size)
        })
        .collect();
    files.sort_by(|a, b| a.0.cmp(&b.0));
    files
}

/// List ALL VFS directories found in the ZIP.
pub fn list_vfs_directories(cd: &ZipCentralDirectory) -> Vec<String> {
    let mut dirs: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for f in cd.filenames() {
        if let Some(rest) = f.strip_prefix("Endfield_Data/StreamingAssets/VFS/")
            && let Some(slash) = rest.find('/')
        {
            dirs.insert(rest[..slash].to_string());
        }
    }
    dirs.into_iter().collect()
}

/// Download a .chk file from the ZIP by name.
pub async fn download_chk(
    client: &Client,
    cd: &ZipCentralDirectory,
    name: &str,
) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    cd.extract(client, name).await
}
