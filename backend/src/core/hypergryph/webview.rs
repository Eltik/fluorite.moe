use reqwest::Client;
use serde::Deserialize;

use crate::core::hypergryph::LAUNCHER_WEB_LANG;

#[derive(Debug, Clone, Deserialize)]
pub enum PoolType {
    #[serde(rename = "E_CharacterGachaPoolType_Beginner")]
    ECharacterGachaPoolTypeBeginner,
    #[serde(rename = "E_CharacterGachaPoolType_Standard")]
    ECharacterGachaPoolTypeStandard,
    #[serde(rename = "E_CharacterGachaPoolType_Special")]
    ECharacterGachaPoolTypeSpecial,
}

impl PoolType {
    pub fn as_str(&self) -> &str {
        match self {
            PoolType::ECharacterGachaPoolTypeBeginner => "E_CharacterGachaPoolType_Beginner",
            PoolType::ECharacterGachaPoolTypeStandard => "E_CharacterGachaPoolType_Standard",
            PoolType::ECharacterGachaPoolTypeSpecial => "E_CharacterGachaPoolType_Special",
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct WebViewRecordChar {
    pub code: i32,
    pub data: WebViewRecordCharData,
    pub msg: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WebViewRecordCharData {
    pub list: Vec<WebViewRecordCharDataList>,
    pub has_more: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WebViewRecordCharDataList {
    pub pool_id: String,
    pub pool_name: String,
    pub char_id: String,
    pub rarity: i8,
    pub is_free: bool,
    pub is_new: bool,
    pub gacha_ts: String,
    pub seq_id: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct WebViewRecordContent {
    pub code: i32,
    pub data: Option<WebViewRecordContentData>,
    pub msg: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct WebViewRecordContentData {
    pub pool: WebViewRecordContentDataPool,
    pub timezone: i64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct WebViewRecordContentDataPool {
    pub pool_gacha_type: String,
    pub pool_name: String,
    pub pool_type: String,
    pub up6_name: String,
    pub up6_image: String,
    pub up5_name: String,
    pub up5_image: String,
    pub up6_item_name: String,
    pub rotate_image: String,
    pub ticket_name: String,
    pub ticket_ten_name: String,
    pub all: Vec<WebViewRecordContentDataPoolAll>,
    // pub rotate_list: ??
}

#[derive(Debug, Clone, Deserialize)]
pub struct WebViewRecordContentDataPoolAll {
    pub id: String,
    pub name: String,
    pub rarity: i8,
}

pub async fn record_char(
    client: &Client,
    token: String,
    server_id: i8,
    pool_type: PoolType,
    seq_id: Option<&str>,
    lang: &str,
) -> Result<WebViewRecordChar, Box<dyn std::error::Error>> {
    if server_id != 2 && server_id != 3 {
        return Err(format!("invalid server id: {}", server_id).into());
    }

    if !LAUNCHER_WEB_LANG.contains(&lang) {
        return Err(format!("invalid lang: {}", lang).into());
    }

    let mut params: Vec<(&str, String)> = vec![
        ("lang", lang.to_string()),
        ("pool_type", pool_type.as_str().to_string()),
        ("token", token),
        ("server_id", server_id.to_string()),
    ];

    if let Some(id) = seq_id {
        params.push(("seq_id", id.to_string()));
    }

    let response = client
        .get("https://ef-webview.gryphline.com/api/record/char")
        .query(&params)
        .send()
        .await?
        .json::<WebViewRecordChar>()
        .await?;
    Ok(response)
}

pub async fn content(
    client: &Client,
    server_id: i8,
    pool_id: &str,
    lang: &str,
) -> Result<WebViewRecordContent, Box<dyn std::error::Error>> {
    if server_id != 2 && server_id != 3 {
        return Err(format!("invalid server id: {}", server_id).into());
    }

    if !LAUNCHER_WEB_LANG.contains(&lang) {
        return Err(format!("invalid lang: {}", lang).into());
    }

    let params: Vec<(&str, String)> = vec![
        ("lang", lang.to_string()),
        ("pool_id", pool_id.to_string()),
        ("server_id", server_id.to_string()),
    ];

    let response = client
        .get("https://ef-webview.gryphline.com/api/content")
        .query(&params)
        .send()
        .await?
        .json::<WebViewRecordContent>()
        .await?;

    Ok(response)
}
