/// Brute-force asset name scanner.
/// Downloads .chk files from VFS directories, runs the full VFS → UnityFS pipeline,
/// collects all asset names/paths, and searches for keywords like "operator", "model", etc.
use backend::core::endfield::{
    config::{ResolveParams, fetch_patch_index, resolve_game_config},
    constants::{app_code, channel, sub_channel},
};
use backend::core::hypergryph::launcher::Platform;
use backend::extraction::unityfs_header;
use backend::extraction::vfs_decrypt;
use backend::extraction::zip_range::{SplitArchivePart, ZipCentralDirectory};
use reqwest::Client;

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

/// VFS container directories (the only ones that go through the VFS → UnityFS pipeline).
const VFS_DIRS: &[(&str, &str)] = &[("7064D8E2", "bundles/configs"), ("0CE8FA57", "textures")];

struct ChkManifest {
    dir: String,
    chk_name: String,
    chk_size: u64,
    vfs_names: Vec<String>,
    bundle_paths: Vec<String>,
    /// Readable strings extracted from raw SerializedFile data.
    embedded_strings: Vec<String>,
}

/// Extract readable ASCII strings (length >= min_len) from binary data.
/// Filters for strings that look like paths or meaningful identifiers.
fn extract_strings(data: &[u8], min_len: usize) -> Vec<String> {
    let mut strings = Vec::new();
    let mut current = Vec::new();

    for &byte in data {
        if (0x20..0x7F).contains(&byte) {
            current.push(byte);
        } else {
            if current.len() >= min_len
                && let Ok(s) = std::str::from_utf8(&current)
            {
                strings.push(s.to_string());
            }
            current.clear();
        }
    }
    if current.len() >= min_len
        && let Ok(s) = std::str::from_utf8(&current)
    {
        strings.push(s.to_string());
    }

    strings
}

/// Download a .chk file from the ZIP, with caching.
async fn download_chk(
    client: &Client,
    cd: &ZipCentralDirectory,
    output_dir: &str,
    dir: &str,
    target_name: Option<&str>,
) -> Option<(String, Vec<u8>)> {
    // Check cache first
    let cache_path = format!("{}/{}_smallest.chk", output_dir, dir);
    if let Ok(cached) = std::fs::read(&cache_path)
        && cached.len() >= 8
        && vfs_decrypt::is_valid_header(&cached)
    {
        return Some((cache_path, cached));
    }

    // Find candidates in ZIP
    let mut candidates: Vec<(String, u64)> = cd
        .filenames()
        .filter(|f| f.ends_with(".chk") && f.contains(dir))
        .map(|f| {
            let size = cd.get(f).map_or(0, |e| e.uncompressed_size);
            (f.to_string(), size)
        })
        .collect();
    candidates.sort_by_key(|(_n, s)| *s);

    // If specific target, filter for it
    if let Some(target) = target_name {
        candidates.retain(|(n, _)| n.contains(target));
    }

    for (name, size) in &candidates {
        println!("    Downloading {} ({})...", name, format_bytes(*size));
        match cd.extract(client, name).await {
            Ok(data) => {
                let save_path = format!("{}/{}_smallest.chk", output_dir, dir);
                let _ = std::fs::write(&save_path, &data);
                return Some((name.clone(), data));
            }
            Err(e) => println!("    Download error: {}", e),
        }
    }
    None
}

/// Process a single .chk file through the full pipeline and collect names.
fn process_chk(dir: &str, chk_name: &str, chk_size: u64, data: &[u8]) -> Option<ChkManifest> {
    let assets = match unityfs_header::process_chk_file(data) {
        Ok(a) => a,
        Err(e) => {
            println!("    Pipeline error: {}", e);
            return None;
        }
    };

    let mut vfs_names = Vec::new();
    let mut bundle_paths = Vec::new();
    let mut embedded_strings = Vec::new();
    let mut seen_strings = std::collections::HashSet::new();

    for asset in &assets {
        if !vfs_names.contains(&asset.vfs_name) {
            vfs_names.push(asset.vfs_name.clone());
        }
        for entry in &asset.bundle_entries {
            if !bundle_paths.contains(&entry.path) {
                bundle_paths.push(entry.path.clone());
            }
        }

        // Extract readable strings from the raw SerializedFile data
        let strings = extract_strings(&asset.data, 8);
        for s in strings {
            if seen_strings.insert(s.clone()) {
                embedded_strings.push(s);
            }
        }
    }

    println!(
        "    Extracted: {} assets, {} VFS names, {} bundle paths, {} embedded strings",
        assets.len(),
        vfs_names.len(),
        bundle_paths.len(),
        embedded_strings.len()
    );

    Some(ChkManifest {
        dir: dir.to_string(),
        chk_name: chk_name.to_string(),
        chk_size,
        vfs_names,
        bundle_paths,
        embedded_strings,
    })
}

/// Search all collected names and embedded strings for keywords and print matches.
fn keyword_search(manifests: &[ChkManifest]) {
    println!("\n=== Keyword Search Results ===\n");

    for keyword in SEARCH_KEYWORDS {
        let kw_lower = keyword.to_lowercase();
        let mut matches: Vec<(&str, &str, &str)> = Vec::new(); // (dir, source, value)

        for manifest in manifests {
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
}

/// Save the full manifest to a file.
fn save_manifest(manifests: &[ChkManifest], output_dir: &str) {
    let mut output = String::new();

    for manifest in manifests {
        output.push_str(&format!("=== VFS Directory: {} ===\n", manifest.dir));
        output.push_str(&format!(
            "CHK: {} ({})\n\n",
            manifest.chk_name,
            format_bytes(manifest.chk_size)
        ));

        output.push_str(&format!("VFS Names ({}):\n", manifest.vfs_names.len()));
        for name in &manifest.vfs_names {
            output.push_str(&format!("  {}\n", name));
        }

        output.push_str(&format!(
            "\nBundle Paths ({}):\n",
            manifest.bundle_paths.len()
        ));
        for path in &manifest.bundle_paths {
            output.push_str(&format!("  {}\n", path));
        }

        // Embedded strings - show path-like and interesting ones
        let path_strings: Vec<&String> = manifest
            .embedded_strings
            .iter()
            .filter(|s| {
                s.contains('/')
                    || s.contains("Assets")
                    || s.contains("Bundle")
                    || s.contains("CAB-")
                    || s.contains(".asset")
                    || s.contains(".prefab")
                    || s.contains(".mat")
            })
            .collect();
        output.push_str(&format!(
            "\nEmbedded Path-like Strings ({} of {} total):\n",
            path_strings.len(),
            manifest.embedded_strings.len()
        ));
        for s in &path_strings {
            output.push_str(&format!("  {}\n", s));
        }
        output.push('\n');
    }

    // Keyword summary
    output.push_str("=== Keyword Matches ===\n\n");
    for keyword in SEARCH_KEYWORDS {
        let kw_lower = keyword.to_lowercase();
        let matches: Vec<String> = manifests
            .iter()
            .flat_map(|m| {
                m.vfs_names
                    .iter()
                    .chain(m.bundle_paths.iter())
                    .chain(m.embedded_strings.iter())
                    .filter(|n| n.to_lowercase().contains(&kw_lower))
                    .map(|n| format!("[{}] {}", m.dir, n))
            })
            .collect();

        if !matches.is_empty() {
            output.push_str(&format!("\"{}\" ({} matches):\n", keyword, matches.len()));
            for m in matches.iter().take(100) {
                output.push_str(&format!("  {}\n", m));
            }
            if matches.len() > 100 {
                output.push_str(&format!("  ... and {} more\n", matches.len() - 100));
            }
            output.push('\n');
        }
    }

    let path = format!("{}/asset_manifest.txt", output_dir);
    match std::fs::write(&path, &output) {
        Ok(()) => println!("\nManifest saved to {}", path),
        Err(e) => println!("\nFailed to save manifest: {}", e),
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("=== Asset Name Scanner ===\n");

    let scan_all = std::env::var("SCAN_ALL").is_ok();
    if scan_all {
        println!("SCAN_ALL mode: will process ALL .chk files in target directories\n");
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

    // Fetch patch.json for file inventory
    println!("\n[2] Fetching patch.json...");
    let mut patch_summary: Vec<(String, usize, u64)> = Vec::new();
    if let Some(main_resource) = config.resources.iter().find(|r| r.name == "main") {
        let patch_index = fetch_patch_index(&client, &main_resource.path).await?;
        println!("    Patch version: {}", patch_index.version);
        println!("    Total chunk files: {}", patch_index.files.len());

        // Group by directory
        let mut dir_files: std::collections::HashMap<
            String,
            Vec<&backend::core::endfield::config::PatchFile>,
        > = std::collections::HashMap::new();
        for file in &patch_index.files {
            if let Some(dir) = file.name.split('/').next() {
                dir_files.entry(dir.to_string()).or_default().push(file);
            }
        }

        println!("\n    Directory breakdown:");
        let mut dirs: Vec<_> = dir_files.iter().collect();
        dirs.sort_by_key(|(name, _)| (*name).clone());
        for (dir, files) in &dirs {
            let total: u64 = files.iter().map(|f| f.size).sum();
            println!(
                "      {} — {} files, {} total",
                dir,
                files.len(),
                format_bytes(total)
            );
            patch_summary.push((dir.to_string(), files.len(), total));
        }
    }

    // Parse ZIP central directory
    println!("\n[3] Parsing ZIP central directory...");
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

    let output_dir = "./output";
    std::fs::create_dir_all(output_dir)?;

    // Process target VFS directories
    println!("\n[4] Scanning VFS directories for asset names...\n");
    let mut manifests: Vec<ChkManifest> = Vec::new();

    for (dir, desc) in VFS_DIRS {
        println!("--- {} ({}) ---", dir, desc);

        if scan_all {
            // Process ALL .chk files in this directory
            let mut candidates: Vec<(String, u64)> = cd
                .filenames()
                .filter(|f| f.ends_with(".chk") && f.contains(dir))
                .map(|f| {
                    let size = cd.get(f).map_or(0, |e| e.uncompressed_size);
                    (f.to_string(), size)
                })
                .collect();
            candidates.sort_by_key(|(_n, s)| *s);

            println!("    Found {} .chk files", candidates.len());
            for (i, (name, size)) in candidates.iter().enumerate() {
                println!(
                    "\n    [{}/{}] Processing {}...",
                    i + 1,
                    candidates.len(),
                    name
                );
                match cd.extract(&client, name).await {
                    Ok(data) => {
                        if vfs_decrypt::is_valid_header(&data) {
                            if let Some(m) = process_chk(dir, name, *size, &data) {
                                manifests.push(m);
                            }
                        } else {
                            println!("      Not a valid VFS container, skipping");
                        }
                    }
                    Err(e) => println!("      Download error: {}", e),
                }
            }
        } else {
            // Just download smallest
            match download_chk(&client, &cd, output_dir, dir, None).await {
                Some((name, data)) => {
                    if !vfs_decrypt::is_valid_header(&data) {
                        println!("    Not a valid VFS container");
                        continue;
                    }
                    let size = data.len() as u64;
                    if let Some(m) = process_chk(dir, &name, size, &data) {
                        manifests.push(m);
                    }
                }
                None => {
                    println!("    No .chk files found");
                }
            }
        }
        println!();
    }

    // Summary
    println!("\n=== Scan Summary ===");
    let total_vfs: usize = manifests.iter().map(|m| m.vfs_names.len()).sum();
    let total_paths: usize = manifests.iter().map(|m| m.bundle_paths.len()).sum();
    let total_strings: usize = manifests.iter().map(|m| m.embedded_strings.len()).sum();
    println!(
        "  Processed {} .chk files across {} directories",
        manifests.len(),
        VFS_DIRS.len()
    );
    println!("  Total VFS names: {}", total_vfs);
    println!("  Total bundle paths: {}", total_paths);
    println!("  Total embedded strings: {}", total_strings);

    // Keyword search
    keyword_search(&manifests);

    // Save manifest
    save_manifest(&manifests, output_dir);

    println!("\n=== Asset Scan Complete ===");
    Ok(())
}
