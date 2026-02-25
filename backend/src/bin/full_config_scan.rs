/// Full config VFS scan — find character stats across ALL 108 .chk files.
///
/// Pipeline: config → ZIP → .chk → VFS decrypt → UnityFS → SerializedFile → TextAsset + MonoBehaviour
///
/// Env vars:
///   PASS=1|2|3      — run specific pass (default: all)
///   MAX_FILES=N     — limit .chk files processed (default: all)
///   SKIP_EXISTING=0 — disable skip of already-extracted TextAssets (default: on)
use backend::core::endfield::{
    config::{ResolveParams, resolve_game_config},
    constants::{app_code, channel, sub_channel},
};
use backend::core::hypergryph::launcher::Platform;
use backend::extraction::unityfs_header;
use backend::extraction::vfs_decrypt;
use backend::extraction::zip_range::{SplitArchivePart, ZipCentralDirectory};
use reqwest::Client;
use std::collections::{BTreeMap, HashMap, HashSet};
use unity_asset_binary::asset::{ObjectInfo, SerializedFile, SerializedFileParser};
use unity_asset_binary::object::ObjectHandle;

/// Keywords that suggest stat/numeric config data.
const STAT_KEYWORDS: &[&str] = &[
    "hp",
    "atk",
    "def",
    "attack",
    "defense",
    "health",
    "crit",
    "stat",
    "property",
    "growth",
    "curve",
    "level",
    "base_",
    "max_",
    "min_",
    "ratio",
    "scale",
    "modifier",
    "attribute",
    "param",
    "formula",
    "damage",
    "resist",
];

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

/// Get Unity class name from class ID.
fn class_name(id: i32) -> String {
    unity_asset_core::get_class_name(id).unwrap_or_else(|| format!("Class_{id}"))
}

fn sanitize_filename(name: &str) -> String {
    name.replace(['\\', ':', '*', '?', '"', '<', '>', '|'], "_")
}

/// Get the script type name for a MonoBehaviour object.
fn get_script_type_name(sf: &SerializedFile, info: &ObjectInfo) -> String {
    if info.type_index >= 0
        && let Some(st) = sf.types.get(info.type_index as usize)
    {
        if !st.type_tree.nodes.is_empty() {
            let root_type = &st.type_tree.nodes[0].type_name;
            if !root_type.is_empty() && root_type != "MonoBehaviour" {
                return root_type.clone();
            }
        }
        if !st.class_name.is_empty() {
            return st.class_name.clone();
        }
        if st.script_id != [0u8; 16] {
            return format!(
                "script_{}",
                st.script_id
                    .iter()
                    .map(|b| format!("{b:02x}"))
                    .collect::<String>()
            );
        }
    }
    format!("class_{}", info.type_id)
}

/// Parse a TextAsset from raw object bytes (little-endian).
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

/// All extracted SerializedFile data from a .chk file.
struct ChkContents {
    sf_data: Vec<Vec<u8>>,
}

fn extract_chk_contents(data: &[u8]) -> Option<ChkContents> {
    if !vfs_decrypt::is_valid_header(data) {
        return None;
    }
    let assets = match unityfs_header::process_chk_file(data) {
        Ok(a) => a,
        Err(_) => return None,
    };

    let mut sf_data: Vec<Vec<u8>> = Vec::new();
    for asset in assets {
        if asset.is_unityfs {
            for entry in &asset.bundle_entries {
                let start = entry.offset as usize;
                let end = (start + entry.size as usize).min(asset.data.len());
                if start >= asset.data.len() {
                    continue;
                }
                if !entry.path.ends_with(".resS") && !entry.path.ends_with(".resource") {
                    sf_data.push(asset.data[start..end].to_vec());
                }
            }
        } else if !asset.vfs_name.ends_with(".resS") && !asset.vfs_name.ends_with(".resource") {
            sf_data.push(asset.data);
        }
    }

    Some(ChkContents { sf_data })
}

/// Census data for a single .chk file.
struct ChkCensus {
    total_objects: usize,
    class_counts: BTreeMap<i32, usize>,
    class_bytes: BTreeMap<i32, u64>,
    text_asset_names: Vec<String>,
    mono_type_names: Vec<String>,
    sf_count: usize,
}

/// Pass 1: catalog all objects in a .chk file.
fn pass1_census(sf_list: &[Vec<u8>]) -> ChkCensus {
    let mut census = ChkCensus {
        total_objects: 0,
        class_counts: BTreeMap::new(),
        class_bytes: BTreeMap::new(),
        text_asset_names: Vec::new(),
        mono_type_names: Vec::new(),
        sf_count: sf_list.len(),
    };

    for sf_bytes in sf_list {
        let sf = match SerializedFileParser::from_bytes(sf_bytes.clone()) {
            Ok(sf) => sf,
            Err(_) => continue,
        };

        for obj in &sf.objects {
            census.total_objects += 1;
            *census.class_counts.entry(obj.type_id).or_insert(0) += 1;
            *census.class_bytes.entry(obj.type_id).or_insert(0) += obj.byte_size as u64;

            // TextAsset names
            if obj.type_id == 49
                && let Ok(bytes) = sf.object_bytes(obj)
                && let Some((name, _)) = parse_text_asset(bytes)
            {
                census.text_asset_names.push(name);
            }

            // MonoBehaviour type names
            if obj.type_id == 114 {
                let type_name = get_script_type_name(&sf, obj);
                if !census.mono_type_names.contains(&type_name) {
                    census.mono_type_names.push(type_name);
                }
            }
        }
    }

    census
}

/// Check if a JSON string contains stat-related keywords.
fn check_stat_keywords(json_text: &str) -> Vec<String> {
    let lower = json_text.to_lowercase();
    STAT_KEYWORDS
        .iter()
        .filter(|kw| lower.contains(**kw))
        .map(|kw| (*kw).to_string())
        .collect()
}

/// Save a TextAsset to disk. Returns (file_type, stat_hits).
fn save_text_asset(
    output_dir: &str,
    name: &str,
    content: &[u8],
) -> Option<(&'static str, Vec<String>)> {
    if content.is_empty() {
        return None;
    }

    let safe_name = sanitize_filename(name);

    if let Some(parent) = std::path::Path::new(&safe_name).parent()
        && parent != std::path::Path::new("")
    {
        let full_parent = format!("{output_dir}/{}", parent.display());
        let _ = std::fs::create_dir_all(&full_parent);
    }

    // Try JSON
    if let Ok(text) = std::str::from_utf8(content) {
        if let Ok(json) = serde_json::from_str::<serde_json::Value>(text) {
            let pretty = match serde_json::to_string_pretty(&json) {
                Ok(p) => p,
                Err(_) => return None,
            };
            let stat_hits = check_stat_keywords(&pretty);
            let path = format!("{output_dir}/{safe_name}.json");
            if std::fs::write(&path, &pretty).is_ok() {
                return Some(("JSON", stat_hits));
            }
        }

        // Valid UTF-8 but not JSON
        if !text.trim().is_empty() {
            let stat_hits = check_stat_keywords(text);
            let path = format!("{output_dir}/{safe_name}.txt");
            if std::fs::write(&path, content).is_ok() {
                return Some(("TEXT", stat_hits));
            }
        }
    }

    // Binary
    let path = format!("{output_dir}/{safe_name}.bytes");
    if std::fs::write(&path, content).is_ok() {
        return Some(("BIN", Vec::new()));
    }

    None
}

/// Extracted MonoBehaviour data.
struct MonoExport {
    type_name: String,
    obj_name: String,
    json: String,
}

/// Extract MonoBehaviour objects from a SerializedFile.
fn extract_monobehaviours(data: Vec<u8>) -> Vec<MonoExport> {
    let sf = match SerializedFileParser::from_bytes(data) {
        Ok(sf) => sf,
        Err(_) => return Vec::new(),
    };

    if !sf.enable_type_tree {
        return Vec::new();
    }

    let mut results = Vec::new();

    for obj in sf.objects_of_type(114) {
        let type_name = get_script_type_name(&sf, obj);
        let handle = ObjectHandle::new(&sf, obj);

        let obj_name = handle
            .peek_name()
            .ok()
            .flatten()
            .filter(|n| !n.is_empty())
            .map_or_else(|| format!("obj_{}", obj.path_id), |n| sanitize_filename(&n));

        let unity_obj = match handle.read() {
            Ok(o) => o,
            Err(_) => continue,
        };

        let props = unity_obj.class.properties();
        let json = match serde_json::to_string_pretty(props) {
            Ok(j) => j,
            Err(_) => continue,
        };

        results.push(MonoExport {
            type_name: sanitize_filename(&type_name),
            obj_name,
            json,
        });
    }

    results
}

/// Collect existing TextAsset names from a directory (for skip logic).
fn collect_existing_names(dir: &str) -> HashSet<String> {
    let mut names = HashSet::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return names;
    };
    for entry in entries.flatten() {
        if let Some(stem) = entry.path().file_stem() {
            names.insert(stem.to_string_lossy().to_string());
        }
    }
    names
}

/// Collect existing MonoBehaviour type/name combos from a directory.
fn collect_existing_monos(dir: &str) -> HashSet<String> {
    let mut seen = HashSet::new();
    let Ok(types) = std::fs::read_dir(dir) else {
        return seen;
    };
    for type_entry in types.flatten() {
        let type_name = type_entry.file_name().to_string_lossy().to_string();
        if let Ok(files) = std::fs::read_dir(type_entry.path()) {
            for file in files.flatten() {
                if let Some(stem) = file.path().file_stem() {
                    seen.insert(format!("{type_name}/{}", stem.to_string_lossy()));
                }
            }
        }
    }
    seen
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("=== Full Config VFS Scan ===\n");

    // Parse env vars
    let pass_filter: Option<u32> = std::env::var("PASS").ok().and_then(|v| v.parse().ok());
    let max_files: Option<usize> = std::env::var("MAX_FILES").ok().and_then(|v| v.parse().ok());
    let skip_existing = std::env::var("SKIP_EXISTING")
        .map(|v| v != "0")
        .unwrap_or(true);

    let run_pass1 = pass_filter.is_none() || pass_filter == Some(1);
    let run_pass2 = pass_filter.is_none() || pass_filter == Some(2);
    let run_pass3 = pass_filter.is_none() || pass_filter == Some(3);

    if let Some(p) = pass_filter {
        println!("PASS={p}");
    }
    if let Some(m) = max_files {
        println!("MAX_FILES={m}");
    }
    if skip_existing {
        println!("SKIP_EXISTING=on (set SKIP_EXISTING=0 to disable)");
    }
    println!();

    // Setup
    let client = Client::new();
    let params = ResolveParams {
        app_code: app_code::game::OS_WIN_REL,
        launcher_app_code: app_code::launcher::OS_WIN_REL,
        channel: channel::OS_WIN_REL,
        sub_channel: sub_channel::OS_WIN_REL,
        launcher_sub_channel: sub_channel::OS_WIN_REL,
        platform: Platform::Windows,
    };

    println!("[0] Resolving game config...");
    let config = resolve_game_config(&client, &params).await?;
    println!("    Version: {}", config.version);

    println!("[0] Parsing ZIP central directory...");
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

    // Only scan 7064D8E2 (configs directory)
    let config_dir = "7064D8E2";
    let mut candidates: Vec<(String, u64)> = cd
        .filenames()
        .filter(|f| f.ends_with(".chk") && f.contains(config_dir))
        .map(|f| {
            let size = cd.get(f).map_or(0, |e| e.uncompressed_size);
            (f.to_string(), size)
        })
        .collect();
    candidates.sort_by_key(|(_n, s)| *s);

    if let Some(max) = max_files {
        candidates.truncate(max);
    }

    let total_size: u64 = candidates.iter().map(|(_, s)| *s).sum();
    println!(
        "\n    Config .chk files: {} ({})\n",
        candidates.len(),
        format_bytes(total_size)
    );

    // Output directories
    let full_dir = "./output/gamedata_full";
    let full_mono_dir = format!("{full_dir}/monobehaviour");
    std::fs::create_dir_all(&full_mono_dir)?;

    let existing_dir = "./output/gamedata";

    // ==================== PASS 1: Census ====================
    if run_pass1 {
        println!("========== PASS 1: Census ==========\n");

        let mut global_class_counts: BTreeMap<i32, usize> = BTreeMap::new();
        let mut global_class_bytes: BTreeMap<i32, u64> = BTreeMap::new();
        let mut global_text_names: Vec<String> = Vec::new();
        let mut global_mono_types: HashSet<String> = HashSet::new();
        let mut total_objects = 0usize;
        let mut total_sf = 0usize;
        let mut processed = 0usize;
        let mut download_errors = 0usize;
        let mut pipeline_errors = 0usize;

        for (i, (name, size)) in candidates.iter().enumerate() {
            print!(
                "  [{}/{}] {} ({}) ... ",
                i + 1,
                candidates.len(),
                name,
                format_bytes(*size)
            );

            let data = match cd.extract(&client, name).await {
                Ok(d) => d,
                Err(e) => {
                    println!("DOWNLOAD ERROR: {e}");
                    download_errors += 1;
                    continue;
                }
            };

            let contents = match extract_chk_contents(&data) {
                Some(c) => c,
                None => {
                    println!("PIPELINE ERROR");
                    pipeline_errors += 1;
                    continue;
                }
            };

            let census = pass1_census(&contents.sf_data);
            processed += 1;
            total_objects += census.total_objects;
            total_sf += census.sf_count;

            for (&class_id, &count) in &census.class_counts {
                *global_class_counts.entry(class_id).or_insert(0) += count;
            }
            for (&class_id, &bytes) in &census.class_bytes {
                *global_class_bytes.entry(class_id).or_insert(0) += bytes;
            }

            global_text_names.extend(census.text_asset_names.iter().cloned());
            for t in &census.mono_type_names {
                global_mono_types.insert(t.clone());
            }

            println!(
                "{} SFs, {} objs, {} TextAssets, {} MonoTypes",
                census.sf_count,
                census.total_objects,
                census.text_asset_names.len(),
                census.mono_type_names.len()
            );
        }

        println!("\n--- Pass 1 Summary ---");
        println!("  Processed: {processed}/{} files", candidates.len());
        println!("  Download errors: {download_errors}");
        println!("  Pipeline errors: {pipeline_errors}");
        println!("  Total SerializedFiles: {total_sf}");
        println!("  Total objects: {total_objects}");

        println!("\n  Class distribution:");
        let mut class_list: Vec<_> = global_class_counts.iter().collect();
        class_list.sort_by_key(|(_, count)| std::cmp::Reverse(**count));
        for &(&cid, &count) in &class_list {
            let bytes = global_class_bytes.get(&cid).copied().unwrap_or(0);
            println!(
                "    {:>6}x {} (classID={}, {})",
                count,
                class_name(cid),
                cid,
                format_bytes(bytes)
            );
        }

        println!("\n  TextAsset names ({} total):", global_text_names.len());
        let unique_text: HashSet<_> = global_text_names.iter().collect();
        let mut text_sorted: Vec<_> = unique_text.into_iter().collect();
        text_sorted.sort();
        for name in &text_sorted {
            println!("    {name}");
        }

        println!(
            "\n  MonoBehaviour types ({} unique):",
            global_mono_types.len()
        );
        let mut mono_sorted: Vec<_> = global_mono_types.iter().collect();
        mono_sorted.sort();
        for t in &mono_sorted {
            println!("    {t}");
        }
    }

    // ==================== PASS 2: Extract TextAssets ====================
    if run_pass2 {
        println!("\n========== PASS 2: Extract TextAssets ==========\n");

        let existing_names = if skip_existing {
            collect_existing_names(existing_dir)
        } else {
            HashSet::new()
        };

        if skip_existing {
            println!(
                "  Skipping {} already-extracted names from {existing_dir}/\n",
                existing_names.len()
            );
        }

        let mut total_extracted = 0u64;
        let mut total_json = 0u64;
        let mut total_text = 0u64;
        let mut total_bin = 0u64;
        let mut stat_hits: Vec<(String, Vec<String>)> = Vec::new();
        let mut seen_names: HashSet<String> = HashSet::new();

        for (i, (name, size)) in candidates.iter().enumerate() {
            print!(
                "  [{}/{}] {} ({}) ... ",
                i + 1,
                candidates.len(),
                name,
                format_bytes(*size)
            );

            let data = match cd.extract(&client, name).await {
                Ok(d) => d,
                Err(e) => {
                    println!("DOWNLOAD ERROR: {e}");
                    continue;
                }
            };

            let contents = match extract_chk_contents(&data) {
                Some(c) => c,
                None => {
                    println!("PIPELINE ERROR");
                    continue;
                }
            };

            let mut file_count = 0u64;
            for sf_bytes in &contents.sf_data {
                let sf = match SerializedFileParser::from_bytes(sf_bytes.clone()) {
                    Ok(sf) => sf,
                    Err(_) => continue,
                };

                for obj in sf.objects_of_type(49) {
                    if let Ok(bytes) = sf.object_bytes(obj)
                        && let Some((ta_name, content)) = parse_text_asset(bytes)
                    {
                        if !seen_names.insert(ta_name.clone()) {
                            continue;
                        }
                        if skip_existing && existing_names.contains(&ta_name) {
                            continue;
                        }

                        if let Some((file_type, hits)) =
                            save_text_asset(full_dir, &ta_name, &content)
                        {
                            file_count += 1;
                            total_extracted += 1;
                            match file_type {
                                "JSON" => total_json += 1,
                                "TEXT" => total_text += 1,
                                "BIN" => total_bin += 1,
                                _ => {}
                            }

                            if !hits.is_empty() {
                                println!(
                                    "\n    STAT_HIT [{file_type}] {ta_name} ({}) — keywords: {}",
                                    format_bytes(content.len() as u64),
                                    hits.join(", ")
                                );
                                stat_hits.push((ta_name.clone(), hits));
                            }
                        }
                    }
                }
            }

            println!("{file_count} new TextAssets");
        }

        println!("\n--- Pass 2 Summary ---");
        println!("  New TextAssets extracted: {total_extracted}");
        println!("    JSON: {total_json}");
        println!("    TEXT: {total_text}");
        println!("    BIN:  {total_bin}");
        println!("  Output: {full_dir}/");

        if !stat_hits.is_empty() {
            println!("\n  *** STAT KEYWORD HITS ({} files): ***", stat_hits.len());
            for (name, keywords) in &stat_hits {
                println!("    {name}: {}", keywords.join(", "));
            }
        } else {
            println!("\n  No stat keyword hits in TextAssets.");
        }
    }

    // ==================== PASS 3: Extract MonoBehaviours ====================
    if run_pass3 {
        println!("\n========== PASS 3: Extract MonoBehaviours ==========\n");

        let existing_monos = if skip_existing {
            let existing_mono_dir = format!("{existing_dir}/monobehaviour");
            collect_existing_monos(&existing_mono_dir)
        } else {
            HashSet::new()
        };

        if skip_existing {
            println!(
                "  Skipping {} already-extracted MonoBehaviours\n",
                existing_monos.len()
            );
        }

        let mut total_mono = 0u64;
        let mut type_counts: HashMap<String, u64> = HashMap::new();
        let mut seen_mono: HashSet<String> = HashSet::new();
        let mut stat_mono_hits: Vec<(String, String, Vec<String>)> = Vec::new();

        for (i, (name, size)) in candidates.iter().enumerate() {
            print!(
                "  [{}/{}] {} ({}) ... ",
                i + 1,
                candidates.len(),
                name,
                format_bytes(*size)
            );

            let data = match cd.extract(&client, name).await {
                Ok(d) => d,
                Err(e) => {
                    println!("DOWNLOAD ERROR: {e}");
                    continue;
                }
            };

            let contents = match extract_chk_contents(&data) {
                Some(c) => c,
                None => {
                    println!("PIPELINE ERROR");
                    continue;
                }
            };

            let mut file_count = 0u64;
            for sf_bytes in contents.sf_data {
                let monos = extract_monobehaviours(sf_bytes);
                for mono in monos {
                    let key = format!("{}/{}", mono.type_name, mono.obj_name);
                    if !seen_mono.insert(key.clone()) {
                        continue;
                    }
                    if skip_existing && existing_monos.contains(&key) {
                        continue;
                    }

                    let type_dir = format!("{full_mono_dir}/{}", mono.type_name);
                    let _ = std::fs::create_dir_all(&type_dir);

                    let path = format!("{type_dir}/{}.json", mono.obj_name);
                    if std::fs::write(&path, &mono.json).is_ok() {
                        file_count += 1;
                        total_mono += 1;
                        *type_counts.entry(mono.type_name.clone()).or_insert(0) += 1;

                        // Check for stat keywords in MonoBehaviour JSON
                        let hits = check_stat_keywords(&mono.json);
                        if !hits.is_empty() {
                            println!(
                                "\n    STAT_HIT [MONO] {}/{} ({}) — keywords: {}",
                                mono.type_name,
                                mono.obj_name,
                                format_bytes(mono.json.len() as u64),
                                hits.join(", ")
                            );
                            stat_mono_hits.push((
                                mono.type_name.clone(),
                                mono.obj_name.clone(),
                                hits,
                            ));
                        }
                    }
                }
            }

            println!("{file_count} new MonoBehaviours");
        }

        println!("\n--- Pass 3 Summary ---");
        println!("  New MonoBehaviours extracted: {total_mono}");
        if !type_counts.is_empty() {
            let mut types: Vec<_> = type_counts.iter().collect();
            types.sort_by_key(|(_, count)| std::cmp::Reverse(**count));
            println!("  By type:");
            for (type_name, count) in &types {
                println!("    {type_name}: {count}");
            }
        }
        println!("  Output: {full_mono_dir}/");

        if !stat_mono_hits.is_empty() {
            println!(
                "\n  *** STAT KEYWORD HITS ({} MonoBehaviours): ***",
                stat_mono_hits.len()
            );
            for (type_name, obj_name, keywords) in &stat_mono_hits {
                println!("    {type_name}/{obj_name}: {}", keywords.join(", "));
            }
        } else {
            println!("\n  No stat keyword hits in MonoBehaviours.");
        }
    }

    println!("\n=== Full Config Scan Complete ===");
    Ok(())
}
