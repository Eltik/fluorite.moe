/// VFS pipeline integration test.
/// Downloads .chk files from the game ZIP, processes them through the full
/// VFS → UnityFS → FairGuard → LZ4 pipeline, and reports results.
use backend::core::endfield::{
    config::{ResolveParams, resolve_game_config},
    constants::{app_code, channel, sub_channel},
};
use backend::core::hypergryph::launcher::Platform;
use backend::extraction::vfs_decrypt;
use backend::extraction::zip_range::{SplitArchivePart, ZipCentralDirectory};
use reqwest::Client;

fn hex_dump(data: &[u8], max_bytes: usize) {
    let dump_len = data.len().min(max_bytes);
    for row in 0..dump_len.div_ceil(16) {
        let off = row * 16;
        if off >= dump_len {
            break;
        }
        print!("  {:04x}: ", off);
        for i in 0..16 {
            if off + i < dump_len {
                print!("{:02x} ", data[off + i]);
            } else {
                print!("   ");
            }
        }
        print!(" |");
        for i in 0..16 {
            if off + i < dump_len {
                let b = data[off + i];
                if b.is_ascii_graphic() || b == b' ' {
                    print!("{}", b as char);
                } else {
                    print!(".");
                }
            }
        }
        println!("|");
    }
}

/// Detect the format of a .chk file by examining its header.
fn detect_format(data: &[u8]) -> &'static str {
    if data.len() >= 8 && vfs_decrypt::is_valid_header(data) {
        return "VFS";
    }
    // :)xD magic = 0x3A297844 stored as bytes [3A, 29, 78, 44]
    if data.len() >= 4 && data[0] == 0x3A && data[1] == 0x29 && data[2] == 0x78 && data[3] == 0x44 {
        return ":)xD";
    }
    "Unknown"
}

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

/// Try to load a cached .chk file for the given VFS directory.
/// Loads any cached file regardless of format (VFS, :)xD, or unknown).
fn load_cached(output_dir: &str, dir: &str) -> Option<Vec<u8>> {
    let path = format!("{}/{}_smallest.chk", output_dir, dir);
    let data = std::fs::read(&path).ok()?;
    if data.len() >= 8 { Some(data) } else { None }
}

/// Download the smallest .chk file for a VFS directory from the ZIP archive.
/// Saves to cache on success.
async fn download_smallest(
    client: &Client,
    cd: &ZipCentralDirectory,
    output_dir: &str,
    dir: &str,
) -> Option<Vec<u8>> {
    let mut candidates: Vec<(String, u64)> = cd
        .filenames()
        .filter(|f| f.ends_with(".chk") && f.contains(dir))
        .map(|f| {
            let size = cd.get(f).map_or(0, |e| e.uncompressed_size);
            (f.to_string(), size)
        })
        .collect();
    candidates.sort_by_key(|(_n, s)| *s);

    for (name, size) in &candidates {
        println!("    Downloading {} ({})...", name, format_bytes(*size));
        match cd.extract(client, name).await {
            Ok(file_data) => {
                // Save unconditionally for analysis
                let save_path = format!("{}/{}_smallest.chk", output_dir, dir);
                let _ = std::fs::write(&save_path, &file_data);

                if vfs_decrypt::is_valid_header(&file_data) {
                    println!("    VFS valid, saved to cache");
                    return Some(file_data);
                }
                // Non-VFS file — still return it for format detection
                if file_data.len() >= 4 {
                    println!(
                        "    Not VFS (magic: {:02x}{:02x}{:02x}{:02x}), saved for analysis",
                        file_data[0], file_data[1], file_data[2], file_data[3]
                    );
                }
                return Some(file_data);
            }
            Err(e) => println!("    Download error: {}", e),
        }
    }

    None
}

/// Print a summary of process_chk_file results.
fn print_pipeline_results(
    assets: &[backend::extraction::unityfs_header::ExtractedAsset],
    max_details: usize,
) {
    let unityfs_count = assets.iter().filter(|a| a.is_unityfs).count();
    let serial_count = assets.iter().filter(|a| !a.is_unityfs).count();
    let total_size: usize = assets.iter().map(|a| a.data.len()).sum();

    println!(
        "    Assets: {} total ({} UnityFS, {} direct SerializedFile)",
        assets.len(),
        unityfs_count,
        serial_count
    );
    println!("    Total output: {}", format_bytes(total_size as u64));

    for (i, asset) in assets.iter().enumerate().take(max_details) {
        if asset.is_unityfs {
            let entry_names: Vec<&str> = asset
                .bundle_entries
                .iter()
                .map(|e| e.path.as_str())
                .collect();
            println!(
                "    Asset[{}] UnityFS \"{}\" -> {} ({} entries: {:?})",
                i,
                asset.vfs_name,
                format_bytes(asset.data.len() as u64),
                asset.bundle_entries.len(),
                entry_names
            );
        } else {
            println!(
                "    Asset[{}] SerializedFile \"{}\" -> {}",
                i,
                asset.vfs_name,
                format_bytes(asset.data.len() as u64)
            );
        }
    }
    if assets.len() > max_details {
        println!("    ... and {} more assets", assets.len() - max_details);
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("=== VFS Pipeline Integration Test ===\n");

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
    println!("    Total entries: {}", cd.len());

    let output_dir = "./output";
    std::fs::create_dir_all(output_dir)?;

    // VFS directories to test (add more as needed)
    let vfs_dirs = [
        "0CE8FA57", "7064D8E2", "24ED34CF", "07A1BB91", "19E3AE45", "1CDDBF1F", "1EBAF5C6",
        "42A8FCA6", "775A31D1", "A31457D0", "E1E7D7CE", "E9D31017", "F668D4EE",
    ];

    println!("\n[3] Processing VFS directories...\n");

    let mut smiley_count = 0u32;
    let mut vfs_count = 0u32;
    let mut unknown_count = 0u32;

    for dir in &vfs_dirs {
        println!("--- {} ---", dir);

        let data = if let Some(cached) = load_cached(output_dir, dir) {
            let format = detect_format(&cached);
            println!(
                "    Loaded from cache ({}, {})",
                format_bytes(cached.len() as u64),
                format
            );
            cached
        } else {
            match download_smallest(&client, &cd, output_dir, dir).await {
                Some(d) => d,
                None => {
                    println!("    SKIP: no .chk file found");
                    println!();
                    unknown_count += 1;
                    continue;
                }
            }
        };

        let format = detect_format(&data);
        match format {
            "VFS" => {
                vfs_count += 1;
                match backend::extraction::unityfs_header::process_chk_file(&data) {
                    Ok(assets) => print_pipeline_results(&assets, 10),
                    Err(e) => println!("    ERROR: {}", e),
                }
            }
            ":)xD" => {
                smiley_count += 1;
                println!("    :)xD format detected — {} total bytes", data.len());
                println!("    First 128 bytes:");
                hex_dump(&data, 128);
            }
            _ => {
                unknown_count += 1;
                println!("    Unknown format — {} total bytes", data.len());
                println!("    First 128 bytes:");
                hex_dump(&data, 128);
            }
        }

        println!();
    }

    println!("=== Format Summary ===");
    println!("  VFS:     {}", vfs_count);
    println!("  :)xD:    {}", smiley_count);
    println!("  Unknown: {}", unknown_count);

    println!("=== VFS Test Complete ===");
    Ok(())
}
