/// List all VFS files in the CDN ZIP archive, including .blc catalogs.
use backend::extraction::pipeline;
use reqwest::Client;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let client = Client::new();
    let resolved = pipeline::resolve_pipeline(&client, true).await?;

    println!("Version: {}", resolved.config.version);
    println!("ZIP entries: {}\n", resolved.cd.len());

    // List all VFS directories
    let dirs = pipeline::list_vfs_directories(&resolved.cd);
    println!("VFS directories ({}):", dirs.len());
    for dir in &dirs {
        let files = pipeline::list_vfs_dir_files(&resolved.cd, dir);
        println!("\n  {dir}/");
        for (name, size) in &files {
            let short = name.rsplit('/').next().unwrap_or(name);
            println!("    {short} ({size} bytes)");
        }
    }

    // Also list patch files
    if let Some(patch) = &resolved.patch_index {
        println!("\n\nPatch files ({}):", patch.files.len());
        for pf in &patch.files {
            println!("  {} ({} bytes)", pf.name, pf.size);
        }
    }

    Ok(())
}
