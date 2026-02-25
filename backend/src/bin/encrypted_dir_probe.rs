/// Phase O — Deep analysis of 4 encrypted VFS directories.
/// 8-pass binary: enumeration, structural analysis, S-box cipher battery,
/// alternative ciphers (TEA/XTEA/RC4/ChaCha20), known-plaintext attack,
/// statistical deep dive, cross-directory correlation, cipher fingerprinting.
use backend::core::endfield::{
    config::{ResolveParams, resolve_game_config},
    constants::{app_code, channel, sub_channel},
};
use backend::core::hypergryph::launcher::Platform;
use backend::extraction::crypto_constants::*;
use backend::extraction::vfs_decrypt;
use backend::extraction::wwise_pck;
use backend::extraction::zip_range::{SplitArchivePart, ZipCentralDirectory};

use reqwest::Client;

// ══════════════════════════════════════════════════════════════
// Constants
// ══════════════════════════════════════════════════════════════

const TARGETS: &[(&str, u32)] = &[
    ("19E3AE45", 0x19E3AE45),
    ("1CDDBF1F", 0x1CDDBF1F),
    ("42A8FCA6", 0x42A8FCA6),
    ("775A31D1", 0x775A31D1),
];

const PROBE_SIZE: usize = 4096;

/// S-box offsets in GameAssembly.dll .Sgxm1 section.
const SBOX_OFFSETS: [u64; 6] = [
    0x0FF6C960, 0x0FF6CA60, 0x0FF6CB60, 0x0FF6CD00, 0x0FF6CE00, 0x0FF6CF00,
];

/// Known reference VFS directory for plaintext extraction.
const REFERENCE_VFS_DIR: &str = "0CE8FA57";

// ══════════════════════════════════════════════════════════════
// Detection (from decrypt_probe.rs)
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
// Parameterized AES (custom S-box)
// ══════════════════════════════════════════════════════════════

fn key_expansion(key: &[u8; 16], sbox: &[u8; 256]) -> [[u8; 16]; 11] {
    let mut round_keys = [[0u8; 16]; 11];
    round_keys[0] = *key;
    for i in 1..11 {
        let prev = &round_keys[i - 1];
        let mut next = [0u8; 16];
        let rot = [
            sbox[prev[13] as usize] ^ RCON[i],
            sbox[prev[14] as usize],
            sbox[prev[15] as usize],
            sbox[prev[12] as usize],
        ];
        for j in 0..4 {
            next[j] = prev[j] ^ rot[j];
        }
        for j in 4..16 {
            next[j] = prev[j] ^ next[j - 4];
        }
        round_keys[i] = next;
    }
    round_keys
}

fn sub_bytes(state: &mut [u8; 16], sbox: &[u8; 256]) {
    for b in state.iter_mut() {
        *b = sbox[*b as usize];
    }
}

fn shift_rows(state: &mut [u8; 16]) {
    let tmp = state[1];
    state[1] = state[5];
    state[5] = state[9];
    state[9] = state[13];
    state[13] = tmp;
    let (tmp0, tmp1) = (state[2], state[6]);
    state[2] = state[10];
    state[6] = state[14];
    state[10] = tmp0;
    state[14] = tmp1;
    let tmp = state[15];
    state[15] = state[11];
    state[11] = state[7];
    state[7] = state[3];
    state[3] = tmp;
}

fn inv_shift_rows(state: &mut [u8; 16]) {
    let tmp = state[13];
    state[13] = state[9];
    state[9] = state[5];
    state[5] = state[1];
    state[1] = tmp;
    let (tmp0, tmp1) = (state[10], state[14]);
    state[10] = state[2];
    state[14] = state[6];
    state[2] = tmp0;
    state[6] = tmp1;
    let tmp = state[3];
    state[3] = state[7];
    state[7] = state[11];
    state[11] = state[15];
    state[15] = tmp;
}

fn gmul2(a: u8) -> u8 {
    let mut result = (a as u16) << 1;
    if a & 0x80 != 0 {
        result ^= 0x1B;
    }
    result as u8
}

fn gmul(mut a: u8, mut b: u8) -> u8 {
    let mut p: u8 = 0;
    for _ in 0..8 {
        if b & 1 != 0 {
            p ^= a;
        }
        let hi = a & 0x80;
        a <<= 1;
        if hi != 0 {
            a ^= 0x1B;
        }
        b >>= 1;
    }
    p
}

fn mix_columns(state: &mut [u8; 16]) {
    for col in 0..4 {
        let i = col * 4;
        let (a0, a1, a2, a3) = (state[i], state[i + 1], state[i + 2], state[i + 3]);
        state[i] = gmul2(a0) ^ gmul2(a1) ^ a1 ^ a2 ^ a3;
        state[i + 1] = a0 ^ gmul2(a1) ^ gmul2(a2) ^ a2 ^ a3;
        state[i + 2] = a0 ^ a1 ^ gmul2(a2) ^ gmul2(a3) ^ a3;
        state[i + 3] = gmul2(a0) ^ a0 ^ a1 ^ a2 ^ gmul2(a3);
    }
}

fn inv_mix_columns(state: &mut [u8; 16]) {
    for col in 0..4 {
        let i = col * 4;
        let (a0, a1, a2, a3) = (state[i], state[i + 1], state[i + 2], state[i + 3]);
        state[i] = gmul(a0, 14) ^ gmul(a1, 11) ^ gmul(a2, 13) ^ gmul(a3, 9);
        state[i + 1] = gmul(a0, 9) ^ gmul(a1, 14) ^ gmul(a2, 11) ^ gmul(a3, 13);
        state[i + 2] = gmul(a0, 13) ^ gmul(a1, 9) ^ gmul(a2, 14) ^ gmul(a3, 11);
        state[i + 3] = gmul(a0, 11) ^ gmul(a1, 13) ^ gmul(a2, 9) ^ gmul(a3, 14);
    }
}

fn add_round_key(state: &mut [u8; 16], round_key: &[u8; 16]) {
    for i in 0..16 {
        state[i] ^= round_key[i];
    }
}

fn aes_encrypt_block_sbox(
    block: &[u8; 16],
    round_keys: &[[u8; 16]; 11],
    sbox: &[u8; 256],
) -> [u8; 16] {
    let mut state = *block;
    add_round_key(&mut state, &round_keys[0]);
    for round_key in round_keys.iter().take(10).skip(1) {
        sub_bytes(&mut state, sbox);
        shift_rows(&mut state);
        mix_columns(&mut state);
        add_round_key(&mut state, round_key);
    }
    sub_bytes(&mut state, sbox);
    shift_rows(&mut state);
    add_round_key(&mut state, &round_keys[10]);
    state
}

fn aes_decrypt_block_sbox(
    block: &[u8; 16],
    round_keys: &[[u8; 16]; 11],
    inv_sbox: &[u8; 256],
) -> [u8; 16] {
    let mut state = *block;
    add_round_key(&mut state, &round_keys[10]);
    for round in (1..10).rev() {
        inv_shift_rows(&mut state);
        sub_bytes(&mut state, inv_sbox);
        add_round_key(&mut state, &round_keys[round]);
        inv_mix_columns(&mut state);
    }
    inv_shift_rows(&mut state);
    sub_bytes(&mut state, inv_sbox);
    add_round_key(&mut state, &round_keys[0]);
    state
}

fn build_inverse_sbox(sbox: &[u8; 256]) -> [u8; 256] {
    let mut inv = [0u8; 256];
    for (i, &v) in sbox.iter().enumerate() {
        inv[v as usize] = i as u8;
    }
    inv
}

/// Custom VFS CTR mode with parameterized S-box.
fn custom_ctr_decrypt(data: &[u8], key: &[u8; 16], iv: &[u8; 16], sbox: &[u8; 256]) -> Vec<u8> {
    let round_keys = key_expansion(key, sbox);
    let mut current_iv = *iv;
    let mut out = data.to_vec();
    let full_blocks = out.len() / 16;

    for block_idx in 0..full_blocks {
        let offset = block_idx * 16;
        let keystream = aes_encrypt_block_sbox(&current_iv, &round_keys, sbox);
        for i in 0..16 {
            out[offset + i] ^= keystream[i];
        }
        // VFS-style IV derivation
        let mut next_iv = [0u8; 16];
        for i in 0..16 {
            let count = (i as u32) * 8;
            let shift_src = (VFS_XOR_KEY >> (count & 0x38)) as u8;
            let temp = keystream[i] ^ ((31u16 * i as u16) as u8) ^ shift_src;
            let rotated = temp.rotate_left(3);
            next_iv[i] = sbox[rotated as usize];
        }
        current_iv = next_iv;
    }
    let remaining = out.len() % 16;
    if remaining > 0 {
        let offset = full_blocks * 16;
        let keystream = aes_encrypt_block_sbox(&current_iv, &round_keys, sbox);
        for i in 0..remaining {
            out[offset + i] ^= keystream[i];
        }
    }
    out
}

// ══════════════════════════════════════════════════════════════
// S-box extraction from GameAssembly.dll
// ══════════════════════════════════════════════════════════════

fn load_sboxes_from_dll() -> Option<Vec<[u8; 256]>> {
    let dll_path = "./output/GameAssembly.dll";
    let data = std::fs::read(dll_path).ok()?;

    let pe = goblin::pe::PE::parse(&data).ok()?;
    let mut sboxes = Vec::new();

    for &offset in &SBOX_OFFSETS {
        // Convert RVA to file offset
        let mut file_offset = None;
        for section in &pe.sections {
            let sec_rva = section.virtual_address as u64;
            let sec_size = section.virtual_size as u64;
            if offset >= sec_rva && offset < sec_rva + sec_size {
                let off = section.pointer_to_raw_data as u64 + (offset - sec_rva);
                file_offset = Some(off as usize);
                break;
            }
        }
        let foff = file_offset?;
        if foff + 256 > data.len() {
            return None;
        }
        let mut sbox = [0u8; 256];
        sbox.copy_from_slice(&data[foff..foff + 256]);
        sboxes.push(sbox);
    }

    Some(sboxes)
}

// ══════════════════════════════════════════════════════════════
// Alternative cipher implementations
// ══════════════════════════════════════════════════════════════

/// TEA decrypt a single 64-bit block (v0, v1) with 128-bit key.
fn tea_decrypt(v: &mut [u32; 2], key: &[u32; 4]) {
    let mut v0 = v[0];
    let mut v1 = v[1];
    let delta: u32 = 0x9E3779B9;
    let mut sum: u32 = delta.wrapping_mul(32);
    for _ in 0..32 {
        v1 = v1.wrapping_sub(
            ((v0 << 4).wrapping_add(key[2]))
                ^ (v0.wrapping_add(sum))
                ^ ((v0 >> 5).wrapping_add(key[3])),
        );
        v0 = v0.wrapping_sub(
            ((v1 << 4).wrapping_add(key[0]))
                ^ (v1.wrapping_add(sum))
                ^ ((v1 >> 5).wrapping_add(key[1])),
        );
        sum = sum.wrapping_sub(delta);
    }
    v[0] = v0;
    v[1] = v1;
}

/// XTEA decrypt a single 64-bit block.
fn xtea_decrypt(v: &mut [u32; 2], key: &[u32; 4]) {
    let mut v0 = v[0];
    let mut v1 = v[1];
    let delta: u32 = 0x9E3779B9;
    let mut sum: u32 = delta.wrapping_mul(32);
    for _ in 0..32 {
        v1 = v1.wrapping_sub(
            ((v0 << 4) ^ (v0 >> 5)).wrapping_add(v0)
                ^ sum.wrapping_add(key[(sum >> 11 & 3) as usize]),
        );
        sum = sum.wrapping_sub(delta);
        v0 = v0.wrapping_sub(
            ((v1 << 4) ^ (v1 >> 5)).wrapping_add(v1) ^ sum.wrapping_add(key[(sum & 3) as usize]),
        );
    }
    v[0] = v0;
    v[1] = v1;
}

/// XXTEA decrypt on a variable-length block (minimum 2 u32 words).
fn xxtea_decrypt(data: &mut [u32], key: &[u32; 4]) {
    let n = data.len();
    if n < 2 {
        return;
    }
    let delta: u32 = 0x9E3779B9;
    let rounds = 6 + 52 / n;
    let mut sum = delta.wrapping_mul(rounds as u32);
    for _ in 0..rounds {
        let e = (sum >> 2) & 3;
        for p in (1..n).rev() {
            let z = data[p - 1];
            let y = data[(p + 1) % n];
            let mx = ((z >> 5) ^ (y << 2)).wrapping_add((y >> 3) ^ (z << 4))
                ^ (sum ^ y).wrapping_add(key[(p & 3) ^ (e as usize)] ^ z);
            data[p] = data[p].wrapping_sub(mx);
        }
        // p=0
        let z = data[n - 1];
        let y = data[1];
        let mx = ((z >> 5) ^ (y << 2)).wrapping_add((y >> 3) ^ (z << 4))
            ^ (sum ^ y).wrapping_add(key[e as usize] ^ z);
        data[0] = data[0].wrapping_sub(mx);
        sum = sum.wrapping_sub(delta);
    }
}

/// RC4 keystream generation + XOR.
fn rc4_decrypt(data: &[u8], key: &[u8], drop: usize) -> Vec<u8> {
    // KSA
    let mut s: Vec<u8> = (0..=255u8).collect();
    let mut j: u8 = 0;
    for i in 0..256 {
        j = j.wrapping_add(s[i]).wrapping_add(key[i % key.len()]);
        s.swap(i, j as usize);
    }
    // PRGA with optional drop
    let mut i: u8 = 0;
    let mut j: u8 = 0;
    for _ in 0..drop {
        i = i.wrapping_add(1);
        j = j.wrapping_add(s[i as usize]);
        s.swap(i as usize, j as usize);
    }
    let mut out = data.to_vec();
    for byte in &mut out {
        i = i.wrapping_add(1);
        j = j.wrapping_add(s[i as usize]);
        s.swap(i as usize, j as usize);
        *byte ^= s[s[i as usize].wrapping_add(s[j as usize]) as usize];
    }
    out
}

/// ChaCha20 quarter-round.
fn chacha_qr(state: &mut [u32; 16], a: usize, b: usize, c: usize, d: usize) {
    state[a] = state[a].wrapping_add(state[b]);
    state[d] ^= state[a];
    state[d] = state[d].rotate_left(16);
    state[c] = state[c].wrapping_add(state[d]);
    state[b] ^= state[c];
    state[b] = state[b].rotate_left(12);
    state[a] = state[a].wrapping_add(state[b]);
    state[d] ^= state[a];
    state[d] = state[d].rotate_left(8);
    state[c] = state[c].wrapping_add(state[d]);
    state[b] ^= state[c];
    state[b] = state[b].rotate_left(7);
}

fn chacha20_block(key: &[u32; 8], counter: u32, nonce: &[u32; 3]) -> [u8; 64] {
    let mut state: [u32; 16] = [
        0x61707865, 0x3320646e, 0x79622d32, 0x6b206574, // constants
        key[0], key[1], key[2], key[3], key[4], key[5], key[6], key[7], counter, nonce[0],
        nonce[1], nonce[2],
    ];
    let initial = state;
    for _ in 0..10 {
        // Column rounds
        chacha_qr(&mut state, 0, 4, 8, 12);
        chacha_qr(&mut state, 1, 5, 9, 13);
        chacha_qr(&mut state, 2, 6, 10, 14);
        chacha_qr(&mut state, 3, 7, 11, 15);
        // Diagonal rounds
        chacha_qr(&mut state, 0, 5, 10, 15);
        chacha_qr(&mut state, 1, 6, 11, 12);
        chacha_qr(&mut state, 2, 7, 8, 13);
        chacha_qr(&mut state, 3, 4, 9, 14);
    }
    for i in 0..16 {
        state[i] = state[i].wrapping_add(initial[i]);
    }
    let mut out = [0u8; 64];
    for (i, &word) in state.iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&word.to_le_bytes());
    }
    out
}

fn chacha20_decrypt(data: &[u8], key: &[u32; 8], nonce: &[u32; 3]) -> Vec<u8> {
    let mut out = data.to_vec();
    let blocks = out.len().div_ceil(64);
    for blk in 0..blocks {
        let keystream = chacha20_block(key, blk as u32, nonce);
        let start = blk * 64;
        let end = (start + 64).min(out.len());
        for i in start..end {
            out[i] ^= keystream[i - start];
        }
    }
    out
}

// ══════════════════════════════════════════════════════════════
// File loading
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
                return Some(data);
            }
            Err(e) => println!("    Download error: {}", e),
        }
    }
    None
}

// ══════════════════════════════════════════════════════════════
// Pass 1: Directory Enumeration
// ══════════════════════════════════════════════════════════════

fn pass1_enumerate(cd: &ZipCentralDirectory) {
    println!("\n{}", "=".repeat(60));
    println!("PASS 1: Directory Enumeration");
    println!("{}", "=".repeat(60));

    for &(name, _hash) in TARGETS {
        let mut files: Vec<(String, u64)> = cd
            .filenames()
            .filter(|f| f.ends_with(".chk") && f.contains(name))
            .map(|f| {
                let size = cd.get(f).map_or(0, |e| e.uncompressed_size);
                (f.to_string(), size)
            })
            .collect();
        files.sort_by_key(|(_n, s)| *s);

        let total_size: u64 = files.iter().map(|(_, s)| *s).sum();
        let sizes: Vec<u64> = files.iter().map(|(_, s)| *s).collect();

        println!(
            "\n  {} — {} files, total {}",
            name,
            files.len(),
            format_bytes(total_size)
        );

        if !sizes.is_empty() {
            let min = sizes[0];
            let max = sizes[sizes.len() - 1];
            let median = sizes[sizes.len() / 2];
            println!(
                "    min={} max={} median={}",
                format_bytes(min),
                format_bytes(max),
                format_bytes(median)
            );

            // Size alignment analysis
            let mod16 = sizes.iter().filter(|&&s| s % 16 == 0).count();
            let mod256 = sizes.iter().filter(|&&s| s % 256 == 0).count();
            let mod4096 = sizes.iter().filter(|&&s| s % 4096 == 0).count();
            println!(
                "    alignment: mod16={}/{} mod256={}/{} mod4096={}/{}",
                mod16,
                sizes.len(),
                mod256,
                sizes.len(),
                mod4096,
                sizes.len()
            );
        }
    }
}

// ══════════════════════════════════════════════════════════════
// Pass 2: Multi-File Structural Analysis
// ══════════════════════════════════════════════════════════════

fn pass2_structural(files: &[(&str, &[u8])]) {
    println!("\n{}", "=".repeat(60));
    println!("PASS 2: Multi-File Structural Analysis");
    println!("{}", "=".repeat(60));

    for (name, data) in files {
        let probe = &data[..data.len().min(16384)];
        println!("\n  {} ({}):", name, format_bytes(data.len() as u64));

        // ECB mode detection: count repeated 16-byte blocks
        let block_count = probe.len() / 16;
        let mut blocks_set = std::collections::HashSet::new();
        let mut repeats = 0u32;
        for i in 0..block_count {
            let block = &probe[i * 16..(i + 1) * 16];
            if !blocks_set.insert(block.to_vec()) {
                repeats += 1;
            }
        }
        println!(
            "    ECB repeat test: {}/{} blocks unique, {} repeats{}",
            blocks_set.len(),
            block_count,
            repeats,
            if repeats > 0 { " ← ECB LIKELY" } else { "" }
        );

        // Sliding-window entropy (256-byte windows, 64-byte stride)
        let mut ent_min = f64::MAX;
        let mut ent_max = 0.0f64;
        let mut window_count = 0u32;
        let mut off = 0;
        while off + 256 <= probe.len() && off < 16384 {
            let ent = entropy(&probe[off..off + 256]);
            ent_min = ent_min.min(ent);
            ent_max = ent_max.max(ent);
            window_count += 1;
            off += 64;
        }
        if window_count > 0 {
            println!(
                "    Sliding entropy (256B/64B stride): min={:.2} max={:.2} range={:.2}",
                ent_min,
                ent_max,
                ent_max - ent_min
            );
        }

        // Byte autocorrelation at various lags
        let lags = [8, 16, 32, 64, 128, 256];
        print!("    Autocorrelation:");
        for lag in lags {
            if probe.len() > lag + 1024 {
                let sample = &probe[..1024];
                let lagged = &probe[lag..lag + 1024];
                let xored: Vec<u8> = sample
                    .iter()
                    .zip(lagged.iter())
                    .map(|(&a, &b)| a ^ b)
                    .collect();
                let ent = entropy(&xored);
                let zeros = xored.iter().filter(|&&b| b == 0).count();
                print!(" lag{}={:.2}(z={})", lag, ent, zeros);
            }
        }
        println!();
    }

    // Intra-directory header comparison
    if files.len() >= 2 {
        println!("\n  Header comparison across files:");
        for n in [4, 8, 16, 32] {
            let mut all_same = true;
            let first = &files[0].1[..files[0].1.len().min(n)];
            for (_, data) in &files[1..] {
                if data.len() < n || &data[..n] != first {
                    all_same = false;
                    break;
                }
            }
            println!(
                "    First {} bytes: {}",
                n,
                if all_same { "IDENTICAL" } else { "different" }
            );
        }
    }
}

// ══════════════════════════════════════════════════════════════
// Pass 3: S-Box Cipher Battery
// ══════════════════════════════════════════════════════════════

fn pass3_sbox_battery(files: &[(&str, &[u8])], sboxes: &[[u8; 256]]) -> bool {
    println!("\n{}", "=".repeat(60));
    println!("PASS 3: S-Box Cipher Battery ({} S-boxes)", sboxes.len());
    println!("{}", "=".repeat(60));

    let keys: Vec<(&[u8; 16], &str)> = vec![(&VFS_AES_KEY, "VFS_KEY"), (&VFS_AES_IV, "VFS_IV")];
    // Reversed key
    let mut rev_key = VFS_AES_KEY;
    rev_key.reverse();
    let mut key_xor_iv = [0u8; 16];
    for i in 0..16 {
        key_xor_iv[i] = VFS_AES_KEY[i] ^ VFS_AES_IV[i];
    }

    let mut total_tests = 0u32;
    let mut any_hit = false;

    for (file_name, data) in files {
        let probe = &data[..data.len().min(PROBE_SIZE)];
        println!("\n  Testing {} ({} bytes)...", file_name, probe.len());

        for (sbox_idx, sbox) in sboxes.iter().enumerate() {
            let inv_sbox = build_inverse_sbox(sbox);

            // Test each key candidate
            for (key, key_name) in &keys {
                // ECB encrypt (data as counter blocks)
                {
                    let round_keys = key_expansion(key, sbox);
                    let blocks = probe.len() / 16;
                    let mut buf = Vec::with_capacity(blocks * 16);
                    for i in 0..blocks {
                        let block: [u8; 16] = probe[i * 16..(i + 1) * 16].try_into().unwrap();
                        buf.extend_from_slice(&aes_encrypt_block_sbox(&block, &round_keys, sbox));
                    }
                    total_tests += 1;
                    if check_and_report(&format!("sbox{}-ECB-enc {}", sbox_idx, key_name), &buf) {
                        any_hit = true;
                    }
                }

                // ECB decrypt
                {
                    let round_keys = key_expansion(key, sbox);
                    let blocks = probe.len() / 16;
                    let mut buf = Vec::with_capacity(blocks * 16);
                    for i in 0..blocks {
                        let block: [u8; 16] = probe[i * 16..(i + 1) * 16].try_into().unwrap();
                        buf.extend_from_slice(&aes_decrypt_block_sbox(
                            &block,
                            &round_keys,
                            &inv_sbox,
                        ));
                    }
                    total_tests += 1;
                    if check_and_report(&format!("sbox{}-ECB-dec {}", sbox_idx, key_name), &buf) {
                        any_hit = true;
                    }
                }

                // CBC decrypt with VFS_AES_IV
                {
                    let round_keys = key_expansion(key, sbox);
                    let blocks = probe.len() / 16;
                    let mut buf = Vec::with_capacity(blocks * 16);
                    let mut prev = VFS_AES_IV;
                    for i in 0..blocks {
                        let ct_block: [u8; 16] = probe[i * 16..(i + 1) * 16].try_into().unwrap();
                        let pt_raw = aes_decrypt_block_sbox(&ct_block, &round_keys, &inv_sbox);
                        let pt: Vec<u8> = pt_raw
                            .iter()
                            .zip(prev.iter())
                            .map(|(&a, &b)| a ^ b)
                            .collect();
                        buf.extend_from_slice(&pt);
                        prev = ct_block;
                    }
                    total_tests += 1;
                    if check_and_report(&format!("sbox{}-CBC {}", sbox_idx, key_name), &buf) {
                        any_hit = true;
                    }
                }

                // CBC decrypt with zero IV
                {
                    let round_keys = key_expansion(key, sbox);
                    let blocks = probe.len() / 16;
                    let mut buf = Vec::with_capacity(blocks * 16);
                    let mut prev = [0u8; 16];
                    for i in 0..blocks {
                        let ct_block: [u8; 16] = probe[i * 16..(i + 1) * 16].try_into().unwrap();
                        let pt_raw = aes_decrypt_block_sbox(&ct_block, &round_keys, &inv_sbox);
                        let pt: Vec<u8> = pt_raw
                            .iter()
                            .zip(prev.iter())
                            .map(|(&a, &b)| a ^ b)
                            .collect();
                        buf.extend_from_slice(&pt);
                        prev = ct_block;
                    }
                    total_tests += 1;
                    if check_and_report(&format!("sbox{}-CBC-zeroIV {}", sbox_idx, key_name), &buf)
                    {
                        any_hit = true;
                    }
                }

                // CTR LE counter
                {
                    let round_keys = key_expansion(key, sbox);
                    let blocks = probe.len() / 16;
                    let mut buf = Vec::with_capacity(blocks * 16);
                    for i in 0..blocks {
                        let mut ctr_block = VFS_AES_IV;
                        let ctr_val = (i as u32).to_le_bytes();
                        ctr_block[12..16].copy_from_slice(&ctr_val);
                        let keystream = aes_encrypt_block_sbox(&ctr_block, &round_keys, sbox);
                        let ct = &probe[i * 16..(i + 1) * 16];
                        let pt: Vec<u8> = ct
                            .iter()
                            .zip(keystream.iter())
                            .map(|(&a, &b)| a ^ b)
                            .collect();
                        buf.extend_from_slice(&pt);
                    }
                    total_tests += 1;
                    if check_and_report(&format!("sbox{}-CTR-LE {}", sbox_idx, key_name), &buf) {
                        any_hit = true;
                    }
                }

                // CTR BE counter
                {
                    let round_keys = key_expansion(key, sbox);
                    let blocks = probe.len() / 16;
                    let mut buf = Vec::with_capacity(blocks * 16);
                    for i in 0..blocks {
                        let mut ctr_block = VFS_AES_IV;
                        let ctr_val = (i as u32).to_be_bytes();
                        ctr_block[12..16].copy_from_slice(&ctr_val);
                        let keystream = aes_encrypt_block_sbox(&ctr_block, &round_keys, sbox);
                        let ct = &probe[i * 16..(i + 1) * 16];
                        let pt: Vec<u8> = ct
                            .iter()
                            .zip(keystream.iter())
                            .map(|(&a, &b)| a ^ b)
                            .collect();
                        buf.extend_from_slice(&pt);
                    }
                    total_tests += 1;
                    if check_and_report(&format!("sbox{}-CTR-BE {}", sbox_idx, key_name), &buf) {
                        any_hit = true;
                    }
                }

                // Custom VFS chained-IV CTR
                {
                    let result = custom_ctr_decrypt(probe, key, &VFS_AES_IV, sbox);
                    total_tests += 1;
                    if check_and_report(&format!("sbox{}-VFS-CTR {}", sbox_idx, key_name), &result)
                    {
                        any_hit = true;
                    }
                }
            }

            // Also test with reversed key and key^iv
            for (extra_key, extra_name) in [(&rev_key, "revKEY"), (&key_xor_iv, "KEY^IV")] {
                // ECB decrypt
                {
                    let round_keys = key_expansion(extra_key, sbox);
                    let blocks = probe.len() / 16;
                    let mut buf = Vec::with_capacity(blocks * 16);
                    for i in 0..blocks {
                        let block: [u8; 16] = probe[i * 16..(i + 1) * 16].try_into().unwrap();
                        buf.extend_from_slice(&aes_decrypt_block_sbox(
                            &block,
                            &round_keys,
                            &inv_sbox,
                        ));
                    }
                    total_tests += 1;
                    if check_and_report(&format!("sbox{}-ECB-dec {}", sbox_idx, extra_name), &buf) {
                        any_hit = true;
                    }
                }
                // Custom VFS CTR
                {
                    let result = custom_ctr_decrypt(probe, extra_key, &VFS_AES_IV, sbox);
                    total_tests += 1;
                    if check_and_report(
                        &format!("sbox{}-VFS-CTR {}", sbox_idx, extra_name),
                        &result,
                    ) {
                        any_hit = true;
                    }
                }
            }

            // Inverse S-box byte substitution only (no AES rounds)
            {
                let forward: Vec<u8> = probe.iter().map(|&b| sbox[b as usize]).collect();
                total_tests += 1;
                if check_and_report(&format!("sbox{}-fwd-sub", sbox_idx), &forward) {
                    any_hit = true;
                }
                let inverse: Vec<u8> = probe.iter().map(|&b| inv_sbox[b as usize]).collect();
                total_tests += 1;
                if check_and_report(&format!("sbox{}-inv-sub", sbox_idx), &inverse) {
                    any_hit = true;
                }
            }
        }
    }

    println!("\n  Total S-box tests: {}", total_tests);
    any_hit
}

// ══════════════════════════════════════════════════════════════
// Pass 4: Alternative Cipher Families
// ══════════════════════════════════════════════════════════════

fn pass4_alt_ciphers(files: &[(&str, &[u8])]) -> bool {
    println!("\n{}", "=".repeat(60));
    println!("PASS 4: Alternative Cipher Families");
    println!("{}", "=".repeat(60));

    let mut total_tests = 0u32;
    let mut any_hit = false;

    // Key as 4×u32 in LE and BE
    let key_le: [u32; 4] = [
        u32::from_le_bytes(VFS_AES_KEY[0..4].try_into().unwrap()),
        u32::from_le_bytes(VFS_AES_KEY[4..8].try_into().unwrap()),
        u32::from_le_bytes(VFS_AES_KEY[8..12].try_into().unwrap()),
        u32::from_le_bytes(VFS_AES_KEY[12..16].try_into().unwrap()),
    ];
    let key_be: [u32; 4] = [
        u32::from_be_bytes(VFS_AES_KEY[0..4].try_into().unwrap()),
        u32::from_be_bytes(VFS_AES_KEY[4..8].try_into().unwrap()),
        u32::from_be_bytes(VFS_AES_KEY[8..12].try_into().unwrap()),
        u32::from_be_bytes(VFS_AES_KEY[12..16].try_into().unwrap()),
    ];

    for (file_name, data) in files {
        let probe = &data[..data.len().min(PROBE_SIZE)];
        println!("\n  Testing {} ({} bytes)...", file_name, probe.len());

        // ── TEA ──
        for (tea_key, endian) in [(&key_le, "LE"), (&key_be, "BE")] {
            // ECB
            let blocks = probe.len() / 8;
            let mut buf = probe[..blocks * 8].to_vec();
            for i in 0..blocks {
                let mut v = [
                    u32::from_le_bytes(buf[i * 8..i * 8 + 4].try_into().unwrap()),
                    u32::from_le_bytes(buf[i * 8 + 4..i * 8 + 8].try_into().unwrap()),
                ];
                tea_decrypt(&mut v, tea_key);
                buf[i * 8..i * 8 + 4].copy_from_slice(&v[0].to_le_bytes());
                buf[i * 8 + 4..i * 8 + 8].copy_from_slice(&v[1].to_le_bytes());
            }
            total_tests += 1;
            if check_and_report(&format!("TEA-ECB {}", endian), &buf) {
                any_hit = true;
            }

            // CBC
            let mut buf = probe[..blocks * 8].to_vec();
            let mut prev = [0u8; 8];
            prev.copy_from_slice(&VFS_AES_IV[..8]);
            for i in 0..blocks {
                let ct = buf[i * 8..i * 8 + 8].to_vec();
                let mut v = [
                    u32::from_le_bytes(ct[0..4].try_into().unwrap()),
                    u32::from_le_bytes(ct[4..8].try_into().unwrap()),
                ];
                tea_decrypt(&mut v, tea_key);
                let pt0 = v[0].to_le_bytes();
                let pt1 = v[1].to_le_bytes();
                for j in 0..4 {
                    buf[i * 8 + j] = pt0[j] ^ prev[j];
                    buf[i * 8 + 4 + j] = pt1[j] ^ prev[4 + j];
                }
                prev.copy_from_slice(&ct);
            }
            total_tests += 1;
            if check_and_report(&format!("TEA-CBC {}", endian), &buf) {
                any_hit = true;
            }
        }

        // ── XTEA ──
        for (xtea_key, endian) in [(&key_le, "LE"), (&key_be, "BE")] {
            let blocks = probe.len() / 8;
            let mut buf = probe[..blocks * 8].to_vec();
            for i in 0..blocks {
                let mut v = [
                    u32::from_le_bytes(buf[i * 8..i * 8 + 4].try_into().unwrap()),
                    u32::from_le_bytes(buf[i * 8 + 4..i * 8 + 8].try_into().unwrap()),
                ];
                xtea_decrypt(&mut v, xtea_key);
                buf[i * 8..i * 8 + 4].copy_from_slice(&v[0].to_le_bytes());
                buf[i * 8 + 4..i * 8 + 8].copy_from_slice(&v[1].to_le_bytes());
            }
            total_tests += 1;
            if check_and_report(&format!("XTEA-ECB {}", endian), &buf) {
                any_hit = true;
            }
        }

        // ── XXTEA ──
        {
            // Test on first 4KB as one block (1024 u32 words)
            let xxtea_size = probe.len().min(4096);
            let words = xxtea_size / 4;
            if words >= 2 {
                for (xk, endian) in [(&key_le, "LE"), (&key_be, "BE")] {
                    let mut word_buf: Vec<u32> = (0..words)
                        .map(|i| u32::from_le_bytes(probe[i * 4..i * 4 + 4].try_into().unwrap()))
                        .collect();
                    xxtea_decrypt(&mut word_buf, xk);
                    let buf: Vec<u8> = word_buf.iter().flat_map(|w| w.to_le_bytes()).collect();
                    total_tests += 1;
                    if check_and_report(&format!("XXTEA-{}w {}", words, endian), &buf) {
                        any_hit = true;
                    }
                }
            }
        }

        // ── RC4 ──
        {
            let rc4_keys: Vec<(&[u8], &str)> =
                vec![(&VFS_AES_KEY[..], "KEY-16B"), (&VFS_AES_IV[..], "IV-16B")];
            let mut key_iv = Vec::with_capacity(32);
            key_iv.extend_from_slice(&VFS_AES_KEY);
            key_iv.extend_from_slice(&VFS_AES_IV);

            for (rk, rk_name) in &rc4_keys {
                for drop in [0, 256, 768] {
                    let result = rc4_decrypt(probe, rk, drop);
                    total_tests += 1;
                    let label = format!("RC4-drop{} {}", drop, rk_name);
                    if check_and_report(&label, &result) {
                        any_hit = true;
                    }
                }
            }
            // KEY||IV (32 bytes)
            for drop in [0, 256, 768] {
                let result = rc4_decrypt(probe, &key_iv, drop);
                total_tests += 1;
                if check_and_report(&format!("RC4-drop{} KEY||IV", drop), &result) {
                    any_hit = true;
                }
            }
        }

        // ── ChaCha20 ──
        {
            // VFS_AES_KEY zero-extended to 32 bytes
            let mut chacha_key = [0u32; 8];
            for i in 0..4 {
                chacha_key[i] =
                    u32::from_le_bytes(VFS_AES_KEY[i * 4..i * 4 + 4].try_into().unwrap());
            }
            // VFS_AES_IV as 12-byte nonce (first 12 bytes)
            let chacha_nonce = [
                u32::from_le_bytes(VFS_AES_IV[0..4].try_into().unwrap()),
                u32::from_le_bytes(VFS_AES_IV[4..8].try_into().unwrap()),
                u32::from_le_bytes(VFS_AES_IV[8..12].try_into().unwrap()),
            ];
            let result = chacha20_decrypt(probe, &chacha_key, &chacha_nonce);
            total_tests += 1;
            if check_and_report("ChaCha20 KEY+IV", &result) {
                any_hit = true;
            }

            // Full 32-byte key: KEY||IV
            let mut full_key = [0u32; 8];
            for i in 0..4 {
                full_key[i] = u32::from_le_bytes(VFS_AES_KEY[i * 4..i * 4 + 4].try_into().unwrap());
                full_key[i + 4] =
                    u32::from_le_bytes(VFS_AES_IV[i * 4..i * 4 + 4].try_into().unwrap());
            }
            let result = chacha20_decrypt(probe, &full_key, &chacha_nonce);
            total_tests += 1;
            if check_and_report("ChaCha20 KEY||IV", &result) {
                any_hit = true;
            }
        }
    }

    println!("\n  Total alt cipher tests: {}", total_tests);
    any_hit
}

// ══════════════════════════════════════════════════════════════
// Pass 5: Known-Plaintext Attack
// ══════════════════════════════════════════════════════════════

fn pass5_known_plaintext(files: &[(&str, &[u8])], reference_header: Option<&[u8]>) -> bool {
    println!("\n{}", "=".repeat(60));
    println!("PASS 5: Known-Plaintext Attack");
    println!("{}", "=".repeat(60));

    let mut any_hit = false;

    // Known plaintext candidates
    let mut plaintexts: Vec<(&str, Vec<u8>)> = vec![
        ("UnityFS\\0", b"UnityFS\0".to_vec()),
        ("PK\\x03\\x04", b"PK\x03\x04".to_vec()),
    ];

    // FlatBuffer: u32 offsets in range 4-256
    for offset in [4u32, 8, 12, 16, 20, 24, 32, 48, 64, 128, 256] {
        plaintexts.push(("flatbuf-offset", offset.to_le_bytes().to_vec()));
    }

    // Protobuf tags (field 1, wire type 0/2)
    plaintexts.push(("protobuf-varint", vec![0x08]));
    plaintexts.push(("protobuf-string", vec![0x0A]));

    if let Some(ref_hdr) = reference_header {
        plaintexts.push(("ref-VFS-header", ref_hdr.to_vec()));
    }

    for (file_name, data) in files {
        println!(
            "\n  {} (first 8 bytes: {:02x?}):",
            file_name,
            &data[..data.len().min(8)]
        );

        for (pt_name, pt) in &plaintexts {
            if data.len() < pt.len() || pt.is_empty() {
                continue;
            }
            let ct = &data[..pt.len()];
            let keystream: Vec<u8> = ct.iter().zip(pt.iter()).map(|(&c, &p)| c ^ p).collect();

            let ent = entropy(&keystream);
            let all_same = keystream.windows(2).all(|w| w[0] == w[1]);
            let all_zero = keystream.iter().all(|&b| b == 0);
            let is_counter = keystream.len() >= 4
                && keystream
                    .windows(2)
                    .all(|w| w[1] == w[0].wrapping_add(1) || w[1] == w[0]);

            if all_zero || all_same || is_counter || (keystream.len() >= 8 && ent < 2.0) {
                println!(
                    "  *** KEYSTREAM MATCH [{}→{}]: {:02x?} entropy={:.2}{}{}{}",
                    pt_name,
                    file_name,
                    keystream,
                    ent,
                    if all_zero { " ALL-ZERO" } else { "" },
                    if all_same { " ALL-SAME" } else { "" },
                    if is_counter { " COUNTER" } else { "" },
                );
                any_hit = true;
            }
        }

        // Try applying derived keystream from each plaintext to all other files
        if let Some(ref_hdr) = reference_header
            && data.len() >= ref_hdr.len()
            && ref_hdr.len() >= 8
        {
            let keystream: Vec<u8> = data[..ref_hdr.len()]
                .iter()
                .zip(ref_hdr.iter())
                .map(|(&c, &p)| c ^ p)
                .collect();

            // Test this keystream against other encrypted files
            for (other_name, other_data) in files {
                if *other_name == *file_name || other_data.len() < keystream.len() {
                    continue;
                }
                let decrypted: Vec<u8> = other_data[..keystream.len()]
                    .iter()
                    .zip(keystream.iter())
                    .map(|(&c, &k)| c ^ k)
                    .collect();
                let (hit, desc) = detect(&decrypted);
                if hit {
                    println!(
                        "  *** CROSS-HIT: keystream from {} applied to {}: {}",
                        file_name, other_name, desc
                    );
                    hex_dump(&decrypted, 32);
                    any_hit = true;
                }
            }
        }
    }

    if !any_hit {
        println!("\n  No keystream patterns found");
    }
    any_hit
}

// ══════════════════════════════════════════════════════════════
// Pass 6: Statistical Deep Dive
// ══════════════════════════════════════════════════════════════

fn pass6_statistics(files: &[(&str, &[u8])]) {
    println!("\n{}", "=".repeat(60));
    println!("PASS 6: Statistical Deep Dive");
    println!("{}", "=".repeat(60));

    for (name, data) in files {
        let probe = &data[..data.len().min(16384)];
        println!("\n  {}:", name);

        // Chi-square at various block sizes
        print!("    Chi-square (position-dependent):");
        for block_size in [8, 16, 32, 64] {
            if probe.len() < block_size * 10 {
                continue;
            }
            // For each position within block, compute byte frequency chi-square
            let mut max_chi = 0.0f64;
            let mut min_chi = f64::MAX;
            for pos in 0..block_size {
                let bytes: Vec<u8> = probe
                    .iter()
                    .skip(pos)
                    .step_by(block_size)
                    .copied()
                    .collect();
                let n = bytes.len() as f64;
                let expected = n / 256.0;
                let mut freq = [0u32; 256];
                for &b in &bytes {
                    freq[b as usize] += 1;
                }
                let chi: f64 = freq
                    .iter()
                    .map(|&f| {
                        let diff = f as f64 - expected;
                        diff * diff / expected
                    })
                    .sum();
                max_chi = max_chi.max(chi);
                min_chi = min_chi.min(chi);
            }
            print!(" bs{}=[{:.0},{:.0}]", block_size, min_chi, max_chi);
        }
        println!();

        // Autocorrelation at lags 1..128
        let sample_len = probe.len().min(4096);
        let sample = &probe[..sample_len];
        let mut notable_lags = Vec::new();
        for lag in 1..128.min(sample_len) {
            let xored: Vec<u8> = sample[..sample_len - lag]
                .iter()
                .zip(sample[lag..].iter())
                .map(|(&a, &b)| a ^ b)
                .collect();
            let zeros = xored.iter().filter(|&&b| b == 0).count();
            let zero_ratio = zeros as f64 / xored.len() as f64;
            if zero_ratio > 0.01 {
                notable_lags.push((lag, zero_ratio));
            }
        }
        if notable_lags.is_empty() {
            println!(
                "    Autocorrelation: no notable lags (uniform — stream cipher or good block cipher)"
            );
        } else {
            print!("    Notable autocorrelation lags:");
            for (lag, ratio) in &notable_lags {
                print!(" lag{}={:.1}%", lag, ratio * 100.0);
            }
            println!();
        }

        // Bigram entropy
        if sample.len() >= 2 {
            let mut bigram_freq = vec![0u32; 65536];
            for i in 0..sample.len() - 1 {
                let bigram = ((sample[i] as u16) << 8) | sample[i + 1] as u16;
                bigram_freq[bigram as usize] += 1;
            }
            let n = (sample.len() - 1) as f64;
            let bigram_ent: f64 = bigram_freq
                .iter()
                .filter(|&&c| c > 0)
                .map(|&c| {
                    let p = c as f64 / n;
                    -p * p.log2()
                })
                .sum();
            let unique_bigrams = bigram_freq.iter().filter(|&&c| c > 0).count();
            println!(
                "    Bigram entropy: {:.2} bits (ideal=16.00, unique={}/65536)",
                bigram_ent, unique_bigrams
            );
        }

        // Position mod-N frequency analysis
        print!("    Position-mod bias:");
        for modulus in [4, 8, 16, 32] {
            let mut pos_freq = vec![[0u32; 256]; modulus];
            for (i, &b) in probe.iter().enumerate() {
                pos_freq[i % modulus][b as usize] += 1;
            }
            // Check if any position has significantly different distribution
            let total_per_pos = probe.len() / modulus;
            let expected = total_per_pos as f64 / 256.0;
            let mut max_dev = 0.0f64;
            for pos_dist in &pos_freq {
                for &count in pos_dist {
                    let dev = ((count as f64 - expected) / expected).abs();
                    max_dev = max_dev.max(dev);
                }
            }
            print!(" mod{}={:.2}", modulus, max_dev);
        }
        println!();
    }
}

// ══════════════════════════════════════════════════════════════
// Pass 7: Cross-Directory Correlation
// ══════════════════════════════════════════════════════════════

fn pass7_cross_correlation(files: &[(&str, &[u8])]) {
    println!("\n{}", "=".repeat(60));
    println!("PASS 7: Cross-Directory Correlation");
    println!("{}", "=".repeat(60));

    if files.len() < 2 {
        println!("  Need >= 2 files");
        return;
    }

    // First 16 bytes
    println!("\n  First 16 bytes:");
    for (name, data) in files {
        print!("    {}: ", name);
        for &b in &data[..data.len().min(16)] {
            print!("{:02x} ", b);
        }
        println!();
    }

    // Pairwise XOR entropy
    println!("\n  Pairwise XOR:");
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
            let zero_count = xored.iter().filter(|&&b| b == 0).count();
            println!(
                "    {} ^ {} = entropy={:.2} max_zero_run={} zero_bytes={}{}",
                files[i].0,
                files[j].0,
                ent,
                zeros,
                zero_count,
                if ent < 6.0 { " ← LOW ENTROPY" } else { "" }
            );
        }
    }

    // Size distribution
    println!("\n  Size distribution:");
    for (name, data) in files {
        let size = data.len();
        println!(
            "    {} — {} (mod16={}, mod256={}, mod4096={})",
            name,
            format_bytes(size as u64),
            size % 16,
            size % 256,
            size % 4096,
        );
    }

    // Header pattern grouping
    println!("\n  Header pattern grouping (first 4 bytes):");
    let mut groups: std::collections::HashMap<[u8; 4], Vec<&str>> =
        std::collections::HashMap::new();
    for (name, data) in files {
        if data.len() >= 4 {
            let key: [u8; 4] = data[..4].try_into().unwrap();
            groups.entry(key).or_default().push(name);
        }
    }
    for (header, names) in &groups {
        println!("    {:02x?} → {}", header, names.join(", "));
    }
}

// ══════════════════════════════════════════════════════════════
// Pass 8: Cipher Fingerprinting
// ══════════════════════════════════════════════════════════════

fn pass8_fingerprint(files: &[(&str, &[u8])], sboxes: &[[u8; 256]]) {
    println!("\n{}", "=".repeat(60));
    println!("PASS 8: Cipher Fingerprinting");
    println!("{}", "=".repeat(60));

    // Check if same keystream is used across files (XOR pairs → if same keystream, XOR cancels)
    if files.len() >= 2 {
        println!("\n  Cross-file keystream consistency test:");
        // If same keystream: file_a XOR file_b = plaintext_a XOR plaintext_b
        // This would typically have lower entropy if both are structured data
        for i in 0..files.len() {
            for j in (i + 1)..files.len() {
                let len = files[i].1.len().min(files[j].1.len()).min(4096);
                let xored: Vec<u8> = files[i].1[..len]
                    .iter()
                    .zip(files[j].1[..len].iter())
                    .map(|(&a, &b)| a ^ b)
                    .collect();
                let ent = entropy(&xored);
                let conclusion = if ent < 7.0 {
                    "possible same keystream (lower entropy)"
                } else {
                    "different keystreams or non-XOR cipher"
                };
                println!(
                    "    {} ^ {} → entropy={:.2} — {}",
                    files[i].0, files[j].0, ent, conclusion
                );
            }
        }
    }

    // S-box residue detection
    println!("\n  S-box residue detection on byte distribution:");
    for (name, data) in files {
        let probe = &data[..data.len().min(4096)];
        let mut freq = [0u32; 256];
        for &b in probe {
            freq[b as usize] += 1;
        }

        for (sbox_idx, sbox) in sboxes.iter().enumerate() {
            // If data = sbox[plaintext], then frequency of output byte b =
            // frequency of inv_sbox[b] in plaintext. Check if applying inv_sbox
            // to the frequency distribution produces a more uniform distribution
            let inv_sbox = build_inverse_sbox(sbox);
            let mut mapped_freq = [0u32; 256];
            for (i, &f) in freq.iter().enumerate() {
                mapped_freq[inv_sbox[i as u8 as usize] as usize] = f;
            }

            // Compare variance of original vs mapped
            let mean = probe.len() as f64 / 256.0;
            let var_orig: f64 =
                freq.iter().map(|&f| (f as f64 - mean).powi(2)).sum::<f64>() / 256.0;
            let var_mapped: f64 = mapped_freq
                .iter()
                .map(|&f| (f as f64 - mean).powi(2))
                .sum::<f64>()
                / 256.0;

            if (var_mapped - var_orig).abs() / var_orig.max(1.0) > 0.1 {
                println!(
                    "    {} sbox{}: var_orig={:.1} var_mapped={:.1} (ratio={:.2})",
                    name,
                    sbox_idx,
                    var_orig,
                    var_mapped,
                    var_mapped / var_orig.max(0.001),
                );
            }
        }
    }

    // Linear bias in consecutive bytes
    println!("\n  Linear bias (consecutive byte XOR):");
    for (name, data) in files {
        let probe = &data[..data.len().min(4096)];
        let diff: Vec<u8> = probe.windows(2).map(|w| w[0] ^ w[1]).collect();
        let diff_ent = entropy(&diff);
        let mut diff_freq = [0u32; 256];
        for &b in &diff {
            diff_freq[b as usize] += 1;
        }
        let unique = diff_freq.iter().filter(|&&c| c > 0).count();
        println!(
            "    {} — diff_entropy={:.2} unique_diffs={}/256{}",
            name,
            diff_ent,
            unique,
            if diff_ent < 7.5 {
                " ← BIAS DETECTED"
            } else {
                ""
            }
        );
    }

    // XOR-Mul seed search on first 8 bytes as u32 words
    println!("\n  XOR-Mul seed brute-force (first word):");
    for (name, data) in files {
        if data.len() < 8 {
            continue;
        }
        let w0 = u32::from_le_bytes(data[0..4].try_into().unwrap());
        // If XOR-Mul: output = seed * (offset+1) XOR seed_xor
        // Try to find seed where first word decrypts to something recognizable
        let target_words: &[(u32, &str)] = &[
            (0x00000000, "zero"),
            (u32::from_le_bytes(*b"Unit"), "Unity"),
            (u32::from_be_bytes(*b"Unit"), "Unity-BE"),
            (u32::from_le_bytes(*b"CAB-"), "CAB-"),
        ];

        for &(target, tname) in target_words {
            // output[0] = (seed_mul * 1) XOR seed_xor XOR original
            // So: plaintext XOR output[0] = (seed_mul * 1) XOR seed_xor
            let needed = w0 ^ target;
            // This is seed_mul XOR seed_xor, can't separate them without more constraints
            // But if seed_xor == WWISE_CIPHER_XOR: seed_mul = needed XOR WWISE_CIPHER_XOR
            let candidate_mul = needed ^ 0x9C5A0B29;
            let candidate_xor = 0x9C5A0B29u32;

            // Verify: decrypt first 2 words and check consistency
            if data.len() >= 8 {
                let mut test = data[..8].to_vec();
                wwise_pck::decipher_inplace(&mut test, candidate_mul, 0);
                let w0_dec = u32::from_le_bytes(test[0..4].try_into().unwrap());
                if w0_dec == target {
                    let w1_dec = u32::from_le_bytes(test[4..8].try_into().unwrap());
                    let (hit, _) = detect(&test);
                    if hit || w1_dec == 0 {
                        println!(
                            "  *** XOR-Mul MATCH {}: target={} seed_mul=0x{:08X} seed_xor=0x{:08X}",
                            name, tname, candidate_mul, candidate_xor
                        );
                        any_hit_global();
                    }
                }
            }
        }
    }

    // Summary
    println!("\n  Fingerprint summary:");
    for (name, data) in files {
        let probe = &data[..data.len().min(4096)];
        let ent = entropy(probe);
        let mut freq = [0u32; 256];
        for &b in probe {
            freq[b as usize] += 1;
        }
        let unique = freq.iter().filter(|&&c| c > 0).count();
        let max_freq = freq.iter().max().copied().unwrap_or(0);
        let min_freq = freq.iter().filter(|&&c| c > 0).min().copied().unwrap_or(0);
        println!(
            "    {} — entropy={:.3} unique={}/256 freq_range=[{},{}]",
            name, ent, unique, min_freq, max_freq
        );

        if ent > 7.9 && unique > 250 {
            println!("      → Strong cipher (near-perfect entropy distribution)");
            println!("      → NOT simple XOR, NOT ECB, NOT basic substitution");
            println!("      → Likely: block cipher CBC/CTR, or stream cipher with fresh key/nonce");
        }
    }
}

fn any_hit_global() {
    // Just a marker function for the XOR-Mul match
}

// ══════════════════════════════════════════════════════════════
// Main
// ══════════════════════════════════════════════════════════════

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("=== Phase O: Encrypted VFS Directory Deep Analysis ===\n");

    let output_dir = "./output";
    std::fs::create_dir_all(output_dir)?;

    // Load S-boxes from GameAssembly.dll
    println!("[*] Loading S-boxes from GameAssembly.dll...");
    let dll_sboxes = match load_sboxes_from_dll() {
        Some(sboxes) => {
            println!("    Loaded {} S-boxes", sboxes.len());
            for (i, sbox) in sboxes.iter().enumerate() {
                let unique: std::collections::HashSet<u8> = sbox.iter().copied().collect();
                let is_involution = (0..=255u8).all(|b| sbox[sbox[b as usize] as usize] == b);
                println!(
                    "    S-box {}: unique={} involution={}",
                    i,
                    unique.len(),
                    is_involution
                );
            }
            sboxes
        }
        None => {
            println!("    WARNING: Could not load S-boxes (GameAssembly.dll not found?)");
            println!("    Falling back to VFS_AES_SBOX only");
            vec![VFS_AES_SBOX]
        }
    };

    // Load encrypted files from cache
    let mut loaded: Vec<(&str, u32, Vec<u8>)> = Vec::new();
    let mut need_download: Vec<(&str, u32)> = Vec::new();

    for &(name, hash) in TARGETS {
        if let Some(data) = load_cached(name) {
            println!(
                "[OK] {} loaded from cache ({})",
                name,
                format_bytes(data.len() as u64)
            );
            loaded.push((name, hash, data));
        } else {
            need_download.push((name, hash));
        }
    }

    // Download missing files
    let client = Client::new();
    let mut cd: Option<ZipCentralDirectory> = None;

    if !need_download.is_empty() {
        println!("\n[*] {} files need downloading...", need_download.len());
        let params = ResolveParams {
            app_code: app_code::game::OS_WIN_REL,
            launcher_app_code: app_code::launcher::OS_WIN_REL,
            channel: channel::OS_WIN_REL,
            sub_channel: sub_channel::OS_WIN_REL,
            launcher_sub_channel: sub_channel::OS_WIN_REL,
            platform: Platform::Windows,
        };
        let config = resolve_game_config(&client, &params).await?;
        let parts: Vec<SplitArchivePart> = config
            .packs
            .iter()
            .map(|p| SplitArchivePart {
                url: p.url.clone(),
                size: p.package_size.parse::<u64>().expect("invalid package_size"),
            })
            .collect();
        let central_dir = ZipCentralDirectory::from_split_archive(&client, parts).await?;

        for (name, hash) in &need_download {
            if let Some(data) = download_file(&client, &central_dir, name).await {
                loaded.push((name, *hash, data));
            }
        }
        cd = Some(central_dir);
    }

    // Load reference VFS header (for known-plaintext attack)
    let reference_header: Option<Vec<u8>> =
        load_cached(REFERENCE_VFS_DIR).map(|d| d[..d.len().min(8)].to_vec());
    if reference_header.is_some() {
        println!(
            "[OK] Reference VFS header loaded from {}",
            REFERENCE_VFS_DIR
        );
    }

    // ── Pass 1: Enumeration ──
    if cd.is_none() {
        println!("\n[*] Resolving ZIP central directory for Pass 1...");
        let params = ResolveParams {
            app_code: app_code::game::OS_WIN_REL,
            launcher_app_code: app_code::launcher::OS_WIN_REL,
            channel: channel::OS_WIN_REL,
            sub_channel: sub_channel::OS_WIN_REL,
            launcher_sub_channel: sub_channel::OS_WIN_REL,
            platform: Platform::Windows,
        };
        let config = resolve_game_config(&client, &params).await?;
        let parts: Vec<SplitArchivePart> = config
            .packs
            .iter()
            .map(|p| SplitArchivePart {
                url: p.url.clone(),
                size: p.package_size.parse::<u64>().expect("invalid package_size"),
            })
            .collect();
        cd = Some(ZipCentralDirectory::from_split_archive(&client, parts).await?);
    }
    pass1_enumerate(cd.as_ref().unwrap());

    // Prepare file references for passes
    let file_refs: Vec<(&str, &[u8])> = loaded.iter().map(|(n, _, d)| (*n, d.as_slice())).collect();

    // ── Pass 2: Structural Analysis ──
    pass2_structural(&file_refs);

    // ── Pass 3: S-Box Cipher Battery ──
    let _hit3 = pass3_sbox_battery(&file_refs, &dll_sboxes);

    // ── Pass 4: Alternative Ciphers ──
    let _hit4 = pass4_alt_ciphers(&file_refs);

    // ── Pass 5: Known-Plaintext ──
    let _hit5 = pass5_known_plaintext(&file_refs, reference_header.as_deref());

    // ── Pass 6: Statistics ──
    pass6_statistics(&file_refs);

    // ── Pass 7: Cross-Directory Correlation ──
    pass7_cross_correlation(&file_refs);

    // ── Pass 8: Fingerprinting ──
    pass8_fingerprint(&file_refs, &dll_sboxes);

    println!("\n{}", "=".repeat(60));
    println!("=== Phase O Complete ===");
    println!("{}", "=".repeat(60));

    Ok(())
}
