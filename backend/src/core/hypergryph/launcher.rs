use reqwest::Client;
use serde::{Deserialize, Serialize};

fn is_valid_semver(version: &str) -> bool {
    let version_core = version.split('-').next().unwrap_or(version);
    let parts: Vec<&str> = version_core.split('.').collect();
    parts.len() == 3 && parts.iter().all(|p| p.parse::<u64>().is_ok())
}

#[derive(Debug, Clone, Deserialize)]
pub struct LauncherLatestGame {
    pub action: i32,
    pub version: String,
    pub request_version: String,
    pub pkg: LauncherLatestGamePkg,
    pub patch: Option<LauncherLatestGamePatch>,
    pub state: i32,
    pub launcher_action: i32,
}

#[derive(Debug, Clone, Deserialize)]
pub struct LauncherLatestGamePkg {
    pub packs: Vec<LauncherLatestGamePack>,
    pub total_size: String,
    pub file_path: String,
    pub url: String,
    pub md5: String,
    pub package_size: String,
    pub file_id: String,
    pub sub_channel: String,
    pub game_files_md5: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct LauncherLatestGamePack {
    pub url: String,
    pub md5: String,
    pub package_size: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct LauncherLatestGamePatch {
    pub url: String,
    pub md5: String,
    pub package_size: String,
    pub total_size: String,
    pub file_id: String,
    pub patches: Vec<LauncherLatestGamePack>,
}

pub async fn latest_game(
    client: &Client,
    app_code: &str,
    launcher_app_code: &str,
    channel: i32,
    sub_channel: i32,
    launcher_sub_channel: i32,
    version: Option<&str>,
) -> Result<LauncherLatestGame, Box<dyn std::error::Error>> {
    if let Some(ver) = &version
        && !is_valid_semver(ver)
    {
        return Err(format!("invalid version string ({})", ver).into());
    }

    let mut params: Vec<(&str, String)> = vec![
        ("appcode", app_code.to_string()),
        ("launcher_appcode", launcher_app_code.to_string()),
        ("channel", channel.to_string()),
        ("sub_channel", sub_channel.to_string()),
        ("launcher_sub_channel", launcher_sub_channel.to_string()),
    ];

    if let Some(ver) = version {
        params.push(("version", ver.to_string()));
    }

    let response = client
        .get("https://launcher.gryphline.com/api/game/get_latest")
        .query(&params)
        .send()
        .await?
        .json::<LauncherLatestGame>()
        .await?;
    Ok(response)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Platform {
    Windows,
    Android,
    #[serde(rename = "iOS")]
    Ios,
    PlayStation,
}

#[derive(Debug, Clone, Deserialize)]
pub struct LauncherLatestGameResources {
    pub resources: Vec<LauncherLatestGameResource>,
    pub configs: String,
    pub res_version: String,
    pub patch_index_path: String,
    pub domain: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct LauncherLatestGameResource {
    pub name: String,
    pub version: String,
    pub path: String,
}

pub async fn latest_game_resources(
    client: &Client,
    app_code: &str,
    game_version: &str,
    version: &str,
    rand_str: &str,
    platform: Platform,
) -> Result<LauncherLatestGameResources, Box<dyn std::error::Error>> {
    if !is_valid_semver(version) {
        return Err(format!("invalid version string ({})", version).into());
    }

    let params: Vec<(&str, String)> = vec![
        ("appcode", app_code.to_string()),
        ("game_version", game_version.to_string()),
        ("version", version.to_string()),
        ("platform", format!("{:?}", platform)),
        ("rand_str", rand_str.to_string()),
    ];

    let response = client
        .get("https://launcher.gryphline.com/api/game/get_latest_resources")
        .query(&params)
        .send()
        .await?
        .json::<LauncherLatestGameResources>()
        .await?;
    Ok(response)
}

#[derive(Debug, Clone, Deserialize)]
pub struct LauncherLatestLauncher {
    pub action: i32,
    pub version: String,
    pub request_version: String,
    pub zip_package_url: String,
    pub md5: String,
    pub package_size: String,
    pub total_size: String,
    pub description: String,
}

pub async fn latest_launcher(
    client: &Client,
    app_code: &str,
    channel: i32,
    sub_channel: i32,
    version: Option<&str>,
    target_app: Option<&str>,
) -> Result<LauncherLatestLauncher, Box<dyn std::error::Error>> {
    if let Some(ver) = version
        && !is_valid_semver(ver)
    {
        return Err(format!("invalid version string ({})", ver).into());
    }

    let mut params: Vec<(&str, String)> = vec![
        ("appcode", app_code.to_string()),
        ("channel", channel.to_string()),
        ("sub_channel", sub_channel.to_string()),
    ];

    if let Some(ver) = version {
        params.push(("version", ver.to_string()));
    }

    if let Some(app) = target_app {
        params.push(("target_app", app.to_string()));
    }

    let response = client
        .get("https://launcher.gryphline.com/api/launcher/get_latest")
        .query(&params)
        .send()
        .await?
        .json::<LauncherLatestLauncher>()
        .await?;

    Ok(response)
}
