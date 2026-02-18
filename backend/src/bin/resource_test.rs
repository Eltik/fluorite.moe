use std::{
    collections::{BTreeSet, HashSet},
    path::Path,
};

use backend::core::hypergryph::launcher::{
    LauncherLatestGame, LauncherLatestGameResources, Platform, Region, latest_game,
    latest_game_resources, latest_launcher, latest_launcher_exe,
};
use chrono::{DateTime, FixedOffset, Utc};
use futures::stream::{self, StreamExt};
use indexmap::IndexMap;
use reqwest::Client;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Map, Value, from_str, json, to_string, to_string_pretty, to_value};
use tokio::fs::{create_dir_all, read_to_string, write};

// App codes
#[allow(dead_code)]
mod app_code {
    pub mod game {
        pub const OS_WIN_REL: &str = "YDUTE5gscDZ229CW";
        pub const CN_WIN_REL: &str = "6LL0KJuqHBVz33WK";
    }
    pub mod launcher {
        pub const OS_WIN_REL: &str = "TiaytKBUIEdoEwRT";
        pub const OS_WIN_REL_EPIC: &str = "BBWoqCzuZ2bZ1Dro";
        pub const CN_WIN_REL: &str = "abYeZZ16BPluCFyT";
    }
}

#[allow(dead_code)]
mod channel {
    pub const OS_WIN_REL: i32 = 6;
    pub const CN_WIN_REL: i32 = 1;
}

#[allow(dead_code)]
mod sub_channel {
    pub const OS_WIN_REL: i32 = 6;
    pub const OS_WIN_REL_EPIC: i32 = 801;
    pub const OS_WIN_REL_GOOGLE_PLAY: i32 = 802;
    pub const CN_WIN_REL: i32 = 1;
}

const NETWORK_CONCURRENCY: usize = 16;

fn utc8() -> FixedOffset {
    FixedOffset::east_opt(8 * 3600).unwrap()
}

fn parse_date_utc8(
    raw: &str,
) -> Result<(DateTime<FixedOffset>, String), Box<dyn std::error::Error>> {
    let dt = raw.parse::<DateTime<Utc>>()?.with_timezone(&utc8());
    let formatted = dt.format("%Y/%m/%d %H:%M:%S").to_string();
    Ok((dt, formatted))
}

async fn read_json_file<T: DeserializeOwned>(path: &Path) -> Result<T, Box<dyn std::error::Error>> {
    Ok(from_str(&read_to_string(path).await?)?)
}

const LAUNCHER_BASE: &str = "ArknightsEndfield";

fn game_base_path(output_dir: &Path, dir_name: &str) -> std::path::PathBuf {
    output_dir.join(LAUNCHER_BASE).join("game").join(dir_name)
}

struct IgnoreRule {
    path: Vec<String>,
    pattern: regex::Regex,
}

fn get_object_diff(
    obj1: &Value,
    obj2: &Value,
    ignore_rules: &[IgnoreRule],
    current_path: &[String],
) -> Map<String, Value> {
    let empty = Map::new();
    let map1 = obj1.as_object().unwrap_or(&empty);
    let map2 = obj2.as_object().unwrap_or(&empty);

    let mut keys: std::collections::HashSet<&String> = HashSet::new();
    keys.extend(map1.keys());
    keys.extend(map2.keys());

    let mut diff = Map::new();

    for key in keys {
        let val1 = map1.get(key).unwrap_or(&Value::Null);
        let val2 = map2.get(key).unwrap_or(&Value::Null);

        if val1 == val2 {
            continue;
        }

        let mut full_path: Vec<String> = current_path.to_vec();
        full_path.push(key.clone());

        let rule = ignore_rules.iter().find(|r| {
            r.path.len() == full_path.len() && r.path.iter().zip(&full_path).all(|(a, b)| a == b)
        });

        if let Some(rule) = rule
            && let (Value::String(s1), Value::String(s2)) = (val1, val2)
        {
            let n1 = rule.pattern.replace_all(s1, "");
            let n2 = rule.pattern.replace_all(s2, "");

            if n1 == n2 {
                continue;
            }
        }

        match (val1, val2) {
            (Value::Object(_), Value::Object(_)) => {
                let nested = get_object_diff(val1, val2, ignore_rules, &full_path);
                if !nested.is_empty() {
                    diff.insert(key.clone(), Value::Object(nested));
                }
            }
            _ => {
                let mut entry = Map::new();
                entry.insert("old".into(), val1.clone());
                entry.insert("new".into(), val2.clone());
                diff.insert(key.clone(), Value::Object(entry));
            }
        }
    }

    diff
}

#[derive(Serialize, Deserialize, Clone)]
struct SaveData<T> {
    req: Value,
    rsp: T,
}

#[derive(Serialize, Deserialize)]
struct StoredData<T> {
    #[serde(rename = "updatedAt")]
    updated_at: String,
    req: Value,
    rsp: T,
}

async fn save_result<T>(
    sub_paths: &[&str],
    version: &str,
    data: &SaveData<T>,
    save_latest: bool,
    ignore_rules: &[IgnoreRule],
    output_dir: &Path,
) -> Result<(), Box<dyn std::error::Error>>
where
    T: Serialize + DeserializeOwned + Clone,
{
    let mut file_path_base = output_dir.to_path_buf();
    for p in sub_paths {
        file_path_base.push(p);
    }

    let mut files_to_check = vec![file_path_base.join(format!("v{version}.json"))];
    if save_latest {
        files_to_check.push(file_path_base.join("latest.json"));
    }

    let data_value = to_value(data)?;
    let data_str = to_string_pretty(data)?;

    for file_path in &files_to_check {
        if file_path.exists() {
            let current: Value = read_json_file(file_path).await?;
            let diff = get_object_diff(&current, &data_value, ignore_rules, &[]);
            if !diff.is_empty() {
                eprintln!(
                    "Diff detected in {}: {}",
                    file_path.display(),
                    to_string_pretty(&Value::Object(diff))?
                );
                write(file_path, &data_str).await?;
            }
        } else {
            if let Some(parent) = file_path.parent() {
                create_dir_all(parent).await?;
            }
            write(file_path, &data_str).await?;
        }
    }

    let all_path = file_path_base.join("all.json");
    let mut all_data: Vec<StoredData<T>> = if all_path.exists() {
        read_json_file(&all_path).await?
    } else {
        Vec::new()
    };

    let already_exists = all_data.iter().any(|e| {
        let existing = json!({
            "req": &e.req,
            "rsp": &e.rsp
        });
        let diff = get_object_diff(&existing, &data_value, ignore_rules, &[]);
        diff.is_empty()
    });

    if !already_exists {
        all_data.push(StoredData {
            updated_at: chrono::Utc::now().to_rfc3339(),
            req: data.req.clone(),
            rsp: data.rsp.clone(),
        });
        write(&all_path, to_string_pretty(&all_data)?).await?;
    }

    Ok(())
}

struct GameTarget {
    name: String,
    launcher_app_code: String,
    sub_channel: i32,
    launcher_sub_channel: i32,
    dir_name: String,
}

fn url_filename(u: &str) -> String {
    url::Url::parse(u)
        .ok()
        .and_then(|p| p.path_segments()?.next_back().map(|s| s.to_string()))
        .unwrap_or_default()
}

fn format_bytes(bytes: i64) -> String {
    const UNITS: &[&str] = &["B", "KB", "MB", "GB", "TB"];
    if bytes == 0 {
        return "0 B".into();
    }
    let mut size = bytes as f64;
    let mut i = 0;
    while size.abs() >= 1024.0 && i < UNITS.len() - 1 {
        size /= 1024.0;
        i += 1;
    }
    format!("{:.2} {}", size, UNITS[i])
}

fn size_table_html(unpacked: i64, packed: i64) -> Vec<String> {
    vec![
        "<table>".into(),
        format!(
            "  <tr><td>Unpacked Size</td><td style=\"text-align: right;\"><b>{}</b></td></tr>",
            format_bytes(unpacked)
        ),
        format!(
            "  <tr><td>Packed Size</td><td style=\"text-align: right;\"><b>{}</b></td></tr>",
            format_bytes(packed)
        ),
        "</table>\n".into(),
    ]
}

fn file_row_md(
    url: &str,
    md5: &str,
    package_size: &str,
) -> Result<String, Box<dyn std::error::Error>> {
    let name = url_filename(url);
    let size = format_bytes(package_size.parse()?);
    Ok(format!("|[{name}]({url})|`{md5}`|{size}|"))
}

async fn generate_game_list_md(
    target: &GameTarget,
    output_dir: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let base = game_base_path(output_dir, &target.dir_name);
    let all_path = base.join("all.json");
    if !all_path.exists() {
        return Ok(());
    }

    let game_all: Vec<StoredData<LauncherLatestGame>> = read_json_file(&all_path).await?;

    let mut lines: Vec<String> = Vec::new();

    lines.push(format!("# Game Packages ({})\n", target.name));

    for e in &game_all {
        let version = &e.rsp.version;
        let (date, date_str) = parse_date_utc8(&e.updated_at)?;
        let anchor_id = format!("ver-{version}-{}", date.timestamp());
        lines.push(format!("- [{version} ({date_str})](#{anchor_id})"));
    }
    lines.push(String::new());

    for e in &game_all {
        let version = &e.rsp.version;
        let (date, date_str) = parse_date_utc8(&e.updated_at)?;
        let anchor_id = format!("ver-{version}-{}", date.timestamp());

        let packed_size: i64 = e
            .rsp
            .pkg
            .packs
            .iter()
            .filter_map(|f| f.package_size.parse::<i64>().ok())
            .sum();
        let total_size: i64 = e.rsp.pkg.total_size.parse()?;
        let unpacked_size = total_size - packed_size;

        lines.push(format!(
            "<h2 id=\"{anchor_id}\">{version} ({date_str})</h2>\n"
        ));
        lines.extend(size_table_html(unpacked_size, packed_size));
        lines.push("|File|MD5 Checksum|Size|".into());
        lines.push("|:--|:--|--:|".into());

        for f in &e.rsp.pkg.packs {
            lines.push(file_row_md(&f.url, &f.md5, &f.package_size)?);
        }
        lines.push(String::new());
    }

    write(base.join("list.md"), lines.join("\n")).await?;
    Ok(())
}

async fn generate_patch_list_md(
    target: &GameTarget,
    output_dir: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let base = game_base_path(output_dir, &target.dir_name);
    let all_path = base.join("all_patch.json");
    if !all_path.exists() {
        return Ok(());
    }

    let patch_all: Vec<StoredData<LauncherLatestGame>> = read_json_file(&all_path).await?;

    let mut lines: Vec<String> = Vec::new();

    lines.push(format!("# Game Patch Packages ({})\n", target.name));

    for e in &patch_all {
        let Some(ref _patch) = e.rsp.patch else {
            continue;
        };

        let version = &e.rsp.version;
        let req_version = &e.rsp.request_version;
        let (date, date_str) = parse_date_utc8(&e.updated_at)?;
        let anchor_id = format!("ver-{req_version}-{version}-{}", date.timestamp());
        lines.push(format!(
            "- [{req_version} → {version} ({date_str})](#{anchor_id})"
        ));
    }
    lines.push(String::new());

    for e in &patch_all {
        let Some(ref patch) = e.rsp.patch else {
            continue;
        };

        let version = &e.rsp.version;
        let req_version = &e.rsp.request_version;
        let (date, date_str) = parse_date_utc8(&e.updated_at)?;
        let anchor_id = format!("ver-{req_version}-{version}-{}", date.timestamp());

        let packed_size: i64 = patch
            .patches
            .iter()
            .filter_map(|f| f.package_size.parse::<i64>().ok())
            .sum();
        let total_size: i64 = patch.total_size.parse()?;
        let unpacked_size = total_size - packed_size;

        lines.push(format!(
            "<h2 id=\"{anchor_id}\">{req_version} → {version} ({date_str})</h2>\n"
        ));
        lines.extend(size_table_html(unpacked_size, packed_size));
        lines.push("|File|MD5 Checksum|Size|".into());
        lines.push("|:--|:--|--:|".into());

        if !patch.url.is_empty() {
            lines.push(file_row_md(&patch.url, &patch.md5, &patch.package_size)?);
        }

        for f in &patch.patches {
            lines.push(file_row_md(&f.url, &f.md5, &f.package_size)?);
        }
        lines.push(String::new());
    }

    write(base.join("list_patch.md"), lines.join("\n")).await?;
    Ok(())
}

struct ResVersionEntry<'a> {
    rsp: &'a StoredData<LauncherLatestGameResources>,
    versions: BTreeSet<String>,
}

async fn generate_resource_list_md(
    channel_str: &str,
    output_dir: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut md_texts: Vec<String> = vec![
        "# Game Resources\n".into(),
        "- [Windows](#res-Windows)".into(),
        "- [Android](#res-Android)".into(),
        "- [iOS](#res-iOS)".into(),
        "- [PlayStation](#res-PlayStation)\n".into(),
    ];

    const PLATFORMS: &[&str] = &["Windows", "Android", "iOS", "PlayStation"];

    for platform in PLATFORMS {
        let resource_path = output_dir
            .join(LAUNCHER_BASE)
            .join("game_resources")
            .join(channel_str)
            .join(platform)
            .join("all.json");
        if !resource_path.exists() {
            continue;
        }

        let game_all: Vec<StoredData<LauncherLatestGameResources>> =
            read_json_file(&resource_path).await?;
        let mut res_version_map: IndexMap<String, ResVersionEntry> = IndexMap::new();
        for e in &game_all {
            let res_ver = &e.rsp.res_version;
            let entry = res_version_map
                .entry(res_ver.clone())
                .or_insert_with(|| ResVersionEntry {
                    rsp: e,
                    versions: BTreeSet::new(),
                });
            if let Some(v) = e.req.get("version").and_then(|v| v.as_str()) {
                entry.versions.insert(v.to_string());
            }
        }

        md_texts.push(format!("<h2 id=\"res-{platform}\">{platform}</h2>\n"));
        md_texts.push("|Date|Initial|Main|Kick|Game version|".into());
        md_texts.push("|--|--|--|--|--|".into());

        for entry in res_version_map.values() {
            let (_, date_str) = parse_date_utc8(&entry.rsp.updated_at)?;

            let initial = entry.rsp.rsp.resources.iter().find(|r| r.name == "initial");
            let main = entry.rsp.rsp.resources.iter().find(|r| r.name == "main");

            let initial_str = match initial {
                Some(r) => format!("[{}]({})", r.version, r.path),
                None => String::new(),
            };
            let main_str = match main {
                Some(r) => format!("[{}]({})", r.version, r.path),
                None => String::new(),
            };

            let kick = serde_json::from_str::<Value>(&entry.rsp.rsp.configs)
                .ok()
                .and_then(|v| v.get("kick_flag")?.as_bool())
                .unwrap_or(false);
            let kick_str = if kick { "✅" } else { "" };

            // Sort versions descending via semver
            let mut versions: Vec<&String> = entry.versions.iter().collect();
            versions.sort_by(|a, b| {
                let va = semver::Version::parse(a).ok();
                let vb = semver::Version::parse(b).ok();
                vb.cmp(&va)
            });
            let versions_str = versions
                .iter()
                .map(|s| s.as_str())
                .collect::<Vec<_>>()
                .join(", ");

            md_texts.push(format!(
                "|{date_str}|{initial_str}|{main_str}|{kick_str}|{versions_str}|"
            ));
        }
        md_texts.push(String::new());
    }

    let out_path = output_dir
        .join(LAUNCHER_BASE)
        .join("game_resources")
        .join(channel_str)
        .join("list.md");
    write(out_path, md_texts.join("\n")).await?;
    Ok(())
}

enum LauncherType {
    Zip,
    Exe,
}

struct LauncherSettings {
    zip: LauncherTypeSettings,
    exe: LauncherTypeSettings,
}

struct LauncherTypeSettings {
    sub_dir: &'static str,
    title: &'static str,
    headers: &'static [&'static str],
    align: &'static [&'static str],
}

struct LauncherRegion {
    id: Region,
    apps: &'static [&'static str],
    code: Option<&'static str>,
    channel: i32,
}

const LAUNCHER_SETTINGS: LauncherSettings = LauncherSettings {
    zip: LauncherTypeSettings {
        sub_dir: "launcher",
        title: "Launcher Packages (zip)",
        headers: &[
            "Date",
            "Version",
            "File",
            "MD5 Checksum",
            "Unpacked",
            "Packed",
        ],
        align: &["---", "---", "---", "---", "--:", "--:"],
    },
    exe: LauncherTypeSettings {
        sub_dir: "launcherExe",
        title: "Launcher Packages (Installer)",
        headers: &["Date", "Version", "File", "Size"],
        align: &["---", "---", "---", "--:"],
    },
};

const LAUNCHER_REGIONS: [LauncherRegion; 2] = [
    LauncherRegion {
        id: Region::Os,
        apps: &["EndField", "Official"],
        code: Some(app_code::launcher::OS_WIN_REL),
        channel: channel::OS_WIN_REL,
    },
    LauncherRegion {
        id: Region::Cn,
        apps: &["EndField", "Arknights", "Official"],
        code: Some(app_code::launcher::CN_WIN_REL),
        channel: channel::CN_WIN_REL,
    },
];

async fn generate_launcher_md(
    launcher_type: LauncherType,
    output_dir: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let settings = match launcher_type {
        LauncherType::Exe => &LAUNCHER_SETTINGS.exe,
        LauncherType::Zip => &LAUNCHER_SETTINGS.zip,
    };

    let mut lines: Vec<String> = vec![format!("# {}\n", settings.title)];

    for r in &LAUNCHER_REGIONS {
        for app in r.apps {
            lines.push(format!(
                "- [{} {}](#launcher-{}-{})",
                r.id.as_str_upper(),
                app,
                r.id,
                app.to_lowercase()
            ));
        }
    }
    lines.push("\n".into());

    for region in &LAUNCHER_REGIONS {
        for app_name in region.apps {
            let json_path = output_dir
                .join(LAUNCHER_BASE)
                .join(settings.sub_dir)
                .join(app_name)
                .join(region.channel.to_string())
                .join("all.json");

            if !json_path.exists() {
                continue;
            }

            let json_data: Vec<StoredData<Value>> = read_json_file(&json_path).await?;

            lines.push(format!(
                "<h2 id=\"launcher-{}-{}\">{} {}</h2>\n",
                region.id,
                app_name.to_lowercase(),
                region.id.as_str_upper(),
                app_name
            ));
            lines.push(format!("|{}|", settings.headers.join("|")));
            lines.push(format!("|{}|", settings.align.join("|")));

            for e in &json_data {
                let rsp = &e.rsp;

                let url = match launcher_type {
                    LauncherType::Zip => rsp.get("zip_package_url"),
                    LauncherType::Exe => rsp.get("exe_url"),
                }
                .and_then(|v| v.as_str())
                .unwrap_or("");

                let file_name = url_filename(url);
                let file_link = format!("[{file_name}]({url})");

                let (_, date_str) = parse_date_utc8(&e.updated_at)?;

                let version = rsp.get("version").and_then(|v| v.as_str()).unwrap_or("");

                let row: Vec<String> = match launcher_type {
                    LauncherType::Zip => {
                        let total: i64 = rsp
                            .get("total_size")
                            .and_then(|v| v.as_str())
                            .unwrap_or("0")
                            .parse()?;
                        let packed: i64 = rsp
                            .get("package_size")
                            .and_then(|v| v.as_str())
                            .unwrap_or("0")
                            .parse()?;
                        let md5 = rsp.get("md5").and_then(|v| v.as_str()).unwrap_or("");
                        vec![
                            date_str,
                            version.into(),
                            file_link,
                            format!("`{md5}`"),
                            format_bytes(total - packed),
                            format_bytes(packed),
                        ]
                    }
                    LauncherType::Exe => {
                        let exe_size: i64 = rsp
                            .get("exe_size")
                            .and_then(|v| v.as_str())
                            .unwrap_or("0")
                            .parse()?;
                        vec![date_str, version.into(), file_link, format_bytes(exe_size)]
                    }
                };

                lines.push(format!("|{}|", row.join("|")));
            }
            lines.push(String::new());
        }
    }

    let out_path = output_dir
        .join(LAUNCHER_BASE)
        .join(settings.sub_dir)
        .join("list.md");
    write(out_path, lines.join("\n")).await?;
    Ok(())
}

async fn fetch_and_save_latest_game(
    client: &Client,
    targets: &[GameTarget],
    output_dir: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    for target in targets {
        println!("Fetching latest game {}...", target.name);
        let rsp = latest_game(
            client,
            app_code::game::OS_WIN_REL,
            &target.launcher_app_code,
            channel::OS_WIN_REL,
            target.sub_channel,
            target.launcher_sub_channel,
            None,
        )
        .await?;

        let packed_size: i64 = rsp
            .pkg
            .packs
            .iter()
            .filter_map(|e| e.package_size.parse::<i64>().ok())
            .sum();
        let total_size: i64 = rsp.pkg.total_size.parse()?;

        println!(
            "Fetched latest game ({}): v{}, {}",
            target.name,
            rsp.version,
            format_bytes(total_size - packed_size)
        );

        let pretty_rsp = SaveData {
            req: json!({
                "appCode": app_code::game::OS_WIN_REL,
                "launcherAppCode": target.launcher_app_code,
                "channel": channel::OS_WIN_REL,
                "subChannel": target.sub_channel,
                "launcherSubChannel": target.launcher_sub_channel,
            }),
            rsp,
        };

        save_result(
            &[LAUNCHER_BASE, "game", &target.dir_name],
            &pretty_rsp.rsp.version.clone(),
            &pretty_rsp,
            true,
            &[],
            output_dir,
        )
        .await?;
    }
    Ok(())
}

async fn fetch_and_save_latest_game_patches(
    client: &Client,
    targets: &[GameTarget],
    output_dir: &Path,
    concurrency: usize,
) -> Result<(), Box<dyn std::error::Error>> {
    for target in targets {
        println!("Fetching latest game patch {}", target.name);

        let base = game_base_path(output_dir, &target.dir_name);
        let game_all_path = base.join("all.json");
        let patch_all_path = base.join("all_patch.json");

        if !game_all_path.exists() {
            continue;
        }

        let game_all: Vec<StoredData<LauncherLatestGame>> = read_json_file(&game_all_path).await?;

        let mut patch_all: Vec<StoredData<LauncherLatestGame>> = if patch_all_path.exists() {
            read_json_file(&patch_all_path).await?
        } else {
            Vec::new()
        };

        let mut versions: BTreeSet<semver::Version> = BTreeSet::new();
        for e in &game_all {
            if let Ok(v) = semver::Version::parse(&e.rsp.version) {
                versions.insert(v);
            }
        }

        let version_list: Vec<String> = versions
            .iter()
            .rev()
            .skip(1)
            .map(|v| v.to_string())
            .collect();

        let existing_strs: Vec<String> = patch_all
            .iter()
            .filter_map(|e| to_string(&json!({ "req": &e.req, "rsp": &e.rsp })).ok())
            .collect();

        let results: Vec<Option<StoredData<LauncherLatestGame>>> = stream::iter(version_list)
            .map(|ver| {
                let existing_strs = &existing_strs;
                let target = &target;
                async move {
                    let rsp = latest_game(
                        client,
                        app_code::game::OS_WIN_REL,
                        &target.launcher_app_code,
                        channel::OS_WIN_REL,
                        target.sub_channel,
                        target.launcher_sub_channel,
                        Some(&ver),
                    )
                    .await
                    .ok()?;

                    rsp.patch.as_ref()?; // skip if no patch

                    let pretty_rsp = SaveData {
                        req: json!({
                            "appCode": app_code::game::OS_WIN_REL,
                            "launcherAppCode": target.launcher_app_code,
                            "channel": channel::OS_WIN_REL,
                            "subChannel": target.sub_channel,
                            "launcherSubChannel": target.launcher_sub_channel,
                            "version": ver,
                        }),
                        rsp,
                    };

                    let pretty_str = serde_json::to_string(&json!({
                        "req": &pretty_rsp.req,
                        "rsp": &pretty_rsp.rsp,
                    }))
                    .ok()?;

                    if existing_strs.contains(&pretty_str) {
                        return None;
                    }

                    let patch = pretty_rsp.rsp.patch.as_ref().unwrap();
                    let total: i64 = patch.total_size.parse().unwrap_or(0);
                    let packed: i64 = patch.package_size.parse().unwrap_or(0);
                    println!(
                        "Fetched latestGame (patch) ({}): v{} -> v{}, {}",
                        target.name,
                        pretty_rsp.rsp.request_version,
                        pretty_rsp.rsp.version,
                        format_bytes(total - packed)
                    );

                    Some(StoredData {
                        updated_at: chrono::Utc::now().to_rfc3339(),
                        req: pretty_rsp.req,
                        rsp: pretty_rsp.rsp,
                    })
                }
            })
            .buffer_unordered(concurrency)
            .collect()
            .await;

        let new_entries: Vec<_> = results.into_iter().flatten().collect();
        if !new_entries.is_empty() {
            patch_all.extend(new_entries);
            write(&patch_all_path, to_string_pretty(&patch_all)?).await?;
        }
    }
    Ok(())
}

struct VersionInfo {
    version: String,
    version_minor: String,
    rand_str: String,
}

async fn fetch_and_save_latest_game_resources(
    client: &Client,
    channel_str: &str,
    output_dir: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    println!("Fetching latest game resources...");

    let platforms = [
        Platform::Windows,
        Platform::Android,
        Platform::Ios,
        Platform::PlayStation,
    ];

    let game_all_path = output_dir
        .join(LAUNCHER_BASE)
        .join("game")
        .join(channel_str)
        .join("all.json");

    if !game_all_path.exists() {
        eprintln!("Skipping latestGameRes: game/all.json not found");
        return Ok(());
    }

    let game_all: Vec<StoredData<LauncherLatestGame>> = read_json_file(&game_all_path).await?;

    let re = regex::Regex::new(r"_([^/]+)/.+?$")?;

    let mut version_info_list: Vec<VersionInfo> = game_all
        .iter()
        .filter_map(|e| {
            let ver = semver::Version::parse(&e.rsp.version).ok()?;
            let rand_str = re
                .captures(&e.rsp.pkg.file_path)?
                .get(1)?
                .as_str()
                .to_string();
            Some(VersionInfo {
                version: e.rsp.version.clone(),
                version_minor: format!("{}.{}", ver.major, ver.minor),
                rand_str,
            })
        })
        .collect();

    version_info_list.sort_by(|a, b| {
        let va = semver::Version::parse(&a.version).ok();
        let vb = semver::Version::parse(&b.version).ok();
        vb.cmp(&va)
    });

    for platform in &platforms {
        let mut is_latest_wrote = false;
        let platform_str = format!("{:?}", platform);

        for entry in &version_info_list {
            if entry.rand_str.is_empty() {
                return Err("version rand_str not found".into());
            }

            let rsp = latest_game_resources(
                client,
                app_code::game::OS_WIN_REL,
                &entry.version_minor,
                &entry.version,
                &entry.rand_str,
                platform.clone(),
            )
            .await?;

            println!(
                "Fetched latestGameRes: {}, v{}, {}",
                platform_str, entry.version, rsp.res_version
            );

            let pretty_rsp = SaveData {
                req: json!({
                    "appCode": app_code::game::OS_WIN_REL,
                    "gameVersion": entry.version_minor,
                    "version": entry.version,
                    "randStr": entry.rand_str,
                    "platform": platform_str,
                }),
                rsp,
            };

            save_result(
                &[LAUNCHER_BASE, "game_resources", channel_str, &platform_str],
                &entry.version,
                &pretty_rsp,
                !is_latest_wrote,
                &[],
                output_dir,
            )
            .await?;

            is_latest_wrote = true;
        }
    }

    Ok(())
}

async fn fetch_and_save_latest_launcher(
    client: &Client,
    output_dir: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    println!("Fetching latest launcher...");

    let ignore_rules = [
        IgnoreRule {
            path: vec!["rsp".into(), "zip_package_url".into()],
            pattern: regex::Regex::new(r"\?auth_key=.*$")?,
        },
        IgnoreRule {
            path: vec!["rsp".into(), "exe_url".into()],
            pattern: regex::Regex::new(r"\?auth_key=.*$")?,
        },
    ];

    for region in &LAUNCHER_REGIONS {
        for app in region.apps {
            let rsp = latest_launcher(
                client,
                region.code.unwrap_or(""),
                region.channel,
                region.channel,
                None,
                Some(*app),
                region.id,
            )
            .await?;

            let app_lower = app.to_lowercase();

            let rsp_exe = latest_launcher_exe(
                client,
                region.code.unwrap_or(""),
                region.channel,
                region.channel,
                None,
                &app_lower,
                region.id,
            )
            .await?;

            let region_str = region.id.as_str();
            println!(
                "Fetched latestLauncher: v{}, {}, {}",
                rsp.version, region_str, app
            );

            let channel_str = region.channel.to_string();

            let pretty_rsp = SaveData {
                req: json!({
                    "appCode": region.code,
                    "channel": region.channel,
                    "subChannel": region.channel,
                    "targetApp": app,
                }),
                rsp,
            };

            let pretty_rsp_exe = SaveData {
                req: json!({
                    "appCode": region.code,
                    "channel": region.channel,
                    "subChannel": region.channel,
                    "ta": app_lower,
                }),
                rsp: rsp_exe,
            };

            let version = pretty_rsp.rsp.version.clone();
            save_result(
                &[LAUNCHER_BASE, "launcher", app, &channel_str],
                &version,
                &pretty_rsp,
                true,
                &ignore_rules,
                output_dir,
            )
            .await?;

            let version_exe = pretty_rsp_exe.rsp.version.clone();
            save_result(
                &[LAUNCHER_BASE, "launcherExe", app, &channel_str],
                &version_exe,
                &pretty_rsp_exe,
                true,
                &ignore_rules,
                output_dir,
            )
            .await?;
        }
    }

    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let client = Client::new();
    let output_dir = Path::new("output");
    let channel_str = channel::OS_WIN_REL.to_string();

    let game_targets = vec![
        GameTarget {
            name: "Official".into(),
            launcher_app_code: app_code::launcher::OS_WIN_REL.into(),
            sub_channel: sub_channel::OS_WIN_REL,
            launcher_sub_channel: sub_channel::OS_WIN_REL,
            dir_name: sub_channel::OS_WIN_REL.to_string(),
        },
        GameTarget {
            name: "Epic".into(),
            launcher_app_code: app_code::launcher::OS_WIN_REL_EPIC.into(),
            sub_channel: sub_channel::OS_WIN_REL_EPIC,
            launcher_sub_channel: sub_channel::OS_WIN_REL_EPIC,
            dir_name: sub_channel::OS_WIN_REL_EPIC.to_string(),
        },
        GameTarget {
            name: "Google Play".into(),
            launcher_app_code: app_code::launcher::OS_WIN_REL_EPIC.into(),
            sub_channel: sub_channel::OS_WIN_REL_GOOGLE_PLAY,
            launcher_sub_channel: sub_channel::OS_WIN_REL_GOOGLE_PLAY,
            dir_name: sub_channel::OS_WIN_REL_GOOGLE_PLAY.to_string(),
        },
    ];

    // Fetch data
    fetch_and_save_latest_game(&client, &game_targets, output_dir).await?;
    fetch_and_save_latest_game_patches(&client, &game_targets, output_dir, NETWORK_CONCURRENCY)
        .await?;
    fetch_and_save_latest_game_resources(&client, &channel_str, output_dir).await?;
    fetch_and_save_latest_launcher(&client, output_dir).await?;

    // Generate markdown
    for target in &game_targets {
        generate_game_list_md(target, output_dir).await?;
        generate_patch_list_md(target, output_dir).await?;
    }
    generate_resource_list_md(&channel_str, output_dir).await?;
    generate_launcher_md(LauncherType::Zip, output_dir).await?;
    generate_launcher_md(LauncherType::Exe, output_dir).await?;

    println!("Done.");
    Ok(())
}
