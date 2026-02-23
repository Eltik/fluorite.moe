/// Phase 2b: Deep investigation of .Sgxm1 S-boxes and beyond_setDecryptionFunc.
///
/// Follow-up to dll_analyze.rs Phase K findings:
/// - 6 S-box permutations in .Sgxm1 (custom VM protection)
/// - "beyond_setDecryptionFunc" string in .rdata
/// - No RIP-relative cross-references to the S-boxes
use backend::extraction::crypto_constants::*;
use goblin::pe::PE;
use iced_x86::{Decoder, DecoderOptions, Formatter, IntelFormatter};
use std::fs;

// ══════════════════════════════════════════════════════════════
// Constants
// ══════════════════════════════════════════════════════════════

const DLL_PATH: &str = "./output/GameAssembly.dll";
const IMAGE_BASE: u64 = 0x180000000;

const SBOX_FILE_OFFSETS: [usize; 6] = [
    0x0FF6C960, 0x0FF6CA60, 0x0FF6CB60, // Group 1
    0x0FF6CD00, 0x0FF6CE00, 0x0FF6CF00, // Group 2
];

const SBOX_RVAS: [u32; 6] = [
    0x10280160, 0x10280260, 0x10280360, // Group 1
    0x10280500, 0x10280600, 0x10280700, // Group 2
];

/// Standard AES (Rijndael) S-box for comparison.
#[rustfmt::skip]
const RIJNDAEL_SBOX: [u8; 256] = [
    0x63,0x7C,0x77,0x7B,0xF2,0x6B,0x6F,0xC5,0x30,0x01,0x67,0x2B,0xFE,0xD7,0xAB,0x76,
    0xCA,0x82,0xC9,0x7D,0xFA,0x59,0x47,0xF0,0xAD,0xD4,0xA2,0xAF,0x9C,0xA4,0x72,0xC0,
    0xB7,0xFD,0x93,0x26,0x36,0x3F,0xF7,0xCC,0x34,0xA5,0xE5,0xF1,0x71,0xD8,0x31,0x15,
    0x04,0xC7,0x23,0xC3,0x18,0x96,0x05,0x9A,0x07,0x12,0x80,0xE2,0xEB,0x27,0xB2,0x75,
    0x09,0x83,0x2C,0x1A,0x1B,0x6E,0x5A,0xA0,0x52,0x3B,0xD6,0xB3,0x29,0xE3,0x2F,0x84,
    0x53,0xD1,0x00,0xED,0x20,0xFC,0xB1,0x5B,0x6A,0xCB,0xBE,0x39,0x4A,0x4C,0x58,0xCF,
    0xD0,0xEF,0xAA,0xFB,0x43,0x4D,0x33,0x85,0x45,0xF9,0x02,0x7F,0x50,0x3C,0x9F,0xA8,
    0x51,0xA3,0x40,0x8F,0x92,0x9D,0x38,0xF5,0xBC,0xB6,0xDA,0x21,0x10,0xFF,0xF3,0xD2,
    0xCD,0x0C,0x13,0xEC,0x5F,0x97,0x44,0x17,0xC4,0xA7,0x7E,0x3D,0x64,0x5D,0x19,0x73,
    0x60,0x81,0x4F,0xDC,0x22,0x2A,0x90,0x88,0x46,0xEE,0xB8,0x14,0xDE,0x5E,0x0B,0xDB,
    0xE0,0x32,0x3A,0x0A,0x49,0x06,0x24,0x5C,0xC2,0xD3,0xAC,0x62,0x91,0x95,0xE4,0x79,
    0xE7,0xC8,0x37,0x6D,0x8D,0xD5,0x4E,0xA9,0x6C,0x56,0xF4,0xEA,0x65,0x7A,0xAE,0x08,
    0xBA,0x78,0x25,0x2E,0x1C,0xA6,0xB4,0xC6,0xE8,0xDD,0x74,0x1F,0x4B,0xBD,0x8B,0x8A,
    0x70,0x3E,0xB5,0x66,0x48,0x03,0xF6,0x0E,0x61,0x35,0x57,0xB9,0x86,0xC1,0x1D,0x9E,
    0xE1,0xF8,0x98,0x11,0x69,0xD9,0x8E,0x94,0x9B,0x1E,0x87,0xE9,0xCE,0x55,0x28,0xDF,
    0x8C,0xA1,0x89,0x0D,0xBF,0xE6,0x42,0x68,0x41,0x99,0x2D,0x0F,0xB0,0x54,0xBB,0x16,
];

// ══════════════════════════════════════════════════════════════
// Shared Infrastructure (from dll_analyze.rs)
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
}

struct PeInfo {
    sections: Vec<Section>,
}

impl PeInfo {
    fn file_offset_to_rva(&self, offset: usize) -> Option<u32> {
        let off = offset as u32;
        for s in &self.sections {
            if off >= s.pointer_to_raw_data && off < s.pointer_to_raw_data + s.size_of_raw_data {
                return Some(off - s.pointer_to_raw_data + s.virtual_address);
            }
        }
        None
    }

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
    PeInfo { sections }
}

fn disassemble_around(data: &[u8], pe: &PeInfo, file_offset: usize, label: &str, count: usize) {
    let start = file_offset.saturating_sub(64);
    let end = (file_offset + 128).min(data.len());
    let code = &data[start..end];
    let start_rva = pe.file_offset_to_rva(start).unwrap_or(0);
    let rip = IMAGE_BASE + start_rva as u64;

    let mut decoder = Decoder::with_ip(64, code, rip, DecoderOptions::NONE);
    let mut formatter = IntelFormatter::new();
    formatter.options_mut().set_first_operand_char_index(10);

    let mut output = String::new();
    let mut instr = iced_x86::Instruction::default();
    let mut n = 0;

    println!("\n  --- {} ---", label);
    while decoder.can_decode() && n < count {
        decoder.decode_out(&mut instr);
        output.clear();
        formatter.format(&instr, &mut output);

        let instr_file = pe.rva_to_file_offset((instr.ip() - IMAGE_BASE) as u32);
        let marker = if instr_file.is_some_and(|off| off.abs_diff(file_offset) < 8) {
            " <<<<".to_string()
        } else {
            String::new()
        };

        println!("    {:016X}  {}{}", instr.ip(), output, marker);
        n += 1;
    }
}

// ══════════════════════════════════════════════════════════════
// Pass A: S-box Relationship Analysis
// ══════════════════════════════════════════════════════════════

fn extract_sboxes(data: &[u8]) -> [[u8; 256]; 6] {
    let mut sboxes = [[0u8; 256]; 6];
    for (i, &off) in SBOX_FILE_OFFSETS.iter().enumerate() {
        sboxes[i].copy_from_slice(&data[off..off + 256]);
    }
    sboxes
}

fn compute_inverse(sbox: &[u8; 256]) -> [u8; 256] {
    let mut inv = [0u8; 256];
    for (i, &v) in sbox.iter().enumerate() {
        inv[v as usize] = i as u8;
    }
    inv
}

fn are_inverses(a: &[u8; 256], b: &[u8; 256]) -> bool {
    (0..256).all(|x| b[a[x] as usize] == x as u8)
}

fn hamming_distance(a: &[u8; 256], b: &[u8; 256]) -> usize {
    a.iter().zip(b.iter()).filter(|&(x, y)| x != y).count()
}

fn fixed_point_count(sbox: &[u8; 256]) -> usize {
    (0..256).filter(|&i| sbox[i] == i as u8).count()
}

fn cycle_structure(sbox: &[u8; 256]) -> Vec<usize> {
    let mut visited = [false; 256];
    let mut cycles = Vec::new();
    for start in 0..256 {
        if visited[start] {
            continue;
        }
        let mut len = 0;
        let mut pos = start;
        loop {
            visited[pos] = true;
            pos = sbox[pos] as usize;
            len += 1;
            if pos == start {
                break;
            }
        }
        cycles.push(len);
    }
    cycles.sort();
    cycles
}

/// Compute DDT max entry for an S-box.
fn ddt_max(sbox: &[u8; 256]) -> u16 {
    let mut max = 0u16;
    for delta_in in 1u16..256 {
        let mut counts = [0u16; 256];
        for x in 0u16..256 {
            let delta_out = sbox[x as usize] ^ sbox[(x ^ delta_in) as usize];
            counts[delta_out as usize] += 1;
        }
        for &c in &counts {
            max = max.max(c);
        }
    }
    max
}

fn pass_a_sbox_analysis(data: &[u8]) {
    println!("\n╔══════════════════════════════════════════════════════════════╗");
    println!("║  Pass A: S-box Relationship Analysis                       ║");
    println!("╚══════════════════════════════════════════════════════════════╝\n");

    let sboxes = extract_sboxes(data);

    // Dump first 64 bytes of each
    for (i, sbox) in sboxes.iter().enumerate() {
        let group = if i < 3 { 1 } else { 2 };
        println!(
            "  S-box {} (Group {}) at file:0x{:08X} rva:0x{:08X}:",
            i, group, SBOX_FILE_OFFSETS[i], SBOX_RVAS[i]
        );
        hex_dump(sbox, SBOX_FILE_OFFSETS[i], 64);
        println!();
    }

    // Inverse pair check
    println!("  === Inverse Pair Check ===");
    for i in 0..6 {
        for j in (i + 1)..6 {
            if are_inverses(&sboxes[i], &sboxes[j]) {
                println!(
                    "    \x1b[1;32mS-box {} and S-box {} are INVERSES\x1b[0m",
                    i, j
                );
            }
        }
        // Also check self-inverse
        if are_inverses(&sboxes[i], &sboxes[i]) {
            println!("    S-box {} is SELF-INVERSE (involution)", i);
        }
    }

    // Check if any S-box's inverse matches another
    println!("\n  === Computed Inverses ===");
    for (i, sbox) in sboxes.iter().enumerate() {
        let inv = compute_inverse(sbox);
        // Check against VFS_AES_SBOX
        let vfs_dist = hamming_distance(&inv, &VFS_AES_SBOX);
        if vfs_dist == 0 {
            println!(
                "    \x1b[1;32mInverse of S-box {} == VFS_AES_SBOX!\x1b[0m",
                i
            );
        } else if vfs_dist < 64 {
            println!(
                "    Inverse of S-box {} has {} byte diffs from VFS_AES_SBOX",
                i, vfs_dist
            );
        }
    }

    // Hamming distances
    println!("\n  === Hamming Distances ===");
    println!("    {:>8} {:>8} {:>8}", "SBox", "vs VFS", "vs Rij");
    for (i, sbox) in sboxes.iter().enumerate() {
        let vfs_d = hamming_distance(sbox, &VFS_AES_SBOX);
        let rij_d = hamming_distance(sbox, &RIJNDAEL_SBOX);
        println!("    {:>8} {:>8} {:>8}", i, vfs_d, rij_d);
    }

    // Pairwise distances
    println!("\n  === Pairwise Hamming Distances ===");
    print!("    {:>4}", "");
    for j in 0..6 {
        print!(" {:>4}", j);
    }
    println!();
    for i in 0..6 {
        print!("    {:>4}", i);
        for j in 0..6 {
            if i == j {
                print!("    -");
            } else {
                print!(" {:>4}", hamming_distance(&sboxes[i], &sboxes[j]));
            }
        }
        println!();
    }

    // DDT, fixed points, cycles
    println!("\n  === Cryptographic Properties ===");
    println!(
        "    {:>4} {:>8} {:>10} {:>8} {:>10}",
        "SBox", "DDT max", "Fixed pts", "Cycles", "Max cycle"
    );
    for (i, sbox) in sboxes.iter().enumerate() {
        let ddt = ddt_max(sbox);
        let fp = fixed_point_count(sbox);
        let cyc = cycle_structure(sbox);
        let max_cyc = cyc.iter().max().copied().unwrap_or(0);
        println!(
            "    {:>4} {:>8} {:>10} {:>8} {:>10}",
            i,
            ddt,
            fp,
            cyc.len(),
            max_cyc
        );
    }

    // Reference: VFS_AES_SBOX and Rijndael
    println!("\n  === Reference S-boxes ===");
    let vfs_ddt = ddt_max(&VFS_AES_SBOX);
    let vfs_fp = fixed_point_count(&VFS_AES_SBOX);
    let rij_ddt = ddt_max(&RIJNDAEL_SBOX);
    let rij_fp = fixed_point_count(&RIJNDAEL_SBOX);
    println!("    VFS_AES:  DDT max={}, fixed pts={}", vfs_ddt, vfs_fp);
    println!("    Rijndael: DDT max={}, fixed pts={}", rij_ddt, rij_fp);
}

// ══════════════════════════════════════════════════════════════
// Pass B: Gap and Context Analysis
// ══════════════════════════════════════════════════════════════

fn pass_b_gap_context(data: &[u8], pe: &PeInfo) {
    println!("\n╔══════════════════════════════════════════════════════════════╗");
    println!("║  Pass B: Gap and Context Analysis                          ║");
    println!("╚══════════════════════════════════════════════════════════════╝\n");

    // Gap between Group 1 and Group 2
    let gap_start = SBOX_FILE_OFFSETS[2] + 256; // end of S-box 2
    let gap_end = SBOX_FILE_OFFSETS[3]; // start of S-box 3
    let gap_size = gap_end - gap_start;

    println!(
        "  Gap between Group 1 and Group 2: {} bytes (0x{:X})",
        gap_size, gap_size
    );
    println!("  File range: 0x{:08X}..0x{:08X}\n", gap_start, gap_end);

    if gap_end <= data.len() {
        let gap_data = &data[gap_start..gap_end];
        hex_dump(gap_data, gap_start, gap_size);
        println!();
        println!("    Entropy: {:.3}", entropy(gap_data));

        // Check if gap contains a permutation (first 256 bytes if large enough)
        if gap_size >= 256 {
            let perm = &gap_data[..256];
            if crate::is_permutation_slice(perm) {
                println!("    \x1b[1;32mFirst 256 bytes of gap is a PERMUTATION!\x1b[0m");
            }
        }

        // Check for 16-byte aligned key material
        let mut key_candidates = Vec::new();
        let aligned = (gap_start + 15) & !15;
        let mut off = aligned;
        while off + 16 <= gap_end {
            let block = &data[off..off + 16];
            if !block.iter().all(|&b| b == 0) && !block.iter().all(|&b| b == 0xFF) {
                let ent = entropy(block);
                if ent > 2.5 && ent < 4.5 {
                    key_candidates.push((off, ent));
                }
            }
            off += 16;
        }
        if !key_candidates.is_empty() {
            println!("\n    Key/IV candidates in gap:");
            for (off, ent) in &key_candidates {
                let rva = pe.file_offset_to_rva(*off).unwrap_or(0);
                print!(
                    "      file:0x{:08X} rva:0x{:08X} ent={:.3}: ",
                    off, rva, ent
                );
                for &b in &data[*off..*off + 16] {
                    print!("{:02x} ", b);
                }
                println!();
            }
        }
    }

    // Context before S-box 0
    let pre_start = SBOX_FILE_OFFSETS[0].saturating_sub(256);
    let pre_end = SBOX_FILE_OFFSETS[0];
    println!("\n  === 256 bytes BEFORE S-box 0 ===");
    println!("  File range: 0x{:08X}..0x{:08X}", pre_start, pre_end);
    if pre_end <= data.len() {
        let pre_data = &data[pre_start..pre_end];
        hex_dump(pre_data, pre_start, 256);
        let ent = entropy(pre_data);
        let perm = is_permutation_slice(pre_data);
        println!("    Entropy: {:.3}, Permutation: {}", ent, perm);
    }

    // Context after S-box 5
    let post_start = SBOX_FILE_OFFSETS[5] + 256;
    let post_end = (post_start + 256).min(data.len());
    println!("\n  === 256 bytes AFTER S-box 5 ===");
    println!("  File range: 0x{:08X}..0x{:08X}", post_start, post_end);
    if post_end <= data.len() {
        let post_data = &data[post_start..post_end];
        hex_dump(post_data, post_start, 256);
        let ent = entropy(post_data);
        let perm = is_permutation_slice(post_data);
        println!("    Entropy: {:.3}, Permutation: {}", ent, perm);
    }
}

fn is_permutation_slice(data: &[u8]) -> bool {
    if data.len() != 256 {
        return false;
    }
    let mut seen = [false; 256];
    for &b in data {
        if seen[b as usize] {
            return false;
        }
        seen[b as usize] = true;
    }
    true
}

// ══════════════════════════════════════════════════════════════
// Pass C: "beyond_*" String Discovery
// ══════════════════════════════════════════════════════════════

struct BeyondString {
    file_offset: usize,
    rva: u32,
    string: String,
}

fn pass_c_beyond_strings(data: &[u8], pe: &PeInfo) -> Vec<BeyondString> {
    println!("\n╔══════════════════════════════════════════════════════════════╗");
    println!("║  Pass C: \"beyond_*\" String Discovery                       ║");
    println!("╚══════════════════════════════════════════════════════════════╝\n");

    let mut results = Vec::new();

    // Search all sections for null-terminated strings containing "beyond_"
    for sec in &pe.sections {
        let range = sec.file_range();
        let start = range.start.min(data.len());
        let end = range.end.min(data.len());
        let sec_data = &data[start..end];

        // Extract null-terminated strings
        let mut str_start = 0;
        let mut current = String::new();

        for (i, &b) in sec_data.iter().enumerate() {
            if b.is_ascii_graphic() || b == b' ' {
                if current.is_empty() {
                    str_start = i;
                }
                current.push(b as char);
            } else {
                if current.len() >= 6 && current.contains("beyond_") {
                    let file_off = start + str_start;
                    let rva = pe.file_offset_to_rva(file_off).unwrap_or(0);
                    results.push(BeyondString {
                        file_offset: file_off,
                        rva,
                        string: current.clone(),
                    });
                }
                current.clear();
            }
        }
        if current.len() >= 6 && current.contains("beyond_") {
            let file_off = start + str_start;
            let rva = pe.file_offset_to_rva(file_off).unwrap_or(0);
            results.push(BeyondString {
                file_offset: file_off,
                rva,
                string: current,
            });
        }
    }

    println!("  Found {} \"beyond_*\" strings:\n", results.len());
    for bs in &results {
        println!(
            "    file:0x{:08X} rva:0x{:08X} \"{}\"",
            bs.file_offset, bs.rva, bs.string
        );

        // Print neighboring strings (±128 bytes)
        let ctx_start = bs.file_offset.saturating_sub(128);
        let ctx_end = (bs.file_offset + bs.string.len() + 128).min(data.len());
        let ctx_data = &data[ctx_start..ctx_end];

        let mut neighbors = Vec::new();
        let mut ns_start = 0;
        let mut ns = String::new();

        for (i, &b) in ctx_data.iter().enumerate() {
            if b.is_ascii_graphic() || b == b' ' {
                if ns.is_empty() {
                    ns_start = i;
                }
                ns.push(b as char);
            } else {
                if ns.len() >= 4 && !ns.contains("beyond_") {
                    neighbors.push((ctx_start + ns_start, ns.clone()));
                }
                ns.clear();
            }
        }
        if ns.len() >= 4 && !ns.contains("beyond_") {
            neighbors.push((ctx_start + ns_start, ns));
        }

        if !neighbors.is_empty() {
            println!("      Neighbors:");
            for (off, s) in neighbors.iter().take(8) {
                let trunc = if s.len() > 80 { &s[..80] } else { s.as_str() };
                println!("        0x{:08X} \"{}\"", off, trunc);
            }
        }
        println!();
    }

    results
}

// ══════════════════════════════════════════════════════════════
// Pass D: String Cross-Reference to Code
// ══════════════════════════════════════════════════════════════

fn pass_d_string_xrefs(data: &[u8], pe: &PeInfo, beyond_strings: &[BeyondString]) {
    println!("\n╔══════════════════════════════════════════════════════════════╗");
    println!("║  Pass D: String Cross-References to Code                   ║");
    println!("╚══════════════════════════════════════════════════════════════╝\n");

    if beyond_strings.is_empty() {
        println!("  No beyond_ strings to search for.");
        return;
    }

    let code_sections: Vec<&Section> = pe
        .sections
        .iter()
        .filter(|s| s.name == ".text" || s.name == "il2cpp")
        .collect();

    for bs in beyond_strings {
        println!(
            "  Searching XREFs for \"{}\" (rva:0x{:08X})...",
            bs.string, bs.rva
        );

        for sec in &code_sections {
            let range = sec.file_range();
            let sec_start = range.start.min(data.len());
            let sec_end = range.end.min(data.len());
            let sec_data = &data[sec_start..sec_end];

            if sec_data.len() < 7 {
                continue;
            }

            let mut count = 0;
            for i in 0..=(sec_data.len() - 4) {
                let rva_here = sec.virtual_address + i as u32;
                let expected_disp = bs.rva.wrapping_sub(rva_here.wrapping_add(4));
                let expected_bytes = expected_disp.to_le_bytes();

                if sec_data[i..i + 4] == expected_bytes && i >= 3 {
                    let b0 = sec_data[i - 3];
                    let b1 = sec_data[i - 2];
                    let b2 = sec_data[i - 1];
                    let modrm_mod = (b2 >> 6) & 3;
                    let modrm_rm = b2 & 7;
                    let rip_rel = modrm_mod == 0 && modrm_rm == 5;

                    let has_rex = (0x40..=0x4F).contains(&b0);
                    let valid_opcode = matches!(b1, 0x8D | 0x8B | 0x89 | 0x03 | 0x0B | 0x33 | 0x3B);

                    let is_rip = (has_rex && valid_opcode && rip_rel) || {
                        let modrm2_mod = (sec_data[i - 1] >> 6) & 3;
                        let modrm2_rm = sec_data[i - 1] & 7;
                        let valid2 = matches!(sec_data[i - 2], 0x8D | 0x8B | 0x89);
                        valid2 && modrm2_mod == 0 && modrm2_rm == 5
                    };

                    if is_rip {
                        count += 1;
                        let file_off = sec_start + i - 3;
                        let instr_rva = sec.virtual_address + (i - 3) as u32;
                        println!(
                            "    XREF in {} at file:0x{:08X} rva:0x{:08X}",
                            sec.name, file_off, instr_rva
                        );

                        if count <= 5 {
                            disassemble_around(
                                data,
                                pe,
                                file_off,
                                &format!("\"{}\" xref #{} in {}", bs.string, count, sec.name),
                                30,
                            );
                        }
                    }
                }
            }

            if count > 5 {
                println!("    ... {} total XREFs in {}", count, sec.name);
            }
            if count == 0 {
                println!("    No RIP-relative XREFs in {}", sec.name);
            }
        }
        println!();
    }
}

// ══════════════════════════════════════════════════════════════
// Pass E: Indirect S-box Reference Search
// ══════════════════════════════════════════════════════════════

fn pass_e_indirect_refs(data: &[u8], pe: &PeInfo) {
    println!("\n╔══════════════════════════════════════════════════════════════╗");
    println!("║  Pass E: Indirect S-box Reference Search                   ║");
    println!("╚══════════════════════════════════════════════════════════════╝\n");

    let sgxm1 = pe.find_section(".Sgxm1");
    let sgxm1_va = sgxm1.map(|s| s.virtual_address).unwrap_or(0);

    // [E1] Full 64-bit VA search
    println!("  [E1] Searching for 64-bit virtual addresses...");
    for (i, &rva) in SBOX_RVAS.iter().enumerate() {
        let va = IMAGE_BASE + rva as u64;
        let pattern = va.to_le_bytes();
        let mut count = 0;

        for sec in &pe.sections {
            let range = sec.file_range();
            let sec_start = range.start.min(data.len());
            let sec_end = range.end.min(data.len());
            let sec_data = &data[sec_start..sec_end];

            if sec_data.len() < 8 {
                continue;
            }
            for j in 0..=(sec_data.len() - 8) {
                if sec_data[j..j + 8] == pattern {
                    let file_off = sec_start + j;
                    let file_rva = pe.file_offset_to_rva(file_off).unwrap_or(0);
                    count += 1;
                    if count <= 3 {
                        println!(
                            "    S-box {} VA 0x{:016X} found in {} at file:0x{:08X} rva:0x{:08X}",
                            i, va, sec.name, file_off, file_rva
                        );
                        hex_dump(
                            &data[file_off.saturating_sub(16)..(file_off + 24).min(data.len())],
                            file_off.saturating_sub(16),
                            40,
                        );
                    }
                }
            }
        }
        if count > 3 {
            println!("    ... {} total hits for S-box {} VA", count, i);
        }
        if count == 0 {
            println!("    S-box {} VA 0x{:016X} — not found", i, va);
        }
    }

    // [E2] 32-bit RVA search
    println!("\n  [E2] Searching for 32-bit RVA values...");
    for (i, &rva) in SBOX_RVAS.iter().enumerate() {
        let pattern = rva.to_le_bytes();
        let mut count = 0;
        let mut sections_hit = Vec::new();

        for sec in &pe.sections {
            let range = sec.file_range();
            let sec_start = range.start.min(data.len());
            let sec_end = range.end.min(data.len());
            let sec_data = &data[sec_start..sec_end];

            if sec_data.len() < 4 {
                continue;
            }
            let mut sec_count = 0;
            for j in 0..=(sec_data.len() - 4) {
                if sec_data[j..j + 4] == pattern {
                    let file_off = sec_start + j;
                    sec_count += 1;
                    count += 1;

                    if count <= 5 {
                        let file_rva = pe.file_offset_to_rva(file_off).unwrap_or(0);
                        println!(
                            "    S-box {} RVA 0x{:08X} in {} at file:0x{:08X} rva:0x{:08X}",
                            i, rva, sec.name, file_off, file_rva
                        );

                        // Disassemble if in code section
                        if sec.name == ".text" || sec.name == "il2cpp" {
                            disassemble_around(
                                data,
                                pe,
                                file_off,
                                &format!("S-box {} RVA ref in {}", i, sec.name),
                                20,
                            );
                        } else {
                            hex_dump(
                                &data[file_off.saturating_sub(16)..(file_off + 24).min(data.len())],
                                file_off.saturating_sub(16),
                                40,
                            );
                        }
                    }
                }
            }
            if sec_count > 0 {
                sections_hit.push(format!("{}:{}", sec.name, sec_count));
            }
        }
        if count > 5 {
            println!(
                "    ... {} total for S-box {} RVA [{}]",
                count,
                i,
                sections_hit.join(", ")
            );
        }
        if count == 0 {
            println!("    S-box {} RVA 0x{:08X} — not found", i, rva);
        }
    }

    // [E3] Section-relative offset search (search .Sgxm0 and .tvm0)
    println!("\n  [E3] Searching for section-relative offsets in .Sgxm0 and .tvm0...");
    let vm_sections: Vec<&Section> = pe
        .sections
        .iter()
        .filter(|s| s.name == ".Sgxm0" || s.name == ".tvm0" || s.name == ".Sgxm1")
        .collect();

    for (i, &rva) in SBOX_RVAS.iter().enumerate() {
        let sec_offset = rva.wrapping_sub(sgxm1_va);
        let pattern = sec_offset.to_le_bytes();
        let mut count = 0;

        for sec in &vm_sections {
            let range = sec.file_range();
            let sec_start = range.start.min(data.len());
            let sec_end = range.end.min(data.len());
            let sec_data = &data[sec_start..sec_end];

            if sec_data.len() < 4 {
                continue;
            }
            for j in 0..=(sec_data.len() - 4) {
                if sec_data[j..j + 4] == pattern {
                    let file_off = sec_start + j;
                    // Skip if this is the S-box itself
                    if SBOX_FILE_OFFSETS.contains(&file_off) {
                        continue;
                    }
                    // Skip if inside any S-box data
                    let inside_sbox = SBOX_FILE_OFFSETS
                        .iter()
                        .any(|&soff| file_off >= soff && file_off < soff + 256);
                    if inside_sbox {
                        continue;
                    }

                    count += 1;
                    if count <= 5 {
                        let file_rva = pe.file_offset_to_rva(file_off).unwrap_or(0);
                        println!(
                            "    S-box {} sec-offset 0x{:08X} in {} at file:0x{:08X} rva:0x{:08X}",
                            i, sec_offset, sec.name, file_off, file_rva
                        );
                        hex_dump(
                            &data[file_off.saturating_sub(16)..(file_off + 24).min(data.len())],
                            file_off.saturating_sub(16),
                            40,
                        );
                    }
                }
            }
        }
        if count > 5 {
            println!("    ... {} total for S-box {} sec-offset", count, i);
        }
        if count == 0 {
            println!(
                "    S-box {} sec-offset 0x{:08X} — not found in VM sections",
                i, sec_offset
            );
        }
    }

    // [E4] Search .reloc section for relocations pointing to S-box VAs
    println!("\n  [E4] Checking .reloc for S-box address fixups...");
    if let Some(reloc) = pe.find_section(".reloc") {
        let range = reloc.file_range();
        let sec_start = range.start.min(data.len());
        let sec_end = range.end.min(data.len());
        let reloc_data = &data[sec_start..sec_end];

        // Search for S-box RVAs in relocation data
        for (i, &rva) in SBOX_RVAS.iter().enumerate() {
            let pattern = rva.to_le_bytes();
            let mut count = 0;
            if reloc_data.len() >= 4 {
                for j in 0..=(reloc_data.len() - 4) {
                    if reloc_data[j..j + 4] == pattern {
                        count += 1;
                        if count <= 3 {
                            let file_off = sec_start + j;
                            println!(
                                "    S-box {} RVA in .reloc at file:0x{:08X} (offset +0x{:X})",
                                i, file_off, j
                            );
                        }
                    }
                }
            }
            if count > 0 {
                println!("    S-box {} has {} relocation entries", i, count);
            }
        }
    } else {
        println!("    No .reloc section found");
    }
}

// ══════════════════════════════════════════════════════════════
// Main
// ══════════════════════════════════════════════════════════════

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("=== GameAssembly.dll Deep Investigation ===\n");
    println!("Follow-up to dll_analyze.rs Phase K findings.\n");

    let data = fs::read(DLL_PATH)?;
    println!(
        "Loaded {} ({})\n",
        format_bytes(data.len() as u64),
        data.len()
    );

    let pe = parse_pe(&data);
    println!("Sections: {}", pe.sections.len());
    for s in &pe.sections {
        println!(
            "  {:>10} VA:0x{:08X} raw:0x{:08X} size:{}",
            s.name,
            s.virtual_address,
            s.pointer_to_raw_data,
            format_bytes(s.size_of_raw_data as u64)
        );
    }

    // Pass A: S-box relationships
    pass_a_sbox_analysis(&data);

    // Pass B: Gap and context
    pass_b_gap_context(&data, &pe);

    // Pass C: "beyond_*" string discovery
    let beyond_strings = pass_c_beyond_strings(&data, &pe);

    // Pass D: String cross-references
    pass_d_string_xrefs(&data, &pe, &beyond_strings);

    // Pass E: Indirect S-box references
    pass_e_indirect_refs(&data, &pe);

    // Summary
    println!("\n╔══════════════════════════════════════════════════════════════╗");
    println!("║  Summary                                                    ║");
    println!("╚══════════════════════════════════════════════════════════════╝\n");

    println!("  beyond_ strings found:    {}", beyond_strings.len());
    for bs in &beyond_strings {
        println!("    \"{}\"", bs.string);
    }

    println!("\n=== Investigation Complete ===");
    Ok(())
}
