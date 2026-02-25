/// Cipher Hunt — VM + Encrypted Directory Analysis.
///
/// 6-pass binary combining deep Sagittarius VM bytecode analysis with
/// multi-sample encrypted directory analysis to identify the cipher
/// protecting 4 encrypted VFS directories.
///
/// Pass 1: HGP export → decrypt function identification (offline)
/// Pass 2: .tvm0 crypto region deep dive (offline)
/// Pass 3: Key & IV material extraction from VM bytecode (offline)
/// Pass 4: Encrypted directory multi-sample download (network)
/// Pass 5: VM-informed decryption attempts (offline, uses 1-4 results)
/// Pass 6: Decrypt pointer resolution chain (offline)
///
/// Env vars:
///   PASS=1|2|3|4|5|6  — run specific pass(es), comma-separated (default: all)
///   OFFLINE=1          — skip network pass (4), use cached samples
///   DIR=19E3AE45       — target specific encrypted directory
use backend::core::endfield::{
    config::{ResolveParams, resolve_game_config},
    constants::{app_code, channel, sub_channel},
};
use backend::core::hypergryph::launcher::Platform;
use backend::extraction::crypto_constants::*;
use backend::extraction::vfs_decrypt;
use backend::extraction::zip_range::{SplitArchivePart, ZipCentralDirectory};

use goblin::pe::PE;
use iced_x86::{
    Decoder, DecoderOptions, FlowControl, Formatter, Instruction, IntelFormatter, Mnemonic, OpKind,
    Register,
};
use reqwest::Client;
use std::collections::HashMap;

// ══════════════════════════════════════════════════════════════
// Constants
// ══════════════════════════════════════════════════════════════

const GA_PATH: &str = "./output/GameAssembly.dll";
const HGP_PATH: &str = "./output/HGP.dll";
const GA_IMAGE_BASE: u64 = 0x180000000;

const ENCRYPTED_DIRS: &[(&str, u32)] = &[
    ("19E3AE45", 0x19E3AE45),
    ("1CDDBF1F", 0x1CDDBF1F),
    ("42A8FCA6", 0x42A8FCA6),
    ("775A31D1", 0x775A31D1),
];

/// RVAs of the 6 S-boxes in GameAssembly.dll (.Sgxm1 section)
const SBOX_RVAS: [u32; 6] = [
    0x10280160, 0x10280260, 0x10280360, // Group 1: S0↔S2 inverse, S1 involution
    0x10280500, 0x10280600, 0x10280700, // Group 2: S3 involution, S4↔S5 inverse
];

/// File offsets of the 6 S-boxes
const SBOX_FILE_OFFSETS: [usize; 6] = [
    0x0FF6C960, 0x0FF6CA60, 0x0FF6CB60, 0x0FF6CD00, 0x0FF6CE00, 0x0FF6CF00,
];

/// RVA of cached beyond_setDecryptionFunc result pointer
const DECRYPT_FUNC_PTR_RVA: u32 = 0x0F60CD80;

/// Known call sites that resolve the function pointer
const CALL_SITE_RVAS: [u32; 2] = [0x0C0376FF, 0x0C08E4B0];

// ══════════════════════════════════════════════════════════════
// PE Infrastructure (from vm_disasm.rs)
// ══════════════════════════════════════════════════════════════

#[derive(Debug, Clone)]
struct Section {
    name: String,
    virtual_address: u32,
    virtual_size: u32,
    pointer_to_raw_data: u32,
    size_of_raw_data: u32,
}

impl Section {
    fn file_range(&self) -> std::ops::Range<usize> {
        let start = self.pointer_to_raw_data as usize;
        start..start + self.size_of_raw_data as usize
    }

    fn va_range(&self, image_base: u64) -> std::ops::Range<u64> {
        let start = image_base + self.virtual_address as u64;
        start..start + self.virtual_size as u64
    }
}

#[derive(Debug, Clone)]
struct ExportInfo {
    name: String,
    rva: u32,
}

struct PeInfo {
    image_base: u64,
    sections: Vec<Section>,
    exports: Vec<ExportInfo>,
}

impl PeInfo {
    fn rva_to_file_offset(&self, rva: u32) -> Option<usize> {
        for s in &self.sections {
            if rva >= s.virtual_address
                && rva < s.virtual_address + s.size_of_raw_data.max(s.virtual_size)
            {
                return Some((rva - s.virtual_address + s.pointer_to_raw_data) as usize);
            }
        }
        None
    }

    fn find_section(&self, name: &str) -> Option<&Section> {
        self.sections.iter().find(|s| s.name == name)
    }
}

fn parse_pe(data: &[u8]) -> PeInfo {
    let pe = PE::parse(data).expect("Failed to parse PE");
    let image_base = pe.image_base as u64;
    let sections = pe
        .sections
        .iter()
        .map(|s| {
            let name = String::from_utf8_lossy(
                &s.name[..s.name.iter().position(|&b| b == 0).unwrap_or(s.name.len())],
            )
            .to_string();
            Section {
                name,
                virtual_address: s.virtual_address,
                virtual_size: s.virtual_size,
                pointer_to_raw_data: s.pointer_to_raw_data,
                size_of_raw_data: s.size_of_raw_data,
            }
        })
        .collect();

    let exports = pe
        .exports
        .iter()
        .filter_map(|e| {
            e.name.map(|name| ExportInfo {
                name: name.to_string(),
                rva: e.rva as u32,
            })
        })
        .collect();

    PeInfo {
        image_base,
        sections,
        exports,
    }
}

/// Check if an address falls within any S-box VA range.
fn is_sbox_va(addr: u64) -> Option<usize> {
    let lo = GA_IMAGE_BASE + SBOX_RVAS[0] as u64;
    let hi = GA_IMAGE_BASE + SBOX_RVAS[5] as u64 + 256;
    if !(lo..hi).contains(&addr) {
        return None;
    }
    for (i, &rva) in SBOX_RVAS.iter().enumerate() {
        let sbox_va = GA_IMAGE_BASE + rva as u64;
        if addr >= sbox_va && addr < sbox_va + 256 {
            return Some(i);
        }
    }
    None
}

fn format_instr(instr: &Instruction) -> String {
    let mut formatter = IntelFormatter::new();
    formatter.options_mut().set_first_operand_char_index(10);
    let mut output = String::new();
    formatter.format(instr, &mut output);
    output
}

// ══════════════════════════════════════════════════════════════
// Helpers
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
    format!("{size:.2} {}", UNITS[i])
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

fn hex_dump(data: &[u8], base_offset: usize, max_bytes: usize) {
    let dump_len = data.len().min(max_bytes);
    for row in 0..dump_len.div_ceil(16) {
        let off = row * 16;
        if off >= dump_len {
            break;
        }
        print!("    {:08x}: ", base_offset + off);
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
        reasons.push(format!("entropy={ent:.2}"));
        confidence += 2;
    } else if ent < 6.5 {
        reasons.push(format!("entropy={ent:.2}"));
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
        reasons.push(format!("zero_run={zeros}"));
        confidence += 1;
    }

    // FlatBuffer check
    if data.len() >= 8 {
        let root_offset = u32::from_le_bytes([data[0], data[1], data[2], data[3]]);
        if root_offset > 0 && root_offset < 1000 && (root_offset as usize + 4) < data.len() {
            let root = root_offset as usize;
            let vtable_offset =
                i32::from_le_bytes([data[root], data[root + 1], data[root + 2], data[root + 3]]);
            let vtable_pos = (root as i64 - vtable_offset as i64) as usize;
            if vtable_pos < data.len().saturating_sub(4) {
                let vtable_size = u16::from_le_bytes([data[vtable_pos], data[vtable_pos + 1]]);
                if vtable_size >= 4 && (vtable_size as usize) < 1000 {
                    reasons.push(format!("flatbuf(root={root},vt_sz={vtable_size})"));
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
        println!("  *** HIT [{label}]: {desc}");
        hex_dump(data, 0, 64);
    }
    hit
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

fn load_sboxes_from_data(ga_data: &[u8], pe: &PeInfo) -> Vec<[u8; 256]> {
    let mut sboxes = Vec::new();
    for &rva in &SBOX_RVAS {
        let Some(foff) = pe.rva_to_file_offset(rva) else {
            continue;
        };
        if foff + 256 > ga_data.len() {
            continue;
        }
        let mut sbox = [0u8; 256];
        sbox.copy_from_slice(&ga_data[foff..foff + 256]);
        sboxes.push(sbox);
    }
    sboxes
}

// ══════════════════════════════════════════════════════════════
// Key candidate structure
// ══════════════════════════════════════════════════════════════

#[derive(Clone)]
struct KeyCandidate {
    data: [u8; 16],
    source: String,
    entropy: f64,
    context: String,
}

// ══════════════════════════════════════════════════════════════
// S-box reference scanning
// ══════════════════════════════════════════════════════════════

/// Scan raw bytes for S-box VA/RVA patterns. Returns (offset_in_data, sbox_index).
fn scan_sbox_refs(data: &[u8], _base_offset: usize) -> Vec<(usize, usize)> {
    let mut refs = Vec::new();

    for (i, &rva) in SBOX_RVAS.iter().enumerate() {
        let va = GA_IMAGE_BASE + rva as u64;
        let va_bytes = va.to_le_bytes();
        let rva_bytes = rva.to_le_bytes();

        // 8-byte VA matches
        for off in 0..data.len().saturating_sub(8) {
            if data[off..off + 8] == va_bytes {
                refs.push((off, i));
            }
        }
        // 4-byte RVA matches
        for off in 0..data.len().saturating_sub(4) {
            if data[off..off + 4] == rva_bytes {
                refs.push((off, i));
            }
        }
    }

    refs.sort_by_key(|(off, _)| *off);
    refs
}

/// Scan disassembled x86-64 instructions for S-box references.
fn scan_sbox_refs_disasm(code: &[u8], base_rip: u64) -> Vec<(u64, usize)> {
    let mut refs = Vec::new();
    let mut decoder = Decoder::with_ip(64, code, base_rip, DecoderOptions::NONE);
    let mut instr = Instruction::default();

    while decoder.can_decode() {
        decoder.decode_out(&mut instr);
        if instr.is_invalid() {
            continue;
        }

        for op_idx in 0..instr.op_count() {
            if instr.op_kind(op_idx) == OpKind::Memory
                && let Some(idx) = is_sbox_va(instr.memory_displacement64())
            {
                refs.push((instr.ip(), idx));
            }
            if instr.op_kind(op_idx) == OpKind::Immediate64
                && let Some(idx) = is_sbox_va(instr.immediate(op_idx))
            {
                refs.push((instr.ip(), idx));
            }
        }

        // LEA RIP-relative
        if instr.mnemonic() == Mnemonic::Lea
            && instr.op1_kind() == OpKind::Memory
            && instr.memory_base() == Register::RIP
        {
            let target = instr
                .ip()
                .wrapping_add(instr.len() as u64)
                .wrapping_add(instr.memory_displacement64());
            if let Some(idx) = is_sbox_va(target) {
                refs.push((instr.ip(), idx));
            }
        }
    }

    refs
}

// ══════════════════════════════════════════════════════════════
// Pass 1: HGP Export → Decrypt Function Identification
// ══════════════════════════════════════════════════════════════

struct ExportScore {
    name: String,
    tvm0_offset: Option<usize>, // offset within .tvm0 section data
    crypto_score: u32,
    sbox_refs: u32,
    key_refs: u32,
    rcon_refs: u32,
    crypto_const_refs: u32,
    region_entropy: f64,
}

fn pass1_hgp_export_identification(ga_data: &[u8], pe: &PeInfo) -> Vec<ExportScore> {
    println!("\n╔══════════════════════════════════════════════════════════════╗");
    println!("║  Pass 1: HGP Export → Decrypt Function Identification      ║");
    println!("╚══════════════════════════════════════════════════════════════╝\n");

    let tvm0 = match pe.find_section(".tvm0") {
        Some(s) => s,
        None => {
            println!("  .tvm0 section not found in GameAssembly.dll");
            return Vec::new();
        }
    };
    let tvm0_range = tvm0.file_range();
    let tvm0_va_range = tvm0.va_range(pe.image_base);
    let tvm0_data = &ga_data[tvm0_range.start..tvm0_range.end.min(ga_data.len())];

    println!(
        "  .tvm0: file 0x{:08X}..0x{:08X} ({}) VA 0x{:X}..0x{:X}",
        tvm0_range.start,
        tvm0_range.end,
        format_bytes(tvm0_data.len() as u64),
        tvm0_va_range.start,
        tvm0_va_range.end,
    );

    // Parse HGP.dll
    let hgp_data = match std::fs::read(HGP_PATH) {
        Ok(d) => d,
        Err(e) => {
            println!("  Cannot read {HGP_PATH}: {e}");
            return Vec::new();
        }
    };
    let hgp_pe = parse_pe(&hgp_data);
    println!(
        "  HGP.dll: {} exports, {} sections",
        hgp_pe.exports.len(),
        hgp_pe.sections.len()
    );

    // Build crypto constant patterns to scan for
    let vfs_key_bytes = VFS_AES_KEY;
    let vfs_iv_bytes = VFS_AES_IV;
    let tea_delta: u32 = 0x9E3779B9;

    let mut scores: Vec<ExportScore> = Vec::new();

    for exp in &hgp_pe.exports {
        let Some(file_off) = hgp_pe.rva_to_file_offset(exp.rva) else {
            continue;
        };
        if file_off + 64 > hgp_data.len() {
            continue;
        }

        // Disassemble export's first 64 bytes to find .tvm0 jump target
        let code = &hgp_data[file_off..(file_off + 64).min(hgp_data.len())];
        let rip = hgp_pe.image_base + exp.rva as u64;
        let mut decoder = Decoder::with_ip(64, code, rip, DecoderOptions::NONE);
        let mut instr = Instruction::default();

        let mut tvm0_target: Option<u64> = None;

        println!("\n  Export: {} (RVA 0x{:08X})", exp.name, exp.rva);

        while decoder.can_decode() {
            decoder.decode_out(&mut instr);
            if instr.is_invalid() {
                break;
            }
            let desc = format_instr(&instr);
            println!("    {:016X}  {}", instr.ip(), desc);

            // Check for VM entry pattern
            if instr.mnemonic() == Mnemonic::Lea
                && instr.op0_register() == Register::RSP
                && instr.op1_kind() == OpKind::Memory
                && instr.memory_base() == Register::RSP
            {
                println!("      ^^^ VM ENTRY PATTERN: lea rsp,[rsp+XX]");
            }
        }

        // Re-decode to find .tvm0 targets
        let mut decoder2 = Decoder::with_ip(64, code, rip, DecoderOptions::NONE);
        while decoder2.can_decode() {
            decoder2.decode_out(&mut instr);
            if instr.is_invalid() {
                break;
            }

            if instr.op0_kind() == OpKind::NearBranch64 {
                let target = instr.near_branch64();
                if target >= tvm0_va_range.start && target < tvm0_va_range.end {
                    tvm0_target = Some(target);
                    println!("      → .tvm0 offset 0x{:X}", target - tvm0_va_range.start);
                }
            }

            for op_idx in 0..instr.op_count() {
                if instr.op_kind(op_idx) == OpKind::Memory {
                    let disp = instr.memory_displacement64();
                    if disp >= tvm0_va_range.start && disp < tvm0_va_range.end {
                        tvm0_target = Some(disp);
                        println!("      → .tvm0 offset 0x{:X}", disp - tvm0_va_range.start);
                    }
                }
            }
        }

        // Analyze 2KB of bytecode at the .tvm0 entry point
        let tvm0_off = tvm0_target.map(|va| (va - tvm0_va_range.start) as usize);

        let mut score = ExportScore {
            name: exp.name.clone(),
            tvm0_offset: tvm0_off,
            crypto_score: 0,
            sbox_refs: 0,
            key_refs: 0,
            rcon_refs: 0,
            crypto_const_refs: 0,
            region_entropy: 0.0,
        };

        if let Some(off) = tvm0_off {
            let region_end = (off + 2048).min(tvm0_data.len());
            if off < tvm0_data.len() {
                let region = &tvm0_data[off..region_end];
                score.region_entropy = entropy(region);

                // Count S-box refs
                let sbox_refs = scan_sbox_refs(region, tvm0_range.start + off);
                score.sbox_refs = sbox_refs.len() as u32;

                // Count VFS key/IV byte sequence occurrences
                for window in region.windows(16) {
                    if window == vfs_key_bytes {
                        score.key_refs += 1;
                    }
                    if window == vfs_iv_bytes {
                        score.key_refs += 1;
                    }
                }

                // Count RCON bytes (first 11 are meaningful)
                for window in region.windows(11) {
                    if *window == RCON[..11] {
                        score.rcon_refs += 1;
                    }
                }

                // Crypto constants: 0x1B (AES polynomial), TEA delta
                for window in region.windows(4) {
                    let val = u32::from_le_bytes(window.try_into().unwrap());
                    if val == tea_delta {
                        score.crypto_const_refs += 1;
                    }
                }
                // Count 0x1B occurrences as potential GF(2^8) reduction
                let x1b_count = region.iter().filter(|&&b| b == 0x1B).count();
                if x1b_count > 10 {
                    score.crypto_const_refs += 1;
                }

                score.crypto_score = score.sbox_refs * 3
                    + score.key_refs * 5
                    + score.rcon_refs * 2
                    + score.crypto_const_refs;
            }
        }

        println!(
            "    CRYPTO_SCORE: {} (sbox={}, key={}, rcon={}, const={}, ent={:.2})",
            score.crypto_score,
            score.sbox_refs,
            score.key_refs,
            score.rcon_refs,
            score.crypto_const_refs,
            score.region_entropy
        );

        scores.push(score);
    }

    // Sort by crypto score descending
    scores.sort_by(|a, b| b.crypto_score.cmp(&a.crypto_score));

    println!("\n  ═══ Pass 1 Summary: Export Ranking ═══");
    for (i, s) in scores.iter().enumerate() {
        let marker = if i == 0 {
            " <<<< DECRYPT CANDIDATE"
        } else {
            ""
        };
        println!(
            "    #{}: {} — score={} (sbox={}, key={}, rcon={}, const={}){marker}",
            i + 1,
            s.name,
            s.crypto_score,
            s.sbox_refs,
            s.key_refs,
            s.rcon_refs,
            s.crypto_const_refs
        );
    }

    scores
}

// ══════════════════════════════════════════════════════════════
// Pass 2: .tvm0 Crypto Region Deep Dive
// ══════════════════════════════════════════════════════════════

struct CryptoRegionInfo {
    sbox_distribution: Vec<(usize, usize)>, // (offset, sbox_index)
    estimated_rounds: u32,
    sbox_pairs_used: Vec<(usize, usize)>, // pairs of S-box indices that alternate
}

fn pass2_tvm0_deep_dive(ga_data: &[u8], pe: &PeInfo, scores: &[ExportScore]) -> CryptoRegionInfo {
    println!("\n╔══════════════════════════════════════════════════════════════╗");
    println!("║  Pass 2: .tvm0 Crypto Region Deep Dive                     ║");
    println!("╚══════════════════════════════════════════════════════════════╝\n");

    let tvm0 = pe.find_section(".tvm0").expect(".tvm0 not found");
    let tvm0_range = tvm0.file_range();
    let tvm0_data = &ga_data[tvm0_range.start..tvm0_range.end.min(ga_data.len())];

    let mut info = CryptoRegionInfo {
        sbox_distribution: Vec::new(),
        estimated_rounds: 0,
        sbox_pairs_used: Vec::new(),
    };

    // Use top-scored export's .tvm0 entry point
    let entry_offset = scores.iter().find_map(|s| s.tvm0_offset).unwrap_or(0);

    println!("  Analyzing .tvm0 entry at offset 0x{:08X}", entry_offset);

    // Dump 8KB of bytecode for deep analysis
    let region_end = (entry_offset + 8192).min(tvm0_data.len());
    if entry_offset >= tvm0_data.len() {
        println!("  ERROR: entry offset out of range");
        return info;
    }
    let region = &tvm0_data[entry_offset..region_end];
    println!("  Region: {} bytes", region.len());

    // Step 1: Find ALL S-box references in 8KB region
    let sbox_refs = scan_sbox_refs(region, tvm0_range.start + entry_offset);
    info.sbox_distribution = sbox_refs.clone();

    println!("\n  S-box references in 8KB region: {}", sbox_refs.len());

    // Plot distribution: offset → S-box index
    let mut sbox_counts = [0u32; 6];
    for &(off, idx) in &sbox_refs {
        if idx < 6 {
            sbox_counts[idx] += 1;
        }
        println!("    S-BOX_REF offset=0x{:04X} sbox={}", off, idx);
    }

    println!("\n  S-box reference counts:");
    for (i, &count) in sbox_counts.iter().enumerate() {
        let pair_info = match i {
            0 => "encrypt (pair with S2)",
            1 => "involution",
            2 => "decrypt (pair with S0)",
            3 => "involution",
            4 => "encrypt (pair with S5)",
            5 => "decrypt (pair with S4)",
            _ => "",
        };
        if count > 0 {
            println!("    S-box {}: {} refs — {}", i, count, pair_info);
        }
    }

    // Step 2: Check for alternating S-box pairs
    if sbox_refs.len() >= 2 {
        let mut pair_histogram: HashMap<(usize, usize), u32> = HashMap::new();
        for window in sbox_refs.windows(2) {
            let pair = (window[0].1, window[1].1);
            *pair_histogram.entry(pair).or_insert(0) += 1;
        }

        println!("\n  Consecutive S-box pair histogram:");
        let mut pairs: Vec<_> = pair_histogram.iter().collect();
        pairs.sort_by(|a, b| b.1.cmp(a.1));
        for ((a, b), count) in pairs.iter().take(10) {
            println!("    S-box {} → S-box {}: {} times", a, b, count);
            if **count >= 3 {
                info.sbox_pairs_used.push((*a, *b));
            }
        }
    }

    // Step 3: Check spacing between S-box references for round structure
    if sbox_refs.len() >= 3 {
        let offsets: Vec<usize> = sbox_refs.iter().map(|(off, _)| *off).collect();
        let mut spacings: Vec<usize> = Vec::new();
        for window in offsets.windows(2) {
            spacings.push(window[1] - window[0]);
        }

        // Check if spacings are roughly uniform (fixed round structure)
        if !spacings.is_empty() {
            let avg = spacings.iter().sum::<usize>() as f64 / spacings.len() as f64;
            let variance: f64 = spacings
                .iter()
                .map(|&s| (s as f64 - avg).powi(2))
                .sum::<f64>()
                / spacings.len() as f64;
            let stdev = variance.sqrt();
            let cv = if avg > 0.0 { stdev / avg } else { f64::MAX };

            println!(
                "\n  S-box ref spacing: avg={:.1} stdev={:.1} CV={:.2}",
                avg, stdev, cv
            );
            if cv < 0.3 {
                println!("    → FIXED ROUND STRUCTURE detected (even spacing)");
                // Estimate round count from number of S-box ref groups
                info.estimated_rounds = sbox_refs.len() as u32 / 2; // rough estimate
            } else {
                println!("    → VARIABLE spacing (complex or multi-phase cipher)");
            }

            // Show first 10 spacings
            for (i, &spacing) in spacings.iter().take(10).enumerate() {
                println!("    gap[{}]: {} bytes", i, spacing);
            }
        }
    }

    // Step 4: Between S-box refs, look for repeated patterns
    println!("\n  Analyzing inter-S-box-ref content...");
    if sbox_refs.len() >= 2 {
        for window in sbox_refs.windows(2) {
            let start = window[0].0;
            let end = window[1].0;
            if end > start && end - start < 256 {
                let between = &region[start..end];
                // Look for 4-byte repeated values (potential opcodes)
                let mut four_byte_patterns: HashMap<[u8; 4], u32> = HashMap::new();
                for chunk in between.chunks_exact(4) {
                    let pattern: [u8; 4] = chunk.try_into().unwrap();
                    *four_byte_patterns.entry(pattern).or_insert(0) += 1;
                }
                let mut sorted: Vec<_> = four_byte_patterns
                    .iter()
                    .filter(|(_, count)| **count > 1)
                    .collect();
                sorted.sort_by(|a, b| b.1.cmp(a.1));
                if !sorted.is_empty() {
                    println!(
                        "    Between offsets 0x{:04X}..0x{:04X} ({} bytes):",
                        start,
                        end,
                        end - start
                    );
                    for (pattern, count) in sorted.iter().take(3) {
                        println!("      {:02x?} × {}", pattern, count);
                    }
                }
            }
        }
    }

    // Step 5: Check if any S-box refs point to unknown S-boxes
    println!("\n  Checking for unknown S-box references...");
    let all_known: Vec<u64> = SBOX_RVAS
        .iter()
        .map(|&r| GA_IMAGE_BASE + r as u64)
        .collect();
    // Scan for any 8-byte value that looks like a VA to .Sgxm1 but NOT a known S-box
    let sgxm1 = pe.find_section(".Sgxm1");
    if let Some(sgxm1_sec) = sgxm1 {
        let sgxm1_va_range = sgxm1_sec.va_range(pe.image_base);
        for off in (0..region.len().saturating_sub(8)).step_by(4) {
            let val = u64::from_le_bytes(region[off..off + 8].try_into().unwrap());
            if val >= sgxm1_va_range.start
                && val < sgxm1_va_range.end
                && !all_known.iter().any(|&k| val >= k && val < k + 256)
            {
                println!("    UNKNOWN .Sgxm1 ref at 0x{:04X}: VA 0x{:016X}", off, val);
            }
        }
    }

    // Step 6: Cross-reference with .Sgxm1 gap data
    let gap_start = SBOX_FILE_OFFSETS[2] + 256; // after S-box 2
    let gap_end = SBOX_FILE_OFFSETS[3]; // before S-box 3
    if gap_start < gap_end && gap_end <= ga_data.len() {
        let gap = &ga_data[gap_start..gap_end];
        println!(
            "\n  .Sgxm1 inter-S-box gap ({} bytes at file 0x{:08X}):",
            gap.len(),
            gap_start
        );
        hex_dump(gap, gap_start, gap.len().min(128));

        // Look for string anchors in the gap
        let gap_str = String::from_utf8_lossy(gap);
        if gap_str.contains("global-metadata.dat") {
            println!("    Contains \"global-metadata.dat\" string");
        }
    }

    // Summary
    println!("\n  ═══ Pass 2 Summary ═══");
    println!("  S-box refs in 8KB region: {}", sbox_refs.len());
    println!("  Estimated rounds: {}", info.estimated_rounds);
    println!("  S-box pairs: {:?}", info.sbox_pairs_used);

    info
}

// ══════════════════════════════════════════════════════════════
// Pass 3: Key & IV Material Extraction from VM Bytecode
// ══════════════════════════════════════════════════════════════

fn pass3_key_extraction(
    ga_data: &[u8],
    pe: &PeInfo,
    crypto_region: &CryptoRegionInfo,
    scores: &[ExportScore],
) -> Vec<KeyCandidate> {
    println!("\n╔══════════════════════════════════════════════════════════════╗");
    println!("║  Pass 3: Key & IV Material Extraction                      ║");
    println!("╚══════════════════════════════════════════════════════════════╝\n");

    let tvm0 = pe.find_section(".tvm0").expect(".tvm0 not found");
    let tvm0_range = tvm0.file_range();
    let tvm0_data = &ga_data[tvm0_range.start..tvm0_range.end.min(ga_data.len())];

    let mut candidates: Vec<KeyCandidate> = Vec::new();

    // Strategy A: Context-aware key search around S-box references
    println!("  Strategy A: Key search around S-box references...");

    let entry_offset = scores.iter().find_map(|s| s.tvm0_offset).unwrap_or(0);

    for &(sbox_off, sbox_idx) in &crypto_region.sbox_distribution {
        let abs_off = entry_offset + sbox_off;
        let scan_start = abs_off.saturating_sub(256);
        let scan_end = (abs_off + 256).min(tvm0_data.len());
        if scan_start >= scan_end || scan_end - scan_start < 16 {
            continue;
        }
        let scan_region = &tvm0_data[scan_start..scan_end];

        for block_off in (0..scan_region.len().saturating_sub(16)).step_by(4) {
            let block: [u8; 16] = scan_region[block_off..block_off + 16].try_into().unwrap();
            let ent = entropy(&block);

            // Filter: key-like entropy (not random, not zero, not all same)
            if ent > 2.5 && ent < 6.5 {
                // Deduplicate
                if !candidates.iter().any(|c| c.data == block) {
                    let source = format!("tvm0+0x{:08X}", scan_start + block_off);
                    let context = format!("near S-box {} ref at +0x{:04X}", sbox_idx, sbox_off);
                    candidates.push(KeyCandidate {
                        data: block,
                        source,
                        entropy: ent,
                        context,
                    });
                }
            }
        }
    }
    println!(
        "    Found {} candidates from S-box context",
        candidates.len()
    );

    // Strategy B: .data section config table parsing
    println!("\n  Strategy B: .data section config table...");
    if let Some(data_sec) = pe.find_section(".data") {
        let data_range = data_sec.file_range();
        let data_section = &ga_data[data_range.start..data_range.end.min(ga_data.len())];

        // Find references to S-box 3 (RVA 0x10280500) and S-box 5 (0x10280700) in .data
        let sbox3_va = (GA_IMAGE_BASE + SBOX_RVAS[3] as u64).to_le_bytes();
        let sbox5_va = (GA_IMAGE_BASE + SBOX_RVAS[5] as u64).to_le_bytes();

        for (needle_name, needle) in [("S-box3", &sbox3_va[..]), ("S-box5", &sbox5_va[..])] {
            for off in 0..data_section.len().saturating_sub(8) {
                if data_section[off..off + 8] == *needle {
                    println!(
                        "    {} ref in .data at +0x{:X} (file 0x{:X})",
                        needle_name,
                        off,
                        data_range.start + off
                    );

                    // Parse surrounding memory for key-like 16-byte blocks
                    let ctx_start = off.saturating_sub(128);
                    let ctx_end = (off + 128).min(data_section.len());
                    let ctx = &data_section[ctx_start..ctx_end];

                    for block_off in (0..ctx.len().saturating_sub(16)).step_by(8) {
                        let block: [u8; 16] = ctx[block_off..block_off + 16].try_into().unwrap();
                        let ent = entropy(&block);
                        if ent > 2.5 && ent < 6.5 && !candidates.iter().any(|c| c.data == block) {
                            candidates.push(KeyCandidate {
                                data: block,
                                source: format!(".data+0x{:X}", ctx_start + block_off),
                                entropy: ent,
                                context: format!("near {} in .data", needle_name),
                            });
                        }
                    }
                }
            }
        }
    }

    // Strategy C: Full .tvm0 S-box neighborhood scan — cluster by proximity
    println!("\n  Strategy C: Full .tvm0 S-box neighborhood scan...");
    let all_refs = scan_sbox_refs(tvm0_data, tvm0_range.start);
    println!("    Total S-box refs in .tvm0: {}", all_refs.len());

    // Cluster references by proximity (within 1KB = same function)
    let mut clusters: Vec<Vec<(usize, usize)>> = Vec::new();
    let mut current_cluster: Vec<(usize, usize)> = Vec::new();

    for &(off, idx) in &all_refs {
        if current_cluster.is_empty() || off - current_cluster.last().unwrap().0 < 1024 {
            current_cluster.push((off, idx));
        } else {
            if current_cluster.len() >= 3 {
                clusters.push(std::mem::take(&mut current_cluster));
            } else {
                current_cluster.clear();
            }
            current_cluster.push((off, idx));
        }
    }
    if current_cluster.len() >= 3 {
        clusters.push(current_cluster);
    }

    println!("    Clusters (>=3 refs within 1KB): {}", clusters.len());

    // Sort clusters by size, extract key material from largest
    clusters.sort_by_key(|c| std::cmp::Reverse(c.len()));
    let before_count = candidates.len();

    for (ci, cluster) in clusters.iter().take(5).enumerate() {
        let cluster_start = cluster.first().unwrap().0;
        let cluster_end = cluster.last().unwrap().0;
        let sbox_indices: Vec<usize> = cluster.iter().map(|(_, idx)| *idx).collect();
        println!(
            "    Cluster {}: {} refs, offsets 0x{:08X}..0x{:08X}, S-boxes: {:?}",
            ci,
            cluster.len(),
            cluster_start,
            cluster_end,
            sbox_indices
        );

        // Extract key candidates from this cluster
        let scan_start = cluster_start.saturating_sub(64);
        let scan_end = (cluster_end + 64).min(tvm0_data.len());
        if scan_end > scan_start {
            let region = &tvm0_data[scan_start..scan_end];
            for block_off in (0..region.len().saturating_sub(16)).step_by(4) {
                let block: [u8; 16] = region[block_off..block_off + 16].try_into().unwrap();
                let ent = entropy(&block);
                if ent > 2.5 && ent < 6.5 && !candidates.iter().any(|c| c.data == block) {
                    candidates.push(KeyCandidate {
                        data: block,
                        source: format!("tvm0+0x{:08X}", scan_start + block_off),
                        entropy: ent,
                        context: format!("cluster {} ({} refs)", ci, cluster.len()),
                    });
                }
            }
        }
    }
    println!(
        "    Added {} candidates from clusters",
        candidates.len() - before_count
    );

    // Strategy D: .Sgxm1 inter-S-box gap analysis
    println!("\n  Strategy D: .Sgxm1 inter-S-box gap...");
    let gap_start = SBOX_FILE_OFFSETS[2] + 256;
    let gap_end = SBOX_FILE_OFFSETS[3];
    if gap_start < gap_end && gap_end <= ga_data.len() {
        let gap = &ga_data[gap_start..gap_end];
        for block_off in (0..gap.len().saturating_sub(16)).step_by(4) {
            let block: [u8; 16] = gap[block_off..block_off + 16].try_into().unwrap();
            let ent = entropy(&block);
            if ent > 2.5 && ent < 6.5 && !candidates.iter().any(|c| c.data == block) {
                candidates.push(KeyCandidate {
                    data: block,
                    source: format!("sgxm1_gap+0x{:X}", block_off),
                    entropy: ent,
                    context: "inter-S-box gap".into(),
                });
            }
        }
    }

    // Also add known VFS key/IV and dir-hash derived keys
    candidates.push(KeyCandidate {
        data: VFS_AES_KEY,
        source: "known_VFS_AES_KEY".into(),
        entropy: entropy(&VFS_AES_KEY),
        context: "known constant".into(),
    });
    candidates.push(KeyCandidate {
        data: VFS_AES_IV,
        source: "known_VFS_AES_IV".into(),
        entropy: entropy(&VFS_AES_IV),
        context: "known constant".into(),
    });

    // Dir hash derived keys
    for &(name, hash) in ENCRYPTED_DIRS {
        let mut key = [0u8; 16];
        for i in 0..4 {
            let shifted = hash.rotate_right(i as u32 * 8);
            key[i * 4..(i + 1) * 4].copy_from_slice(&shifted.to_le_bytes());
        }
        candidates.push(KeyCandidate {
            data: key,
            source: format!("dir_hash({name})"),
            entropy: entropy(&key),
            context: "derived from dir hash".into(),
        });
    }

    // S-box fold keys
    let sboxes = load_sboxes_from_data(ga_data, pe);
    for (i, sbox) in sboxes.iter().enumerate() {
        let mut fold = [0u8; 16];
        for (j, &b) in sbox.iter().enumerate() {
            fold[j % 16] ^= b;
        }
        candidates.push(KeyCandidate {
            data: fold,
            source: format!("sbox{i}_fold"),
            entropy: entropy(&fold),
            context: "XOR-folded S-box".into(),
        });

        let mut first16 = [0u8; 16];
        first16.copy_from_slice(&sbox[..16]);
        candidates.push(KeyCandidate {
            data: first16,
            source: format!("sbox{i}_first16"),
            entropy: entropy(&first16),
            context: "first 16 bytes of S-box".into(),
        });
    }

    // Summary
    println!("\n  ═══ Pass 3 Summary ═══");
    println!("  Total key candidates: {}", candidates.len());
    println!("  Top 10 by entropy:");
    let mut by_ent = candidates.clone();
    by_ent.sort_by(|a, b| b.entropy.partial_cmp(&a.entropy).unwrap());
    for (i, c) in by_ent.iter().take(10).enumerate() {
        println!(
            "    #{}: ent={:.2} src={} ctx={} key={:02x?}",
            i + 1,
            c.entropy,
            c.source,
            c.context,
            c.data
        );
    }

    candidates
}

// ══════════════════════════════════════════════════════════════
// Pass 4: Encrypted Directory Multi-Sample Download
// ══════════════════════════════════════════════════════════════

struct DirSample {
    dir_name: String,
    filename: String,
    size: u64,
    data: Vec<u8>,
    percentile: &'static str,
}

async fn pass4_multi_sample_download(
    client: &Client,
    cd: &ZipCentralDirectory,
    dir_filter: Option<&str>,
) -> Vec<DirSample> {
    println!("\n╔══════════════════════════════════════════════════════════════╗");
    println!("║  Pass 4: Encrypted Directory Multi-Sample Download         ║");
    println!("╚══════════════════════════════════════════════════════════════╝\n");

    let output_dir = "./output/encrypted_samples";
    std::fs::create_dir_all(output_dir).ok();

    let mut all_samples: Vec<DirSample> = Vec::new();

    for &(name, _hash) in ENCRYPTED_DIRS {
        if dir_filter.is_some_and(|d| d != name) {
            continue;
        }

        let mut candidates: Vec<(String, u64)> = cd
            .filenames()
            .filter(|f| f.ends_with(".chk") && f.contains(name))
            .map(|f| {
                let size = cd.get(f).map_or(0, |e| e.uncompressed_size);
                (f.to_string(), size)
            })
            .collect();
        candidates.sort_by_key(|(_n, s)| *s);

        if candidates.is_empty() {
            println!("  {} — no .chk files found", name);
            continue;
        }

        // Pick 5 samples: smallest, 25th%, median, 75th%, largest
        let len = candidates.len();
        let mut pick_indices = vec![
            (0, "smallest"),
            (len / 4, "p25"),
            (len / 2, "median"),
            (3 * len / 4, "p75"),
            (len - 1, "largest"),
        ];
        // Deduplicate indices
        pick_indices.sort_by_key(|(i, _)| *i);
        pick_indices.dedup_by_key(|(i, _)| *i);

        println!(
            "  {} — picking {} samples from {} files",
            name,
            pick_indices.len(),
            len
        );

        for &(idx, label) in &pick_indices {
            let (ref filename, size) = candidates[idx];
            let short = filename.rsplit('/').next().unwrap_or(filename);
            let cache_path = format!("{}/{}_{}_{}", output_dir, name, label, short);

            let data = if let Ok(cached) = std::fs::read(&cache_path) {
                println!(
                    "    [cached] {} ({}) — {}",
                    label,
                    short,
                    format_bytes(size)
                );
                cached
            } else {
                println!(
                    "    [downloading] {} ({}) — {}...",
                    label,
                    short,
                    format_bytes(size)
                );
                match cd.extract(client, filename).await {
                    Ok(data) => {
                        let _ = std::fs::write(&cache_path, &data);
                        data
                    }
                    Err(e) => {
                        println!("      ERROR: {e}");
                        continue;
                    }
                }
            };

            all_samples.push(DirSample {
                dir_name: name.to_string(),
                filename: short.to_string(),
                size,
                data,
                percentile: label,
            });
        }
    }

    // Cross-file structural analysis
    println!("\n  ═══ Cross-File Structural Analysis ═══");
    for &(name, _) in ENCRYPTED_DIRS {
        if dir_filter.is_some_and(|d| d != name) {
            continue;
        }
        let dir_samples: Vec<&DirSample> =
            all_samples.iter().filter(|s| s.dir_name == name).collect();

        if dir_samples.is_empty() {
            continue;
        }

        println!("\n  [{}] {} samples:", name, dir_samples.len());

        // Compare first 8/16/32 bytes across files
        for header_len in [8, 16, 32] {
            let headers: Vec<Vec<u8>> = dir_samples
                .iter()
                .filter(|s| s.data.len() >= header_len)
                .map(|s| s.data[..header_len].to_vec())
                .collect();

            if headers.len() >= 2 {
                let all_same = headers.iter().all(|h| h == &headers[0]);
                if all_same {
                    println!("    First {} bytes: IDENTICAL across all files", header_len);
                    hex_dump(&headers[0], 0, header_len);
                } else {
                    // Check which bytes differ
                    let mut differ_positions = Vec::new();
                    for pos in 0..header_len {
                        let vals: Vec<u8> = headers.iter().map(|h| h[pos]).collect();
                        if vals.iter().any(|&v| v != vals[0]) {
                            differ_positions.push(pos);
                        }
                    }
                    println!(
                        "    First {} bytes: differ at positions {:?}",
                        header_len, differ_positions
                    );
                }
            }
        }

        // XOR pairs of same-sized files
        if dir_samples.len() >= 2 {
            for i in 0..dir_samples.len() {
                for j in (i + 1)..dir_samples.len() {
                    let a = &dir_samples[i];
                    let b = &dir_samples[j];
                    let min_len = a.data.len().min(b.data.len()).min(1024);
                    if min_len < 16 {
                        continue;
                    }
                    let xored: Vec<u8> = a.data[..min_len]
                        .iter()
                        .zip(b.data[..min_len].iter())
                        .map(|(&x, &y)| x ^ y)
                        .collect();
                    let xor_ent = entropy(&xored);
                    let xor_zeros = xored.iter().filter(|&&b| b == 0).count();
                    println!(
                        "    XOR({} ^ {}): ent={:.2} zeros={}/{}",
                        a.percentile, b.percentile, xor_ent, xor_zeros, min_len
                    );
                }
            }
        }

        // Check if stripping first N bytes produces valid VFS headers
        for strip in [8, 16, 32, 48] {
            for s in &dir_samples {
                if s.data.len() > strip + 8 {
                    let stripped = &s.data[strip..];
                    if vfs_decrypt::is_valid_header(stripped) {
                        println!(
                            "    *** VFS HEADER VALID after stripping {} bytes from {}!",
                            strip, s.percentile
                        );
                    }
                }
            }
        }

        // Size distribution summary
        let sizes: Vec<u64> = dir_samples.iter().map(|s| s.size).collect();
        let mod16 = sizes.iter().filter(|&&s| s % 16 == 0).count();
        println!(
            "    Size alignment: {} of {} files are mod16",
            mod16,
            sizes.len()
        );

        // Entropy profiles
        for s in &dir_samples {
            let probe = &s.data[..s.data.len().min(4096)];
            let ent = entropy(probe);
            println!(
                "    {} ({}): ent={:.3} size={}",
                s.percentile,
                s.filename,
                ent,
                format_bytes(s.size)
            );
        }
    }

    all_samples
}

fn load_cached_samples(dir_filter: Option<&str>) -> Vec<DirSample> {
    let sample_dir = "./output/encrypted_samples";
    let mut samples = Vec::new();

    let entries = match std::fs::read_dir(sample_dir) {
        Ok(e) => e,
        Err(_) => return samples,
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let fname = path.file_name().and_then(|n| n.to_str()).unwrap_or("");

        if let Some(filter) = dir_filter
            && !fname.starts_with(filter)
        {
            continue;
        }

        if let Ok(data) = std::fs::read(&path)
            && !data.is_empty()
        {
            // Parse dir name and percentile from filename: {DIRNAME}_{percentile}_{chkname}
            let parts: Vec<&str> = fname.splitn(3, '_').collect();
            let (dir_name, percentile) = if parts.len() >= 2 {
                (parts[0].to_string(), parts[1])
            } else {
                (fname.to_string(), "unknown")
            };

            let size = data.len() as u64;
            samples.push(DirSample {
                dir_name,
                filename: fname.to_string(),
                size,
                data,
                percentile: match percentile {
                    "smallest" => "smallest",
                    "p25" => "p25",
                    "median" => "median",
                    "p75" => "p75",
                    "largest" => "largest",
                    _ => "cached",
                },
            });
        }
    }

    samples
}

// ══════════════════════════════════════════════════════════════
// Pass 5: VM-Informed Decryption Attempts
// ══════════════════════════════════════════════════════════════

fn pass5_decryption_attempts(
    ga_data: &[u8],
    pe: &PeInfo,
    key_candidates: &[KeyCandidate],
    samples: &[DirSample],
    crypto_region: &CryptoRegionInfo,
) {
    println!("\n╔══════════════════════════════════════════════════════════════╗");
    println!("║  Pass 5: VM-Informed Decryption Attempts                   ║");
    println!("╚══════════════════════════════════════════════════════════════╝\n");

    if samples.is_empty() {
        println!("  No samples available — need Pass 4 or cached samples");
        return;
    }

    let sboxes = load_sboxes_from_data(ga_data, pe);
    if sboxes.is_empty() {
        println!("  No S-boxes loaded");
        return;
    }

    // Determine which S-box pairs to prioritize from Pass 2
    let prioritized_sbox_pairs: Vec<(usize, usize)> = if !crypto_region.sbox_pairs_used.is_empty() {
        crypto_region.sbox_pairs_used.clone()
    } else {
        // Default: try all known pairs
        vec![(0, 2), (4, 5), (1, 1), (3, 3)]
    };

    println!(
        "  {} key candidates × {} samples × {} S-box configs",
        key_candidates.len(),
        samples.len(),
        prioritized_sbox_pairs.len() + 2 // +2 for standard AES and VFS S-box
    );

    let mut total_tests = 0u32;
    let mut any_hit = false;

    // Use a subset of samples (one per dir, smallest for speed)
    let mut test_samples: Vec<&DirSample> = Vec::new();
    for &(name, _) in ENCRYPTED_DIRS {
        if let Some(s) = samples
            .iter()
            .find(|s| s.dir_name == name && s.percentile == "smallest")
        {
            test_samples.push(s);
        } else if let Some(s) = samples.iter().find(|s| s.dir_name == name) {
            test_samples.push(s);
        }
    }

    for sample in &test_samples {
        let probe = &sample.data[..sample.data.len().min(4096)];
        if probe.len() < 16 {
            continue;
        }

        println!(
            "\n  Testing {}/{} ({})...",
            sample.dir_name,
            sample.percentile,
            format_bytes(sample.size)
        );

        for key_cand in key_candidates {
            let key = &key_cand.data;

            // Test 1: Standard AES ECB/CBC/CTR with each key candidate
            {
                use aes::cipher::{BlockDecrypt, KeyInit};
                let cipher = aes::Aes128::new(key.into());
                let blocks = probe.len() / 16;
                let mut buf = probe[..blocks * 16].to_vec();
                for i in 0..blocks {
                    let block = aes::Block::from_mut_slice(&mut buf[i * 16..(i + 1) * 16]);
                    cipher.decrypt_block(block);
                }
                total_tests += 1;
                if check_and_report(&format!("stdAES-ECB {}", key_cand.source), &buf) {
                    any_hit = true;
                }
            }

            // Test 2: Standard AES CBC with zero IV
            {
                use aes::cipher::{BlockDecrypt, KeyInit};
                let cipher = aes::Aes128::new(key.into());
                let blocks = probe.len() / 16;
                let mut buf = probe[..blocks * 16].to_vec();
                let mut prev = [0u8; 16];
                for i in 0..blocks {
                    let ct: [u8; 16] = buf[i * 16..(i + 1) * 16].try_into().unwrap();
                    let mut block = aes::Block::from(ct);
                    cipher.decrypt_block(&mut block);
                    for j in 0..16 {
                        buf[i * 16 + j] = block[j] ^ prev[j];
                    }
                    prev = ct;
                }
                total_tests += 1;
                if check_and_report(&format!("stdAES-CBC-zeroIV {}", key_cand.source), &buf) {
                    any_hit = true;
                }
            }

            // Test 3: Custom S-box AES with prioritized S-box pairs
            for &(enc_idx, dec_idx) in &prioritized_sbox_pairs {
                if enc_idx >= sboxes.len() || dec_idx >= sboxes.len() {
                    continue;
                }

                // ECB decrypt with inverse S-box
                let round_keys = key_expansion(key, &sboxes[enc_idx]);
                let inv_sbox = build_inverse_sbox(&sboxes[enc_idx]);
                let blocks = probe.len() / 16;
                let mut buf = Vec::with_capacity(blocks * 16);
                for i in 0..blocks {
                    let block: [u8; 16] = probe[i * 16..(i + 1) * 16].try_into().unwrap();
                    buf.extend_from_slice(&aes_decrypt_block_sbox(&block, &round_keys, &inv_sbox));
                }
                total_tests += 1;
                if check_and_report(&format!("sbox{enc_idx}-ECB {}", key_cand.source), &buf) {
                    any_hit = true;
                }

                // CBC with zero IV
                let mut buf = probe[..blocks * 16].to_vec();
                let mut prev = [0u8; 16];
                for i in 0..blocks {
                    let ct: [u8; 16] = buf[i * 16..(i + 1) * 16].try_into().unwrap();
                    let pt = aes_decrypt_block_sbox(&ct, &round_keys, &inv_sbox);
                    for j in 0..16 {
                        buf[i * 16 + j] = pt[j] ^ prev[j];
                    }
                    prev = ct;
                }
                total_tests += 1;
                if check_and_report(
                    &format!("sbox{enc_idx}-CBC-zeroIV {}", key_cand.source),
                    &buf,
                ) {
                    any_hit = true;
                }

                // Custom VFS CTR
                let result = custom_ctr_decrypt(probe, key, &VFS_AES_IV, &sboxes[enc_idx]);
                total_tests += 1;
                if check_and_report(
                    &format!("sbox{enc_idx}-vfsCTR {}", key_cand.source),
                    &result,
                ) {
                    any_hit = true;
                }
            }

            // Test 4: VFS S-box with custom CTR
            {
                let result = custom_ctr_decrypt(probe, key, &VFS_AES_IV, &VFS_AES_SBOX);
                total_tests += 1;
                if check_and_report(&format!("vfsSbox-CTR {}", key_cand.source), &result) {
                    any_hit = true;
                }
            }
        }

        // Also try VFS process_vfs() directly and with header modifications
        total_tests += 1;
        if vfs_decrypt::is_valid_header(probe) {
            println!("  *** RAW VFS HEADER VALID!");
            match vfs_decrypt::process_vfs(probe) {
                Ok((_vfs, data)) => {
                    println!("  *** DECRYPTED via raw VFS! {} bytes", data.len());
                    hex_dump(&data, 0, 64);
                    any_hit = true;
                }
                Err(e) => println!("    VFS parse failed: {e}"),
            }
        }

        // XOR first 4/8 bytes with dir hash then try VFS
        for &(dname, dhash) in ENCRYPTED_DIRS {
            for xor_len in [4, 8] {
                if probe.len() < xor_len + 8 {
                    continue;
                }
                let mut modified = probe.to_vec();
                for (i, byte) in modified.iter_mut().enumerate().take(xor_len) {
                    *byte ^= ((dhash >> ((i % 4) * 8)) & 0xFF) as u8;
                }
                total_tests += 1;
                if vfs_decrypt::is_valid_header(&modified) {
                    println!("  *** VFS HEADER VALID after {xor_len}-byte XOR with {dname}!");
                    match vfs_decrypt::process_vfs(&modified) {
                        Ok((_vfs, data)) => {
                            println!("  *** DECRYPTED! {} bytes", data.len());
                            hex_dump(&data, 0, 64);
                            any_hit = true;
                        }
                        Err(e) => println!("    VFS parse failed: {e}"),
                    }
                }
            }
        }
    }

    println!("\n  ═══ Pass 5 Summary ═══");
    println!("  Total decryption tests: {total_tests}");
    if any_hit {
        println!("  *** BREAKTHROUGH — decryption succeeded!");
    } else {
        println!("  No hits — cipher/key combination not yet found");
    }
}

// ══════════════════════════════════════════════════════════════
// Pass 6: Decrypt Pointer Resolution Chain
// ══════════════════════════════════════════════════════════════

fn pass6_decrypt_chain(ga_data: &[u8], pe: &PeInfo) {
    println!("\n╔══════════════════════════════════════════════════════════════╗");
    println!("║  Pass 6: Decrypt Pointer Resolution Chain                  ║");
    println!("╚══════════════════════════════════════════════════════════════╝\n");

    // Step 1: Find the cached function pointer
    let ptr_va = GA_IMAGE_BASE + DECRYPT_FUNC_PTR_RVA as u64;
    println!(
        "  Cached decrypt func pointer: VA 0x{:016X} (RVA 0x{:08X})",
        ptr_va, DECRYPT_FUNC_PTR_RVA
    );

    if let Some(ptr_off) = pe.rva_to_file_offset(DECRYPT_FUNC_PTR_RVA)
        && ptr_off + 8 <= ga_data.len()
    {
        let stored_val = u64::from_le_bytes(ga_data[ptr_off..ptr_off + 8].try_into().unwrap());
        println!("  Static value at pointer location: 0x{:016X}", stored_val);
    }

    // Step 2: Disassemble the 2 known call sites
    println!("\n  Known call sites:");
    for (i, &rva) in CALL_SITE_RVAS.iter().enumerate() {
        let va = GA_IMAGE_BASE + rva as u64;
        let Some(file_off) = pe.rva_to_file_offset(rva) else {
            println!(
                "    Call site {} (RVA 0x{:08X}): cannot resolve",
                i + 1,
                rva
            );
            continue;
        };

        println!("\n  Call site {} at VA 0x{:016X}:", i + 1, va);

        // Disassemble 256 bytes of context around the call site
        let ctx_start = file_off.saturating_sub(64);
        let ctx_end = (file_off + 256).min(ga_data.len());
        let code = &ga_data[ctx_start..ctx_end];
        let ctx_rva = rva.saturating_sub(64);
        let ctx_rip = GA_IMAGE_BASE + ctx_rva as u64;

        let mut decoder = Decoder::with_ip(64, code, ctx_rip, DecoderOptions::NONE);
        let mut instr = Instruction::default();
        let mut n = 0;

        while decoder.can_decode() && n < 40 {
            decoder.decode_out(&mut instr);
            if instr.is_invalid() {
                continue;
            }
            n += 1;
            let desc = format_instr(&instr);

            // Mark the call site and any references to the function pointer
            let mut markers = Vec::new();
            if instr.ip() == va {
                markers.push("CALL_SITE");
            }

            // Check for references to the function pointer
            for op_idx in 0..instr.op_count() {
                if instr.op_kind(op_idx) == OpKind::Memory {
                    let disp = instr.memory_displacement64();
                    if disp == ptr_va {
                        markers.push("READS_DECRYPT_PTR");
                    }
                }
            }

            // Check LEA targets
            if instr.mnemonic() == Mnemonic::Lea
                && instr.op1_kind() == OpKind::Memory
                && instr.memory_base() == Register::RIP
            {
                let target = instr
                    .ip()
                    .wrapping_add(instr.len() as u64)
                    .wrapping_add(instr.memory_displacement64());
                if target == ptr_va {
                    markers.push("LEA_DECRYPT_PTR");
                }
            }

            let marker_str = if markers.is_empty() {
                String::new()
            } else {
                format!(" <<<< {}", markers.join(", "))
            };
            println!("    {:016X}  {}{}", instr.ip(), desc, marker_str);
        }
    }

    // Step 3: Search for "Unsafe.VFS.dll" references
    println!("\n  Step 3: Searching for \"Unsafe.VFS.dll\" string references...");
    let needle = b"Unsafe.VFS.dll";
    let mut found_offsets = Vec::new();

    for off in 0..ga_data.len().saturating_sub(needle.len()) {
        if ga_data[off..off + needle.len()] == *needle {
            found_offsets.push(off);
        }
    }

    println!("    Found {} occurrences", found_offsets.len());
    for &off in found_offsets.iter().take(5) {
        // Show context
        let ctx_start = off.saturating_sub(16);
        let ctx_end = (off + needle.len() + 32).min(ga_data.len());
        println!("    At file offset 0x{:08X}:", off);
        hex_dump(&ga_data[ctx_start..ctx_end], ctx_start, ctx_end - ctx_start);

        // Try to find which section this is in
        for s in &pe.sections {
            let sec_start = s.pointer_to_raw_data as usize;
            let sec_end = sec_start + s.size_of_raw_data as usize;
            if off >= sec_start && off < sec_end {
                println!("      In section: {}", s.name);
                break;
            }
        }
    }

    // Step 4: Search for "HGP" string references (indirect loading)
    println!("\n  Step 4: Searching for HGP-related strings...");
    let hgp_needles: &[&[u8]] = &[
        b"HGP.dll",
        b"H\0G\0P\0.\0d\0l\0l", // UTF-16
        b"beyond_setDecryptionFunc",
        b"luaopen_xlua",
        b"__InternalDynamic",
    ];

    for needle in hgp_needles {
        let needle_str = String::from_utf8_lossy(needle);
        let mut hits = 0u32;
        for off in 0..ga_data.len().saturating_sub(needle.len()) {
            if ga_data[off..off + needle.len()] == **needle {
                hits += 1;
                if hits <= 3 {
                    // Find section
                    let section_name = pe
                        .sections
                        .iter()
                        .find(|s| {
                            let start = s.pointer_to_raw_data as usize;
                            let end = start + s.size_of_raw_data as usize;
                            off >= start && off < end
                        })
                        .map(|s| s.name.as_str())
                        .unwrap_or("unknown");
                    println!(
                        "    \"{}\" at file 0x{:08X} [{}]",
                        needle_str, off, section_name
                    );
                }
            }
        }
        if hits > 3 {
            println!("    \"{}\" — {} total hits", needle_str, hits);
        } else if hits == 0 {
            println!("    \"{}\" — not found", needle_str);
        }
    }

    // Step 5: Follow call sites forward + scan for S-box refs in context
    println!("\n  Step 5: Forward trace from call sites...");
    for &rva in &CALL_SITE_RVAS {
        let Some(file_off) = pe.rva_to_file_offset(rva) else {
            continue;
        };

        let end = (file_off + 512).min(ga_data.len());
        let code = &ga_data[file_off..end];
        let rip = GA_IMAGE_BASE + rva as u64;

        // Check for S-box references in the 512-byte call site context
        let sbox_disasm_refs = scan_sbox_refs_disasm(code, rip);
        if !sbox_disasm_refs.is_empty() {
            println!(
                "    S-box refs near call site RVA 0x{:08X}: {} refs",
                rva,
                sbox_disasm_refs.len()
            );
            for (addr, idx) in &sbox_disasm_refs {
                println!("      VA 0x{:016X} → S-box {}", addr, idx);
            }
        }

        let mut decoder = Decoder::with_ip(64, code, rip, DecoderOptions::NONE);
        let mut instr = Instruction::default();
        let mut calls_found = Vec::new();

        while decoder.can_decode() {
            decoder.decode_out(&mut instr);
            if instr.is_invalid() {
                continue;
            }

            if instr.flow_control() == FlowControl::Call && instr.op0_kind() == OpKind::NearBranch64
            {
                let target = instr.near_branch64();
                let target_rva = (target - GA_IMAGE_BASE) as u32;

                // Check which section the target is in
                let section_name = pe
                    .sections
                    .iter()
                    .find(|s| {
                        target_rva >= s.virtual_address
                            && target_rva < s.virtual_address + s.virtual_size
                    })
                    .map(|s| s.name.as_str())
                    .unwrap_or("unknown");

                calls_found.push((instr.ip(), target, section_name.to_string()));
            }

            // Stop at return
            if instr.mnemonic() == Mnemonic::Ret {
                break;
            }
        }

        println!(
            "    From RVA 0x{:08X}: {} calls found",
            rva,
            calls_found.len()
        );
        for (from, to, section) in &calls_found {
            let to_rva = (*to - GA_IMAGE_BASE) as u32;
            println!(
                "      {:016X} → {:016X} (RVA 0x{:08X}) [{}]",
                from, to, to_rva, section
            );
        }
    }

    println!("\n  ═══ Pass 6 Summary ═══");
    println!("  Traced decrypt pointer resolution chain.");
    println!("  Check output above for Unsafe.VFS.dll integration points.");
}

// ══════════════════════════════════════════════════════════════
// Main
// ══════════════════════════════════════════════════════════════

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("╔══════════════════════════════════════════════════════════════╗");
    println!("║  Cipher Hunt: VM + Encrypted Directory Analysis            ║");
    println!("║  6-Pass Deep Bytecode + Multi-Sample Cipher Identification ║");
    println!("╚══════════════════════════════════════════════════════════════╝\n");

    let output_dir = "./output";
    std::fs::create_dir_all(output_dir)?;
    std::fs::create_dir_all("./output/encrypted_samples")?;

    // Parse env vars
    let pass_filter = std::env::var("PASS").ok();
    let should_run = |pass: &str| -> bool {
        pass_filter
            .as_ref()
            .is_none_or(|f| f.split(',').any(|p| p.trim() == pass))
    };
    let offline = std::env::var("OFFLINE").is_ok_and(|v| v == "1");
    let dir_filter = std::env::var("DIR").ok();

    if offline {
        println!("[*] OFFLINE mode — skipping network pass (4)");
    }
    if let Some(ref df) = dir_filter {
        println!("[*] Targeting directory: {df}");
    }
    if let Some(ref pf) = pass_filter {
        println!("[*] Running passes: {pf}");
    }

    // Load GameAssembly.dll (needed for all offline passes)
    let ga_data;
    let pe;
    let need_ga =
        should_run("1") || should_run("2") || should_run("3") || should_run("5") || should_run("6");

    if need_ga {
        println!("[*] Loading GameAssembly.dll...");
        ga_data = std::fs::read(GA_PATH).map_err(|e| {
            format!("Cannot read {GA_PATH}: {e}. Place GameAssembly.dll in ./output/")
        })?;
        println!("    {} loaded", format_bytes(ga_data.len() as u64));
        pe = parse_pe(&ga_data);

        println!("    Sections:");
        for s in &pe.sections {
            println!(
                "      {:10} VA 0x{:08X} size {} (file 0x{:08X}..0x{:08X})",
                s.name,
                s.virtual_address,
                format_bytes(s.virtual_size as u64),
                s.pointer_to_raw_data,
                s.pointer_to_raw_data + s.size_of_raw_data
            );
        }
    } else {
        // Dummy — won't be used
        ga_data = Vec::new();
        pe = PeInfo {
            image_base: 0,
            sections: Vec::new(),
            exports: Vec::new(),
        };
    }

    // ── Pass 1: HGP Export Identification ──
    let mut export_scores: Vec<ExportScore> = Vec::new();
    if should_run("1") {
        export_scores = pass1_hgp_export_identification(&ga_data, &pe);
    }

    // ── Pass 2: .tvm0 Crypto Region Deep Dive ──
    let mut crypto_region = CryptoRegionInfo {
        sbox_distribution: Vec::new(),
        estimated_rounds: 0,
        sbox_pairs_used: Vec::new(),
    };
    if should_run("2") {
        crypto_region = pass2_tvm0_deep_dive(&ga_data, &pe, &export_scores);
    }

    // ── Pass 3: Key Material Extraction ──
    let mut key_candidates: Vec<KeyCandidate> = Vec::new();
    if should_run("3") {
        key_candidates = pass3_key_extraction(&ga_data, &pe, &crypto_region, &export_scores);
    }

    // ── Pass 4: Encrypted Dir Multi-Sample Download ──
    let mut samples: Vec<DirSample> = Vec::new();
    if should_run("4") {
        if offline {
            println!("\n[*] OFFLINE mode — loading cached samples for Pass 4...");
            samples = load_cached_samples(dir_filter.as_deref());
            println!("    Loaded {} cached samples", samples.len());
        } else {
            let client = Client::new();
            let params = ResolveParams {
                app_code: app_code::game::OS_WIN_REL,
                launcher_app_code: app_code::launcher::OS_WIN_REL,
                channel: channel::OS_WIN_REL,
                sub_channel: sub_channel::OS_WIN_REL,
                launcher_sub_channel: sub_channel::OS_WIN_REL,
                platform: Platform::Windows,
            };
            println!("\n[*] Resolving game config...");
            let config = resolve_game_config(&client, &params).await?;
            let parts: Vec<SplitArchivePart> = config
                .packs
                .iter()
                .map(|p| SplitArchivePart {
                    url: p.url.clone(),
                    size: p.package_size.parse::<u64>().expect("invalid package_size"),
                })
                .collect();
            println!("[*] Building ZIP central directory...");
            let cd = ZipCentralDirectory::from_split_archive(&client, parts).await?;
            samples = pass4_multi_sample_download(&client, &cd, dir_filter.as_deref()).await;
        }
    }

    // ── Pass 5: VM-Informed Decryption Attempts ──
    if should_run("5") {
        // Load cached samples if we don't have any
        if samples.is_empty() {
            println!("\n[*] Loading cached samples for Pass 5...");
            samples = load_cached_samples(dir_filter.as_deref());
            println!("    Loaded {} cached samples", samples.len());
        }
        // Generate minimal key candidates if we don't have any from Pass 3
        if key_candidates.is_empty() {
            println!("[*] No key candidates from Pass 3, using known constants...");
            key_candidates.push(KeyCandidate {
                data: VFS_AES_KEY,
                source: "VFS_AES_KEY".into(),
                entropy: entropy(&VFS_AES_KEY),
                context: "known".into(),
            });
            key_candidates.push(KeyCandidate {
                data: VFS_AES_IV,
                source: "VFS_AES_IV".into(),
                entropy: entropy(&VFS_AES_IV),
                context: "known".into(),
            });
        }
        pass5_decryption_attempts(&ga_data, &pe, &key_candidates, &samples, &crypto_region);
    }

    // ── Pass 6: Decrypt Pointer Resolution Chain ──
    if should_run("6") {
        pass6_decrypt_chain(&ga_data, &pe);
    }

    println!("\n╔══════════════════════════════════════════════════════════════╗");
    println!("║  Cipher Hunt Complete                                      ║");
    println!("╚══════════════════════════════════════════════════════════════╝");

    Ok(())
}
