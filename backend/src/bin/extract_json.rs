/// Extract game JSON/text files from VFS SerializedFile data.
/// Pipeline: config → ZIP → .chk → VFS decrypt → UnityFS → SerializedFile → TextAsset
use backend::core::endfield::{
    config::{ResolveParams, resolve_game_config},
    constants::{app_code, channel, sub_channel},
};
use backend::core::hypergryph::launcher::Platform;
use backend::extraction::unityfs_header;
use backend::extraction::vfs_decrypt;
use backend::extraction::zip_range::{SplitArchivePart, ZipCentralDirectory};
use reqwest::Client;
use std::collections::HashSet;
use unity_asset_binary::asset::SerializedFileParser;

/// VFS container directories.
const VFS_DIRS: &[(&str, &str)] = &[("7064D8E2", "bundles/configs"), ("0CE8FA57", "textures")];

fn format_bytes(bytes: u64) -> String {
    const UNITS: &[&str] = &["B", "KB", "MB", "GB", "TB"];
    if bytes == 0 {
        return "0 B".into();
    }
    let mut size = bytes as f64;
    let mut i = 0;
    while size >= 1024.0 && i < UNITS.len() - 1 {
        size /= 1024.0;
        i += 1;
    }
    format!("{:.2} {}", size, UNITS[i])
}

/// Parse a TextAsset from raw object bytes (little-endian).
///
/// Binary layout:
/// ```text
/// u32  m_Name length
/// [N]  m_Name bytes (UTF-8)
///      align to 4 bytes
/// u32  m_Script length
/// [N]  m_Script bytes
/// ```
fn parse_text_asset(bytes: &[u8]) -> Option<(String, Vec<u8>)> {
    if bytes.len() < 8 {
        return None;
    }

    let name_len = u32::from_le_bytes(bytes[0..4].try_into().ok()?) as usize;
    if name_len > 10_000 || 4 + name_len > bytes.len() {
        return None;
    }
    let name = String::from_utf8_lossy(&bytes[4..4 + name_len]).to_string();

    let aligned = (4 + name_len + 3) & !3;
    if aligned + 4 > bytes.len() {
        return None;
    }

    let script_len = u32::from_le_bytes(bytes[aligned..aligned + 4].try_into().ok()?) as usize;
    let script_start = aligned + 4;
    if script_len > 100_000_000 || script_start + script_len > bytes.len() {
        return None;
    }

    let script = bytes[script_start..script_start + script_len].to_vec();
    Some((name, script))
}

/// Extract TextAsset objects (classID=49) from a SerializedFile.
fn extract_text_assets(data: Vec<u8>) -> Vec<(String, Vec<u8>)> {
    let sf = match SerializedFileParser::from_bytes(data) {
        Ok(sf) => sf,
        Err(_) => return Vec::new(),
    };

    let mut results = Vec::new();

    for obj in sf.objects_of_type(49) {
        if let Ok(bytes) = sf.object_bytes(obj)
            && let Some(pair) = parse_text_asset(bytes)
        {
            results.push(pair);
        }
    }

    results
}

/// Sanitize a filename: replace invalid characters, preserve `/` for subdirectories.
fn sanitize_filename(name: &str) -> String {
    name.replace(['\\', ':', '*', '?', '"', '<', '>', '|'], "_")
}

/// Save a TextAsset to disk. Returns the file type label.
fn save_asset(output_dir: &str, name: &str, content: &[u8]) -> Option<&'static str> {
    if content.is_empty() {
        return None;
    }

    let safe_name = sanitize_filename(name);

    if let Some(parent) = std::path::Path::new(&safe_name).parent()
        && parent != std::path::Path::new("")
    {
        let full_parent = format!("{}/{}", output_dir, parent.display());
        let _ = std::fs::create_dir_all(&full_parent);
    }

    // Try JSON
    if let Ok(text) = std::str::from_utf8(content) {
        if let Ok(json) = serde_json::from_str::<serde_json::Value>(text) {
            let path = format!("{}/{}.json", output_dir, safe_name);
            if let Ok(pretty) = serde_json::to_string_pretty(&json)
                && std::fs::write(&path, &pretty).is_ok()
            {
                return Some("JSON");
            }
        }

        // Valid UTF-8 but not JSON — save as .txt
        if !text.trim().is_empty() {
            let path = format!("{}/{}.txt", output_dir, safe_name);
            if std::fs::write(&path, content).is_ok() {
                return Some("TEXT");
            }
        }
    }

    // Binary → .bytes
    let path = format!("{}/{}.bytes", output_dir, safe_name);
    if std::fs::write(&path, content).is_ok() {
        return Some("BIN");
    }

    None
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("=== Game JSON Extractor ===\n");

    let scan_all = std::env::var("SCAN_ALL").is_ok();

    if scan_all {
        println!("SCAN_ALL: processing ALL .chk files\n");
    }

    let client = Client::new();
    let params = ResolveParams {
        app_code: app_code::game::OS_WIN_REL,
        launcher_app_code: app_code::launcher::OS_WIN_REL,
        channel: channel::OS_WIN_REL,
        sub_channel: sub_channel::OS_WIN_REL,
        launcher_sub_channel: sub_channel::OS_WIN_REL,
        platform: Platform::Windows,
    };

    println!("[1] Resolving game config...");
    let config = resolve_game_config(&client, &params).await?;
    println!("    Version: {}", config.version);

    println!("\n[2] Parsing ZIP central directory...");
    let parts: Vec<SplitArchivePart> = config
        .packs
        .iter()
        .map(|p| SplitArchivePart {
            url: p.url.clone(),
            size: p.package_size.parse::<u64>().expect("invalid package_size"),
        })
        .collect();
    let cd = ZipCentralDirectory::from_split_archive(&client, parts).await?;
    println!("    Total ZIP entries: {}", cd.len());

    let output_dir = "./output/gamedata";
    std::fs::create_dir_all(output_dir)?;

    println!("\n[3] Extracting TextAssets...\n");
    let mut total_assets = 0u64;
    let mut total_json = 0u64;
    let mut total_text = 0u64;
    let mut total_bin = 0u64;
    let mut seen_names: HashSet<String> = HashSet::new();

    for (dir, desc) in VFS_DIRS {
        println!("--- {} ({}) ---", dir, desc);

        let mut candidates: Vec<(String, u64)> = cd
            .filenames()
            .filter(|f| f.ends_with(".chk") && f.contains(dir))
            .map(|f| {
                let size = cd.get(f).map_or(0, |e| e.uncompressed_size);
                (f.to_string(), size)
            })
            .collect();
        candidates.sort_by_key(|(_n, s)| *s);

        if !scan_all {
            candidates.truncate(1);
        }

        println!("  Processing {} .chk file(s)\n", candidates.len());

        for (i, (name, size)) in candidates.iter().enumerate() {
            if candidates.len() > 1 {
                println!(
                    "  [{}/{}] {} ({})",
                    i + 1,
                    candidates.len(),
                    name,
                    format_bytes(*size)
                );
            } else {
                println!("  {} ({})", name, format_bytes(*size));
            }

            let data = match cd.extract(&client, name).await {
                Ok(d) => d,
                Err(e) => {
                    println!("    Download error: {}", e);
                    continue;
                }
            };

            if !vfs_decrypt::is_valid_header(&data) {
                println!("    Not a valid VFS container, skipping");
                continue;
            }

            let assets = match unityfs_header::process_chk_file(&data) {
                Ok(a) => a,
                Err(e) => {
                    println!("    Pipeline error: {}", e);
                    continue;
                }
            };

            println!("    {} assets from VFS pipeline", assets.len());

            for asset in assets {
                // For UnityFS bundles, extract individual files from directory entries
                let serialized_files: Vec<Vec<u8>> = if asset.is_unityfs {
                    asset
                        .bundle_entries
                        .iter()
                        .filter_map(|entry| {
                            let start = entry.offset as usize;
                            let end = (start + entry.size as usize).min(asset.data.len());
                            if start < asset.data.len() {
                                Some(asset.data[start..end].to_vec())
                            } else {
                                None
                            }
                        })
                        .collect()
                } else {
                    vec![asset.data]
                };

                for sf_data in serialized_files {
                    let text_assets = extract_text_assets(sf_data);

                    for (ta_name, content) in text_assets {
                        if !seen_names.insert(ta_name.clone()) {
                            continue; // Skip duplicates
                        }
                        total_assets += 1;

                        if let Some(file_type) = save_asset(output_dir, &ta_name, &content) {
                            match file_type {
                                "JSON" => total_json += 1,
                                "TEXT" => total_text += 1,
                                "BIN" => total_bin += 1,
                                _ => {}
                            }
                            println!(
                                "    [{}] {} ({})",
                                file_type,
                                ta_name,
                                format_bytes(content.len() as u64)
                            );
                        }
                    }
                }
            }
        }
        println!();
    }

    println!("=== Extraction Summary ===");
    println!("  TextAssets found: {}", total_assets);
    println!("  JSON files:       {}", total_json);
    println!("  Text files:       {}", total_text);
    println!("  Binary files:     {}", total_bin);
    println!("  Output directory: {}", output_dir);
    println!("\n=== Done ===");

    Ok(())
}
