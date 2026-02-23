/// Systematic decryption probe for 4 encrypted VFS directories.
/// These directories don't have VFS headers and their encryption is unknown.
/// Runs a battery of decryption strategies and reports any hits.
use backend::core::endfield::{
    config::{ResolveParams, resolve_game_config},
    constants::{app_code, channel, sub_channel},
};
use backend::core::hypergryph::launcher::Platform;
use backend::extraction::crypto_constants::*;
use backend::extraction::vfs_aes;
use backend::extraction::vfs_decrypt;
use backend::extraction::wwise_pck;
use backend::extraction::zip_range::{SplitArchivePart, ZipCentralDirectory};

use aes::Aes128;
use aes::cipher::{BlockDecrypt, BlockEncrypt, KeyInit, generic_array::GenericArray};
use reqwest::Client;

/// Target encrypted directories: (name, hash_as_u32)
const TARGETS: &[(&str, u32)] = &[
    ("19E3AE45", 0x19E3AE45),
    ("1CDDBF1F", 0x1CDDBF1F),
    ("42A8FCA6", 0x42A8FCA6),
    ("775A31D1", 0x775A31D1),
];

/// Only probe the first N bytes of each file for speed.
const PROBE_SIZE: usize = 4096;

/// Wwise cipher constants (private in wwise_pck, replicated here for probing).
const WWISE_CIPHER_XOR: u32 = 0x9C5A0B29;
const WWISE_CIPHER_MUL: u32 = 0x04E11C23;

// ══════════════════════════════════════════════════════════════
// Detection
// ══════════════════════════════════════════════════════════════

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

/// Check if decrypted output looks promising.
/// Returns (is_hit, description).
///
/// Detection is deliberately strict to avoid false positives from random seeds
/// that happen to produce `{` or `[` as the first byte. A single first-byte
/// match is NOT enough — we require corroborating evidence (low entropy, high
/// ASCII ratio, etc.).
fn detect(data: &[u8]) -> (bool, String) {
    let sample = &data[..data.len().min(1024)];
    let ent = entropy(sample);
    let ascii32 = ascii_ratio(&data[..data.len().min(32)]);
    let ascii256 = ascii_ratio(&data[..data.len().min(256)]);
    let zeros = max_zero_run(sample);
    let fb = looks_like_flatbuffer(data);

    let mut reasons = Vec::new();
    let mut confidence = 0u32; // need >= 2 for a real hit

    // Strong signals (confidence += 2, enough on their own)
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

    // Medium signals (confidence += 1, need corroboration)
    if ent < 5.0 {
        reasons.push(format!("entropy={:.2}", ent));
        confidence += 2; // very low entropy is strong on its own
    } else if ent < 6.5 {
        reasons.push(format!("entropy={:.2}", ent));
        confidence += 1;
    }

    if ascii256 > 0.80 {
        reasons.push(format!("ascii256={:.0}%", ascii256 * 100.0));
        confidence += 2; // 80%+ ASCII over 256 bytes is very strong
    } else if ascii32 > 0.75 {
        reasons.push(format!("ascii32={:.0}%", ascii32 * 100.0));
        confidence += 1;
    }

    if zeros > 16 {
        reasons.push(format!("zero_run={}", zeros));
        confidence += 1;
    }

    if fb {
        // FlatBuffer check: root offset points to valid vtable area
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

    // JSON: require first byte + substantial ASCII follow-through
    if !data.is_empty() && (data[0] == b'{' || data[0] == b'[') && ascii32 > 0.60 {
        reasons.push("JSON-like".into());
        confidence += 1;
    }

    let is_hit = confidence >= 2;
    (is_hit, reasons.join(", "))
}

/// Run detection and print if hit. Returns true if hit.
fn check_and_report(label: &str, data: &[u8]) -> bool {
    let (hit, desc) = detect(data);
    if hit {
        println!("  *** HIT [{}]: {}", label, desc);
        hex_dump(data, 64);
    }
    hit
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

// ══════════════════════════════════════════════════════════════
// Probe 1: XOR-Mul with extended seeds
// ══════════════════════════════════════════════════════════════

fn probe_xor_mul(data: &[u8], dir_hash: u32) -> bool {
    println!("  [1] XOR-Mul with extended seeds...");

    let seeds: Vec<(u32, &str)> = vec![
        // Directory hash itself
        (dir_hash, "dir_hash"),
        // All known crypto constants
        (HEADER_XOR_A, "HEADER_XOR_A"),
        (HEADER_XOR_B, "HEADER_XOR_B"),
        (CBI_UBI_FINAL_XOR, "CBI_UBI_FINAL_XOR"),
        (FLAGS_XOR, "FLAGS_XOR"),
        (BLOCKS_COUNT_READ_XOR, "BLOCKS_COUNT_READ_XOR"),
        (BLOCKS_COUNT_FINAL_XOR, "BLOCKS_COUNT_FINAL_XOR"),
        (NODES_COUNT_READ_XOR, "NODES_COUNT_READ_XOR"),
        (NODES_COUNT_FINAL_XOR, "NODES_COUNT_FINAL_XOR"),
        (NODE_A_XOR, "NODE_A_XOR"),
        (NODE_FLAGS_XOR, "NODE_FLAGS_XOR"),
        (SIZE_MERGE_XOR, "SIZE_MERGE_XOR"),
        // Wwise constants
        (WWISE_CIPHER_XOR, "WWISE_CIPHER_XOR"),
        (WWISE_CIPHER_MUL, "WWISE_CIPHER_MUL"),
        // Common test values
        (0, "zero"),
        (1, "one"),
        (0xDEADBEEF, "DEADBEEF"),
        (0x12345678, "12345678"),
        (data.len() as u32, "file_size"),
        // VFS XOR key halves
        ((VFS_XOR_KEY >> 32) as u32, "VFS_XOR_KEY_hi"),
        (VFS_XOR_KEY as u32, "VFS_XOR_KEY_lo"),
    ];

    // Also add sequential 1-256
    let mut all_seeds = seeds;
    for i in 2..=256u32 {
        all_seeds.push((i, "seq"));
    }

    let probe = &data[..data.len().min(PROBE_SIZE)];
    let mut found = false;

    for (seed, label) in &all_seeds {
        let mut buf = probe.to_vec();
        wwise_pck::decipher_inplace(&mut buf, *seed, 0);
        if check_and_report(&format!("XOR-Mul seed=0x{:08X} ({})", seed, label), &buf) {
            found = true;
        }
    }

    if !found {
        println!("    No hits ({} seeds tested)", all_seeds.len());
    }
    found
}

// ══════════════════════════════════════════════════════════════
// Probe 2: Multi-byte repeating XOR
// ══════════════════════════════════════════════════════════════

fn cyclic_xor(data: &[u8], key: &[u8]) -> Vec<u8> {
    data.iter()
        .enumerate()
        .map(|(i, &b)| b ^ key[i % key.len()])
        .collect()
}

fn probe_multi_byte_xor(data: &[u8], dir_hash: u32) -> bool {
    println!("  [2] Multi-byte repeating XOR...");

    let probe = &data[..data.len().min(PROBE_SIZE)];

    // VFS AES KEY as 16-byte XOR pad
    let key_xor_iv: Vec<u8> = VFS_AES_KEY
        .iter()
        .zip(VFS_AES_IV.iter())
        .map(|(&a, &b)| a ^ b)
        .collect();
    let vfs_xor_bytes = VFS_XOR_KEY.to_le_bytes();
    let dir_hash_bytes = dir_hash.to_le_bytes();

    let keys: Vec<(&[u8], &str)> = vec![
        (&VFS_AES_KEY, "VFS_AES_KEY"),
        (&VFS_AES_IV, "VFS_AES_IV"),
        (key_xor_iv.as_slice(), "KEY_XOR_IV"),
        (&vfs_xor_bytes, "VFS_XOR_KEY_8b"),
        (&dir_hash_bytes, "dir_hash_4b"),
    ];

    let mut found = false;
    for (key, label) in &keys {
        let result = cyclic_xor(probe, key);
        if check_and_report(&format!("XOR-repeat {}", label), &result) {
            found = true;
        }
    }

    if !found {
        println!("    No hits ({} keys tested)", keys.len());
    }
    found
}

// ══════════════════════════════════════════════════════════════
// Probe 3: Standard AES-128 ECB/CBC/CTR (Rijndael S-box)
// ══════════════════════════════════════════════════════════════

fn probe_standard_aes(data: &[u8]) -> bool {
    println!("  [3] Standard AES-128 (Rijndael S-box)...");

    if data.len() < 16 {
        println!("    Data too short for AES");
        return false;
    }

    let key = GenericArray::from_slice(&VFS_AES_KEY);
    let cipher = Aes128::new(key);
    let probe = &data[..data.len().min(PROBE_SIZE)];
    let mut found = false;

    // ECB: decrypt all blocks
    if probe.len() >= 32 {
        let block_count = probe.len() / 16;
        let mut buf = Vec::with_capacity(block_count * 16);
        for i in 0..block_count {
            let mut block = GenericArray::clone_from_slice(&probe[i * 16..(i + 1) * 16]);
            cipher.decrypt_block(&mut block);
            buf.extend_from_slice(&block);
        }
        if check_and_report("AES-128-ECB all blocks", &buf) {
            found = true;
        }
    }

    // CBC: decrypt with VFS_AES_IV
    if probe.len() >= 32 {
        let block_count = probe.len() / 16;
        let mut buf = Vec::with_capacity(block_count * 16);
        let mut prev = VFS_AES_IV;
        for i in 0..block_count {
            let ct_block = &probe[i * 16..(i + 1) * 16];
            let mut block = GenericArray::clone_from_slice(ct_block);
            cipher.decrypt_block(&mut block);
            let pt: Vec<u8> = block
                .iter()
                .zip(prev.iter())
                .map(|(&a, &b)| a ^ b)
                .collect();
            buf.extend_from_slice(&pt);
            prev.copy_from_slice(ct_block);
        }
        if check_and_report("AES-128-CBC KEY+IV", &buf) {
            found = true;
        }
    }

    // CBC: decrypt with zero IV
    if probe.len() >= 32 {
        let block_count = probe.len() / 16;
        let mut buf = Vec::with_capacity(block_count * 16);
        let mut prev = [0u8; 16];
        for i in 0..block_count {
            let ct_block = &probe[i * 16..(i + 1) * 16];
            let mut block = GenericArray::clone_from_slice(ct_block);
            cipher.decrypt_block(&mut block);
            let pt: Vec<u8> = block
                .iter()
                .zip(prev.iter())
                .map(|(&a, &b)| a ^ b)
                .collect();
            buf.extend_from_slice(&pt);
            prev.copy_from_slice(ct_block);
        }
        if check_and_report("AES-128-CBC KEY+zeroIV", &buf) {
            found = true;
        }
    }

    // CTR: standard counter mode with VFS_AES_IV as nonce
    if probe.len() >= 32 {
        let block_count = probe.len() / 16;
        let mut buf = Vec::with_capacity(block_count * 16);
        for i in 0..block_count {
            // Counter = IV with last 4 bytes as LE counter
            let mut ctr_block = VFS_AES_IV;
            let ctr_val = (i as u32).to_le_bytes();
            ctr_block[12] = ctr_val[0];
            ctr_block[13] = ctr_val[1];
            ctr_block[14] = ctr_val[2];
            ctr_block[15] = ctr_val[3];

            let mut keystream = GenericArray::clone_from_slice(&ctr_block);
            cipher.encrypt_block(&mut keystream);

            let ct = &probe[i * 16..(i + 1) * 16];
            let pt: Vec<u8> = ct
                .iter()
                .zip(keystream.iter())
                .map(|(&a, &b)| a ^ b)
                .collect();
            buf.extend_from_slice(&pt);
        }
        if check_and_report("AES-128-CTR KEY+IV(std)", &buf) {
            found = true;
        }
    }

    // CTR: big-endian counter in last 4 bytes
    if probe.len() >= 32 {
        let block_count = probe.len() / 16;
        let mut buf = Vec::with_capacity(block_count * 16);
        for i in 0..block_count {
            let mut ctr_block = VFS_AES_IV;
            let ctr_val = (i as u32).to_be_bytes();
            ctr_block[12] = ctr_val[0];
            ctr_block[13] = ctr_val[1];
            ctr_block[14] = ctr_val[2];
            ctr_block[15] = ctr_val[3];

            let mut keystream = GenericArray::clone_from_slice(&ctr_block);
            cipher.encrypt_block(&mut keystream);

            let ct = &probe[i * 16..(i + 1) * 16];
            let pt: Vec<u8> = ct
                .iter()
                .zip(keystream.iter())
                .map(|(&a, &b)| a ^ b)
                .collect();
            buf.extend_from_slice(&pt);
        }
        if check_and_report("AES-128-CTR KEY+IV(BE)", &buf) {
            found = true;
        }
    }

    if !found {
        println!("    No hits (ECB, CBC, CTR tested)");
    }
    found
}

// ══════════════════════════════════════════════════════════════
// Probe 4: S-box substitution (no full AES, just byte mapping)
// ══════════════════════════════════════════════════════════════

fn build_inverse_sbox() -> [u8; 256] {
    let mut inv = [0u8; 256];
    for (i, &v) in VFS_AES_SBOX.iter().enumerate() {
        inv[v as usize] = i as u8;
    }
    inv
}

fn probe_sbox_sub(data: &[u8]) -> bool {
    println!("  [4] S-box byte substitution...");

    let probe = &data[..data.len().min(PROBE_SIZE)];
    let inv_sbox = build_inverse_sbox();
    let mut found = false;

    // Forward S-box
    let forward: Vec<u8> = probe.iter().map(|&b| VFS_AES_SBOX[b as usize]).collect();
    if check_and_report("forward S-box", &forward) {
        found = true;
    }

    // Inverse S-box
    let inverse: Vec<u8> = probe.iter().map(|&b| inv_sbox[b as usize]).collect();
    if check_and_report("inverse S-box", &inverse) {
        found = true;
    }

    if !found {
        println!("    No hits");
    }
    found
}

// ══════════════════════════════════════════════════════════════
// Probe 5: Custom VFS AES-CTR with alternative IVs
// ══════════════════════════════════════════════════════════════

fn probe_custom_aes_varied(data: &[u8], dir_hash: u32) -> bool {
    println!("  [5] Custom VFS AES-CTR with varied IVs...");

    let probe = &data[..data.len().min(PROBE_SIZE)];
    let mut found = false;

    // Standard VFS key+IV (should have been tried before, but confirm)
    {
        let mut buf = probe.to_vec();
        vfs_aes::decrypt(&mut buf, &VFS_AES_KEY, &VFS_AES_IV);
        if check_and_report("custom AES-CTR KEY+IV", &buf) {
            found = true;
        }
    }

    // Zero IV
    {
        let mut buf = probe.to_vec();
        let zero_iv = [0u8; 16];
        vfs_aes::decrypt(&mut buf, &VFS_AES_KEY, &zero_iv);
        if check_and_report("custom AES-CTR KEY+zeroIV", &buf) {
            found = true;
        }
    }

    // Dir hash as IV (repeated to fill 16 bytes)
    {
        let mut buf = probe.to_vec();
        let hash_bytes = dir_hash.to_le_bytes();
        let mut hash_iv = [0u8; 16];
        for i in 0..16 {
            hash_iv[i] = hash_bytes[i % 4];
        }
        vfs_aes::decrypt(&mut buf, &VFS_AES_KEY, &hash_iv);
        if check_and_report("custom AES-CTR KEY+hashIV", &buf) {
            found = true;
        }
    }

    // VFS scatter/gather decrypt (the mode used for VFS blocks)
    {
        let mut buf = probe.to_vec();
        vfs_decrypt::decrypt_block(&mut buf);
        if check_and_report("VFS scatter/gather decrypt", &buf) {
            found = true;
        }
    }

    if !found {
        println!("    No hits");
    }
    found
}

// ══════════════════════════════════════════════════════════════
// Probe 6: Offset probing (skip potential header)
// ══════════════════════════════════════════════════════════════

fn probe_with_offsets(data: &[u8], dir_hash: u32) -> bool {
    println!("  [6] Offset probing (skip header bytes)...");

    let offsets = [4, 8, 16, 32, 42, 48, 64, 128, 256];
    let mut found = false;

    for &off in &offsets {
        if off >= data.len() {
            continue;
        }
        let slice = &data[off..];
        let sample = &slice[..slice.len().min(1024)];
        let ent = entropy(sample);

        // Check raw data at this offset
        let (raw_hit, raw_desc) = detect(sample);
        if raw_hit {
            println!("  *** HIT [raw @ offset {}]: {}", off, raw_desc);
            hex_dump(sample, 64);
            found = true;
        }

        // Only run expensive probes if entropy differs from offset 0
        let base_ent = entropy(&data[..data.len().min(1024)]);
        if (ent - base_ent).abs() > 0.5 {
            println!(
                "    offset {} entropy={:.2} (base={:.2}) — trying probes",
                off, ent, base_ent
            );

            // XOR-Mul with dir hash at this offset
            let probe_slice = &slice[..slice.len().min(PROBE_SIZE)];
            let mut buf = probe_slice.to_vec();
            wwise_pck::decipher_inplace(&mut buf, dir_hash, 0);
            if check_and_report(&format!("XOR-Mul(dir_hash)@{}", off), &buf) {
                found = true;
            }

            // VFS AES-CTR at this offset
            let mut buf = probe_slice.to_vec();
            vfs_aes::decrypt(&mut buf, &VFS_AES_KEY, &VFS_AES_IV);
            if check_and_report(&format!("custom-AES-CTR@{}", off), &buf) {
                found = true;
            }

            // Standard AES-128-ECB at this offset
            if probe_slice.len() >= 16 {
                let key = GenericArray::from_slice(&VFS_AES_KEY);
                let aes_cipher = Aes128::new(key);
                let block_count = probe_slice.len().min(256) / 16;
                let mut buf = Vec::with_capacity(block_count * 16);
                for i in 0..block_count {
                    let mut block =
                        GenericArray::clone_from_slice(&probe_slice[i * 16..(i + 1) * 16]);
                    aes_cipher.decrypt_block(&mut block);
                    buf.extend_from_slice(&block);
                }
                if check_and_report(&format!("AES-ECB@{}", off), &buf) {
                    found = true;
                }
            }
        }
    }

    if !found {
        println!("    No hits at any offset");
    }
    found
}

// ══════════════════════════════════════════════════════════════
// Cross-directory comparison
// ══════════════════════════════════════════════════════════════

fn cross_compare(files: &[(&str, &[u8])]) {
    println!("\n{}", "=".repeat(60));
    println!("Cross-Directory Comparison");
    println!("{}", "=".repeat(60));

    if files.len() < 2 {
        println!("  Need at least 2 files to compare");
        return;
    }

    // Compare first 16 bytes across all files
    println!("\n  First 16 bytes of each file:");
    for (name, data) in files {
        print!("    {}: ", name);
        for &b in &data[..data.len().min(16)] {
            print!("{:02x} ", b);
        }
        println!();
    }

    // Check if any files share the same first N bytes
    for n in [4, 8, 16, 32] {
        let mut all_same = true;
        let first = &files[0].1[..files[0].1.len().min(n)];
        for (_, data) in &files[1..] {
            if data.len() < n || &data[..n] != first {
                all_same = false;
                break;
            }
        }
        if all_same {
            println!("  First {} bytes IDENTICAL across all files", n);
        }
    }

    // Byte frequency distribution comparison
    println!("\n  Byte frequency analysis (first 4KB):");
    for (name, data) in files {
        let sample = &data[..data.len().min(4096)];
        let ent = entropy(sample);
        let mut freq = [0u32; 256];
        for &b in sample {
            freq[b as usize] += 1;
        }
        let unique = freq.iter().filter(|&&c| c > 0).count();
        let mut top: Vec<(u8, u32)> = freq
            .iter()
            .enumerate()
            .filter(|(_, c)| **c > 0)
            .map(|(i, c)| (i as u8, *c))
            .collect();
        top.sort_by(|a, b| b.1.cmp(&a.1));
        println!(
            "    {} entropy={:.2} unique={}/256 top3=[0x{:02X}={}, 0x{:02X}={}, 0x{:02X}={}]",
            name,
            ent,
            unique,
            top.first().map_or(0, |t| t.0),
            top.first().map_or(0, |t| t.1),
            top.get(1).map_or(0, |t| t.0),
            top.get(1).map_or(0, |t| t.1),
            top.get(2).map_or(0, |t| t.0),
            top.get(2).map_or(0, |t| t.1),
        );
    }

    // XOR files pairwise and check if result has lower entropy
    println!("\n  Pairwise XOR entropy:");
    for i in 0..files.len() {
        for j in (i + 1)..files.len() {
            let len = files[i].1.len().min(files[j].1.len()).min(4096);
            let xored: Vec<u8> = files[i].1[..len]
                .iter()
                .zip(files[j].1[..len].iter())
                .map(|(&a, &b)| a ^ b)
                .collect();
            let ent = entropy(&xored);
            let zeros = max_zero_run(&xored);
            println!(
                "    {} ^ {} = entropy={:.2} max_zero_run={}{}",
                files[i].0,
                files[j].0,
                ent,
                zeros,
                if ent < 6.0 { " ← LOW" } else { "" }
            );
        }
    }
}

// ══════════════════════════════════════════════════════════════
// File loading with download fallback
// ══════════════════════════════════════════════════════════════

fn load_cached(dir: &str) -> Option<Vec<u8>> {
    let path = format!("./output/{}_smallest.chk", dir);
    std::fs::read(&path).ok()
}

async fn download_file(client: &Client, cd: &ZipCentralDirectory, dir: &str) -> Option<Vec<u8>> {
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
            Ok(data) => {
                let save_path = format!("./output/{}_smallest.chk", dir);
                let _ = std::fs::write(&save_path, &data);
                println!("    Saved to cache");
                return Some(data);
            }
            Err(e) => println!("    Download error: {}", e),
        }
    }
    None
}

// ══════════════════════════════════════════════════════════════
// Main
// ══════════════════════════════════════════════════════════════

fn run_all_probes(name: &str, data: &[u8], dir_hash: u32) {
    println!(
        "\n  Raw file: {} bytes, entropy={:.2}",
        data.len(),
        entropy(&data[..data.len().min(4096)])
    );
    println!("  First 64 bytes:");
    hex_dump(data, 64);
    println!();

    let mut any_hit = false;

    any_hit |= probe_xor_mul(data, dir_hash);
    any_hit |= probe_multi_byte_xor(data, dir_hash);
    any_hit |= probe_standard_aes(data);
    any_hit |= probe_sbox_sub(data);
    any_hit |= probe_custom_aes_varied(data, dir_hash);
    any_hit |= probe_with_offsets(data, dir_hash);

    if !any_hit {
        println!("\n  ✗ ALL PROBES FAILED for {}", name);
        println!("  → Encryption is likely a completely custom cipher.");
        println!("  → Next step: Reverse engineer GameAssembly.dll");
    } else {
        println!("\n  ✓ HITS FOUND for {} — investigate above results!", name);
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("=== Encrypted Directory Decryption Probe ===\n");

    let output_dir = "./output";
    std::fs::create_dir_all(output_dir)?;

    // Try to load all 4 files from cache first
    let mut loaded: Vec<(&str, u32, Vec<u8>)> = Vec::new();
    let mut need_download: Vec<(&str, u32)> = Vec::new();

    for &(name, hash) in TARGETS {
        if let Some(data) = load_cached(name) {
            println!(
                "[✓] {} loaded from cache ({})",
                name,
                format_bytes(data.len() as u64)
            );
            loaded.push((name, hash, data));
        } else {
            need_download.push((name, hash));
        }
    }

    // Download missing files
    if !need_download.is_empty() {
        println!("\n[*] {} files need downloading...", need_download.len());

        let client = Client::new();
        let params = ResolveParams {
            app_code: app_code::game::OS_WIN_REL,
            launcher_app_code: app_code::launcher::OS_WIN_REL,
            channel: channel::OS_WIN_REL,
            sub_channel: sub_channel::OS_WIN_REL,
            launcher_sub_channel: sub_channel::OS_WIN_REL,
            platform: Platform::Windows,
        };

        println!("  Resolving game config...");
        let config = resolve_game_config(&client, &params).await?;
        println!("  Version: {}", config.version);

        let parts: Vec<SplitArchivePart> = config
            .packs
            .iter()
            .map(|p| SplitArchivePart {
                url: p.url.clone(),
                size: p.package_size.parse::<u64>().expect("invalid package_size"),
            })
            .collect();
        let cd = ZipCentralDirectory::from_split_archive(&client, parts).await?;
        println!("  ZIP entries: {}", cd.len());

        for (name, hash) in need_download {
            println!("\n  Downloading {}...", name);
            match download_file(&client, &cd, name).await {
                Some(data) => {
                    println!("  Got {} bytes", data.len());
                    loaded.push((name, hash, data));
                }
                None => {
                    println!("  SKIP: no .chk file found for {}", name);
                }
            }
        }
    }

    // Run probes on each file
    println!("\n{}", "=".repeat(60));
    println!("Running Probe Battery");
    println!("{}", "=".repeat(60));

    for (name, hash, data) in &loaded {
        println!("\n╔══════════════════════════════════════════════════╗");
        println!("║  {} (0x{:08X})  ║", name, hash);
        println!("╚══════════════════════════════════════════════════╝");
        run_all_probes(name, data, *hash);
    }

    // Cross-directory comparison
    if loaded.len() >= 2 {
        let refs: Vec<(&str, &[u8])> = loaded.iter().map(|(n, _, d)| (*n, d.as_slice())).collect();
        cross_compare(&refs);
    }

    println!("\n=== Probe Complete ===");
    Ok(())
}
