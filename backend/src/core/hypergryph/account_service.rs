use reqwest::Client;
use serde::{Deserialize, Serialize, de::DeserializeOwned};

#[derive(Debug, Clone, Serialize)]
pub struct TokenByEmailRequest {
    pub email: String,
    pub password: String,
    pub from: i8,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TokenByEmailResponse {
    pub data: TokenByEmailData,
    pub msg: String,
    pub status: i8,
    #[serde(rename = "type")]
    pub token_by_email_type: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TokenByEmailData {
    pub token: String,
    pub hg_id: String,
    pub email: String,
    pub is_latest_user_agreement: bool,
}

pub async fn token_by_email_password(
    client: &Client,
    email: &str,
    password: &str,
    from: Option<i8>,
) -> Result<TokenByEmailResponse, Box<dyn std::error::Error>> {
    let body = TokenByEmailRequest {
        email: email.to_string(),
        password: password.to_string(),
        from: from.unwrap_or(0),
    };

    let response = client
        .post("https://as.gryphline.com/user/auth/v1/token_by_email_password")
        .json(&body)
        .send()
        .await?
        .json::<TokenByEmailResponse>()
        .await?;

    Ok(response)
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct OAuth2GrantRequest {
    pub app_code: String,
    pub token: String,
    #[serde(rename = "type")]
    pub grant_type: i8,
}

#[derive(Debug, Clone, Deserialize)]
pub struct OAuth2GrantResponse {
    pub data: OAuth2GrantData,
    pub msg: String,
    pub status: i32,
    #[serde(rename = "type")]
    pub response_type: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct OAuth2GrantData {
    pub uid: String,
    pub code: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct OAuth2GrantType1Response {
    pub data: OAuth2GrantType1Data,
    pub msg: String,
    pub status: i32,
    #[serde(rename = "type")]
    pub response_type: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct OAuth2GrantType1Data {
    #[serde(rename = "hgId")]
    pub hg_id: String,
    pub token: String,
}

/*
* // type 0
  let rsp = oauth2_grant::<OAuth2GrantResponse>(client, app_code, token, 0).await?;

  // type 1
  let rsp = oauth2_grant::<OAuth2GrantType1Response>(client, app_code, token, 1).await?;
*
*/
pub async fn oauth2_grant<T: DeserializeOwned>(
    client: &Client,
    app_code: &str,
    token: &str,
    grant_type: i8,
) -> Result<T, Box<dyn std::error::Error>> {
    let body = OAuth2GrantRequest {
        app_code: app_code.to_string(),
        token: token.to_string(),
        grant_type,
    };

    let response = client
        .post("https://as.gryphline.com/user/oauth2/v2/grant")
        .json(&body)
        .send()
        .await?
        .json::<T>()
        .await?;

    Ok(response)
}

#[derive(Debug, Clone, Deserialize)]
pub struct InfoBasic {
    pub data: InfoBasicData,
    pub msg: String,
    pub status: i32,
    #[serde(rename = "type")]
    pub response_type: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InfoBasicData {
    pub hg_id: String,
    pub email: String,
    pub real_email: String,
    pub is_latest_user_agreement: bool,
    pub nick_name: String,
    pub email_subscription: bool,
    pub extension: InfoBasicExtension,
    pub age_gate: InfoBasicAgeGate,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InfoBasicExtension {
    pub firebase_hashed_info: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InfoBasicAgeGate {
    pub age_auth_state: i32,
    pub bind_email: bool,
    pub parent_auth_state: i32,
    pub region_info: std::collections::HashMap<String, String>,
    pub region_code: String,
}

pub async fn info_basic(
    client: &Client,
    app_code: &str,
    token: &str,
) -> Result<InfoBasic, Box<dyn std::error::Error>> {
    let params: Vec<(&str, String)> = vec![
        ("appCode", app_code.to_string()),
        ("token", token.to_string()),
    ];

    let response = client
        .get("https://as.gryphline.com/user/info/v1/basic")
        .query(&params)
        .send()
        .await?
        .json::<InfoBasic>()
        .await?;
    Ok(response)
}

#[derive(Debug, Clone, Deserialize)]
pub struct InfoThirdParty {
    pub data: InfoThirdPartyData,
    pub msg: String,
    pub status: i32,
    #[serde(rename = "type")]
    pub response_type: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InfoThirdPartyData {
    pub third_party_info: Vec<InfoThirdPartyInfo>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InfoThirdPartyInfo {
    pub name: String,
    pub channel_id: i32,
}

pub async fn info_third_party(
    client: &Client,
    app_code: &str,
    token: &str,
) -> Result<InfoThirdParty, Box<dyn std::error::Error>> {
    let params: Vec<(&str, String)> = vec![
        ("appCode", app_code.to_string()),
        ("token", token.to_string()),
    ];

    let response = client
        .get("https://as.gryphline.com/user/info/v1/third_party")
        .query(&params)
        .send()
        .await?
        .json::<InfoThirdParty>()
        .await?;
    Ok(response)
}
