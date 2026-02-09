use reqwest::Client;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize)]
pub struct ChannelToken {
    pub code: String,
    #[serde(rename = "type")]
    pub token_type: i8,
    #[serde(rename = "isSuc")]
    pub is_suc: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ChannelTokenRequest {
    pub app_code: String,
    pub channel_master_id: i32,
    pub channel_token: String,
    #[serde(rename = "type")]
    pub channel_type: i8,
    pub platform: i8,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ChannelTokenResponse {
    pub data: ChannelTokenResponseData,
    pub msg: String,
    pub status: i8,
    #[serde(rename = "type")]
    pub response_type: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ChannelTokenResponseData {
    pub token: String,
    #[serde(rename = "isNew")]
    pub is_new: bool,
    pub uid: String,
}

pub async fn channel_token(
    client: &Client,
    app_code: &str,
    channel_master_id: i32,
    channel_token: &str,
    platform: Option<i8>,
    channel_type: Option<i8>,
) -> Result<ChannelTokenResponse, Box<dyn std::error::Error>> {
    let channel_token_json = serde_json::to_string(&ChannelToken {
        code: channel_token.to_string(),
        token_type: 1,
        is_suc: true,
    })?;

    let body = ChannelTokenRequest {
        app_code: app_code.to_string(),
        channel_master_id,
        channel_token: channel_token_json,
        channel_type: channel_type.unwrap_or(0),
        platform: platform.unwrap_or(2),
    };

    let response = client
        .post("https://u8.gryphline.com/u8/user/auth/v2/token_by_channel_token")
        .json(&body)
        .send()
        .await?
        .json::<ChannelTokenResponse>()
        .await?;

    Ok(response)
}

#[derive(Debug, Clone, Serialize)]
pub struct GrantRequest {
    pub token: String,
    pub platform: i8,
    #[serde(rename = "type")]
    pub grant_type: i8,
}

#[derive(Debug, Clone, Deserialize)]
pub struct GrantResponse {
    pub data: GrantResponseData,
    pub msg: String,
    pub status: i8,
    #[serde(rename = "type")]
    pub response_type: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct GrantResponseData {
    pub uid: String,
    pub code: String,
    pub token: String,
}

pub async fn grant(
    client: &Client,
    token: &str,
    platform: Option<i8>,
    grant_type: Option<i8>,
) -> Result<GrantResponse, Box<dyn std::error::Error>> {
    let body = GrantRequest {
        token: token.to_string(),
        platform: platform.unwrap_or(2),
        grant_type: grant_type.unwrap_or(0),
    };

    let response = client
        .post("https://u8.gryphline.com/u8/user/auth/v2/grant")
        .json(&body)
        .send()
        .await?
        .json::<GrantResponse>()
        .await?;

    Ok(response)
}

#[derive(Debug, Clone, Serialize)]
pub struct ServerListRequest {
    pub token: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ServerListResponse {
    pub data: ServerListResponseData,
    pub msg: String,
    pub status: i8,
    #[serde(rename = "type")]
    pub response_type: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ServerListResponseData {
    #[serde(rename = "serverList")]
    pub server_list: Vec<ServerListData>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerListData {
    pub server_id: String,
    pub server_name: String,
    pub server_domain: String,
    pub default_choose: bool,
    pub role_id: String,
    pub level: i32,
    pub extension: String,
}

pub async fn server_list(
    client: &Client,
    token: &str,
) -> Result<ServerListResponse, Box<dyn std::error::Error>> {
    let body = ServerListRequest {
        token: token.to_string(),
    };

    let response = client
        .post("https://u8.gryphline.com/game/server/v1/server_list")
        .json(&body)
        .send()
        .await?
        .json::<ServerListResponse>()
        .await?;

    Ok(response)
}

#[derive(Debug, Clone, Serialize)]
pub struct ConfirmServerRequest {
    pub token: String,
    #[serde(rename = "serverId")]
    pub server_id: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ConfirmServerResponse {
    pub msg: String,
    pub status: i8,
    #[serde(rename = "type")]
    pub response_type: String,
}

pub async fn confirm_server(
    client: &Client,
    token: &str,
    server_id: i32,
) -> Result<ConfirmServerResponse, Box<dyn std::error::Error>> {
    let body = ConfirmServerRequest {
        token: token.to_string(),
        server_id: server_id.to_string(),
    };

    let response = client
        .post("https://u8.gryphline.com/game/role/v1/confirm_server")
        .json(&body)
        .send()
        .await?
        .json::<ConfirmServerResponse>()
        .await?;

    Ok(response)
}
