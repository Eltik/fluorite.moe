/// VFS index file decryption investigation.
/// Downloads index_initial.json and index_main.json from the game ZIP,
/// runs diagnostic analysis, and attempts multiple decryption strategies.
use backend::core::endfield::{
    config::{ResolveParams, resolve_game_config},
    constants::{app_code, channel, sub_channel},
};
use backend::core::hypergryph::launcher::Platform;
use backend::extraction::crypto_constants::{VFS_AES_IV, VFS_AES_KEY};
use backend::extraction::vfs_aes;
use backend::extraction::vfs_decrypt;
use backend::extraction::wwise_pck;
use backend::extraction::zip_range::{SplitArchivePart, ZipCentralDirectory};
use reqwest::Client;
use std::io::Read as _;

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

/// Check if data is valid JSON by actually parsing it.
fn looks_like_json(data: &[u8]) -> bool {
    serde_json::from_slice::<serde_json::Value>(data).is_ok()
}

/// Shannon entropy over a byte slice.
fn entropy(data: &[u8]) -> f64 {
    if data.is_empty() {
        return 0.0;
    }
    let mut freq = [0u32; 256];
    for &b in data {
        freq[b as usize] += 1;
    }
    let len_f = data.len() as f64;
    freq.iter()
        .filter(|&&c| c > 0)
        .map(|&c| {
            let p = c as f64 / len_f;
            -p * p.log2()
        })
        .sum()
}

/// Run diagnostic analysis on a data blob.
fn analyze(name: &str, data: &[u8]) {
    println!("\n--- {} ---", name);
    println!(
        "  Size: {} ({} bytes)",
        format_bytes(data.len() as u64),
        data.len()
    );

    let sample = &data[..data.len().min(4096)];
    let ent = entropy(sample);
    println!("  Entropy (first 4KB): {:.2} bits/byte", ent);
    if ent > 7.9 {
        println!("  -> Likely encrypted (near-uniform distribution)");
    } else if ent > 6.0 {
        println!("  -> Possibly compressed");
    } else {
        println!("  -> Likely plaintext/structured");
    }

    // Byte frequency top-5
    let mut freq = [0u32; 256];
    for &b in sample {
        freq[b as usize] += 1;
    }
    let mut top: Vec<(u8, u32)> = freq
        .iter()
        .enumerate()
        .filter(|(_, c)| **c > 0)
        .map(|(i, c)| (i as u8, *c))
        .collect();
    top.sort_by(|a, b| b.1.cmp(&a.1));
    print!("  Top-5 bytes: ");
    for (byte, count) in top.iter().take(5) {
        print!("0x{:02X}={} ", byte, count);
    }
    println!();
    println!("  Unique byte values: {}/256", top.len());

    // Magic checks
    if !data.is_empty() {
        if looks_like_json(data) {
            println!("  -> Starts with JSON delimiter!");
        }
        if data.len() >= 8 && &data[..8] == b"UnityFS\0" {
            println!("  -> UnityFS magic!");
        }
        if data.len() >= 4 && data[..4] == [0x3A, 0x29, 0x78, 0x44] {
            println!("  -> :)xD magic!");
        }
        if data.len() >= 8 && vfs_decrypt::is_valid_header(data) {
            println!("  -> VFS checksum valid!");
        }
    }

    println!("  First 256 bytes:");
    hex_dump(data, 256);
}

/// Attempt A: VFS AES-CTR decryption with known key/IV.
fn try_vfs_aes(data: &[u8]) -> Option<Vec<u8>> {
    let mut buf = data.to_vec();
    vfs_aes::decrypt(&mut buf, &VFS_AES_KEY, &VFS_AES_IV);
    if looks_like_json(&buf) {
        Some(buf)
    } else {
        None
    }
}

/// Attempt B: VFS full container pipeline.
fn try_vfs_container(data: &[u8]) -> Option<Vec<u8>> {
    if !vfs_decrypt::is_valid_header(data) {
        return None;
    }
    match vfs_decrypt::process_vfs(data) {
        Ok((_vfs, output)) => Some(output),
        Err(_) => None,
    }
}

/// Attempt C: XOR-Mul (Wwise) cipher with various seeds.
fn try_xor_mul(data: &[u8]) -> Option<(u32, Vec<u8>)> {
    let seeds: Vec<(u32, &str)> = vec![
        (0, "zero"),
        (1, "one"),
        (data.len() as u32, "file_len"),
        (0x9C5A0B29, "CIPHER_XOR"),
        (0x04E11C23, "CIPHER_MUL"),
    ];
    // Also try bytes[4..8] as u32 LE if available
    let extra_seed = if data.len() >= 8 {
        Some(u32::from_le_bytes([data[4], data[5], data[6], data[7]]))
    } else {
        None
    };

    for (seed, label) in &seeds {
        let mut buf = data.to_vec();
        wwise_pck::decipher_inplace(&mut buf, *seed, 0);
        if looks_like_json(&buf) {
            println!("    -> XOR-Mul match with {} (seed=0x{:08X})", label, seed);
            return Some((*seed, buf));
        }
    }
    if let Some(seed) = extra_seed {
        let mut buf = data.to_vec();
        wwise_pck::decipher_inplace(&mut buf, seed, 0);
        if looks_like_json(&buf) {
            println!(
                "    -> XOR-Mul match with bytes[4..8] (seed=0x{:08X})",
                seed
            );
            return Some((seed, buf));
        }
    }
    None
}

/// Attempt D: Single-byte XOR brute-force.
fn try_single_byte_xor(data: &[u8]) -> Option<(u8, Vec<u8>)> {
    for key in 1..=255u8 {
        let mut buf = data.to_vec();
        for b in &mut buf {
            *b ^= key;
        }
        if looks_like_json(&buf) {
            return Some((key, buf));
        }
    }
    None
}

/// Attempt E: VFS AES with varied IVs.
fn try_aes_varied_ivs(data: &[u8]) -> Option<(&'static str, Vec<u8>)> {
    // All-zeros IV
    let zero_iv = [0u8; 16];
    let mut buf = data.to_vec();
    vfs_aes::decrypt(&mut buf, &VFS_AES_KEY, &zero_iv);
    if looks_like_json(&buf) {
        return Some(("zero IV", buf));
    }

    // First 16 bytes of data as IV
    if data.len() >= 32 {
        let mut iv = [0u8; 16];
        iv.copy_from_slice(&data[..16]);
        let mut buf = data[16..].to_vec();
        vfs_aes::decrypt(&mut buf, &VFS_AES_KEY, &iv);
        if looks_like_json(&buf) {
            return Some(("first-16-bytes IV", buf));
        }
    }

    // Last 16 bytes as IV
    if data.len() >= 32 {
        let mut iv = [0u8; 16];
        iv.copy_from_slice(&data[data.len() - 16..]);
        let mut buf = data[..data.len() - 16].to_vec();
        vfs_aes::decrypt(&mut buf, &VFS_AES_KEY, &iv);
        if looks_like_json(&buf) {
            return Some(("last-16-bytes IV", buf));
        }
    }

    None
}

/// Attempt F: Decompression (deflate, gzip, LZ4).
fn try_decompress(data: &[u8]) -> Option<(&'static str, Vec<u8>)> {
    // Deflate
    {
        let mut decoder = flate2::read::DeflateDecoder::new(data);
        let mut buf = Vec::new();
        if decoder.read_to_end(&mut buf).is_ok() && looks_like_json(&buf) {
            return Some(("deflate", buf));
        }
    }

    // Gzip
    {
        let mut decoder = flate2::read::GzDecoder::new(data);
        let mut buf = Vec::new();
        if decoder.read_to_end(&mut buf).is_ok() && looks_like_json(&buf) {
            return Some(("gzip", buf));
        }
    }

    // Zlib
    {
        let mut decoder = flate2::read::ZlibDecoder::new(data);
        let mut buf = Vec::new();
        if decoder.read_to_end(&mut buf).is_ok() && looks_like_json(&buf) {
            return Some(("zlib", buf));
        }
    }

    // LZ4 (try a few estimated output sizes)
    for multiplier in [2, 4, 8, 16] {
        let est_size = data.len() * multiplier;
        if let Ok(buf) = lz4_flex::decompress(data, est_size)
            && looks_like_json(&buf)
        {
            return Some(("lz4", buf));
        }
    }

    None
}

/// Run all decryption attempts on a data blob.
fn run_attempts(name: &str, data: &[u8]) {
    println!("\n  [Attempt A] VFS AES-CTR (standard key/IV)...");
    if let Some(decrypted) = try_vfs_aes(data) {
        println!("    SUCCESS! Decrypted {} bytes of JSON", decrypted.len());
        save_result(name, "vfs_aes", &decrypted);
        return;
    }
    println!("    No JSON detected");

    println!("  [Attempt B] VFS full container pipeline...");
    if let Some(output) = try_vfs_container(data) {
        println!("    SUCCESS! Got {} bytes of output", output.len());
        if looks_like_json(&output) {
            println!("    Output is JSON!");
            save_result(name, "vfs_container", &output);
            return;
        }
        println!("    Output is not JSON, dumping first 128 bytes:");
        hex_dump(&output, 128);
    } else {
        println!("    Not a valid VFS container");
    }

    println!("  [Attempt C] XOR-Mul cipher (Wwise)...");
    if let Some((seed, decrypted)) = try_xor_mul(data) {
        println!(
            "    SUCCESS! seed=0x{:08X}, {} bytes of JSON",
            seed,
            decrypted.len()
        );
        save_result(name, "xor_mul", &decrypted);
        return;
    }
    println!("    No JSON detected with any seed");

    println!("  [Attempt D] Single-byte XOR brute-force...");
    if let Some((key, decrypted)) = try_single_byte_xor(data) {
        println!(
            "    SUCCESS! key=0x{:02X}, {} bytes of JSON",
            key,
            decrypted.len()
        );
        save_result(name, "xor_byte", &decrypted);
        return;
    }
    println!("    No JSON detected with any byte");

    println!("  [Attempt E] VFS AES with varied IVs...");
    if let Some((iv_desc, decrypted)) = try_aes_varied_ivs(data) {
        println!(
            "    SUCCESS! IV={}, {} bytes of JSON",
            iv_desc,
            decrypted.len()
        );
        save_result(name, "aes_varied_iv", &decrypted);
        return;
    }
    println!("    No JSON detected");

    println!("  [Attempt F] Decompression (deflate/gzip/zlib/LZ4)...");
    if let Some((method, decompressed)) = try_decompress(data) {
        println!(
            "    SUCCESS! method={}, {} bytes of JSON",
            method,
            decompressed.len()
        );
        save_result(name, method, &decompressed);
        return;
    }
    println!("    No valid decompression found");

    // Attempt G: Base64 decode then retry all cipher attempts
    println!("  [Attempt G] Base64 decode + cipher attempts...");
    // Check if data is all valid base64 characters
    let is_base64 = data.iter().all(|&b| {
        b.is_ascii_alphanumeric() || b == b'+' || b == b'/' || b == b'=' || b == b'\n' || b == b'\r'
    });
    if is_base64 && data.len() > 16 {
        use base64::Engine;
        if let Ok(decoded) = base64::engine::general_purpose::STANDARD.decode(data) {
            println!(
                "    Base64 decoded: {} -> {} bytes",
                data.len(),
                decoded.len()
            );
            println!("    First 32 bytes hex:");
            hex_dump(&decoded, 32);

            let ent = entropy(&decoded);
            println!("    Entropy: {:.2} bits/byte", ent);

            // Try VFS AES on decoded
            println!("    [G.1] VFS AES on decoded...");
            if let Some(result) = try_vfs_aes(&decoded) {
                println!("    SUCCESS via base64 + VFS AES!");
                save_result(name, "b64_vfs_aes", &result);
                return;
            }

            // Try first 16 bytes as IV, rest as ciphertext
            if decoded.len() > 16 {
                println!("    [G.2] First 16 bytes as IV, AES on rest...");
                let mut iv = [0u8; 16];
                iv.copy_from_slice(&decoded[..16]);
                let mut ct = decoded[16..].to_vec();
                vfs_aes::decrypt(&mut ct, &VFS_AES_KEY, &iv);
                if looks_like_json(&ct) {
                    println!("    SUCCESS via base64 + custom IV AES!");
                    save_result(name, "b64_custom_iv_aes", &ct);
                    return;
                }
            }

            // Try XOR-Mul on decoded
            println!("    [G.3] XOR-Mul on decoded...");
            if let Some((seed, result)) = try_xor_mul(&decoded) {
                println!("    SUCCESS via base64 + XOR-Mul (seed=0x{:08X})!", seed);
                save_result(name, "b64_xor_mul", &result);
                return;
            }

            // Try decompression on decoded
            println!("    [G.4] Decompression on decoded...");
            if let Some((method, result)) = try_decompress(&decoded) {
                println!("    SUCCESS via base64 + {} decompress!", method);
                save_result(name, "b64_decompress", &result);
                return;
            }

            println!("    No JSON found after base64 decode");
        }
    } else {
        println!("    Not base64 encoded");
    }

    // Attempt H: Skip header bytes and retry (for .blc files starting with 03 00 00 00)
    if data.len() > 4 && data[..4] == [0x03, 0x00, 0x00, 0x00] {
        println!("  [Attempt H] Skip 4-byte .blc header + cipher attempts...");
        let body = &data[4..];
        println!(
            "    Body size: {} bytes (mod 16 = {})",
            body.len(),
            body.len() % 16
        );

        println!("    [H.1] VFS AES on body...");
        if let Some(result) = try_vfs_aes(body) {
            println!("    SUCCESS via header-skip + VFS AES!");
            save_result(name, "blc_vfs_aes", &result);
            return;
        }

        // Try with first 16 bytes of body as IV
        if body.len() > 16 {
            println!("    [H.2] Body[0:16] as IV, AES on body[16:]...");
            let mut iv = [0u8; 16];
            iv.copy_from_slice(&body[..16]);
            let mut ct = body[16..].to_vec();
            vfs_aes::decrypt(&mut ct, &VFS_AES_KEY, &iv);
            if looks_like_json(&ct) {
                println!("    SUCCESS via header-skip + custom IV!");
                save_result(name, "blc_custom_iv", &ct);
                return;
            }
        }

        println!("    [H.3] XOR-Mul on body...");
        if let Some((seed, result)) = try_xor_mul(body) {
            println!(
                "    SUCCESS via header-skip + XOR-Mul (seed=0x{:08X})!",
                seed
            );
            save_result(name, "blc_xor_mul", &result);
            return;
        }

        println!("    [H.4] Decompression on body...");
        if let Some((method, result)) = try_decompress(body) {
            println!("    SUCCESS via header-skip + {} decompress!", method);
            save_result(name, "blc_decompress", &result);
            return;
        }

        println!("    No JSON found after header skip");
    }

    println!("\n  ALL ATTEMPTS FAILED for {}", name);
    println!("  Raw file saved to ./output/ for manual analysis");
}

/// Save a successful decryption result.
fn save_result(name: &str, method: &str, data: &[u8]) {
    let safe_name = name.replace(['/', '\\'], "_");
    let path = format!("./output/{}_decrypted_{}.json", safe_name, method);
    match std::fs::write(&path, data) {
        Ok(()) => println!("    Saved to {}", path),
        Err(e) => println!("    Failed to save: {}", e),
    }

    // Try to parse and summarize the JSON structure
    if let Ok(val) = serde_json::from_slice::<serde_json::Value>(data) {
        println!("    Valid JSON!");
        match &val {
            serde_json::Value::Object(map) => {
                println!("    Top-level keys ({}):", map.len());
                for (key, value) in map.iter().take(20) {
                    match value {
                        serde_json::Value::Array(arr) => {
                            println!("      \"{}\": array[{}]", key, arr.len())
                        }
                        serde_json::Value::Object(obj) => {
                            println!("      \"{}\": object{{{}keys}}", key, obj.len())
                        }
                        serde_json::Value::String(s) => {
                            let preview = if s.len() > 50 {
                                format!("{}...", &s[..50])
                            } else {
                                s.clone()
                            };
                            println!("      \"{}\": \"{}\"", key, preview);
                        }
                        other => println!("      \"{}\": {}", key, other),
                    }
                }
                if map.len() > 20 {
                    println!("      ... and {} more keys", map.len() - 20);
                }
            }
            serde_json::Value::Array(arr) => {
                println!("    Top-level array with {} elements", arr.len());
            }
            _ => println!("    Top-level: {}", val),
        }
    } else {
        println!("    Looks like JSON but failed to parse (may have trailing garbage)");
        // Show first 200 chars
        let preview = String::from_utf8_lossy(&data[..data.len().min(200)]);
        println!("    Preview: {}", preview);
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("=== VFS Index File Decryption Investigation ===\n");

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

    // List all VFS-related files for discovery
    println!("\n[3] Searching for VFS index files in ZIP...");
    let vfs_files: Vec<String> = cd
        .filenames()
        .filter(|f| f.contains("VFS/") || f.contains("index"))
        .map(String::from)
        .collect();
    for f in &vfs_files {
        let size = cd.get(f).map_or(0, |e| e.uncompressed_size);
        println!("    {} ({})", f, format_bytes(size));
    }

    // Target index files
    let index_files = [
        "Endfield_Data/StreamingAssets/index_initial.json",
        "Endfield_Data/StreamingAssets/index_main.json",
    ];

    // Also try .blc catalog files — these may contain asset name mappings
    let blc_files = [
        "Endfield_Data/StreamingAssets/VFS/7064D8E2/7064D8E2.blc",
        "Endfield_Data/StreamingAssets/VFS/0CE8FA57/0CE8FA57.blc",
    ];

    println!("\n[4] Downloading and analyzing index files...");

    for filename in &index_files {
        println!("\n=== Processing: {} ===", filename);

        // Check cache first
        let safe_name = filename.replace('/', "_");
        let cache_path = format!("{}/{}", output_dir, safe_name);

        let data = if let Ok(cached) = std::fs::read(&cache_path) {
            println!("  Loaded from cache ({} bytes)", cached.len());
            cached
        } else {
            match cd.extract(&client, filename).await {
                Ok(file_data) => {
                    let _ = std::fs::write(&cache_path, &file_data);
                    println!("  Downloaded and cached ({} bytes)", file_data.len());
                    file_data
                }
                Err(e) => {
                    println!("  ERROR: Failed to extract: {}", e);
                    continue;
                }
            }
        };

        if data.is_empty() {
            println!("  Empty file, skipping");
            continue;
        }

        // Run diagnostics
        analyze(filename, &data);

        // Run decryption attempts
        println!("\n  === Decryption Attempts ===");
        run_attempts(filename, &data);
    }

    // Process .blc catalog files
    println!("\n\n[5] Downloading and analyzing .blc catalog files...");

    for filename in &blc_files {
        println!("\n=== Processing: {} ===", filename);

        let safe_name = filename.replace(['/', '\\'], "_");
        let cache_path = format!("{}/{}", output_dir, safe_name);

        let data = if let Ok(cached) = std::fs::read(&cache_path) {
            println!("  Loaded from cache ({} bytes)", cached.len());
            cached
        } else {
            match cd.extract(&client, filename).await {
                Ok(file_data) => {
                    let _ = std::fs::write(&cache_path, &file_data);
                    println!("  Downloaded and cached ({} bytes)", file_data.len());
                    file_data
                }
                Err(e) => {
                    println!("  ERROR: Failed to extract: {}", e);
                    continue;
                }
            }
        };

        if data.is_empty() {
            println!("  Empty file, skipping");
            continue;
        }

        analyze(filename, &data);
        println!("\n  === Decryption Attempts ===");
        run_attempts(filename, &data);
    }

    println!("\n=== Index File Investigation Complete ===");
    Ok(())
}
