/// Unified CLI for Arknights Endfield asset extraction.
///
/// Subcommands:
///   info          — show game config, version, patch info
///   extract       — extract Unity assets (JSON, textures, gamedata) from AES dirs
///   extract-file  — extract a single .chk by name
///   decrypt       — decrypt ChaCha20-encrypted VFS directories (tables, configs, lua)
///   list          — list all VFS directories and files
///   scan          — scan for asset names/paths
///   survey        — survey all Unity objects, types, counts
use backend::extraction::blc;
use backend::extraction::pipeline::{self, VFS_DIRS};
use backend::extraction::texture;
use backend::extraction::unity_extract::{
    ChkContents, class_name, extract_chk_contents, get_script_type_name, parse_text_asset,
};
use backend::extraction::utils::{format_bytes, hex_dump, sanitize_filename};
use backend::extraction::vfs_decrypt;
use clap::{Parser, Subcommand};
use reqwest::Client;
use std::collections::{BTreeMap, HashMap, HashSet};
use unity_asset_binary::asset::SerializedFileParser;
use unity_asset_binary::object::ObjectHandle;

#[derive(Parser)]
#[command(name = "endfield", about = "Arknights Endfield asset extraction CLI")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Show game config, version, patch info
    Info,
    /// Extract assets from VFS
    Extract {
        /// Extract JSON/TextAssets
        #[arg(long)]
        json: bool,
        /// Extract Texture2D as PNG
        #[arg(long)]
        textures: bool,
        /// Extract MonoBehaviour + registry data
        #[arg(long)]
        gamedata: bool,
        /// Extract all types (default if no flags given)
        #[arg(long)]
        all: bool,
        /// Specific VFS directory hash (default: all known)
        #[arg(long)]
        dir: Option<String>,
        /// Output directory
        #[arg(long, default_value = "./output")]
        output: String,
        /// Process ALL .chk files (default: smallest per directory)
        #[arg(long)]
        scan_all: bool,
    },
    /// Extract a single .chk by name
    ExtractFile {
        /// The .chk filename (or substring) to extract
        chk_name: String,
        /// Which VFS directory hash
        #[arg(long)]
        dir: Option<String>,
        /// Output directory
        #[arg(long, default_value = "./output")]
        output: String,
    },
    /// Decrypt ChaCha20-encrypted VFS directories (game data tables, JSON configs, Lua)
    Decrypt {
        /// Specific directory hash to decrypt (default: all encrypted dirs)
        #[arg(long)]
        dir: Option<String>,
        /// Output directory
        #[arg(long, default_value = "./output/decrypted")]
        output: String,
    },
    /// List all VFS directories and files in the CDN archive
    List,
    /// Scan for asset names/paths
    Scan {
        /// Process ALL .chk files
        #[arg(long)]
        scan_all: bool,
    },
    /// Survey all Unity objects, types, counts
    Survey {
        /// Process ALL .chk files
        #[arg(long)]
        scan_all: bool,
    },
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();
    let client = Client::new();

    match cli.command {
        Commands::Info => cmd_info(&client).await,
        Commands::Extract {
            json,
            textures,
            gamedata,
            all,
            dir,
            output,
            scan_all,
        } => {
            let (do_json, do_tex, do_gamedata) = if all || (!json && !textures && !gamedata) {
                (true, true, true)
            } else {
                (json, textures, gamedata)
            };
            cmd_extract(
                &client,
                do_json,
                do_tex,
                do_gamedata,
                dir,
                &output,
                scan_all,
            )
            .await
        }
        Commands::ExtractFile {
            chk_name,
            dir,
            output,
        } => cmd_extract_file(&client, &chk_name, dir, &output).await,
        Commands::Decrypt { dir, output } => cmd_decrypt(&client, dir, &output).await,
        Commands::List => cmd_list(&client).await,
        Commands::Scan { scan_all } => cmd_scan(&client, scan_all).await,
        Commands::Survey { scan_all } => cmd_survey(&client, scan_all).await,
    }
}

// ═══════════════════════════════════════════════════════════
// info — show game config
// ═══════════════════════════════════════════════════════════

async fn cmd_info(client: &Client) -> Result<(), Box<dyn std::error::Error>> {
    println!("=== Endfield Game Info ===\n");

    let resolved = pipeline::resolve_pipeline(client, true).await?;
    let config = &resolved.config;

    println!("Version:      {}", config.version);
    println!("Game version: {}", config.game_version);
    println!("Rand str:     {}", config.rand_str);
    println!("Packs:        {} files", config.packs.len());
    println!("Resources:    {} entries", config.resources.len());
    println!("Domain:       {}", config.domain);
    println!("Res version:  {}", config.res_version);

    for res in &config.resources {
        println!("  Resource: {} v{}", res.name, res.version);
    }

    if let Some(patch) = &resolved.patch_index {
        println!("\nPatch version: {}", patch.version);
        println!("Chunk files:   {}", patch.files.len());
        let total_bytes: u64 = patch.files.iter().map(|f| f.size).sum();
        for (i, file) in patch.files.iter().enumerate() {
            println!(
                "  [{}] {} ({}, md5: {})",
                i,
                file.name,
                format_bytes(file.size),
                file.md5
            );
        }
        println!("Total size: {}", format_bytes(total_bytes));
    }

    println!("\nZIP entries: {}", resolved.cd.len());
    println!("\n=== Done ===");
    Ok(())
}

// ═══════════════════════════════════════════════════════════
// extract — JSON, textures, gamedata
// ═══════════════════════════════════════════════════════════

async fn cmd_extract(
    client: &Client,
    do_json: bool,
    do_tex: bool,
    do_gamedata: bool,
    dir_filter: Option<String>,
    output: &str,
    scan_all: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    println!("=== Endfield Asset Extraction ===\n");

    let modes: Vec<&str> = [
        do_json.then_some("JSON"),
        do_tex.then_some("textures"),
        do_gamedata.then_some("gamedata"),
    ]
    .into_iter()
    .flatten()
    .collect();
    println!("Modes: {}", modes.join(", "));

    let resolved = pipeline::resolve_pipeline(client, false).await?;
    println!("Version: {}", resolved.config.version);
    println!("ZIP entries: {}\n", resolved.cd.len());

    let dirs: Vec<(&str, &str)> = if let Some(ref d) = dir_filter {
        VFS_DIRS
            .iter()
            .filter(|(hash, _)| hash == &d.as_str())
            .copied()
            .collect()
    } else {
        VFS_DIRS.to_vec()
    };

    std::fs::create_dir_all(output)?;

    let gamedata_dir = format!("{}/gamedata", output);
    let mono_dir = format!("{}/gamedata/monobehaviour", output);
    let tex_dir = format!("{}/textures", output);
    if do_json || do_gamedata {
        std::fs::create_dir_all(&gamedata_dir)?;
        std::fs::create_dir_all(&mono_dir)?;
    }
    if do_tex {
        std::fs::create_dir_all(&tex_dir)?;
    }

    let mut total_text_assets = 0u64;
    let mut total_json = 0u64;
    let mut total_text = 0u64;
    let mut total_bin = 0u64;
    let mut total_mono = 0u64;
    let mut total_tex_extracted = 0u64;
    let mut total_tex_failed = 0u64;
    let mut mono_type_counts: HashMap<String, u64> = HashMap::new();
    let mut seen_names: HashSet<String> = HashSet::new();
    let mut seen_mono: HashSet<String> = HashSet::new();

    for (dir, desc) in &dirs {
        println!("--- {} ({}) ---", dir, desc);
        let mut candidates = pipeline::list_chk_files(&resolved.cd, dir);
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

            let data = match pipeline::download_chk(client, &resolved.cd, name).await {
                Ok(d) => d,
                Err(e) => {
                    println!("    Download error: {}", e);
                    continue;
                }
            };

            let Some(chk) = extract_chk_contents(&data) else {
                println!("    Not a valid VFS container, skipping");
                continue;
            };

            println!(
                "    {} SerializedFile(s), {} .resS stream(s)",
                chk.sf_data.len(),
                chk.resources.len()
            );

            // JSON/TextAsset extraction
            if do_json {
                for sf_bytes in &chk.sf_data {
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
                            total_text_assets += 1;
                            if let Some(file_type) =
                                save_text_asset(&gamedata_dir, &ta_name, &content)
                            {
                                match file_type {
                                    "JSON" => total_json += 1,
                                    "TEXT" => total_text += 1,
                                    "BIN" => total_bin += 1,
                                    _ => {}
                                }
                            }
                        }
                    }
                }
            }

            // MonoBehaviour extraction
            if do_gamedata {
                for sf_bytes in &chk.sf_data {
                    let sf = match SerializedFileParser::from_bytes(sf_bytes.clone()) {
                        Ok(sf) => sf,
                        Err(_) => continue,
                    };
                    if !sf.enable_type_tree {
                        continue;
                    }
                    for obj in sf.objects_of_type(114) {
                        let type_name = get_script_type_name(&sf, obj);
                        let handle = ObjectHandle::new(&sf, obj);

                        let obj_name = handle
                            .peek_name()
                            .ok()
                            .flatten()
                            .filter(|n| !n.is_empty())
                            .map_or_else(
                                || format!("obj_{}", obj.path_id),
                                |n| sanitize_filename(&n),
                            );

                        let unity_obj = match handle.read() {
                            Ok(o) => o,
                            Err(_) => continue,
                        };

                        let props = unity_obj.class.properties();
                        let json_str = match serde_json::to_string_pretty(props) {
                            Ok(j) => j,
                            Err(_) => continue,
                        };

                        let safe_type = sanitize_filename(&type_name);
                        let key = format!("{}/{}", safe_type, obj_name);
                        if !seen_mono.insert(key) {
                            continue;
                        }

                        let type_dir = format!("{}/{}", mono_dir, safe_type);
                        let _ = std::fs::create_dir_all(&type_dir);
                        let path = format!("{}/{}.json", type_dir, obj_name);
                        if std::fs::write(&path, &json_str).is_ok() {
                            total_mono += 1;
                            *mono_type_counts.entry(safe_type).or_insert(0) += 1;
                        }
                    }
                }
            }

            // Texture extraction
            if do_tex {
                let tex_path = std::path::Path::new(&tex_dir);
                for sf_bytes in &chk.sf_data {
                    let sf = match SerializedFileParser::from_bytes(sf_bytes.clone()) {
                        Ok(sf) => sf,
                        Err(_) => continue,
                    };
                    let (ext, fail, _, _) =
                        texture::extract_textures(&sf, &chk.resources, tex_path);
                    total_tex_extracted += ext;
                    total_tex_failed += fail;
                }
            }
        }
        println!();
    }

    println!("=== Extraction Summary ===");
    if do_json {
        println!(
            "  TextAssets: {} (JSON: {}, Text: {}, Bin: {})",
            total_text_assets, total_json, total_text, total_bin
        );
    }
    if do_gamedata {
        println!("  MonoBehaviours: {}", total_mono);
        if !mono_type_counts.is_empty() {
            let mut types: Vec<_> = mono_type_counts.iter().collect();
            types.sort_by_key(|(_, count)| std::cmp::Reverse(*count));
            for (type_name, count) in types.iter().take(20) {
                println!("    {}: {}", type_name, count);
            }
        }
    }
    if do_tex {
        println!(
            "  Textures extracted: {}, failed: {}",
            total_tex_extracted, total_tex_failed
        );
    }
    println!("  Output: {}", output);
    println!("\n=== Done ===");
    Ok(())
}

/// Save a TextAsset to disk. Returns the file type label.
fn save_text_asset(output_dir: &str, name: &str, content: &[u8]) -> Option<&'static str> {
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
    if let Ok(text) = std::str::from_utf8(content) {
        if let Ok(json) = serde_json::from_str::<serde_json::Value>(text) {
            let path = format!("{}/{}.json", output_dir, safe_name);
            if let Ok(pretty) = serde_json::to_string_pretty(&json)
                && std::fs::write(&path, &pretty).is_ok()
            {
                return Some("JSON");
            }
        }
        if !text.trim().is_empty() {
            let path = format!("{}/{}.txt", output_dir, safe_name);
            if std::fs::write(&path, content).is_ok() {
                return Some("TEXT");
            }
        }
    }
    let path = format!("{}/{}.bytes", output_dir, safe_name);
    if std::fs::write(&path, content).is_ok() {
        return Some("BIN");
    }
    None
}

// ═══════════════════════════════════════════════════════════
// extract-file — single .chk extraction
// ═══════════════════════════════════════════════════════════

async fn cmd_extract_file(
    client: &Client,
    chk_name: &str,
    dir_filter: Option<String>,
    output: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    println!("=== Extract Single File ===\n");

    let resolved = pipeline::resolve_pipeline(client, false).await?;
    println!("Version: {}", resolved.config.version);
    println!("ZIP entries: {}\n", resolved.cd.len());

    // Find the .chk file
    let matching: Vec<String> = resolved
        .cd
        .filenames()
        .filter(|f| f.ends_with(".chk") && f.contains(chk_name))
        .filter(|f| dir_filter.as_ref().is_none_or(|d| f.contains(d.as_str())))
        .map(|f| f.to_string())
        .collect();

    if matching.is_empty() {
        println!("No .chk files matching '{}'", chk_name);
        return Ok(());
    }

    println!("Found {} matching file(s):", matching.len());
    for name in &matching {
        let size = resolved.cd.get(name).map_or(0, |e| e.uncompressed_size);
        println!("  {} ({})", name, format_bytes(size));
    }

    let target = &matching[0];
    println!("\nExtracting: {}", target);

    let data = pipeline::download_chk(client, &resolved.cd, target).await?;

    if !vfs_decrypt::is_valid_header(&data) {
        println!("Not a valid VFS container");
        println!("First 64 bytes:");
        hex_dump(&data, 64);
        return Ok(());
    }

    let assets = backend::extraction::unityfs_header::process_chk_file(&data)?;
    println!("{} assets from VFS pipeline\n", assets.len());

    std::fs::create_dir_all(output)?;

    for (i, asset) in assets.iter().enumerate() {
        if asset.is_unityfs {
            println!(
                "[{}] UnityFS \"{}\" ({}, {} entries)",
                i,
                asset.vfs_name,
                format_bytes(asset.data.len() as u64),
                asset.bundle_entries.len()
            );
            for entry in &asset.bundle_entries {
                println!(
                    "  {} ({} at offset {})",
                    entry.path,
                    format_bytes(entry.size as u64),
                    entry.offset
                );
            }
        } else {
            println!(
                "[{}] \"{}\" ({})",
                i,
                asset.vfs_name,
                format_bytes(asset.data.len() as u64)
            );
        }

        let save_path = format!("{}/{}", output, sanitize_filename(&asset.vfs_name));
        std::fs::write(&save_path, &asset.data)?;
        println!("  → saved to {}", save_path);
    }

    println!("\n=== Done ===");
    Ok(())
}

// ═══════════════════════════════════════════════════════════
// decrypt — ChaCha20-encrypted VFS directories
// ═══════════════════════════════════════════════════════════

async fn cmd_decrypt(
    client: &Client,
    dir_filter: Option<String>,
    output: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    println!("=== ChaCha20 VFS Decryption ===\n");

    let resolved = pipeline::resolve_pipeline(client, true).await?;
    println!("Version: {}", resolved.config.version);
    println!("ZIP entries: {}\n", resolved.cd.len());

    let output_path = std::path::Path::new(output);
    std::fs::create_dir_all(output_path)?;

    let dirs: Vec<&str> = if let Some(ref d) = dir_filter {
        blc::ENCRYPTED_DIRS
            .iter()
            .filter(|h| h == &&d.as_str())
            .copied()
            .collect()
    } else {
        blc::ENCRYPTED_DIRS.to_vec()
    };

    if dirs.is_empty() {
        println!("No matching directories found.");
        if let Some(d) = &dir_filter {
            println!("Available: {:?}", blc::ENCRYPTED_DIRS);
            println!("You specified: {}", d);
        }
        return Ok(());
    }

    let mut total_extracted = 0usize;
    let mut total_sparkbuffer = 0usize;

    for dir_hash in &dirs {
        println!("{}", "=".repeat(60));
        println!("=== {} ===", dir_hash);

        match blc::extract_directory(client, &resolved, dir_hash, output_path).await {
            Ok(result) => {
                println!(
                    "  {} ({}): {}/{} files extracted, {} SparkBuffer→JSON",
                    result.group_name,
                    result.dir_hash,
                    result.extracted,
                    result.total_files,
                    result.sparkbuffer_parsed,
                );
                if !result.errors.is_empty() {
                    println!("  {} errors:", result.errors.len());
                    for e in result.errors.iter().take(5) {
                        println!("    {}", e);
                    }
                    if result.errors.len() > 5 {
                        println!("    ... and {} more", result.errors.len() - 5);
                    }
                }
                total_extracted += result.extracted;
                total_sparkbuffer += result.sparkbuffer_parsed;
            }
            Err(e) => {
                println!("  Error: {}", e);
            }
        }
    }

    println!("\n=== Decryption Summary ===");
    println!("  Directories: {}", dirs.len());
    println!("  Files extracted: {}", total_extracted);
    println!("  SparkBuffer→JSON: {}", total_sparkbuffer);
    println!("  Output: {}", output);
    println!("\n=== Done ===");
    Ok(())
}

// ═══════════════════════════════════════════════════════════
// list — VFS directory listing
// ═══════════════════════════════════════════════════════════

async fn cmd_list(client: &Client) -> Result<(), Box<dyn std::error::Error>> {
    println!("=== VFS Directory Listing ===\n");

    let resolved = pipeline::resolve_pipeline(client, true).await?;
    println!("Version: {}", resolved.config.version);
    println!("ZIP entries: {}\n", resolved.cd.len());

    let dirs = pipeline::list_vfs_directories(&resolved.cd);
    println!("VFS directories ({}):\n", dirs.len());

    for dir in &dirs {
        let files = pipeline::list_vfs_dir_files(&resolved.cd, dir);
        let total_size: u64 = files.iter().map(|(_, s)| *s).sum();
        println!(
            "  {}/  ({} files, {})",
            dir,
            files.len(),
            format_bytes(total_size)
        );
        for (name, size) in &files {
            let short = name.rsplit('/').next().unwrap_or(name);
            println!("    {} ({})", short, format_bytes(*size));
        }
    }

    if let Some(patch) = &resolved.patch_index {
        println!("\nPatch files ({}):", patch.files.len());
        for pf in &patch.files {
            println!("  {} ({})", pf.name, format_bytes(pf.size));
        }
    }

    println!("\n=== Done ===");
    Ok(())
}

// ═══════════════════════════════════════════════════════════
// scan — asset name/path discovery
// ═══════════════════════════════════════════════════════════

const SEARCH_KEYWORDS: &[&str] = &[
    "operator",
    "character",
    "model",
    "mesh",
    "skeleton",
    "texture",
    "material",
    "avatar",
    "npc",
    "char_",
    "prefab",
    "anim",
    "skin",
    "weapon",
    "body",
];

async fn cmd_scan(client: &Client, scan_all: bool) -> Result<(), Box<dyn std::error::Error>> {
    println!("=== Asset Name Scanner ===\n");

    let resolved = pipeline::resolve_pipeline(client, true).await?;
    println!("Version: {}", resolved.config.version);
    println!("ZIP entries: {}\n", resolved.cd.len());

    if let Some(patch) = &resolved.patch_index {
        println!("Patch version: {}", patch.version);
        println!("Chunk files: {}\n", patch.files.len());
    }

    let output_dir = "./output";
    std::fs::create_dir_all(output_dir)?;

    #[allow(dead_code)]
    struct ChkManifest {
        dir: String,
        chk_name: String,
        vfs_names: Vec<String>,
        bundle_paths: Vec<String>,
        embedded_strings: Vec<String>,
    }

    let mut manifests: Vec<ChkManifest> = Vec::new();

    for (dir, desc) in VFS_DIRS {
        println!("--- {} ({}) ---", dir, desc);
        let mut candidates = pipeline::list_chk_files(&resolved.cd, dir);
        if !scan_all {
            candidates.truncate(1);
        }

        for (i, (name, size)) in candidates.iter().enumerate() {
            println!(
                "  [{}/{}] {} ({})",
                i + 1,
                candidates.len(),
                name,
                format_bytes(*size)
            );
            let data = match pipeline::download_chk(client, &resolved.cd, name).await {
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

            let assets = match backend::extraction::unityfs_header::process_chk_file(&data) {
                Ok(a) => a,
                Err(e) => {
                    println!("    Pipeline error: {}", e);
                    continue;
                }
            };

            let mut vfs_names = Vec::new();
            let mut bundle_paths = Vec::new();
            let mut embedded_strings = Vec::new();
            let mut seen_strings = HashSet::new();

            for asset in &assets {
                if !vfs_names.contains(&asset.vfs_name) {
                    vfs_names.push(asset.vfs_name.clone());
                }
                for entry in &asset.bundle_entries {
                    if !bundle_paths.contains(&entry.path) {
                        bundle_paths.push(entry.path.clone());
                    }
                }
                // Extract readable strings from raw data
                let mut current = Vec::new();
                for &byte in &asset.data {
                    if (0x20..0x7F).contains(&byte) {
                        current.push(byte);
                    } else {
                        if current.len() >= 8
                            && let Ok(s) = std::str::from_utf8(&current)
                            && seen_strings.insert(s.to_string())
                        {
                            embedded_strings.push(s.to_string());
                        }
                        current.clear();
                    }
                }
                if current.len() >= 8
                    && let Ok(s) = std::str::from_utf8(&current)
                    && seen_strings.insert(s.to_string())
                {
                    embedded_strings.push(s.to_string());
                }
            }

            println!(
                "    {} assets, {} VFS names, {} bundle paths, {} embedded strings",
                assets.len(),
                vfs_names.len(),
                bundle_paths.len(),
                embedded_strings.len()
            );

            manifests.push(ChkManifest {
                dir: dir.to_string(),
                chk_name: name.clone(),
                vfs_names,
                bundle_paths,
                embedded_strings,
            });
        }
        println!();
    }

    // Keyword search
    println!("=== Keyword Search Results ===\n");
    for keyword in SEARCH_KEYWORDS {
        let kw_lower = keyword.to_lowercase();
        let mut matches: Vec<(&str, &str, &str)> = Vec::new();
        for manifest in &manifests {
            for name in &manifest.vfs_names {
                if name.to_lowercase().contains(&kw_lower) {
                    matches.push((&manifest.dir, "vfs", name));
                }
            }
            for path in &manifest.bundle_paths {
                if path.to_lowercase().contains(&kw_lower) {
                    matches.push((&manifest.dir, "bundle", path));
                }
            }
            for s in &manifest.embedded_strings {
                if s.to_lowercase().contains(&kw_lower) {
                    matches.push((&manifest.dir, "embedded", s));
                }
            }
        }
        if matches.is_empty() {
            println!("  \"{}\": no matches", keyword);
        } else {
            println!("  \"{}\" ({} matches):", keyword, matches.len());
            for (dir, source, value) in matches.iter().take(30) {
                println!("    [{}:{}] {}", dir, source, value);
            }
            if matches.len() > 30 {
                println!("    ... and {} more", matches.len() - 30);
            }
        }
    }

    println!("\n=== Done ===");
    Ok(())
}

// ═══════════════════════════════════════════════════════════
// survey — object census
// ═══════════════════════════════════════════════════════════

async fn cmd_survey(client: &Client, scan_all: bool) -> Result<(), Box<dyn std::error::Error>> {
    println!("=== Asset Survey ===\n");

    let resolved = pipeline::resolve_pipeline(client, false).await?;
    println!("Version: {}", resolved.config.version);
    println!("ZIP entries: {}\n", resolved.cd.len());

    let mut all_chks: Vec<ChkContents> = Vec::new();

    for (dir, desc) in VFS_DIRS {
        println!("--- {} ({}) ---", dir, desc);
        let mut candidates = pipeline::list_chk_files(&resolved.cd, dir);
        if !scan_all {
            candidates.truncate(1);
        }

        for (name, size) in &candidates {
            println!("  {} ({})", name, format_bytes(*size));
            let data = match pipeline::download_chk(client, &resolved.cd, name).await {
                Ok(d) => d,
                Err(e) => {
                    println!("    Download error: {}", e);
                    continue;
                }
            };
            if let Some(chk) = extract_chk_contents(&data) {
                println!(
                    "    {} SerializedFile(s), {} .resS stream(s)",
                    chk.sf_data.len(),
                    chk.resources.len()
                );
                all_chks.push(chk);
            }
        }
        println!();
    }

    // Pass 1: Class ID Census
    println!("╔══════════════════════════════════════════╗");
    println!("║  Class ID Census                         ║");
    println!("╚══════════════════════════════════════════╝\n");

    let mut class_counts: BTreeMap<i32, u64> = BTreeMap::new();
    let mut class_total_bytes: BTreeMap<i32, u64> = BTreeMap::new();
    let mut total_objects = 0u64;

    for chk in &all_chks {
        for sf_bytes in &chk.sf_data {
            let sf = match SerializedFileParser::from_bytes(sf_bytes.clone()) {
                Ok(sf) => sf,
                Err(_) => continue,
            };
            for obj in &sf.objects {
                *class_counts.entry(obj.type_id).or_insert(0) += 1;
                *class_total_bytes.entry(obj.type_id).or_insert(0) += obj.byte_size as u64;
                total_objects += 1;
            }
        }
    }

    println!("  Total objects: {}\n", total_objects);
    println!(
        "  {:>8} | {:>6} | {:>10} | Name",
        "ClassID", "Count", "Total Size"
    );
    println!("  {}", "-".repeat(55));

    let mut sorted_classes: Vec<_> = class_counts.iter().collect();
    sorted_classes.sort_by_key(|(_, count)| std::cmp::Reverse(*count));

    for (class_id, count) in &sorted_classes {
        let total = class_total_bytes.get(class_id).copied().unwrap_or(0);
        println!(
            "  {:>8} | {:>6} | {:>10} | {}",
            class_id,
            count,
            format_bytes(total),
            class_name(**class_id)
        );
    }

    let total_sfs: usize = all_chks.iter().map(|c| c.sf_data.len()).sum();
    let total_res: usize = all_chks.iter().map(|c| c.resources.len()).sum();
    let total_res_bytes: u64 = all_chks
        .iter()
        .flat_map(|c| c.resources.values())
        .map(|v| v.len() as u64)
        .sum();

    println!("\n  SerializedFiles: {}", total_sfs);
    println!("  Total objects:   {}", total_objects);
    println!("  Unique classes:  {}", class_counts.len());
    println!("  Resource files:  {}", total_res);
    println!("  Resource data:   {}", format_bytes(total_res_bytes));

    println!("\n=== Done ===");
    Ok(())
}
