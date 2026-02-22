use backend::core::endfield::{
    config::{ResolveParams, resolve_game_config},
    constants::{app_code, channel, sub_channel},
};
use backend::core::hypergryph::launcher::Platform;
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

fn read_u32_le(data: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(data[offset..offset + 4].try_into().unwrap())
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

fn entropy(data: &[u8]) -> f64 {
    let mut counts = [0u32; 256];
    for &b in data {
        counts[b as usize] += 1;
    }
    let len = data.len() as f64;
    counts
        .iter()
        .filter(|&&c| c > 0)
        .map(|&c| {
            let p = c as f64 / len;
            -p * p.log2()
        })
        .sum()
}

/// Search for a byte pattern in data, return offsets of first N matches
fn find_pattern(data: &[u8], pattern: &[u8], max_matches: usize) -> Vec<usize> {
    let mut matches = Vec::new();
    for i in 0..data.len().saturating_sub(pattern.len()) {
        if &data[i..i + pattern.len()] == pattern {
            matches.push(i);
            if matches.len() >= max_matches {
                break;
            }
        }
    }
    matches
}

/// Extract all readable ASCII strings of length >= min_len
fn extract_strings(data: &[u8], min_len: usize, max_strings: usize) -> Vec<(usize, String)> {
    let mut results = Vec::new();
    let mut start = None;
    for (i, &b) in data.iter().enumerate() {
        if b.is_ascii_graphic() || b == b' ' {
            if start.is_none() {
                start = Some(i);
            }
        } else {
            if let Some(s) = start {
                let len = i - s;
                if len >= min_len {
                    let st = String::from_utf8_lossy(&data[s..i]).to_string();
                    results.push((s, st));
                    if results.len() >= max_strings {
                        return results;
                    }
                }
            }
            start = None;
        }
    }
    if let Some(s) = start {
        let len = data.len() - s;
        if len >= min_len {
            results.push((s, String::from_utf8_lossy(&data[s..]).to_string()));
        }
    }
    results
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let client = Client::new();

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
    println!("Version: {}", config.version);

    let parts: Vec<SplitArchivePart> = config
        .packs
        .iter()
        .map(|p| SplitArchivePart {
            url: p.url.clone(),
            size: p.package_size.parse::<u64>().expect("invalid package_size"),
        })
        .collect();

    println!("\n=== Parsing ZIP central directory ===");
    let cd = ZipCentralDirectory::from_split_archive(&client, parts).await?;
    println!("Total entries: {}", cd.len());

    // Create output directory
    std::fs::create_dir_all("/Users/eltik/Documents/Coding/fluorite.moe/output")?;

    // === TASK 1: FlatBuffer / binary-table dirs ===
    let analysis_dirs = [
        ("23D53F5D", "FlatBuffer (game config?)"),
        ("C3442D43", "FlatBuffer (PVH/0AH strings)"),
        ("D6E622F7", "Offset table"),
        ("3C9D9D2D", "Offset table"),
    ];

    for (dir, label) in &analysis_dirs {
        let target = cd
            .filenames()
            .filter(|f| f.contains(dir) && f.ends_with(".chk"))
            .min_by_key(|f| cd.get(f).map_or(u64::MAX, |e| e.compressed_size))
            .map(|s| s.to_string());

        let Some(path) = target else {
            println!("\n=== {} ({}) — no .chk files ===", dir, label);
            continue;
        };

        let entry = cd.get(&path).unwrap();
        println!("\n============================================================");
        println!("=== {} ({}) ===", dir, label);
        println!("  File: {}", path.rsplit('/').next().unwrap_or(&path));
        println!(
            "  Compressed: {}, Uncompressed: {}",
            format_bytes(entry.compressed_size),
            format_bytes(entry.uncompressed_size)
        );

        let data = cd.extract(&client, &path).await?;
        println!("  Extracted: {} bytes", data.len());

        // Save to disk
        let out_path = format!(
            "/Users/eltik/Documents/Coding/fluorite.moe/output/{}_smallest.chk",
            dir
        );
        std::fs::write(&out_path, &data)?;
        println!("  Saved to: {}", out_path);

        // Entropy
        let sample_end = data.len().min(4096);
        println!(
            "  Entropy (first 4KB): {:.2} bits/byte",
            entropy(&data[..sample_end])
        );

        // Hex dump first 512 bytes
        println!("\n  --- First 512 bytes ---");
        hex_dump(&data, 512);

        // Read as LE u32 values (first 64 values = 256 bytes)
        let num_u32s = (data.len() / 4).min(64);
        println!("\n  --- First {} u32 LE values ---", num_u32s);
        for i in 0..num_u32s {
            let val = read_u32_le(&data, i * 4);
            if i % 8 == 0 {
                print!("  {:04x}:", i * 4);
            }
            print!(" {:08x}", val);
            if i % 8 == 7 || i == num_u32s - 1 {
                println!();
            }
        }

        // Check for monotonically increasing pattern (offset table indicator)
        let u32_vals: Vec<u32> = (0..num_u32s).map(|i| read_u32_le(&data, i * 4)).collect();
        let monotonic_runs: Vec<usize> = {
            let mut runs = Vec::new();
            let mut run_len = 1;
            for w in u32_vals.windows(2) {
                if w[1] > w[0] && w[1] - w[0] < 0x10000 {
                    run_len += 1;
                } else {
                    if run_len >= 4 {
                        runs.push(run_len);
                    }
                    run_len = 1;
                }
            }
            if run_len >= 4 {
                runs.push(run_len);
            }
            runs
        };
        if !monotonic_runs.is_empty() {
            println!(
                "  Monotonically increasing runs (len>=4): {:?}",
                monotonic_runs
            );
        }

        // FlatBuffer detection: first u32 is offset to root table
        let first_u32 = read_u32_le(&data, 0);
        println!("\n  --- FlatBuffer analysis ---");
        println!(
            "  Root table offset (first u32): 0x{:x} ({})",
            first_u32, first_u32
        );
        if (first_u32 as usize) < data.len().saturating_sub(4) {
            let vtable_offset_pos = first_u32 as usize;
            let soffset = i32::from_le_bytes(
                data[vtable_offset_pos..vtable_offset_pos + 4]
                    .try_into()
                    .unwrap(),
            );
            println!("  At root table: soffset_t to vtable = {}", soffset);
            let vtable_pos = vtable_offset_pos as i64 - soffset as i64;
            if vtable_pos >= 0 && (vtable_pos as usize) + 4 <= data.len() {
                let vp = vtable_pos as usize;
                let vtable_size = u16::from_le_bytes(data[vp..vp + 2].try_into().unwrap());
                let table_size = u16::from_le_bytes(data[vp + 2..vp + 4].try_into().unwrap());
                println!(
                    "  VTable at 0x{:x}: vtable_size={}, table_size={}",
                    vp, vtable_size, table_size
                );
                if (4..200).contains(&vtable_size) {
                    let num_fields = (vtable_size as usize - 4) / 2;
                    print!("  VTable fields ({}):", num_fields);
                    for f in 0..num_fields.min(20) {
                        let field_off = u16::from_le_bytes(
                            data[vp + 4 + f * 2..vp + 6 + f * 2].try_into().unwrap(),
                        );
                        print!(" {}", field_off);
                    }
                    println!();
                    println!("  -> Looks like a valid FlatBuffer!");
                }
            }
        }

        // Search for readable ASCII strings (min 4 chars)
        let search_len = data.len().min(65536); // first 64KB
        let strings = extract_strings(&data[..search_len], 4, 50);
        if !strings.is_empty() {
            println!("\n  --- ASCII strings (first 64KB, min 4 chars) ---");
            for (offset, s) in &strings {
                println!(
                    "    0x{:04x}: \"{}\"",
                    offset,
                    if s.len() > 80 { &s[..80] } else { s }
                );
            }
        }

        // Search for signatures
        let search_range = &data[..data.len().min(1048576)]; // first 1MB
        for (sig, name) in &[
            (b"UnityFS\0".as_slice(), "UnityFS"),
            (b"CRID".as_slice(), "CRID"),
            (b"OggS".as_slice(), "OggS"),
        ] {
            let hits = find_pattern(search_range, sig, 3);
            if !hits.is_empty() {
                println!("  Found '{}' at offsets: {:x?}", name, hits);
            }
        }
    }

    // === TASK 3: Deep analysis of 0CE8FA57 and 7064D8E2 ===
    let unknown_dirs = [
        ("0CE8FA57", "unknown-binary (dd 0c pattern)"),
        ("7064D8E2", "unknown-binary (dd 0c pattern, huge)"),
    ];

    for (dir, label) in &unknown_dirs {
        let target = cd
            .filenames()
            .filter(|f| f.contains(dir) && f.ends_with(".chk"))
            .min_by_key(|f| cd.get(f).map_or(u64::MAX, |e| e.compressed_size))
            .map(|s| s.to_string());

        let Some(path) = target else {
            continue;
        };
        let entry = cd.get(&path).unwrap();

        println!("\n============================================================");
        println!("=== {} ({}) ===", dir, label);
        println!("  File: {}", path.rsplit('/').next().unwrap_or(&path));
        println!(
            "  Compressed: {}, Uncompressed: {}",
            format_bytes(entry.compressed_size),
            format_bytes(entry.uncompressed_size)
        );

        let data = cd.extract(&client, &path).await?;
        println!("  Extracted: {} bytes", data.len());

        // Save to disk
        let out_path = format!(
            "/Users/eltik/Documents/Coding/fluorite.moe/output/{}_smallest.chk",
            dir
        );
        std::fs::write(&out_path, &data)?;
        println!("  Saved to: {}", out_path);

        // Entropy
        println!(
            "  Entropy (first 4KB): {:.2} bits/byte",
            entropy(&data[..data.len().min(4096)])
        );

        // Hex dump first 1024 bytes
        println!("\n  --- First 1024 bytes ---");
        hex_dump(&data, 1024);

        // Read as u32 LE (first 64 values)
        let num_u32s = (data.len() / 4).min(64);
        println!("\n  --- First {} u32 LE values ---", num_u32s);
        for i in 0..num_u32s {
            let val = read_u32_le(&data, i * 4);
            if i % 8 == 0 {
                print!("  {:04x}:", i * 4);
            }
            print!(" {:08x}", val);
            if i % 8 == 7 || i == num_u32s - 1 {
                println!();
            }
        }

        // Search for dd 0c in first 4096 bytes
        let scan_len = data.len().min(4096);
        let dd0c_hits = find_pattern(&data[..scan_len], &[0xdd, 0x0c], 30);
        println!("\n  --- 'dd 0c' occurrences in first 4KB ---");
        println!("  Count: {}", dd0c_hits.len());
        for off in &dd0c_hits {
            println!(
                "    0x{:04x}: context = {:02x?}",
                off,
                &data[*off..(*off + 16).min(scan_len)]
            );
        }

        // Search for other repeating patterns
        for (pattern, name) in &[
            ([0xc5, 0xa8, 0x1c, 0x43].as_slice(), "c5 a8 1c 43"),
            ([0x84, 0x5d, 0x1e, 0x05].as_slice(), "84 5d 1e 05"),
            ([0xe5, 0x8b, 0xee].as_slice(), "e5 8b ee"),
        ] {
            let hits = find_pattern(&data[..scan_len], pattern, 10);
            if !hits.is_empty() {
                println!("  Pattern '{}' at offsets: {:x?}", name, hits);
            }
        }

        // XOR brute-force: try XORing first 8 bytes with single-byte keys to find "UnityFS\0"
        println!("\n  --- XOR brute-force for UnityFS magic ---");
        let unity_magic = b"UnityFS\0";
        let mut found_xor = false;
        for key in 0u8..=255 {
            let xored: Vec<u8> = data[..8].iter().map(|&b| b ^ key).collect();
            if xored == unity_magic {
                println!("  MATCH: XOR key 0x{:02x} reveals 'UnityFS\\0'", key);
                found_xor = true;
            }
        }
        if !found_xor {
            println!("  No single-byte XOR key reveals 'UnityFS\\0' at offset 0");
        }

        // Try XOR at every offset in first 4096 bytes
        let mut xor_unity_offsets = Vec::new();
        for off in 0..data.len().min(4096).saturating_sub(8) {
            for key in 0u8..=255 {
                let xored: Vec<u8> = data[off..off + 8].iter().map(|&b| b ^ key).collect();
                if &xored[..] == unity_magic {
                    xor_unity_offsets.push((off, key));
                }
            }
        }
        if !xor_unity_offsets.is_empty() {
            println!("  XOR UnityFS matches in first 4KB:");
            for (off, key) in &xor_unity_offsets {
                println!("    offset 0x{:x}, key 0x{:02x}", off, key);
            }
        }

        // Search for UnityFS in first 1MB (raw, no XOR)
        let search_1mb = data.len().min(1048576);
        let unity_hits = find_pattern(&data[..search_1mb], b"UnityFS\0", 5);
        if !unity_hits.is_empty() {
            println!("  Raw UnityFS magic in first 1MB: {:x?}", unity_hits);
        } else {
            println!("  No raw UnityFS magic in first 1MB");
        }

        // Compare u32 values between files at specific offsets
        println!("\n  --- Structural analysis (u32 LE at key offsets) ---");
        let key_offsets = [0, 4, 8, 12, 16, 20, 24, 28, 32, 36, 40, 44, 48, 56, 60];
        for &off in &key_offsets {
            if off + 4 <= data.len() {
                let val = read_u32_le(&data, off);
                println!("    [0x{:02x}] = 0x{:08x} ({})", off, val, val);
            }
        }

        // ASCII strings
        let strings = extract_strings(&data[..data.len().min(65536)], 4, 30);
        if !strings.is_empty() {
            println!("\n  --- ASCII strings (first 64KB) ---");
            for (offset, s) in &strings {
                println!(
                    "    0x{:04x}: \"{}\"",
                    offset,
                    if s.len() > 80 { &s[..80] } else { s }
                );
            }
        }
    }

    println!("\nDone.");
    Ok(())
}
