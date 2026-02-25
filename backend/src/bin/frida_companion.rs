/// Phase P: Frida Capture Companion — parse + analyze runtime intercept logs.
///
/// Pass 1: Parse Frida event log (block-delimited format)
/// Pass 2: Validate decrypted output with detect() oracle
/// Pass 3: Cipher I/O analysis (keystream, determinism, block patterns)
/// Pass 4: Key extraction from register dumps
/// Pass 5: Auto-decrypt cached .chk files if cipher identified
use backend::extraction::vfs_decrypt;
use std::collections::HashMap;
use std::fs;

// ══════════════════════════════════════════════════════════════
// Constants
// ══════════════════════════════════════════════════════════════

const ENCRYPTED_DIRS: &[&str] = &["19E3AE45", "1CDDBF1F", "42A8FCA6", "775A31D1"];
const VFS_CACHE_DIR: &str = "./output/vfs_cache";

// ══════════════════════════════════════════════════════════════
// Data structures
// ══════════════════════════════════════════════════════════════

#[derive(Debug, Clone)]
enum EventType {
    DecryptCall,
    DecryptRegister,
    HgpCall,
    VfsLoad,
    Unknown(String),
}

#[derive(Debug, Clone)]
#[expect(dead_code)]
struct FridaEvent {
    event_type: EventType,
    timestamp: u64,
    func_addr: u64,
    fields: HashMap<String, String>,
    input_hex: Vec<u8>,
    output_hex: Vec<u8>,
}

#[derive(Debug)]
#[expect(dead_code)]
struct IoPair {
    input: Vec<u8>,
    output: Vec<u8>,
    func_addr: u64,
    event_idx: usize,
}

// ══════════════════════════════════════════════════════════════
// Shared infrastructure
// ══════════════════════════════════════════════════════════════

fn format_bytes(bytes: u64) -> String {
    const UNITS: &[&str] = &["B", "KB", "MB", "GB"];
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

fn hex_dump(data: &[u8], max_bytes: usize) {
    let dump_len = data.len().min(max_bytes);
    for row in 0..dump_len.div_ceil(16) {
        let off = row * 16;
        if off >= dump_len {
            break;
        }
        print!("    {:04x}: ", off);
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

fn ascii_ratio(data: &[u8]) -> f64 {
    if data.is_empty() {
        return 0.0;
    }
    let count = data
        .iter()
        .filter(|&&b| b.is_ascii_graphic() || b == b' ' || b == b'\n' || b == b'\r' || b == b'\t')
        .count();
    count as f64 / data.len() as f64
}

fn max_zero_run(data: &[u8]) -> usize {
    let mut max = 0usize;
    let mut run = 0usize;
    for &b in data {
        if b == 0 {
            run += 1;
            max = max.max(run);
        } else {
            run = 0;
        }
    }
    max
}

fn looks_like_flatbuffer(data: &[u8]) -> bool {
    if data.len() < 8 {
        return false;
    }
    let root_offset = u32::from_le_bytes([data[0], data[1], data[2], data[3]]);
    root_offset > 0 && root_offset < 1000 && (root_offset as usize + 4) < data.len()
}

const MAGIC_STRINGS: &[(&[u8], &str)] = &[
    (b"UnityFS\0", "UnityFS"),
    (b"CAB-", "CAB-"),
    (b"AKPK", "AKPK"),
    (b"RIFF", "RIFF"),
    (b"BKHD", "BKHD"),
    (b"CRID", "CRID"),
    (b":)xD", ":)xD"),
    (b"\x89PNG", "PNG"),
    (b"PK\x03\x04", "ZIP"),
    (b"FBS\0", "FBS"),
    (b"\x00\x00\x00\x14ftypiso", "MP4"),
];

fn detect(data: &[u8]) -> (bool, String) {
    let sample = &data[..data.len().min(1024)];
    let ent = entropy(sample);
    let ascii32 = ascii_ratio(&data[..data.len().min(32)]);
    let ascii256 = ascii_ratio(&data[..data.len().min(256)]);
    let zeros = max_zero_run(sample);
    let fb = looks_like_flatbuffer(data);

    let mut reasons = Vec::new();
    let mut confidence = 0u32;

    for (magic, name) in MAGIC_STRINGS {
        if data.len() >= magic.len() && &data[..magic.len()] == *magic {
            reasons.push((*name).into());
            confidence += 2;
        }
    }
    if data.len() >= 8 && vfs_decrypt::is_valid_header(data) {
        reasons.push("VFS-header".into());
        confidence += 2;
    }

    if ent < 5.0 {
        reasons.push(format!("entropy={:.2}", ent));
        confidence += 2;
    } else if ent < 6.5 {
        reasons.push(format!("entropy={:.2}", ent));
        confidence += 1;
    }

    if ascii256 > 0.80 {
        reasons.push(format!("ascii256={:.0}%", ascii256 * 100.0));
        confidence += 2;
    } else if ascii32 > 0.75 {
        reasons.push(format!("ascii32={:.0}%", ascii32 * 100.0));
        confidence += 1;
    }

    if zeros > 16 {
        reasons.push(format!("zero_run={}", zeros));
        confidence += 1;
    }

    if fb {
        let root = u32::from_le_bytes([data[0], data[1], data[2], data[3]]) as usize;
        if root + 4 < data.len() {
            let vtable_offset =
                i32::from_le_bytes([data[root], data[root + 1], data[root + 2], data[root + 3]]);
            let vtable_pos = (root as i64 - vtable_offset as i64) as usize;
            if vtable_pos < data.len().saturating_sub(4) {
                let vtable_size = u16::from_le_bytes([data[vtable_pos], data[vtable_pos + 1]]);
                if vtable_size >= 4 && (vtable_size as usize) < 1000 {
                    reasons.push(format!("flatbuf(root={},vt_sz={})", root, vtable_size));
                    confidence += 2;
                }
            }
        }
    }

    if !data.is_empty() && (data[0] == b'{' || data[0] == b'[') && ascii32 > 0.60 {
        reasons.push("JSON-like".into());
        confidence += 1;
    }

    let is_hit = confidence >= 2;
    (is_hit, reasons.join(", "))
}

fn decode_hex(s: &str) -> Vec<u8> {
    let s = s.trim();
    let mut bytes = Vec::new();
    let mut i = 0;
    let chars: Vec<char> = s.chars().collect();
    while i + 1 < chars.len() {
        if let (Some(hi), Some(lo)) = (chars[i].to_digit(16), chars[i + 1].to_digit(16)) {
            bytes.push((hi * 16 + lo) as u8);
            i += 2;
        } else {
            i += 1;
        }
    }
    bytes
}

// ══════════════════════════════════════════════════════════════
// Pass 1: Parse Frida event log
// ══════════════════════════════════════════════════════════════

fn pass_1_parse(content: &str) -> Vec<FridaEvent> {
    println!("\n╔══════════════════════════════════════════════════════════════╗");
    println!("║  Pass 1: Parse Frida Capture Log                           ║");
    println!("╚══════════════════════════════════════════════════════════════╝\n");

    let mut events: Vec<FridaEvent> = Vec::new();
    let mut current_fields: HashMap<String, String> = HashMap::new();
    let mut in_event = false;

    for line in content.lines() {
        let trimmed = line.trim();

        if trimmed.starts_with("=== FRIDA_EVENT") {
            in_event = true;
            current_fields.clear();

            // Extract type from header
            if let Some(type_val) = trimmed
                .split("type=")
                .nth(1)
                .and_then(|s| s.split_whitespace().next())
            {
                current_fields.insert("type".into(), type_val.to_string());
            }
            continue;
        }

        if trimmed == "=== END_EVENT ===" {
            if in_event {
                let event_type = match current_fields.get("type").map(|s| s.as_str()) {
                    Some("DECRYPT_CALL") => EventType::DecryptCall,
                    Some("DECRYPT_REGISTER") => EventType::DecryptRegister,
                    Some("HGP_CALL") => EventType::HgpCall,
                    Some("VFS_LOAD") => EventType::VfsLoad,
                    Some(other) => EventType::Unknown(other.to_string()),
                    None => EventType::Unknown("none".to_string()),
                };

                let timestamp = current_fields
                    .get("timestamp")
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(0);

                let func_addr = current_fields
                    .get("func_addr")
                    .and_then(|s| u64::from_str_radix(s.trim_start_matches("0x"), 16).ok())
                    .unwrap_or(0);

                let input_hex = current_fields
                    .get("arg0_hex")
                    .or_else(|| current_fields.get("input_hex"))
                    .map(|s| decode_hex(s))
                    .unwrap_or_default();

                let output_hex = current_fields
                    .get("result_hex")
                    .or_else(|| current_fields.get("output_hex"))
                    .map(|s| decode_hex(s))
                    .unwrap_or_default();

                events.push(FridaEvent {
                    event_type,
                    timestamp,
                    func_addr,
                    fields: current_fields.clone(),
                    input_hex,
                    output_hex,
                });
            }
            in_event = false;
            continue;
        }

        if in_event && let Some((key, val)) = trimmed.split_once('=') {
            current_fields.insert(key.to_string(), val.to_string());
        }
    }

    // Summary
    let mut type_counts: HashMap<String, u32> = HashMap::new();
    for ev in &events {
        let type_name = match &ev.event_type {
            EventType::DecryptCall => "DECRYPT_CALL",
            EventType::DecryptRegister => "DECRYPT_REGISTER",
            EventType::HgpCall => "HGP_CALL",
            EventType::VfsLoad => "VFS_LOAD",
            EventType::Unknown(s) => s.as_str(),
        };
        *type_counts.entry(type_name.to_string()).or_insert(0) += 1;
    }

    println!("  Parsed {} events:", events.len());
    for (t, c) in &type_counts {
        println!("    {:20} : {}", t, c);
    }

    let with_io = events
        .iter()
        .filter(|e| !e.input_hex.is_empty() && !e.output_hex.is_empty())
        .count();
    println!("  Events with I/O pairs: {}", with_io);

    events
}

// ══════════════════════════════════════════════════════════════
// Pass 2: Validate decrypted output
// ══════════════════════════════════════════════════════════════

fn pass_2_validate(events: &[FridaEvent]) {
    println!("\n╔══════════════════════════════════════════════════════════════╗");
    println!("║  Pass 2: Output Validation (detect oracle)                 ║");
    println!("╚══════════════════════════════════════════════════════════════╝\n");

    let mut hits = 0u32;
    let mut misses = 0u32;

    for (i, ev) in events.iter().enumerate() {
        if ev.output_hex.is_empty() {
            continue;
        }

        let (is_hit, reasons) = detect(&ev.output_hex);
        let ent = entropy(&ev.output_hex[..ev.output_hex.len().min(1024)]);

        if is_hit {
            hits += 1;
            println!(
                "  Event #{}: HIT — {} (output {} bytes, entropy={:.2})",
                i,
                reasons,
                ev.output_hex.len(),
                ent
            );
            println!("    Output preview:");
            hex_dump(&ev.output_hex, 64);
        } else {
            misses += 1;
            if misses <= 5 {
                println!(
                    "  Event #{}: miss — entropy={:.2}, {} bytes",
                    i,
                    ent,
                    ev.output_hex.len()
                );
            }
        }
    }

    println!("\n  Results: {} hits, {} misses", hits, misses);
    if hits > 0 {
        println!("  Decrypt function CONFIRMED — output contains recognizable data!");
    }
}

// ══════════════════════════════════════════════════════════════
// Pass 3: Cipher I/O analysis
// ══════════════════════════════════════════════════════════════

fn pass_3_cipher_analysis(events: &[FridaEvent]) {
    println!("\n╔══════════════════════════════════════════════════════════════╗");
    println!("║  Pass 3: Cipher I/O Analysis                               ║");
    println!("╚══════════════════════════════════════════════════════════════╝\n");

    // Collect I/O pairs
    let pairs: Vec<IoPair> = events
        .iter()
        .enumerate()
        .filter(|(_, e)| !e.input_hex.is_empty() && !e.output_hex.is_empty())
        .map(|(i, e)| IoPair {
            input: e.input_hex.clone(),
            output: e.output_hex.clone(),
            func_addr: e.func_addr,
            event_idx: i,
        })
        .collect();

    if pairs.is_empty() {
        println!("  No I/O pairs to analyze.");
        return;
    }

    println!("  Analyzing {} I/O pairs...\n", pairs.len());

    // Check 1: Determinism — same input → same output?
    println!("  === Check 1: Determinism ===");
    let mut input_map: HashMap<Vec<u8>, Vec<usize>> = HashMap::new();
    for (i, pair) in pairs.iter().enumerate() {
        input_map.entry(pair.input.clone()).or_default().push(i);
    }

    let repeated = input_map.values().filter(|v| v.len() > 1).count();
    println!("  Unique inputs: {}", input_map.len());
    println!("  Repeated inputs: {}", repeated);

    for (input, indices) in &input_map {
        if indices.len() < 2 {
            continue;
        }
        let first_output = &pairs[indices[0]].output;
        let all_same = indices.iter().all(|&i| pairs[i].output == *first_output);
        println!(
            "    Input ({} bytes, {} occurrences): outputs {}",
            input.len(),
            indices.len(),
            if all_same {
                "IDENTICAL (deterministic)"
            } else {
                "DIFFER (non-deterministic/stateful)"
            }
        );
    }

    // Check 2: XOR keystream analysis
    println!("\n  === Check 2: XOR Keystream Analysis ===");
    for (i, pair) in pairs.iter().take(10).enumerate() {
        let min_len = pair.input.len().min(pair.output.len());
        if min_len == 0 {
            continue;
        }

        let keystream: Vec<u8> = pair
            .input
            .iter()
            .zip(&pair.output)
            .map(|(&a, &b)| a ^ b)
            .collect();

        let ks_ent = entropy(&keystream);
        let ks_zeros = max_zero_run(&keystream);

        // Check for periodicity in keystream
        let period = find_period(&keystream);

        println!(
            "    Pair #{}: in={} out={} keystream_entropy={:.2} zero_run={} period={}",
            i,
            pair.input.len(),
            pair.output.len(),
            ks_ent,
            ks_zeros,
            if let Some(p) = period {
                format!("{}", p)
            } else {
                "none".into()
            }
        );

        if ks_ent < 6.0 {
            println!("      Low entropy keystream — possible simple XOR cipher!");
            println!("      Keystream preview:");
            hex_dump(&keystream, 64);
        }
    }

    // Check 3: Block-level ECB detection
    println!("\n  === Check 3: Block-Level ECB Test ===");
    for block_size in [8, 16, 32] {
        let mut input_blocks: HashMap<Vec<u8>, Vec<Vec<u8>>> = HashMap::new();

        for pair in &pairs {
            let min_len = pair.input.len().min(pair.output.len());
            let full_blocks = min_len / block_size;

            for b in 0..full_blocks {
                let off = b * block_size;
                let in_block = pair.input[off..off + block_size].to_vec();
                let out_block = pair.output[off..off + block_size].to_vec();
                input_blocks.entry(in_block).or_default().push(out_block);
            }
        }

        let repeated_blocks = input_blocks.values().filter(|v| v.len() > 1).count();
        let ecb_consistent = input_blocks
            .values()
            .filter(|v| v.len() > 1 && v.iter().all(|b| *b == v[0]))
            .count();

        println!(
            "    Block size {}: {} unique, {} repeated, {} ECB-consistent",
            block_size,
            input_blocks.len(),
            repeated_blocks,
            ecb_consistent,
        );

        if ecb_consistent > 0 && repeated_blocks > 0 {
            let ratio = ecb_consistent as f64 / repeated_blocks as f64;
            if ratio > 0.9 {
                println!(
                    "      HIGH ECB consistency ({:.0}%) — likely ECB mode with {}-byte blocks!",
                    ratio * 100.0,
                    block_size
                );
            }
        }
    }

    // Check 4: S-box residue in keystream
    println!("\n  === Check 4: S-box Residue Analysis ===");
    // Load S-boxes from DLL
    if let Ok(ga_data) = fs::read("./output/GameAssembly.dll") {
        let sbox_offsets = [
            0x0FF6C960usize,
            0x0FF6CA60,
            0x0FF6CB60,
            0x0FF6CD00,
            0x0FF6CE00,
            0x0FF6CF00,
        ];

        for (si, &off) in sbox_offsets.iter().enumerate() {
            if off + 256 > ga_data.len() {
                continue;
            }
            let sbox = &ga_data[off..off + 256];

            // Check if any pair's output bytes follow S-box distribution
            for (pi, pair) in pairs.iter().take(5).enumerate() {
                if pair.output.len() < 16 {
                    continue;
                }

                // Apply inverse S-box to output, check if result has lower entropy
                let mut inv_sbox = [0u8; 256];
                for (i, &v) in sbox.iter().enumerate() {
                    inv_sbox[v as usize] = i as u8;
                }

                let inv_output: Vec<u8> =
                    pair.output.iter().map(|&b| inv_sbox[b as usize]).collect();
                let orig_ent = entropy(&pair.output);
                let inv_ent = entropy(&inv_output);

                if (inv_ent - orig_ent).abs() > 0.5 {
                    println!(
                        "    S-box {} on pair #{}: orig_ent={:.2} → inv_ent={:.2} (delta={:.2})",
                        si,
                        pi,
                        orig_ent,
                        inv_ent,
                        inv_ent - orig_ent
                    );
                }
            }
        }
    }
}

/// Find the shortest period in a byte sequence
fn find_period(data: &[u8]) -> Option<usize> {
    if data.len() < 4 {
        return None;
    }
    for period in 1..=data.len() / 2 {
        let matches = data
            .iter()
            .enumerate()
            .skip(period)
            .filter(|(i, b)| **b == data[i % period])
            .count();
        let total = data.len() - period;
        if total > 0 && matches as f64 / total as f64 > 0.95 {
            return Some(period);
        }
    }
    None
}

// ══════════════════════════════════════════════════════════════
// Pass 4: Key extraction from register dumps
// ══════════════════════════════════════════════════════════════

fn pass_4_key_extraction(events: &[FridaEvent]) {
    println!("\n╔══════════════════════════════════════════════════════════════╗");
    println!("║  Pass 4: Key Extraction from Register Dumps                ║");
    println!("╚══════════════════════════════════════════════════════════════╝\n");

    let mut key_candidates: Vec<(usize, String, Vec<u8>)> = Vec::new();

    for (i, ev) in events.iter().enumerate() {
        // Look for register values that might be key pointers
        for key_name in [
            "arg0_ptr", "arg1_ptr", "arg2_ptr", "arg3_ptr", "rcx", "rdx", "r8", "r9",
        ] {
            if let Some(val) = ev.fields.get(key_name)
                && let Ok(addr) = u64::from_str_radix(val.trim_start_matches("0x"), 16)
            {
                // Check if there's hex data associated
                let hex_key = format!("{}_hex", key_name);
                if let Some(hex_data) = ev.fields.get(&hex_key) {
                    let bytes = decode_hex(hex_data);
                    if bytes.len() >= 16 {
                        let ent = entropy(&bytes[..16]);
                        if ent > 2.0 && ent < 7.0 {
                            key_candidates.push((i, format!("{} @ 0x{:X}", key_name, addr), bytes));
                        }
                    }
                }
            }
        }

        // Check for explicit key field
        if let Some(key_hex) = ev.fields.get("key_hex") {
            let bytes = decode_hex(key_hex);
            if bytes.len() >= 16 {
                key_candidates.push((i, "explicit_key".into(), bytes));
            }
        }
    }

    if key_candidates.is_empty() {
        println!("  No key candidates found in register dumps.");
        println!("  Tip: Add key buffer dumps to Frida hooks.\n");
    } else {
        println!("  Found {} key candidates:\n", key_candidates.len());
        for (event_idx, source, bytes) in &key_candidates {
            let ent = entropy(&bytes[..bytes.len().min(32)]);
            println!(
                "    Event #{}, source={}: {} bytes, entropy={:.2}",
                event_idx,
                source,
                bytes.len(),
                ent
            );
            hex_dump(bytes, bytes.len().min(64));
        }
    }
}

// ══════════════════════════════════════════════════════════════
// Pass 5: Auto-decrypt cached .chk files
// ══════════════════════════════════════════════════════════════

fn pass_5_auto_decrypt(events: &[FridaEvent]) {
    println!("\n╔══════════════════════════════════════════════════════════════╗");
    println!("║  Pass 5: Auto-Decrypt Cached Files                         ║");
    println!("╚══════════════════════════════════════════════════════════════╝\n");

    // Check if we have any successful decrypt pairs to learn from
    let successful_pairs: Vec<_> = events
        .iter()
        .filter(|e| {
            !e.output_hex.is_empty() && {
                let (is_hit, _) = detect(&e.output_hex);
                is_hit
            }
        })
        .collect();

    if successful_pairs.is_empty() {
        println!("  No confirmed decrypt pairs available.");
        println!("  Cannot auto-decrypt without known good I/O pair.\n");
        return;
    }

    println!(
        "  {} confirmed decrypt pairs available.",
        successful_pairs.len()
    );

    // Look for cached encrypted files
    let mut encrypted_files = Vec::new();
    for dir_name in ENCRYPTED_DIRS {
        let dir_path = format!("{}/{}", VFS_CACHE_DIR, dir_name);
        if let Ok(entries) = fs::read_dir(&dir_path) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().is_some_and(|e| e == "chk")
                    && let Ok(data) = fs::read(&path)
                {
                    encrypted_files.push((path, data));
                }
            }
        }
    }

    if encrypted_files.is_empty() {
        println!(
            "  No cached encrypted .chk files found in {}",
            VFS_CACHE_DIR
        );
        return;
    }

    println!(
        "  Found {} cached encrypted files.\n",
        encrypted_files.len()
    );

    // Try XOR-based decryption using learned keystream
    println!("  Attempting XOR keystream decryption...");
    for pair in &successful_pairs {
        if pair.input_hex.is_empty() {
            continue;
        }

        let keystream: Vec<u8> = pair
            .input_hex
            .iter()
            .zip(&pair.output_hex)
            .map(|(&a, &b)| a ^ b)
            .collect();

        if keystream.is_empty() {
            continue;
        }

        let ks_ent = entropy(&keystream);
        println!(
            "    Keystream from event: {} bytes, entropy={:.2}",
            keystream.len(),
            ks_ent
        );

        // Try applying this keystream to cached files
        for (path, data) in &encrypted_files {
            if data.len() < keystream.len() {
                continue;
            }

            let decrypted: Vec<u8> = data
                .iter()
                .zip(keystream.iter().cycle())
                .map(|(&d, &k)| d ^ k)
                .collect();

            let (is_hit, reasons) = detect(&decrypted);
            if is_hit {
                println!(
                    "    SUCCESS: {} → {} ({})",
                    path.display(),
                    reasons,
                    format_bytes(data.len() as u64)
                );
                hex_dump(&decrypted, 64);

                // Save decrypted file
                let out_path = path.with_extension("decrypted.chk");
                if let Err(e) = fs::write(&out_path, &decrypted) {
                    println!("      Failed to save: {}", e);
                } else {
                    println!("      Saved to: {}", out_path.display());
                }
            }
        }
    }
}

// ══════════════════════════════════════════════════════════════
// Pass 6: Extract & classify decrypted buffers
// ══════════════════════════════════════════════════════════════

fn pass_6_extract_and_classify(events: &[FridaEvent], output_dir: &str) {
    println!("\n╔══════════════════════════════════════════════════════════════╗");
    println!("║  Pass 6: Extract & Classify Decrypted Buffers              ║");
    println!("╚══════════════════════════════════════════════════════════════╝\n");

    let _ = fs::create_dir_all(output_dir);

    let mut saved = 0u32;
    let mut by_type: HashMap<String, u32> = HashMap::new();

    for (i, ev) in events.iter().enumerate() {
        // Collect potential output buffers
        let result_decoded = ev
            .fields
            .get("result_hex")
            .map(|s| decode_hex(s))
            .unwrap_or_default();

        let mut outputs: Vec<(&str, &[u8])> = Vec::new();
        if ev.output_hex.len() >= 16 {
            outputs.push(("output_hex", &ev.output_hex));
        }
        if result_decoded.len() >= 16 {
            outputs.push(("result", &result_decoded));
        }

        for (source, data) in &outputs {
            let (is_hit, reasons) = detect(data);
            if !is_hit {
                continue;
            }

            // Classify
            let file_type = if data.len() >= 8 && &data[..8] == b"UnityFS\0" {
                "unityfs"
            } else if data.len() >= 4 && &data[..4] == b"CAB-" {
                "cab"
            } else if data.len() >= 4 && &data[..4] == b"AKPK" {
                "akpk"
            } else if data.len() >= 4 && &data[..4] == b"\x89PNG" {
                "png"
            } else if data.len() >= 2 && data[0] == b'{' {
                "json"
            } else if data.len() >= 2 && data[0] == b'[' {
                "json_array"
            } else if looks_like_flatbuffer(data) {
                "flatbuf"
            } else if vfs_decrypt::is_valid_header(data) {
                "vfs"
            } else if ascii_ratio(&data[..data.len().min(256)]) > 0.80 {
                "text"
            } else {
                "binary"
            };

            *by_type.entry(file_type.to_string()).or_insert(0) += 1;

            let ext = match file_type {
                "unityfs" => "unity3d",
                "json" | "json_array" => "json",
                "png" => "png",
                "text" => "txt",
                "flatbuf" => "fbs",
                "vfs" => "vfs",
                _ => "bin",
            };

            let filename = format!("event_{:04}_{}.{}", i, file_type, ext);
            let path = format!("{}/{}", output_dir, filename);

            if let Err(e) = fs::write(&path, data) {
                println!("  ERROR saving {}: {}", filename, e);
            } else {
                saved += 1;
                println!(
                    "  Saved: {} ({}, {}) [{}]",
                    filename,
                    format_bytes(data.len() as u64),
                    reasons,
                    source,
                );
            }
        }
    }

    println!("\n  Extracted {} files to {}/", saved, output_dir);
    for (t, c) in &by_type {
        println!("    {:12} : {}", t, c);
    }
    if saved == 0 {
        println!("  No recognizable decrypted data found in events.");
        println!("  Tip: Play the game and load different areas to trigger encrypted dir reads.");
    }
}

// ══════════════════════════════════════════════════════════════
// Pass 7: Stat table scan in decrypted data
// ══════════════════════════════════════════════════════════════

fn pass_7_stat_scan(events: &[FridaEvent], output_dir: &str) {
    println!("\n╔══════════════════════════════════════════════════════════════╗");
    println!("║  Pass 7: Stat Table Scan                                    ║");
    println!("╚══════════════════════════════════════════════════════════════╝\n");

    // Scan all decrypted output for stat-like patterns
    let stat_keywords = [
        "maxhp",
        "max_hp",
        "basehp",
        "base_hp",
        "health",
        "atk",
        "attack",
        "baseatk",
        "base_atk",
        "def",
        "defense",
        "basedef",
        "base_def",
        "crit",
        "critrate",
        "crit_rate",
        "critdmg",
        "hp_base",
        "atk_base",
        "def_base",
        "leveldata",
        "growthcurve",
        "stattable",
        "CharacterStat",
        "EnemyStat",
        "LevelUp",
    ];

    let mut hits: Vec<(usize, String, String)> = Vec::new(); // (event_idx, keyword, context)

    for (i, ev) in events.iter().enumerate() {
        // Check output bytes as potential text
        for data in [&ev.output_hex, &ev.input_hex] {
            if data.len() < 32 {
                continue;
            }

            // Try interpreting as UTF-8 text
            if let Ok(text) = std::str::from_utf8(data) {
                let lower = text.to_lowercase();
                for kw in &stat_keywords {
                    if lower.contains(&kw.to_lowercase()) {
                        let pos = lower.find(&kw.to_lowercase()).unwrap();
                        let start = pos.saturating_sub(30);
                        let end = (pos + kw.len() + 30).min(text.len());
                        let context = &text[start..end];
                        hits.push((i, kw.to_string(), context.to_string()));
                    }
                }
            }

            // Also check for UTF-16 strings (Windows/Unity common)
            if data.len() >= 64 {
                let mut utf16_buf = Vec::new();
                for chunk in data.chunks(2) {
                    if chunk.len() == 2 {
                        let ch = u16::from_le_bytes([chunk[0], chunk[1]]);
                        if ch > 0 && ch < 128 {
                            utf16_buf.push(ch as u8 as char);
                        } else if ch == 0 && !utf16_buf.is_empty() {
                            let s: String = utf16_buf.iter().collect();
                            let lower = s.to_lowercase();
                            for kw in &stat_keywords {
                                if lower.contains(&kw.to_lowercase()) {
                                    hits.push((i, kw.to_string(), s.clone()));
                                }
                            }
                            utf16_buf.clear();
                        } else {
                            utf16_buf.clear();
                        }
                    }
                }
            }
        }
    }

    // Also scan files saved by Pass 6
    if let Ok(entries) = fs::read_dir(output_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if let Ok(data) = fs::read(&path) {
                // Check for FlatBuffer stat tables
                if looks_like_flatbuffer(&data) {
                    println!(
                        "  FlatBuffer detected: {} ({})",
                        path.display(),
                        format_bytes(data.len() as u64)
                    );
                    // Scan for stat keywords in raw bytes
                    if let Ok(text) = std::str::from_utf8(&data) {
                        let lower = text.to_lowercase();
                        for kw in &stat_keywords {
                            if lower.contains(&kw.to_lowercase()) {
                                println!("    STAT_KEYWORD: '{}' found in {}", kw, path.display());
                            }
                        }
                    }
                }

                // Check JSON files for stat fields
                if path.extension().is_some_and(|e| e == "json")
                    && let Ok(text) = std::str::from_utf8(&data)
                    && let Ok(val) = serde_json::from_str::<serde_json::Value>(text)
                {
                    scan_json_for_stats(&val, &path.display().to_string());
                }
            }
        }
    }

    if hits.is_empty() {
        println!("  No stat keywords found in decrypted event data.");
        println!("  This is expected — stats are likely in FlatBuffer or binary format.");
        println!(
            "  Check the extracted files in {} for FlatBuffer tables.",
            output_dir
        );
    } else {
        println!("  Found {} stat keyword matches:\n", hits.len());
        for (idx, kw, ctx) in &hits {
            println!("    Event #{}: '{}' in context: \"{}\"", idx, kw, ctx);
        }
    }
}

fn scan_json_for_stats(val: &serde_json::Value, source: &str) {
    match val {
        serde_json::Value::Object(map) => {
            let stat_keys = [
                "hp",
                "atk",
                "def",
                "health",
                "attack",
                "defense",
                "max_hp",
                "base_atk",
                "base_def",
                "crit_rate",
                "crit_dmg",
                "level",
                "exp",
                "growth",
            ];

            for (k, v) in map {
                let lower = k.to_lowercase();
                for sk in &stat_keys {
                    if lower.contains(sk) {
                        println!("    JSON_STAT: {}.'{}' = {} (in {})", source, k, v, source);
                    }
                }
                // Recurse
                scan_json_for_stats(v, source);
            }
        }
        serde_json::Value::Array(arr) => {
            for item in arr {
                scan_json_for_stats(item, source);
            }
        }
        _ => {}
    }
}

// ══════════════════════════════════════════════════════════════
// Main
// ══════════════════════════════════════════════════════════════

fn main() {
    println!("╔══════════════════════════════════════════════════════════════╗");
    println!("║  Phase P: Frida Capture Companion                          ║");
    println!("║  Parse + Analyze Runtime Intercept Logs                     ║");
    println!("╚══════════════════════════════════════════════════════════════╝\n");

    let args: Vec<String> = std::env::args().collect();

    if args.len() < 2 {
        eprintln!("Usage: {} <capture.log> [--output-dir <dir>]", args[0]);
        eprintln!();
        eprintln!("Parses Frida capture logs produced by frida_hooks.js");
        eprintln!("and analyzes decrypt I/O pairs.");
        eprintln!();
        eprintln!("Expected log format:");
        eprintln!("  === FRIDA_EVENT type=DECRYPT_CALL ===");
        eprintln!("  timestamp=...");
        eprintln!("  func_addr=0x...");
        eprintln!("  arg0_hex=...");
        eprintln!("  result_hex=...");
        eprintln!("  === END_EVENT ===");
        std::process::exit(1);
    }

    let log_path = &args[1];
    let content = match fs::read_to_string(log_path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("ERROR: Cannot read {}: {}", log_path, e);
            std::process::exit(1);
        }
    };

    println!(
        "  Loaded: {} ({})",
        log_path,
        format_bytes(content.len() as u64)
    );

    // Pass 1: Parse
    let events = pass_1_parse(&content);

    if events.is_empty() {
        println!("\n  No events found. Check log format.");
        return;
    }

    // Pass 2: Validate outputs
    pass_2_validate(&events);

    // Pass 3: Cipher analysis
    pass_3_cipher_analysis(&events);

    // Pass 4: Key extraction
    pass_4_key_extraction(&events);

    // Pass 5: Auto-decrypt
    pass_5_auto_decrypt(&events);

    // Pass 6: Extract & classify decrypted buffers
    let output_dir = args
        .iter()
        .position(|a| a == "--output-dir")
        .and_then(|i| args.get(i + 1))
        .map(|s| s.as_str())
        .unwrap_or("./output/frida_decrypted");
    pass_6_extract_and_classify(&events, output_dir);

    // Pass 7: Stat table scan
    pass_7_stat_scan(&events, output_dir);

    println!("\n╔══════════════════════════════════════════════════════════════╗");
    println!("║  Frida Companion Complete                                   ║");
    println!("╚══════════════════════════════════════════════════════════════╝");
}
