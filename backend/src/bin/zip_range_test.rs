use backend::core::endfield::{
    config::{ResolveParams, resolve_game_config},
    constants::{app_code, channel, sub_channel},
};
use backend::core::hypergryph::launcher::Platform;
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

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let client = Client::new();

    // 1. Resolve game config to get pack URLs
    println!("=== Resolving game config ===");
    let params = ResolveParams {
        app_code: app_code::game::OS_WIN_REL,
        launcher_app_code: app_code::launcher::OS_WIN_REL,
        channel: channel::OS_WIN_REL,
        sub_channel: sub_channel::OS_WIN_REL,
        launcher_sub_channel: sub_channel::OS_WIN_REL,
        platform: Platform::Windows,
    };
    let config = resolve_game_config(&client, &params).await?;
    println!("Version: {}, Packs: {}", config.version, config.packs.len());

    // 2. Build SplitArchivePart list from packs
    let parts: Vec<SplitArchivePart> = config
        .packs
        .iter()
        .map(|p| SplitArchivePart {
            url: p.url.clone(),
            size: p.package_size.parse::<u64>().expect("invalid package_size"),
        })
        .collect();
    println!("Total parts: {}", parts.len());

    // 3. Parse central directory
    println!("\n=== Parsing ZIP central directory ===");
    let cd = ZipCentralDirectory::from_split_archive(&client, parts).await?;
    println!("Total entries: {}", cd.len());

    // 4. List .chk files
    let mut chk_files: Vec<_> = cd.filenames().filter(|f| f.ends_with(".chk")).collect();
    chk_files.sort();
    println!("\n=== .chk files in archive ({}) ===", chk_files.len());
    for name in &chk_files {
        if let Some(entry) = cd.get(name) {
            println!(
                "  {} (compressed: {}, uncompressed: {}, method: {})",
                name,
                format_bytes(entry.compressed_size),
                format_bytes(entry.uncompressed_size),
                entry.compression_method,
            );
        }
    }

    // 5. Extract the smallest .chk as proof of concept
    if let Some(&smallest) = chk_files
        .iter()
        .min_by_key(|&&name| cd.get(name).map_or(u64::MAX, |e| e.compressed_size))
    {
        println!("\n=== Extracting smallest .chk: {} ===", smallest);
        let entry = cd.get(smallest).unwrap();
        println!(
            "Compressed: {}, Uncompressed: {}",
            format_bytes(entry.compressed_size),
            format_bytes(entry.uncompressed_size),
        );

        let data = cd.extract(&client, smallest).await?;
        println!("Extracted {} bytes", data.len());

        // Peek at first few bytes to verify it looks like data
        if data.len() >= 8 {
            println!("First 8 bytes: {:02x?}", &data[..8]);
        }
    }

    println!("\nDone.");
    Ok(())
}
