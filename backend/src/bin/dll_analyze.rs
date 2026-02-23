/// Phase 2: GameAssembly.dll crypto analysis.
///
/// Uses known VFS crypto constants as anchors to locate the crypto module region,
/// searches for the 4 encrypted directory hashes, and scans for unknown cipher
/// constants near the known crypto data.
use backend::extraction::crypto_constants::*;
use goblin::pe::PE;
use iced_x86::{Decoder, DecoderOptions, Formatter, IntelFormatter};
use std::collections::BTreeMap;
use std::fs;

// ══════════════════════════════════════════════════════════════
// Target directory hashes
// ══════════════════════════════════════════════════════════════

const DIR_HASHES: &[(&str, u32)] = &[
    ("19E3AE45", 0x19E3AE45),
    ("1CDDBF1F", 0x1CDDBF1F),
    ("42A8FCA6", 0x42A8FCA6),
    ("775A31D1", 0x775A31D1),
];

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

/// Check if a 256-byte block is a permutation of 0..256 (valid S-box).
fn is_permutation(data: &[u8]) -> bool {
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
// PE Section Info
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
        let end = start + self.size_of_raw_data as usize;
        start..end
    }

    fn contains_rva(&self, rva: u32) -> bool {
        rva >= self.virtual_address && rva < self.virtual_address + self.virtual_size
    }

    fn rva_to_file_offset(&self, rva: u32) -> Option<usize> {
        if self.contains_rva(rva) {
            Some((rva - self.virtual_address + self.pointer_to_raw_data) as usize)
        } else {
            None
        }
    }

    fn file_offset_to_rva(&self, offset: usize) -> Option<u32> {
        let off = offset as u32;
        if off >= self.pointer_to_raw_data && off < self.pointer_to_raw_data + self.size_of_raw_data
        {
            Some(off - self.pointer_to_raw_data + self.virtual_address)
        } else {
            None
        }
    }
}

struct PeInfo {
    image_base: u64,
    sections: Vec<Section>,
}

impl PeInfo {
    fn file_offset_to_rva(&self, offset: usize) -> Option<u32> {
        for s in &self.sections {
            if let Some(rva) = s.file_offset_to_rva(offset) {
                return Some(rva);
            }
        }
        None
    }

    fn rva_to_file_offset(&self, rva: u32) -> Option<usize> {
        for s in &self.sections {
            if let Some(off) = s.rva_to_file_offset(rva) {
                return Some(off);
            }
        }
        None
    }

    #[expect(dead_code)]
    fn find_section(&self, name: &str) -> Option<&Section> {
        self.sections.iter().find(|s| s.name == name)
    }
}

// ══════════════════════════════════════════════════════════════
// Pass 1: PE Section Mapping
// ══════════════════════════════════════════════════════════════

fn pass1_pe_sections(data: &[u8]) -> PeInfo {
    println!("\n╔══════════════════════════════════════════════════════════════╗");
    println!("║  Pass 1: PE Section Mapping                                ║");
    println!("╚══════════════════════════════════════════════════════════════╝\n");

    let pe = PE::parse(data).expect("Failed to parse PE");
    let image_base = pe.image_base as u64;
    println!("  Image base: 0x{:016X}", image_base);
    println!(
        "  {:>10} {:>12} {:>12} {:>12} {:>12}",
        "Name", "VirtAddr", "VirtSize", "RawOffset", "RawSize"
    );
    println!("  {}", "-".repeat(62));

    let mut sections = Vec::new();
    for s in &pe.sections {
        let name = String::from_utf8_lossy(
            &s.name[..s.name.iter().position(|&b| b == 0).unwrap_or(s.name.len())],
        )
        .to_string();
        let sec = Section {
            name: name.clone(),
            virtual_address: s.virtual_address,
            virtual_size: s.virtual_size,
            pointer_to_raw_data: s.pointer_to_raw_data,
            size_of_raw_data: s.size_of_raw_data,
        };
        println!(
            "  {:>10} 0x{:08X}   {}   0x{:08X}   {}",
            name,
            sec.virtual_address,
            format_bytes(sec.virtual_size as u64),
            sec.pointer_to_raw_data,
            format_bytes(sec.size_of_raw_data as u64),
        );
        sections.push(sec);
    }

    PeInfo {
        image_base,
        sections,
    }
}

// ══════════════════════════════════════════════════════════════
// Pass 2: Known Constant Anchoring
// ══════════════════════════════════════════════════════════════

/// Search for a byte pattern in a section, returning file offsets.
fn find_pattern(data: &[u8], section: &Section, pattern: &[u8]) -> Vec<usize> {
    let range = section.file_range();
    let end = range.end.min(data.len());
    let start = range.start.min(end);
    let section_data = &data[start..end];

    let mut results = Vec::new();
    if section_data.len() < pattern.len() {
        return results;
    }
    for i in 0..=(section_data.len() - pattern.len()) {
        if section_data[i..i + pattern.len()] == *pattern {
            results.push(start + i);
        }
    }
    results
}

struct AnchorResult {
    /// File offset of the S-box (primary anchor)
    sbox_offset: Option<usize>,
    /// All anchor offsets found
    anchors: Vec<(String, usize)>,
}

fn pass2_known_constants(data: &[u8], pe: &PeInfo) -> AnchorResult {
    println!("\n╔══════════════════════════════════════════════════════════════╗");
    println!("║  Pass 2: Known Constant Anchoring                          ║");
    println!("╚══════════════════════════════════════════════════════════════╝\n");

    // Search ALL sections — il2cpp (178MB) likely contains the constants
    let search_sections: Vec<&Section> = pe.sections.iter().collect();

    if search_sections.is_empty() {
        println!("  WARNING: No sections found!");
        return AnchorResult {
            sbox_offset: None,
            anchors: Vec::new(),
        };
    }

    let mut anchors: Vec<(String, usize)> = Vec::new();
    let mut sbox_offset = None;

    // Known constants to search for
    let constants: Vec<(&str, Vec<u8>)> = vec![
        ("VFS_AES_SBOX", VFS_AES_SBOX.to_vec()),
        ("VFS_AES_KEY", VFS_AES_KEY.to_vec()),
        ("VFS_AES_IV", VFS_AES_IV.to_vec()),
        ("RCON", RCON.to_vec()),
        ("VFS_XOR_KEY", VFS_XOR_KEY.to_le_bytes().to_vec()),
        ("HEADER_XOR_A", HEADER_XOR_A.to_le_bytes().to_vec()),
        ("HEADER_XOR_B", HEADER_XOR_B.to_le_bytes().to_vec()),
    ];

    for sec in &search_sections {
        println!(
            "  Searching section {} ({}):",
            sec.name,
            format_bytes(sec.size_of_raw_data as u64)
        );

        for (name, pattern) in &constants {
            let matches = find_pattern(data, sec, pattern);
            for &off in &matches {
                let rva = pe.file_offset_to_rva(off).unwrap_or(0);
                // Skip small (4-byte) constants if they're far from S-box (filter later)
                println!(
                    "    {} at file:0x{:08X} rva:0x{:08X} ({} bytes)",
                    name,
                    off,
                    rva,
                    pattern.len()
                );

                if *name == "VFS_AES_SBOX" {
                    sbox_offset = Some(off);
                }

                anchors.push((name.to_string(), off));
            }

            if matches.is_empty() && pattern.len() >= 8 {
                // Only note missing for larger patterns (small ones may be coincidental)
                println!("    {} — NOT FOUND", name);
            }
        }
    }

    println!(
        "\n  Summary: {} anchors found, S-box at {:?}",
        anchors.len(),
        sbox_offset.map(|o| format!("0x{:08X}", o))
    );

    AnchorResult {
        sbox_offset,
        anchors,
    }
}

// ══════════════════════════════════════════════════════════════
// Pass 3: Directory Hash Search
// ══════════════════════════════════════════════════════════════

struct HashMatch {
    dir_name: String,
    hash: u32,
    file_offset: usize,
    rva: u32,
}

fn pass3_directory_hashes(data: &[u8], pe: &PeInfo) -> Vec<HashMatch> {
    println!("\n╔══════════════════════════════════════════════════════════════╗");
    println!("║  Pass 3: Directory Hash Search                             ║");
    println!("╚══════════════════════════════════════════════════════════════╝\n");

    let mut all_matches = Vec::new();

    // Search ALL sections for the hashes (il2cpp has the actual game code)
    for &(name, hash) in DIR_HASHES {
        let pattern = hash.to_le_bytes();
        let mut total_count = 0;

        for sec in &pe.sections {
            let range = sec.file_range();
            let end = range.end.min(data.len());
            let start = range.start;
            let sec_data = &data[start..end];
            let mut count = 0;

            if sec_data.len() >= 4 {
                for i in 0..=(sec_data.len() - 4) {
                    if sec_data[i..i + 4] == pattern {
                        let file_off = start + i;
                        let rva = pe.file_offset_to_rva(file_off).unwrap_or(0);
                        count += 1;
                        total_count += 1;

                        if count <= 5 {
                            println!(
                                "  {} (0x{:08X}) in {} at file:0x{:08X} rva:0x{:08X}",
                                name, hash, sec.name, file_off, rva
                            );
                        }

                        all_matches.push(HashMatch {
                            dir_name: name.to_string(),
                            hash,
                            file_offset: file_off,
                            rva,
                        });
                    }
                }
            }

            if count > 5 {
                println!("  ... and {} more in {} for {}", count - 5, sec.name, name);
            }
        }

        if total_count == 0 {
            println!("  {} (0x{:08X}) — NOT FOUND in any section", name, hash);
        }
    }

    println!("\n  Total hash matches: {}", all_matches.len());
    all_matches
}

// ══════════════════════════════════════════════════════════════
// Pass 4: String Extraction
// ══════════════════════════════════════════════════════════════

fn pass4_string_extraction(data: &[u8], pe: &PeInfo) {
    println!("\n╔══════════════════════════════════════════════════════════════╗");
    println!("║  Pass 4: Crypto-Related String Extraction                  ║");
    println!("╚══════════════════════════════════════════════════════════════╝\n");

    // Focused keywords — filter out generic "key"/"block"/"ctr" noise
    let keywords = [
        "encrypt",
        "decrypt",
        "cipher",
        "Crypt",
        "Decrypt",
        "Encrypt",
        "AES",
        "aes",
        "CBC",
        "cbc",
        "CTR",
        "SBOX",
        "SBox",
        "sbox",
        "SubBytes",
        "ShiftRows",
        "MixColumns",
        "BlockCipher",
        "StreamCipher",
        "KeyExpansion",
        "RoundKey",
        "VFS",
        "vfs",
        "chk",
        "catalog",
        "beyond_set",
        "DecryptionFunc",
        "FLoadBlock",
        "VfsLoaded",
    ];

    // Search all sections for strings
    let mut strings: Vec<(usize, String)> = Vec::new();

    for sec in &pe.sections {
        let range = sec.file_range();
        let end = range.end.min(data.len());
        let start = range.start;
        let section_data = &data[start..end];

        let mut current_start = 0;
        let mut current = String::new();

        for (i, &b) in section_data.iter().enumerate() {
            if b.is_ascii_graphic() || b == b' ' {
                if current.is_empty() {
                    current_start = i;
                }
                current.push(b as char);
            } else {
                if current.len() >= 6 {
                    strings.push((start + current_start, current.clone()));
                }
                current.clear();
            }
        }
        if current.len() >= 6 {
            strings.push((start + current_start, current));
        }
    }

    println!("  Total strings found (≥6 chars): {}", strings.len());

    // Filter by keywords (case-insensitive)
    let mut hits: Vec<(usize, &str, String)> = Vec::new();
    for (off, s) in &strings {
        let lower = s.to_lowercase();
        for &kw in &keywords {
            if lower.contains(&kw.to_lowercase()) {
                hits.push((*off, kw, s.clone()));
                break; // only report once per string
            }
        }
    }

    // Deduplicate and sort
    hits.sort_by_key(|(off, _, _)| *off);
    hits.dedup_by_key(|(off, _, _)| *off);

    println!("  Keyword matches: {}\n", hits.len());
    for (off, kw, s) in &hits {
        let rva = pe.file_offset_to_rva(*off).unwrap_or(0);
        let truncated = if s.len() > 120 { &s[..120] } else { s.as_str() };
        println!(
            "    [{}] file:0x{:08X} rva:0x{:08X} \"{}\"",
            kw, off, rva, truncated
        );
    }
}

// ══════════════════════════════════════════════════════════════
// Pass 5: Unknown Crypto Detection
// ══════════════════════════════════════════════════════════════

struct CryptoCandidate {
    file_offset: usize,
    rva: u32,
    kind: String,
    entropy: f64,
    is_permutation: bool,
}

fn pass5_unknown_crypto(
    data: &[u8],
    pe: &PeInfo,
    sbox_offset: Option<usize>,
) -> Vec<CryptoCandidate> {
    println!("\n╔══════════════════════════════════════════════════════════════╗");
    println!("║  Pass 5: Unknown Crypto Constant Detection                 ║");
    println!("╚══════════════════════════════════════════════════════════════╝\n");

    let mut candidates = Vec::new();

    // If we have an anchor, scan around it. Otherwise scan ALL data-like sections.
    let scan_regions: Vec<(usize, usize, &str)> = if let Some(anchor) = sbox_offset {
        println!("  S-box anchor at file:0x{:08X}, scanning ±64KB", anchor);
        let radius: usize = 64 * 1024;
        vec![(
            anchor.saturating_sub(radius),
            (anchor + 256 + radius).min(data.len()),
            "anchor region",
        )]
    } else {
        println!("  No S-box anchor found — scanning all data sections for S-box permutations");
        // Scan .rdata, .data, il2cpp, and other non-code sections
        pe.sections
            .iter()
            .filter(|s| {
                !s.name.starts_with(".text")
                    && !s.name.starts_with(".pdata")
                    && !s.name.starts_with(".reloc")
            })
            .map(|s| {
                let range = s.file_range();
                (
                    range.start.min(data.len()),
                    range.end.min(data.len()),
                    s.name.as_str(),
                )
            })
            .collect()
    };

    // [5a] Scan for 256-byte S-box candidates (permutations only when no anchor)
    println!("\n  [5a] Scanning for 256-byte S-box permutations...");
    let mut sbox_count = 0;

    for &(scan_start, scan_end, region_name) in &scan_regions {
        if scan_end <= scan_start + 256 {
            continue;
        }
        println!(
            "    Scanning {} ({})",
            region_name,
            format_bytes((scan_end - scan_start) as u64)
        );

        let aligned_start = (scan_start + 15) & !15;
        let mut off = aligned_start;
        while off + 256 <= scan_end {
            let block = &data[off..off + 256];

            // Quick pre-check: first byte must not equal second byte (fast reject)
            // Full permutation check is expensive, so only check high-entropy blocks
            let ent = entropy(block);
            if ent > 7.5 {
                let perm = is_permutation(block);
                // Skip the known S-box if found
                let is_known_sbox =
                    sbox_offset.is_some_and(|k| (off as isize - k as isize).unsigned_abs() < 256);

                if perm && !is_known_sbox {
                    let rva = pe.file_offset_to_rva(off).unwrap_or(0);
                    sbox_count += 1;
                    println!(
                        "    \x1b[1;32mS-box PERMUTATION\x1b[0m at file:0x{:08X} rva:0x{:08X} entropy={:.3} section={}",
                        off, rva, ent, region_name
                    );
                    if sbox_count <= 10 {
                        hex_dump(block, off, 64);
                    }
                    candidates.push(CryptoCandidate {
                        file_offset: off,
                        rva,
                        kind: format!("S-box permutation ({})", region_name),
                        entropy: ent,
                        is_permutation: true,
                    });
                }
            }
            // Step by 1 byte (not 16) to catch non-aligned S-boxes
            off += 1;
        }
    }
    println!("  Found {} S-box permutations\n", sbox_count);

    // [5b] If we found S-box permutations, scan ±4KB around each for key/IV candidates
    if !candidates.is_empty() {
        println!("  [5b] Scanning for 16-byte key/IV candidates near S-box hits...");
        let sbox_offsets: Vec<usize> = candidates.iter().map(|c| c.file_offset).collect();
        let mut key_count = 0;

        for &soff in &sbox_offsets {
            let key_start = soff.saturating_sub(4096);
            let key_end = (soff + 256 + 4096).min(data.len());
            let aligned = (key_start + 15) & !15;
            let mut off = aligned;

            while off + 16 <= key_end {
                let block = &data[off..off + 16];

                // Skip zero, FF, known constants, and inside S-boxes
                let skip = block.iter().all(|&b| b == 0)
                    || block.iter().all(|&b| b == 0xFF)
                    || block == VFS_AES_KEY
                    || block == VFS_AES_IV
                    || sbox_offsets.iter().any(|&s| off >= s && off < s + 256);

                if !skip {
                    let ent = entropy(block);
                    if ent > 3.0 && ent < 4.5 {
                        let rva = pe.file_offset_to_rva(off).unwrap_or(0);
                        key_count += 1;
                        if key_count <= 20 {
                            println!(
                                "    Key candidate at file:0x{:08X} rva:0x{:08X} entropy={:.3}",
                                off, rva, ent
                            );
                            print!("      ");
                            for &b in block {
                                print!("{:02x} ", b);
                            }
                            println!();
                            candidates.push(CryptoCandidate {
                                file_offset: off,
                                rva,
                                kind: "Key/IV candidate".into(),
                                entropy: ent,
                                is_permutation: false,
                            });
                        }
                    }
                }
                off += 16;
            }
        }
        println!("  Found {} key/IV candidates\n", key_count);
    }

    candidates
}

// ══════════════════════════════════════════════════════════════
// Pass 6: Cross-Reference Analysis
// ══════════════════════════════════════════════════════════════

struct CrossRef {
    /// File offset in .text where the reference instruction is
    text_offset: usize,
    /// RVA of the instruction
    text_rva: u32,
    /// The target being referenced (description)
    target_desc: String,
    /// RVA of the target (used in summary output)
    #[expect(dead_code)]
    target_rva: u32,
}

fn pass6_cross_references(
    data: &[u8],
    pe: &PeInfo,
    anchors: &AnchorResult,
    crypto_candidates: &[CryptoCandidate],
    hash_matches: &[HashMatch],
) -> Vec<CrossRef> {
    println!("\n╔══════════════════════════════════════════════════════════════╗");
    println!("║  Pass 6: Cross-Reference Analysis                          ║");
    println!("╚══════════════════════════════════════════════════════════════╝\n");

    // Search all code sections (.text + il2cpp) for cross-references
    let code_sections: Vec<&Section> = pe
        .sections
        .iter()
        .filter(|s| s.name == ".text" || s.name == "il2cpp")
        .collect();

    if code_sections.is_empty() {
        println!("  ERROR: No code sections found");
        return Vec::new();
    }

    // Collect all targets to search for (RVA → description)
    let mut targets: BTreeMap<u32, String> = BTreeMap::new();

    for (name, off) in &anchors.anchors {
        if let Some(rva) = pe.file_offset_to_rva(*off) {
            targets.insert(rva, format!("anchor:{}", name));
        }
    }
    for c in crypto_candidates {
        if c.rva != 0 {
            targets.insert(c.rva, format!("crypto:{}", c.kind));
        }
    }

    if targets.is_empty() {
        println!("  No targets to search for cross-references");
    }

    let mut xrefs = Vec::new();

    for code_sec in &code_sections {
        let sec_range = code_sec.file_range();
        let sec_start = sec_range.start;
        let sec_end = sec_range.end.min(data.len());
        let sec_data = &data[sec_start..sec_end];

        println!(
            "  Searching {} targets in {} ({})...",
            targets.len(),
            code_sec.name,
            format_bytes(code_sec.size_of_raw_data as u64)
        );

        // For each position, check if a RIP-relative displacement points to any target
        if sec_data.len() >= 7 {
            for (&target_rva, target_desc) in &targets {
                let mut count = 0;
                for i in 0..=(sec_data.len() - 4) {
                    let rva_here = code_sec.virtual_address + i as u32;
                    let expected_disp = target_rva.wrapping_sub(rva_here.wrapping_add(4));
                    let expected_bytes = expected_disp.to_le_bytes();

                    if sec_data[i..i + 4] == expected_bytes {
                        let is_rip_relative = if i >= 3 {
                            let b0 = sec_data[i - 3];
                            let b1 = sec_data[i - 2];
                            let b2 = sec_data[i - 1];
                            let modrm_mod = (b2 >> 6) & 3;
                            let modrm_rm = b2 & 7;
                            let rip_rel = modrm_mod == 0 && modrm_rm == 5;

                            let has_rex = (0x40..=0x4F).contains(&b0);
                            let valid_opcode =
                                matches!(b1, 0x8D | 0x8B | 0x89 | 0x03 | 0x0B | 0x33 | 0x3B);

                            (has_rex && valid_opcode && rip_rel) || {
                                let b_op = sec_data[i - 2];
                                let b_modrm = sec_data[i - 1];
                                let modrm_mod2 = (b_modrm >> 6) & 3;
                                let modrm_rm2 = b_modrm & 7;
                                let valid_op2 =
                                    matches!(b_op, 0x8D | 0x8B | 0x89 | 0x03 | 0x0B | 0x33 | 0x3B);
                                valid_op2 && modrm_mod2 == 0 && modrm_rm2 == 5
                            }
                        } else {
                            false
                        };

                        if is_rip_relative {
                            count += 1;
                            let file_off = sec_start + i;
                            let instr_rva = code_sec.virtual_address + (i.saturating_sub(3)) as u32;

                            if count <= 5 {
                                println!(
                                    "    XREF: {} → {} file:0x{:08X} rva:0x{:08X}",
                                    target_desc, code_sec.name, file_off, instr_rva
                                );
                            }

                            xrefs.push(CrossRef {
                                text_offset: file_off.saturating_sub(3),
                                text_rva: instr_rva,
                                target_desc: target_desc.clone(),
                                target_rva,
                            });
                        }
                    }
                }
                if count > 5 {
                    println!(
                        "    ... {} total XREFs for {} in {}",
                        count, target_desc, code_sec.name
                    );
                } else if count == 0 && target_desc.contains("S-box") {
                    println!(
                        "    No XREFs found for {} in {}",
                        target_desc, code_sec.name
                    );
                }
            }
        }
    }

    // Also find xrefs to directory hash matches that are near other xrefs
    println!("\n  Hash match instruction context:");
    for hm in hash_matches.iter().take(20) {
        // The hash value itself is a 4-byte immediate. Look at surrounding code.
        let ctx_start = hm.file_offset.saturating_sub(8);
        let ctx_end = (hm.file_offset + 8).min(data.len());
        if ctx_start < ctx_end {
            let prefix = &data[ctx_start..hm.file_offset];
            let prefix_hex: Vec<String> = prefix.iter().map(|b| format!("{:02x}", b)).collect();
            println!(
                "    {} at rva:0x{:08X} preceding bytes: {}",
                hm.dir_name,
                hm.rva,
                prefix_hex.join(" ")
            );
        }
    }

    println!("\n  Total cross-references: {}", xrefs.len());
    xrefs
}

// ══════════════════════════════════════════════════════════════
// Pass 7: Code Disassembly
// ══════════════════════════════════════════════════════════════

fn disassemble_around(
    data: &[u8],
    pe: &PeInfo,
    file_offset: usize,
    label: &str,
    mark_rva: Option<u32>,
) {
    // Disassemble ~20 instructions around the offset
    let window_before = 64; // bytes before
    let start = file_offset.saturating_sub(window_before);
    let end = (file_offset + 128).min(data.len());
    let code = &data[start..end];

    let start_rva = pe.file_offset_to_rva(start).unwrap_or(0);
    let rip = pe.image_base + start_rva as u64;

    let mut decoder = Decoder::with_ip(64, code, rip, DecoderOptions::NONE);
    let mut formatter = IntelFormatter::new();
    formatter.options_mut().set_first_operand_char_index(10);

    let mut output = String::new();
    let mut instr = iced_x86::Instruction::default();
    let mut count = 0;

    println!("\n  --- Disassembly: {} ---", label);

    while decoder.can_decode() && count < 30 {
        decoder.decode_out(&mut instr);
        output.clear();
        formatter.format(&instr, &mut output);

        let instr_rva = (instr.ip() - pe.image_base) as u32;
        let is_target = mark_rva.is_some_and(|m| {
            let instr_file_off = pe.rva_to_file_offset(instr_rva);
            instr_file_off.is_some_and(|off| {
                // Mark if this instruction is within a few bytes of the target
                let target_off = pe.rva_to_file_offset(m).unwrap_or(0);
                off.abs_diff(target_off) < 8 || off.abs_diff(file_offset) < 4
            })
        });

        let marker = if is_target { " <<<<" } else { "" };
        println!("    {:016X}  {}{}", instr.ip(), output, marker);
        count += 1;
    }
}

fn pass7_disassembly(data: &[u8], pe: &PeInfo, xrefs: &[CrossRef], hash_matches: &[HashMatch]) {
    println!("\n╔══════════════════════════════════════════════════════════════╗");
    println!("║  Pass 7: Code Disassembly                                  ║");
    println!("╚══════════════════════════════════════════════════════════════╝\n");

    // Disassemble around S-box cross-references (most important)
    let sbox_xrefs: Vec<&CrossRef> = xrefs
        .iter()
        .filter(|x| x.target_desc.contains("SBOX") || x.target_desc.contains("S-box"))
        .collect();

    if !sbox_xrefs.is_empty() {
        println!("  === S-box Cross-References ({}) ===", sbox_xrefs.len());
        for (i, xref) in sbox_xrefs.iter().enumerate().take(10) {
            disassemble_around(
                data,
                pe,
                xref.text_offset,
                &format!("SBOX xref #{} → {}", i, xref.target_desc),
                Some(xref.text_rva),
            );
        }
    }

    // Disassemble around KEY/IV cross-references
    let key_xrefs: Vec<&CrossRef> = xrefs
        .iter()
        .filter(|x| x.target_desc.contains("KEY") || x.target_desc.contains("IV"))
        .collect();

    if !key_xrefs.is_empty() {
        println!("\n  === Key/IV Cross-References ({}) ===", key_xrefs.len());
        for (i, xref) in key_xrefs.iter().enumerate().take(10) {
            disassemble_around(
                data,
                pe,
                xref.text_offset,
                &format!("Key/IV xref #{} → {}", i, xref.target_desc),
                Some(xref.text_rva),
            );
        }
    }

    // Disassemble around directory hash matches (critical for finding the dispatch function)
    let unique_hash_offsets: Vec<&HashMatch> = {
        let mut seen = std::collections::HashSet::new();
        hash_matches
            .iter()
            .filter(|hm| seen.insert(hm.file_offset))
            .collect()
    };

    if !unique_hash_offsets.is_empty() {
        println!(
            "\n  === Directory Hash Instruction Sites ({}) ===",
            unique_hash_offsets.len()
        );
        for (i, hm) in unique_hash_offsets.iter().enumerate().take(15) {
            disassemble_around(
                data,
                pe,
                hm.file_offset.saturating_sub(1), // start before the immediate
                &format!("dir hash {} (0x{:08X}) #{}", hm.dir_name, hm.hash, i),
                Some(hm.rva),
            );
        }
    }

    // Disassemble around crypto candidate cross-references
    let crypto_xrefs: Vec<&CrossRef> = xrefs
        .iter()
        .filter(|x| x.target_desc.contains("crypto:"))
        .collect();

    if !crypto_xrefs.is_empty() {
        println!(
            "\n  === Crypto Candidate Cross-References ({}) ===",
            crypto_xrefs.len()
        );
        for (i, xref) in crypto_xrefs.iter().enumerate().take(10) {
            disassemble_around(
                data,
                pe,
                xref.text_offset,
                &format!("crypto xref #{} → {}", i, xref.target_desc),
                Some(xref.text_rva),
            );
        }
    }

    if sbox_xrefs.is_empty() && key_xrefs.is_empty() && unique_hash_offsets.is_empty() {
        println!("  No disassembly targets found.");
    }
}

// ══════════════════════════════════════════════════════════════
// Main
// ══════════════════════════════════════════════════════════════

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = "./output/GameAssembly.dll";
    println!("=== GameAssembly.dll Crypto Analysis ===\n");
    println!("Loading {}...", path);

    let data = fs::read(path)?;
    println!(
        "Loaded {} ({})\n",
        format_bytes(data.len() as u64),
        data.len()
    );

    // Pass 1: PE sections
    let pe = pass1_pe_sections(&data);

    // Pass 2: Known constant anchoring
    let anchors = pass2_known_constants(&data, &pe);

    // Pass 3: Directory hash search
    let hash_matches = pass3_directory_hashes(&data, &pe);

    // Pass 4: String extraction
    pass4_string_extraction(&data, &pe);

    // Pass 5: Unknown crypto detection
    let crypto_candidates = pass5_unknown_crypto(&data, &pe, anchors.sbox_offset);

    // Pass 6: Cross-reference analysis
    let xrefs = pass6_cross_references(&data, &pe, &anchors, &crypto_candidates, &hash_matches);

    // Pass 7: Disassembly
    pass7_disassembly(&data, &pe, &xrefs, &hash_matches);

    // ── Summary ──
    println!("\n╔══════════════════════════════════════════════════════════════╗");
    println!("║  Summary                                                    ║");
    println!("╚══════════════════════════════════════════════════════════════╝\n");

    println!("  PE sections:          {}", pe.sections.len());
    println!("  Known anchors:        {}", anchors.anchors.len());
    println!("  S-box found:          {}", anchors.sbox_offset.is_some());
    println!("  Directory hash hits:  {}", hash_matches.len());
    println!("  Crypto candidates:    {}", crypto_candidates.len());
    println!("  Cross-references:     {}", xrefs.len());

    if !crypto_candidates.is_empty() {
        println!("\n  Crypto candidates worth investigating:");
        for c in &crypto_candidates {
            println!(
                "    file:0x{:08X} rva:0x{:08X} {} (entropy={:.3}{})",
                c.file_offset,
                c.rva,
                c.kind,
                c.entropy,
                if c.is_permutation {
                    ", PERMUTATION"
                } else {
                    ""
                }
            );
        }
    }

    println!("\n=== Analysis Complete ===");
    Ok(())
}
