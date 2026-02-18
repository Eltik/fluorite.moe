use reqwest::Client;
use serde::{Deserialize, Serialize};

use crate::core::hypergryph::launcher::{
    self, LauncherLatestGamePack, LauncherLatestGameResource, Platform,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EndfieldGameConfig {
    pub version: String,
    pub game_version: String,
    pub rand_str: String,
    pub packs: Vec<LauncherLatestGamePack>,
    pub resources: Vec<LauncherLatestGameResource>,
    pub domain: String,
    pub res_version: String,
    pub patch_index_path: String,
    pub configs: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PatchIndex {
    pub version: String,
    pub files: Vec<PatchFile>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PatchFile {
    pub name: String,
    pub md5: String,
    pub size: u64,
    #[serde(rename = "diffType")]
    pub diff_type: i32,
    #[serde(default)]
    pub patch: Vec<PatchEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PatchEntry {
    pub base_file: String,
    pub base_md5: String,
    pub base_size: u64,
    pub patch: String,
    pub patch_size: u64,
}

pub fn extract_rand_str(file_path: &str) -> Result<String, Box<dyn std::error::Error>> {
    let re = regex::Regex::new(r"_([^/]+)/.+?$")?;
    let caps = re
        .captures(file_path)
        .ok_or_else(|| format!("rand_str not found in file_path: {}", file_path))?;
    let rand_str = caps
        .get(1)
        .ok_or_else(|| format!("rand_str capture group missing in file_path: {}", file_path))?
        .as_str()
        .to_string();
    Ok(rand_str)
}

pub fn derive_game_version(version: &str) -> Result<String, Box<dyn std::error::Error>> {
    let ver = semver::Version::parse(version)
        .map_err(|e| format!("invalid version '{}': {}", version, e))?;
    Ok(format!("{}.{}", ver.major, ver.minor))
}

pub struct ResolveParams<'a> {
    pub app_code: &'a str,
    pub launcher_app_code: &'a str,
    pub channel: i32,
    pub sub_channel: i32,
    pub launcher_sub_channel: i32,
    pub platform: Platform,
}

pub async fn resolve_game_config(
    client: &Client,
    params: &ResolveParams<'_>,
) -> Result<EndfieldGameConfig, Box<dyn std::error::Error>> {
    let game = launcher::latest_game(
        client,
        params.app_code,
        params.launcher_app_code,
        params.channel,
        params.sub_channel,
        params.launcher_sub_channel,
        None,
    )
    .await?;

    let rand_str = extract_rand_str(&game.pkg.file_path)?;
    let game_version = derive_game_version(&game.version)?;

    let resources = launcher::latest_game_resources(
        client,
        params.app_code,
        &game_version,
        &game.version,
        &rand_str,
        params.platform.clone(),
    )
    .await?;

    Ok(EndfieldGameConfig {
        version: game.version,
        game_version,
        rand_str,
        packs: game.pkg.packs,
        resources: resources.resources,
        domain: resources.domain,
        res_version: resources.res_version,
        patch_index_path: resources.patch_index_path,
        configs: resources.configs,
    })
}

pub async fn fetch_patch_index(
    client: &Client,
    resource_path: &str,
) -> Result<PatchIndex, Box<dyn std::error::Error>> {
    let url = format!("{}/patch.json", resource_path);
    let patch_index = client.get(&url).send().await?.json::<PatchIndex>().await?;
    Ok(patch_index)
}
