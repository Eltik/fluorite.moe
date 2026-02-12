use std::io::{self, Write};

use backend::core::hypergryph::{
    account_service::{
        LoginResult, OAuth2GrantResponse, OAuth2GrantType1Response, info_basic, oauth2_grant,
        token_by_email_password,
    },
    binding::{BindingRoles, binding_list},
    u8::{channel_token, confirm_server, server_list},
    webview::{
        PoolType, WebViewRecordCharDataList, WebViewRecordContentDataPool, content, record_char,
    },
};
use chrono::DateTime;
use comfy_table::{Table, presets::UTF8_FULL_CONDENSED};

fn prompt(message: &str) -> Result<String, Box<dyn std::error::Error>> {
    eprint!("{}: ", message);
    io::stderr().flush()?;
    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    let trimmed = input.trim().to_string();
    if trimmed.is_empty() {
        return Err("Aborted".into());
    }
    Ok(trimmed)
}

#[allow(dead_code)]
struct GachaRecord {
    pool_type: PoolType,
    entry: WebViewRecordCharDataList,
}

struct GachaPoolInfo {
    pool_id: String,
    pool: WebViewRecordContentDataPool,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    let client = reqwest::Client::new();

    let mut token = args.get(1).cloned().unwrap_or_default();
    let mut need_retrieve_token = token.is_empty();

    if token.is_empty() {
        eprintln!("Gryphline account service token has not been specified.");
        eprint!("Enter Gryphline account service token: ");
        io::stderr().flush()?;
        let mut input = String::new();
        io::stdin().read_line(&mut input)?;
        let trimmed = input.trim().to_string();
        if trimmed.is_empty() {
            need_retrieve_token = true;
        } else {
            token = trimmed;
            need_retrieve_token = false;
        }
    }

    let mut oauth2_rsp: Option<OAuth2GrantResponse> = None;

    if !need_retrieve_token {
        eprintln!("Retrieving account service OAuth 2.0 code ...");

        let body: serde_json::Value = client
            .post("https://as.gryphline.com/user/oauth2/v2/grant")
            .json(&serde_json::json!({
                "appCode": "d9f6dbb6bbd6bb33",
                "token": token,
                "type": 0,
            }))
            .send()
            .await?
            .json()
            .await?;

        if body["status"] == 3 {
            need_retrieve_token = true;
        } else {
            oauth2_rsp = Some(serde_json::from_value(body)?);
        }
    }

    if need_retrieve_token {
        let email = match args.get(2) {
            Some(e) if !e.is_empty() => e.clone(),
            _ => {
                eprintln!("Gryphline account email has not been specified. Requesting ...");
                prompt("Enter Gryphline account email")?
            }
        };

        let password = match args.get(3) {
            Some(p) if !p.is_empty() => p.clone(),
            _ => prompt("Enter Gryphline account password")?,
        };

        eprintln!("Retrieving account service token ...");
        match token_by_email_password(&client, &email, &password, None).await? {
            LoginResult::Success(rsp) => {
                token = rsp.data.token;
            }
            LoginResult::CaptchaRequired(challenge) => {
                return Err(format!(
                    "CAPTCHA required (id={}, type={}). Automated solving is not supported.",
                    challenge.geetest_id, challenge.risk_type
                )
                .into());
            }
        }
    }

    if let Some(rsp) = &oauth2_rsp {
        eprintln!(
            "Retrieving account service OAuth 2.0 code from uid {}",
            rsp.data.uid
        );
    }

    let oauth2_token_rsp = match oauth2_rsp {
        Some(rsp) => rsp,
        None => oauth2_grant::<OAuth2GrantResponse>(&client, "d9f6dbb6bbd6bb33", &token, 0).await?,
    };

    let oauth2_token_bind_rsp =
        oauth2_grant::<OAuth2GrantType1Response>(&client, "3dacefa138426cfe", &token, 1).await?;
    let u8_token_rsp = channel_token(
        &client,
        "973bd727dd11cbb6ead8",
        6,
        &oauth2_token_rsp.data.code,
        None,
        None,
    )
    .await?;

    println!("Authentication successful!");

    println!("Retrieving user account data...");
    let user_acc_data = info_basic(&client, "d9f6dbb6bbd6bb33", &token).await?;

    println!("Retrieving user game server data...");
    let user_game_data = server_list(&client, &u8_token_rsp.data.token).await?;
    let user_game_binding_data = binding_list(&client, &oauth2_token_bind_rsp.data.token).await?;

    println!("Retrieving gacha records...");

    let selected_server_id = {
        let endfield_entry = user_game_binding_data
            .data
            .list
            .iter()
            .find(|f| f.app_code == "endfield")
            .ok_or("endfield app not found")?;

        let mut roles: Vec<&BindingRoles> = endfield_entry
            .binding_list
            .first()
            .ok_or("no binding list found")?
            .roles
            .iter()
            .filter(|r| !r.is_banned)
            .collect();
        roles.sort_by(|a, b| b.level.cmp(&a.level));

        let selected = roles.first().ok_or("Game account not found")?;
        let id = &selected.server_id;

        eprintln!("Confirming server availability...");
        let confirm_rsp =
            confirm_server(&client, &u8_token_rsp.data.token, id.parse::<i32>()?).await?;

        if confirm_rsp.status != 0 {
            return Err(format!("Game server availability error: {:?}", confirm_rsp).into());
        }

        id.clone()
    };

    let gacha_record_rsp = {
        let pool_type_list = [
            PoolType::ECharacterGachaPoolTypeStandard,
            PoolType::ECharacterGachaPoolTypeBeginner,
            PoolType::ECharacterGachaPoolTypeSpecial,
        ];

        let mut record_arr: Vec<GachaRecord> = Vec::new();
        let server_id: i8 = selected_server_id.parse()?;

        for pool_type in &pool_type_list {
            let mut seq_id: Option<String> = None;
            loop {
                let rsp = record_char(
                    &client,
                    u8_token_rsp.data.token.clone(),
                    server_id,
                    pool_type.clone(),
                    seq_id.as_deref(),
                    "en-us",
                )
                .await?;

                for entry in rsp.data.list.iter() {
                    record_arr.push(GachaRecord {
                        pool_type: pool_type.clone(),
                        entry: entry.clone(),
                    });
                }

                eprintln!(
                    "Loaded: {:?}, {} entries, hasMore={}",
                    pool_type,
                    record_arr.len(),
                    rsp.data.has_more
                );

                if !rsp.data.has_more {
                    break;
                }

                match rsp.data.list.last() {
                    Some(last) => seq_id = Some(last.seq_id.clone()),
                    None => break,
                }
            }
        }

        record_arr.reverse();
        record_arr
    };

    let server_id: i8 = selected_server_id.parse()?;

    let gacha_pool_info_list = {
        eprintln!("Retrieving gacha pool info...");

        let pool_id_list: Vec<String> = {
            let mut ids: Vec<String> = gacha_record_rsp
                .iter()
                .map(|e| e.entry.pool_id.clone())
                .collect();
            ids.sort();
            ids.dedup();
            ids
        };

        let mut arr: Vec<GachaPoolInfo> = Vec::new();

        for pool_id in &pool_id_list {
            let rsp = content(&client, server_id, pool_id, "en-us").await?;
            if let Some(data) = rsp.data {
                arr.push(GachaPoolInfo {
                    pool_id: pool_id.clone(),
                    pool: data.pool,
                });
            } else {
                eprintln!(
                    "Warning: no pool data for pool_id {} (code: {})",
                    pool_id, rsp.code
                );
            }
        }

        arr
    };

    println!("Data retrieval completed!");

    let endfield_binding = user_game_binding_data
        .data
        .list
        .iter()
        .find(|e| e.app_code == "endfield")
        .ok_or("endfield not found")?;
    let binding = endfield_binding.binding_list.first().ok_or("no binding")?;

    {
        let mut table = Table::new();
        table.load_preset(UTF8_FULL_CONDENSED);

        let nickname = if !user_acc_data.data.nick_name.is_empty() {
            user_acc_data.data.nick_name.clone()
        } else {
            binding
                .roles
                .iter()
                .find(|r| !r.nick_name.is_empty())
                .map(|r| r.nick_name.clone())
                .unwrap_or_else(|| "(none)".to_string())
        };

        let registered = DateTime::from_timestamp(binding.register_ts, 0)
            .map(|dt| dt.format("%Y/%m/%d %H:%M:%S").to_string())
            .unwrap_or_default();

        let rows = vec![
            vec!["Hypergryph ID", &user_acc_data.data.hg_id],
            vec!["OAuth Grant ID", &oauth2_token_rsp.data.uid],
            vec!["Game Overall UID", &binding.uid],
            vec!["Email", &user_acc_data.data.real_email],
            vec!["Nickname", &nickname],
            vec![
                "Age Region",
                user_acc_data
                    .data
                    .age_gate
                    .region_info
                    .get("en-us")
                    .map(|s| s.as_str())
                    .unwrap_or(""),
            ],
            vec!["Registered", &registered],
        ];

        for row in rows {
            table.add_row(row);
        }
        println!("{table}");
    }

    {
        let mut table = Table::new();
        table.load_preset(UTF8_FULL_CONDENSED);
        table.add_row(vec!["ID", "Time", "Name", "Domain", "Port"]);

        for s in &user_game_data.data.server_list {
            let ext: serde_json::Value = serde_json::from_str(&s.extension)?;
            let offset_seconds = ext["offsetSeconds"].as_i64().unwrap_or(0);
            let sign = if offset_seconds < 0 { "" } else { "+" };
            let timezone = format!("UTC{}{}", sign, offset_seconds / 3600);

            let domain: serde_json::Value = serde_json::from_str(&s.server_domain)?;
            let host = domain[0]["host"].as_str().unwrap_or("");
            let port = domain[0]["port"].as_i64().unwrap_or(0).to_string();

            table.add_row(vec![&s.server_id, &timezone, &s.server_name, host, &port]);
        }
        println!("{table}");
    }

    {
        let mut table = Table::new();
        table.load_preset(UTF8_FULL_CONDENSED);
        table.add_row(vec![
            "ID",
            "UID",
            "Lv",
            "Found",
            "Default",
            "Nickname",
            "Registered",
        ]);

        for s in &user_game_data.data.server_list {
            let role = binding.roles.iter().find(|r| r.server_id == s.server_id);
            let found = role.is_some().to_string();
            let default = s.default_choose.to_string();
            let nickname = role.map(|r| r.nick_name.as_str()).unwrap_or("");
            let registered = role
                .and_then(|r| DateTime::from_timestamp(r.register_ts, 0))
                .map(|dt| dt.format("%Y/%m/%d %H:%M:%S").to_string())
                .unwrap_or_default();

            let role_id = s.role_id.as_deref().unwrap_or("");
            let level = s.level.map(|l| l.to_string()).unwrap_or_default();
            table.add_row(vec![
                &s.server_id,
                role_id,
                &level,
                &found,
                &default,
                nickname,
                &registered,
            ]);
        }
        println!("{table}");
    }

    {
        let mut table = Table::new();
        table.load_preset(UTF8_FULL_CONDENSED);
        table.add_row(vec![
            "Pool ID",
            "Pool Name",
            "Pulls",
            "*6",
            "*5",
            "*4",
            "Pity *6",
            "*5",
            "Latest",
        ]);

        for pool_info in &gacha_pool_info_list {
            let records: Vec<&GachaRecord> = gacha_record_rsp
                .iter()
                .filter(|r| r.entry.pool_id == pool_info.pool_id)
                .collect();

            let total = records.len().to_string();
            let r6 = records
                .iter()
                .filter(|r| r.entry.rarity == 6)
                .count()
                .to_string();
            let r5 = records
                .iter()
                .filter(|r| r.entry.rarity == 5)
                .count()
                .to_string();
            let r4 = records
                .iter()
                .filter(|r| r.entry.rarity == 4)
                .count()
                .to_string();

            let pity6 = {
                let mut counter = 0;
                for r in records.iter().rev() {
                    if r.entry.rarity >= 6 {
                        break;
                    }
                    counter += 1;
                }
                counter.to_string()
            };

            let pity5 = {
                let mut counter = 0;
                for r in records.iter().rev() {
                    if r.entry.rarity >= 5 {
                        break;
                    }
                    counter += 1;
                }
                counter.to_string()
            };

            let latest = records
                .last()
                .map(|r| format!("*{} {}", r.entry.rarity, r.entry.char_id))
                .unwrap_or_default();

            table.add_row(vec![
                &pool_info.pool_id,
                &pool_info.pool.pool_name,
                &total,
                &r6,
                &r5,
                &r4,
                &pity6,
                &pity5,
                &latest,
            ]);
        }
        println!("{table}");
    }

    {
        let mut table = Table::new();
        table.load_preset(UTF8_FULL_CONDENSED);
        table.add_row(vec!["Pool ID", "Pool Name", "Pulled", "Pity", "Character"]);

        for pool_info in &gacha_pool_info_list {
            let records: Vec<&GachaRecord> = gacha_record_rsp
                .iter()
                .filter(|r| r.entry.pool_id == pool_info.pool_id)
                .collect();

            let pool_name = if pool_info.pool.pool_name.is_empty() {
                records
                    .first()
                    .map(|r| r.entry.pool_name.clone())
                    .unwrap_or_default()
            } else {
                pool_info.pool.pool_name.clone()
            };

            let mut sub_rows: Vec<Vec<String>> = Vec::new();
            let mut pity_r6: i32 = 0;

            for record in &records {
                pity_r6 += 1;
                if record.entry.rarity == 6 {
                    let ts: i64 = record.entry.gacha_ts.parse().unwrap_or(0);
                    let pulled = DateTime::from_timestamp_millis(ts)
                        .map(|dt| dt.format("%Y/%m/%d %H:%M:%S").to_string())
                        .unwrap_or_default();

                    sub_rows.push(vec![
                        pool_info.pool_id.clone(),
                        pool_name.clone(),
                        pulled,
                        pity_r6.to_string(),
                        format!("*{} {}", record.entry.rarity, record.entry.char_id),
                    ]);
                    pity_r6 = 0;
                }
            }

            sub_rows.reverse();
            if !sub_rows.is_empty() {
                if table.row_count() > 1 {
                    table.add_row(vec!["", "", "", "", ""]);
                }
                for row in sub_rows {
                    table.add_row(row);
                }
            }
        }
        println!("{table}");
    }

    Ok(())
}
