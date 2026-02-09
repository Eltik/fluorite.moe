use reqwest::Client;
use serde::{Deserialize, de::DeserializeOwned};
use serde_json::{Value, json};

use crate::core::hypergryph::LAUNCHER_WEB_LANG;

#[derive(Debug, Clone, Deserialize)]
pub struct LauncherWebSidebar {
    pub data_version: String,
    pub sidebars: Vec<Sidebar>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Sidebar {
    pub display_type: String,
    pub media: String,
    pub pic: Option<SidebarPic>,
    pub sidebar_labels: Vec<SidebarLabel>,
    pub grid_info: Option<serde_json::Value>,
    pub jump_url: String,
    pub need_token: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SidebarPic {
    pub url: String,
    pub md5: String,
    pub description: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SidebarLabel {
    pub content: String,
    pub jump_url: String,
    pub need_token: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct LauncherWebSingleEnt {
    pub single_ent: SingleEnt,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SingleEnt {
    pub version_url: String,
    pub version_md5: String,
    pub jump_url: String,
    pub button_url: String,
    pub button_md5: String,
    pub button_hover_url: String,
    pub button_hover_md5: String,
    pub need_token: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct LauncherWebMainBgImage {
    pub data_version: String,
    pub main_bg_image: MainBgImage,
}

#[derive(Debug, Clone, Deserialize)]
pub struct MainBgImage {
    pub url: String,
    pub md5: String,
    pub video_url: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct LauncherWebBanner {
    pub data_version: String,
    pub banners: Vec<Banner>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Banner {
    pub url: String,
    pub md5: String,
    pub jump_url: String,
    pub id: String,
    pub need_token: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct LauncherWebAnnouncement {
    pub data_version: String,
    pub tabs: Vec<Tab>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Tab {
    #[serde(rename = "tabName")]
    pub tab_name: String,
    pub announcements: Vec<Announcement>,
    pub tab_id: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Announcement {
    pub content: String,
    pub jump_url: String,
    pub start_ts: String, // Check, could be i64
    pub id: String,
    pub need_token: bool,
}

async fn batch_proxy<T: DeserializeOwned>(
    client: &Client,
    kind: &str,
    language: &str,
    req_fields: Value,
) -> Result<T, Box<dyn std::error::Error>> {
    if !LAUNCHER_WEB_LANG.contains(&language) {
        return Err(format!("invalid lang: {}", language).into());
    }

    let req_key = format!("{}_req", kind);
    let rsp_key = format!("{}_rsp", kind);

    let body = json!({
        "proxy_reqs": [{
            "kind": kind,
            req_key: req_fields,
        }]
    });

    let response: Value = client
        .post("https://launcher.gryphline.com/api/proxy/web/batch_proxy")
        .json(&body)
        .send()
        .await?
        .json()
        .await?;

    let rsp = response["proxy_rsps"][0][&rsp_key].clone();
    let result: T = serde_json::from_value(rsp)?;
    Ok(result)
}

pub async fn sidebar(
    client: &Client,
    app_code: &str,
    channel: i32,
    sub_channel: i32,
    language: &str,
    platform: Option<&str>,
) -> Result<LauncherWebSidebar, Box<dyn std::error::Error>> {
    batch_proxy(
        client,
        "get_sidebar",
        language,
        json!({
            "appcode": app_code,
            "channel": channel.to_string(),
            "sub_channel": sub_channel.to_string(),
            "language": language,
            "platform": platform.unwrap_or("Windows"),
            "source": "launcher",
        }),
    )
    .await
}

pub async fn single_ent(
    client: &Client,
    app_code: &str,
    channel: i32,
    sub_channel: i32,
    language: &str,
    platform: Option<&str>,
) -> Result<LauncherWebSingleEnt, Box<dyn std::error::Error>> {
    batch_proxy(
        client,
        "get_single_ent",
        language,
        json!({
            "appcode": app_code,
            "channel": channel.to_string(),
            "sub_channel": sub_channel.to_string(),
            "language": language,
            "platform": platform.unwrap_or("Windows"),
            "source": "launcher",
        }),
    )
    .await
}

pub async fn main_bg_image(
    client: &Client,
    app_code: &str,
    channel: i32,
    sub_channel: i32,
    language: &str,
    platform: Option<&str>,
) -> Result<LauncherWebMainBgImage, Box<dyn std::error::Error>> {
    batch_proxy(
        client,
        "get_main_bg_image",
        language,
        json!({
            "appcode": app_code,
            "channel": channel.to_string(),
            "sub_channel": sub_channel.to_string(),
            "language": language,
            "platform": platform.unwrap_or("Windows"),
            "source": "launcher",
        }),
    )
    .await
}

pub async fn banner(
    client: &Client,
    app_code: &str,
    channel: i32,
    sub_channel: i32,
    language: &str,
    platform: Option<&str>,
) -> Result<LauncherWebBanner, Box<dyn std::error::Error>> {
    batch_proxy(
        client,
        "get_banner",
        language,
        json!({
            "appcode": app_code,
            "channel": channel.to_string(),
            "sub_channel": sub_channel.to_string(),
            "language": language,
            "platform": platform.unwrap_or("Windows"),
            "source": "launcher",
        }),
    )
    .await
}

pub async fn announcement(
    client: &Client,
    app_code: &str,
    channel: i32,
    sub_channel: i32,
    language: &str,
    platform: Option<&str>,
) -> Result<LauncherWebAnnouncement, Box<dyn std::error::Error>> {
    batch_proxy(
        client,
        "get_announcement",
        language,
        json!({
            "appcode": app_code,
            "channel": channel.to_string(),
            "sub_channel": sub_channel.to_string(),
            "language": language,
            "platform": platform.unwrap_or("Windows"),
            "source": "launcher",
        }),
    )
    .await
}
