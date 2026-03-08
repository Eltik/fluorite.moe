/// Download and analyze encrypted VFS .chk files from CDN.
use backend::extraction::pipeline;
use backend::extraction::vfs_decrypt;
use reqwest::Client;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let client = Client::new();
    let resolved = pipeline::resolve_pipeline(&client, true).await?;
    println!("Version: {}", resolved.config.version);
    println!("ZIP entries: {}\n", resolved.cd.len());

    let output_dir = std::path::Path::new("./output/cdn_encrypted");
    std::fs::create_dir_all(output_dir)?;

    // Download patch files for encrypted dirs
    if let Some(patch) = &resolved.patch_index {
        println!("Patch version: {}", patch.version);
        println!("Patch files: {}\n", patch.files.len());

        // Find the resource path for downloading patch files
        let main_res = resolved
            .config
            .resources
            .iter()
            .find(|r| r.name == "main")
            .expect("no main resource");

        for pf in &patch.files {
            println!("  Patch file: {} ({} bytes, md5: {})", pf.name, pf.size, pf.md5);

            // Download all patch files that aren't already downloaded
            let safe_name = pf.name.replace('/', "_");
            let out_path = output_dir.join(&safe_name);
            if out_path.exists() {
                let meta = std::fs::metadata(&out_path)?;
                if meta.len() == pf.size {
                    println!("    Already downloaded, analyzing...");
                    let data = std::fs::read(&out_path)?;
                    analyze(&data, &pf.name);
                    continue;
                }
            }

            // Download from CDN using resource path
            let file_url = format!("{}/{}", main_res.path, pf.name);
            println!("    Downloading from {}...", file_url);
            match client.get(&file_url).send().await {
                Ok(resp) => {
                    if resp.status().is_success() {
                        let data = resp.bytes().await?.to_vec();
                        println!("    Downloaded {} bytes", data.len());
                        std::fs::write(&out_path, &data)?;
                        analyze(&data, &pf.name);
                    } else {
                        println!("    HTTP {}", resp.status());
                    }
                }
                Err(e) => println!("    Error: {}", e),
            }
        }
    }

    Ok(())
}

fn analyze(data: &[u8], name: &str) {
    println!("\n  --- Analysis of {} ---", name);
    println!("  Size: {} bytes", data.len());

    // First 64 bytes hex
    let first = &data[..data.len().min(64)];
    print!("  First 64: ");
    for b in first {
        print!("{:02x}", b);
    }
    println!();

    // ASCII of first 64
    print!("  ASCII:    ");
    for b in first {
        if *b >= 32 && *b < 127 {
            print!("{}", *b as char);
        } else {
            print!(".");
        }
    }
    println!();

    // Check VFS header
    let is_vfs = vfs_decrypt::is_valid_header(data);
    println!("  VFS header valid: {}", is_vfs);

    if is_vfs {
        println!("  *** THIS FILE HAS A VALID VFS HEADER! ***");
        match vfs_decrypt::process_vfs(data) {
            Ok((vfs, output)) => {
                println!("  VFS blocks: {}", vfs.blocks.len());
                println!("  VFS nodes: {}", vfs.nodes.len());
                println!("  Output size: {} bytes", output.len());
                for node in &vfs.nodes {
                    println!("    Node: {} ({} bytes)", node.name, node.size);
                }
            }
            Err(e) => println!("  VFS parse error: {}", e),
        }
    }

    // Check common magics
    if data.len() >= 8 {
        if &data[..7] == b"UnityFS" {
            println!("  Magic: UnityFS");
        }
        if &data[..4] == b":)xD" {
            println!("  Magic: Wwise PCK");
        }
        if &data[..4] == b"CRID" {
            println!("  Magic: CRI Video");
        }
    }

    // Entropy
    let mut freq = [0u32; 256];
    for b in data {
        freq[*b as usize] += 1;
    }
    let unique = freq.iter().filter(|&&c| c > 0).count();
    let len = data.len() as f64;
    let entropy: f64 = freq
        .iter()
        .filter(|&&c| c > 0)
        .map(|&c| {
            let p = c as f64 / len;
            -p * p.log2()
        })
        .sum();
    println!("  Entropy: {:.4} bits/byte", entropy);
    println!("  Unique bytes: {}/256", unique);

    // Check for VFS headers anywhere in the file
    let mut vfs_offsets = Vec::new();
    for i in 0..data.len().saturating_sub(8) {
        if vfs_decrypt::is_valid_header(&data[i..]) {
            vfs_offsets.push(i);
            if vfs_offsets.len() >= 20 {
                break;
            }
        }
    }
    if !vfs_offsets.is_empty() {
        println!(
            "  VFS headers found at offsets: {:?}{}",
            &vfs_offsets[..vfs_offsets.len().min(10)],
            if vfs_offsets.len() > 10 { "..." } else { "" }
        );
    }
}
