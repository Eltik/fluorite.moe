use reqwest::Client;
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct BindingListResponse {
    pub data: BindingListData,
    pub msg: String,
    pub status: i8,
}

#[derive(Debug, Clone, Deserialize)]
pub struct BindingListData {
    pub list: Vec<List>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct List {
    pub app_code: String,
    pub app_name: String,
    pub support_multi_server: bool,
    pub binding_list: Vec<BindingList>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BindingList {
    pub uid: String,
    pub is_official: bool,
    pub is_default: bool,
    pub channel_master_id: i8,
    pub channel_name: String,
    pub is_deleted: bool,
    pub is_banned: bool,
    pub register_ts: i64,
    pub roles: Vec<BindingRoles>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BindingRoles {
    pub is_banned: bool,
    pub server_id: String,
    pub server_name: String,
    pub role_id: String,
    pub nick_name: String,
    pub level: i32,
    pub is_default: bool,
    pub register_ts: i64,
}

pub async fn binding_list(
    client: &Client,
    token: &str,
) -> Result<BindingListResponse, Box<dyn std::error::Error>> {
    let params: Vec<(&str, String)> = vec![("token", token.to_string())];

    let response = client
        .get("https://binding-api-account-prod.gryphline.com/account/binding/v1/binding_list")
        .query(&params)
        .send()
        .await?
        .json::<BindingListResponse>()
        .await?;
    Ok(response)
}

#[derive(Debug, Clone, Deserialize)]
pub struct U8TokenResponse {
    pub data: U8Token,
    pub msg: String,
    pub status: i8,
}

#[derive(Debug, Clone, Deserialize)]
pub struct U8Token {
    pub token: String,
}

pub async fn u8_token_by_uid(
    client: &Client,
    token: &str,
    uid: &str,
) -> Result<U8TokenResponse, Box<dyn std::error::Error>> {
    let params: Vec<(&str, String)> = vec![("token", token.to_string()), ("uid", uid.to_string())];

    let response = client
        .get("https://binding-api-account-prod.gryphline.com/account/binding/v1/u8_token_by_uid")
        .query(&params)
        .send()
        .await?
        .json::<U8TokenResponse>()
        .await?;
    Ok(response)
}
