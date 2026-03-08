use backend::core::endfield::{
    config::{ResolveParams, resolve_game_config},
    constants::{app_code, channel, sub_channel},
};
use backend::core::hypergryph::launcher::Platform;
/// Extract files from the game's split ZIP archive.
/// Usage: gameassembly-extract [filename_pattern]
/// Without args: extracts GameAssembly.dll
/// With args: extracts files matching the pattern
use backend::extraction::utils::format_bytes;
use backend::extraction::zip_range::{SplitArchivePart, ZipCentralDirectory};
use reqwest::Client;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    let patterns: Vec<&str> = if args.len() > 1 {
        args[1..].iter().map(|s| s.as_str()).collect()
    } else {
        vec!["GameAssembly.dll"]
    };

    println!("=== ZIP File Extractor ===\n");

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

    // If pattern is "list-dlls", just list all DLL files
    if patterns.contains(&"list-dlls") {
        println!("\n=== All .dll files in archive ===");
        let mut dlls: Vec<_> = cd
            .filenames()
            .filter(|f| f.to_lowercase().ends_with(".dll"))
            .collect();
        dlls.sort();
        for f in &dlls {
            if let Some(entry) = cd.get(f) {
                println!("  {} ({})", f, format_bytes(entry.uncompressed_size));
            }
        }
        println!("\nTotal: {} DLL files", dlls.len());
        return Ok(());
    }

    println!("\n[3] Searching for matching files...");
    let mut targets: Vec<String> = Vec::new();
    for pattern in &patterns {
        let pattern_lower = pattern.to_lowercase();
        let matches: Vec<_> = cd
            .filenames()
            .filter(|f| f.to_lowercase().contains(&pattern_lower))
            .collect();
        if matches.is_empty() {
            println!("    No files matching '{}'", pattern);
        } else {
            for name in &matches {
                if let Some(entry) = cd.get(name) {
                    println!(
                        "    Found: {} (compressed: {}, uncompressed: {})",
                        name,
                        format_bytes(entry.compressed_size),
                        format_bytes(entry.uncompressed_size)
                    );
                }
                targets.push(name.to_string());
            }
        }
    }

    if targets.is_empty() {
        return Err("No matching files found".into());
    }

    let output_dir = "./data/game-binaries";
    std::fs::create_dir_all(output_dir)?;

    for (i, target) in targets.iter().enumerate() {
        println!("\n[{}] Extracting {}...", 4 + i, target);
        let data = cd.extract(&client, target).await?;
        println!(
            "    Extracted {} bytes ({})",
            data.len(),
            format_bytes(data.len() as u64)
        );

        // Use just the filename for output
        let filename = target.rsplit('/').next().unwrap_or(target);
        let output_path = format!("{}/{}", output_dir, filename);
        std::fs::write(&output_path, &data)?;
        println!("    Saved to {}", output_path);

        // Quick format check
        if data.len() >= 2 && data[0] == b'M' && data[1] == b'Z' {
            println!("    PE header verified (MZ magic)");
        } else if data.len() >= 4 {
            println!("    First 16 bytes: {:02x?}", &data[..16.min(data.len())]);
        }
    }

    println!("\n=== Done ===");
    Ok(())
}
