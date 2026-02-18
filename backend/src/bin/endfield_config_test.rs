use backend::core::endfield::{
    config::{ResolveParams, derive_game_version, fetch_patch_index, resolve_game_config},
    constants::{app_code, channel, sub_channel},
};
use backend::core::hypergryph::launcher::Platform;
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

    // Test helper functions
    println!("=== Helper function tests ===");
    let gv = derive_game_version("1.0.14")?;
    assert_eq!(gv, "1.0");
    println!("derive_game_version(\"1.0.14\") = {gv}");

    // Resolve full config
    println!("\n=== Resolving Endfield game config (OS Windows) ===");

    let params = ResolveParams {
        app_code: app_code::game::OS_WIN_REL,
        launcher_app_code: app_code::launcher::OS_WIN_REL,
        channel: channel::OS_WIN_REL,
        sub_channel: sub_channel::OS_WIN_REL,
        launcher_sub_channel: sub_channel::OS_WIN_REL,
        platform: Platform::Windows,
    };

    let config = resolve_game_config(&client, &params).await?;

    println!("Version:          {}", config.version);
    println!("Game version:     {}", config.game_version);
    println!("Rand str:         {}", config.rand_str);
    println!("Packs:            {} files", config.packs.len());
    println!("Resources:        {} entries", config.resources.len());
    println!("Domain:           {}", config.domain);
    println!("Res version:      {}", config.res_version);

    for res in &config.resources {
        println!("  Resource: {} v{}", res.name, res.version);
    }

    // Fetch patch.json
    println!("\n=== Fetching patch.json ===");

    if let Some(main_resource) = config.resources.iter().find(|r| r.name == "main") {
        let patch_index = fetch_patch_index(&client, &main_resource.path).await?;
        println!("Patch version: {}", patch_index.version);
        println!("Chunk files:   {}", patch_index.files.len());

        let total_bytes: u64 = patch_index.files.iter().map(|f| f.size).sum();

        for (i, file) in patch_index.files.iter().enumerate() {
            println!(
                "  [{}] {} ({}, md5: {})",
                i,
                file.name,
                format_bytes(file.size),
                file.md5
            );
        }

        println!("Total size: {}", format_bytes(total_bytes));
    } else {
        println!("No 'main' resource found");
    }

    println!("\nDone.");
    Ok(())
}
