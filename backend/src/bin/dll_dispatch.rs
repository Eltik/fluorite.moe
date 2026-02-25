/// Phase 2c: Decryption Dispatch & Cross-DLL Crypto Analysis.
///
/// Follow-up to dll_investigate.rs Phase 2b findings:
/// - 3 cipher pairs in .Sgxm1 (S-box 0↔2, S-box 1 involution, S-box 3 involution, S-box 4↔5)
/// - beyond_setDecryptionFunc is the only beyond_ API function
/// - Resolved function pointer cached at [0x18F60CD80]
/// - Actual decryption function comes from external native DLL
///
/// Goal: Find WHO reads the cached pointer (the dispatcher) and WHICH DLL has the decryption.
use backend::extraction::crypto_constants::*;
use goblin::pe::PE;
use iced_x86::{Decoder, DecoderOptions, Formatter, IntelFormatter};
use std::fs;

// ══════════════════════════════════════════════════════════════
// Constants
// ══════════════════════════════════════════════════════════════

const GA_PATH: &str = "./output/GameAssembly.dll";
const GA_IMAGE_BASE: u64 = 0x180000000;

/// RVA of cached beyond_setDecryptionFunc result pointer
const DECRYPT_FUNC_PTR_RVA: u32 = 0x0F60CD80;

/// Known call sites that resolve the function pointer
const CALL_SITE_1_RVA: u32 = 0x0C0376FF;
const CALL_SITE_2_RVA: u32 = 0x0C08E4B0; // function prologue

/// Other DLLs to scan for crypto content
const OTHER_DLLS: &[&str] = &[
    "./output/EndfieldBase.dll",
    "./output/UnityFileSystemApi.dll",
    "./output/HGP.dll",
    "./output/UnityPlayer.dll",
];

/// 4 encrypted directory hashes
const DIR_HASHES: [(u32, &str); 4] = [
    (0x19E3AE45, "19E3AE45"),
    (0x1CDDBF1F, "1CDDBF1F"),
    (0x42A8FCA6, "42A8FCA6"),
    (0x775A31D1, "775A31D1"),
];

/// File offsets of the 6 S-boxes found in Phase 2b (.Sgxm1 section of GameAssembly.dll)
const SBOX_FILE_OFFSETS: [usize; 6] = [
    0x0FF6C960, 0x0FF6CA60, 0x0FF6CB60, // Group 1
    0x0FF6CD00, 0x0FF6CE00, 0x0FF6CF00, // Group 2
];

/// Crypto-related keyword patterns for string scanning
const CRYPTO_KEYWORDS: &[&str] = &[
    "beyond",
    "decrypt",
    "encrypt",
    "VFS",
    "vfs",
    "cipher",
    "AES",
    "aes",
    "sbox",
    "SBox",
    "Crypt",
    "BlockCipher",
    "KeyExpansion",
    "catalog",
    "chk",
];

// ══════════════════════════════════════════════════════════════
// Shared Infrastructure
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
    image_base: u64,
    sections: Vec<Section>,
    exports: Vec<ExportInfo>,
}

#[derive(Debug, Clone)]
struct ExportInfo {
    name: String,
    rva: u32,
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

    #[expect(dead_code)]
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

fn disassemble_at(data: &[u8], pe: &PeInfo, file_offset: usize, count: usize, label: &str) {
    let end = (file_offset + count * 15).min(data.len()); // ~15 bytes per instruction max
    let code = &data[file_offset..end];
    let start_rva = pe.file_offset_to_rva(file_offset).unwrap_or(0);
    let rip = pe.image_base + start_rva as u64;

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
        println!("    {:016X}  {}", instr.ip(), output);
        n += 1;
    }
}

fn disassemble_around(data: &[u8], pe: &PeInfo, file_offset: usize, label: &str, count: usize) {
    let start = file_offset.saturating_sub(64);
    let end = (file_offset + 256).min(data.len());
    let code = &data[start..end];
    let start_rva = pe.file_offset_to_rva(start).unwrap_or(0);
    let rip = pe.image_base + start_rva as u64;

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

        let instr_file = pe.rva_to_file_offset((instr.ip() - pe.image_base) as u32);
        let marker = if instr_file.is_some_and(|off| off.abs_diff(file_offset) < 8) {
            " <<<<".to_string()
        } else {
            String::new()
        };

        println!("    {:016X}  {}{}", instr.ip(), output, marker);
        n += 1;
    }
}

/// Scan backwards from `offset` looking for function start (int3 padding or CC CC pattern).
fn find_function_start(data: &[u8], offset: usize) -> usize {
    // Look for CC CC (int3 padding) pattern, then take the byte after
    let search_start = offset.saturating_sub(4096);
    let mut i = offset;
    while i > search_start {
        i -= 1;
        // CC CC followed by non-CC = function boundary
        if i >= 1
            && data[i] == 0xCC
            && data[i - 1] == 0xCC
            && (i + 1 >= data.len() || data[i + 1] != 0xCC)
        {
            return i + 1;
        }
        // Also check for typical function prologue patterns:
        // push rbp (0x55) or sub rsp, imm (0x48 0x83 0xEC or 0x48 0x81 0xEC)
        if i + 3 < data.len() {
            // mov [rsp+...], rbx  or  push rbx  after CC padding
            if i >= 1 && data[i - 1] == 0xCC {
                return i;
            }
        }
    }
    // Fallback: couldn't find, use original offset minus some buffer
    offset.saturating_sub(256)
}

/// Extract ASCII strings from data matching any keyword.
fn extract_crypto_strings(data: &[u8], base_offset: usize) -> Vec<(usize, String)> {
    let mut results = Vec::new();
    let mut current_start = 0;
    let mut current = String::new();

    for (i, &b) in data.iter().enumerate() {
        if b.is_ascii_graphic() || b == b' ' {
            if current.is_empty() {
                current_start = i;
            }
            current.push(b as char);
        } else {
            if current.len() >= 4 {
                let lower = current.to_lowercase();
                for &kw in CRYPTO_KEYWORDS {
                    if lower.contains(&kw.to_lowercase()) {
                        results.push((base_offset + current_start, current.clone()));
                        break;
                    }
                }
            }
            current.clear();
        }
    }
    if current.len() >= 4 {
        let lower = current.to_lowercase();
        for &kw in CRYPTO_KEYWORDS {
            if lower.contains(&kw.to_lowercase()) {
                results.push((base_offset + current_start, current.clone()));
                break;
            }
        }
    }
    results
}

// ══════════════════════════════════════════════════════════════
// Pass A: Other DLL Crypto Scan
// ══════════════════════════════════════════════════════════════

struct DllScanResult {
    path: String,
    has_crypto: bool,
    has_beyond_exports: bool,
}

fn pass_a_other_dll_scan(ga_data: &[u8]) -> Vec<DllScanResult> {
    println!("\n╔══════════════════════════════════════════════════════════════╗");
    println!("║  Pass A: Other DLL Crypto Scan (HIGHEST PRIORITY)         ║");
    println!("╚══════════════════════════════════════════════════════════════╝\n");

    // Extract S-box fingerprints (first 32 bytes of each) from GameAssembly.dll
    let mut sbox_fingerprints: Vec<[u8; 32]> = Vec::new();
    for &off in &SBOX_FILE_OFFSETS {
        if off + 32 <= ga_data.len() {
            let mut fp = [0u8; 32];
            fp.copy_from_slice(&ga_data[off..off + 32]);
            sbox_fingerprints.push(fp);
        }
    }

    let vfs_sbox_fp: [u8; 32] = VFS_AES_SBOX[..32].try_into().unwrap();

    let mut results = Vec::new();

    for &dll_path in OTHER_DLLS {
        println!("  ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
        println!("  Scanning: {}", dll_path);

        let data = match fs::read(dll_path) {
            Ok(d) => d,
            Err(e) => {
                println!("    ERROR: {}", e);
                results.push(DllScanResult {
                    path: dll_path.to_string(),
                    has_crypto: false,
                    has_beyond_exports: false,
                });
                continue;
            }
        };

        println!("    Size: {}", format_bytes(data.len() as u64));

        let pe = parse_pe(&data);
        println!("    Image base: 0x{:016X}", pe.image_base);
        println!("    Sections: {}", pe.sections.len());
        for s in &pe.sections {
            println!(
                "      {:>10} VA:0x{:08X} raw:0x{:08X} size:{}",
                s.name,
                s.virtual_address,
                s.pointer_to_raw_data,
                format_bytes(s.size_of_raw_data as u64)
            );
        }

        // List ALL exports
        let mut has_beyond_exports = false;
        let mut has_crypto = false;

        println!("\n    Exports ({}):", pe.exports.len());
        if pe.exports.is_empty() {
            println!("      (none)");
        } else {
            for exp in &pe.exports {
                let lower = exp.name.to_lowercase();
                let interesting = lower.contains("beyond")
                    || lower.contains("decrypt")
                    || lower.contains("encrypt")
                    || lower.contains("vfs")
                    || lower.contains("cipher")
                    || lower.contains("aes")
                    || lower.contains("crypt");

                if interesting {
                    println!(
                        "      \x1b[1;33m*** {:50} RVA:0x{:08X}\x1b[0m",
                        exp.name, exp.rva
                    );
                    if lower.contains("beyond") || lower.contains("decrypt") {
                        has_beyond_exports = true;
                    }
                    has_crypto = true;
                } else {
                    println!("      {:50} RVA:0x{:08X}", exp.name, exp.rva);
                }
            }
        }

        // Search for S-box fingerprints (first 32 bytes)
        println!("\n    S-box fingerprint search:");
        for (i, fp) in sbox_fingerprints.iter().enumerate() {
            let mut count = 0;
            if data.len() >= 32 {
                for j in 0..=(data.len() - 32) {
                    if data[j..j + 32] == fp[..] {
                        count += 1;
                        if count <= 3 {
                            let rva = pe.file_offset_to_rva(j).unwrap_or(0);
                            println!(
                                "      \x1b[1;32mGA S-box {} fingerprint at file:0x{:08X} rva:0x{:08X}\x1b[0m",
                                i, j, rva
                            );
                            has_crypto = true;
                        }
                    }
                }
            }
            if count == 0 {
                println!("      GA S-box {} — not found", i);
            }
        }

        // Search for VFS_AES_SBOX fingerprint
        {
            let mut count = 0;
            if data.len() >= 32 {
                for j in 0..=(data.len() - 32) {
                    if data[j..j + 32] == vfs_sbox_fp[..] {
                        count += 1;
                        if count <= 3 {
                            let rva = pe.file_offset_to_rva(j).unwrap_or(0);
                            println!(
                                "      \x1b[1;32mVFS_AES_SBOX fingerprint at file:0x{:08X} rva:0x{:08X}\x1b[0m",
                                j, rva
                            );
                            has_crypto = true;
                        }
                    }
                }
            }
            if count == 0 {
                println!("      VFS_AES_SBOX — not found");
            }
        }

        // Search for VFS_AES_KEY and VFS_AES_IV
        for (name, pattern) in [
            ("VFS_AES_KEY", &VFS_AES_KEY[..]),
            ("VFS_AES_IV", &VFS_AES_IV[..]),
        ] {
            let plen = pattern.len();
            let mut count = 0;
            if data.len() >= plen {
                for j in 0..=(data.len() - plen) {
                    if data[j..j + plen] == *pattern {
                        count += 1;
                        if count <= 3 {
                            let rva = pe.file_offset_to_rva(j).unwrap_or(0);
                            println!(
                                "      \x1b[1;32m{} at file:0x{:08X} rva:0x{:08X}\x1b[0m",
                                name, j, rva
                            );
                            has_crypto = true;
                        }
                    }
                }
            }
            if count == 0 {
                println!("      {} — not found", name);
            }
        }

        // Search for 4 directory hash values as u32 LE
        println!("\n    Directory hash search:");
        for (hash, name) in &DIR_HASHES {
            let pattern = hash.to_le_bytes();
            let mut count = 0;
            if data.len() >= 4 {
                for j in 0..=(data.len() - 4) {
                    if data[j..j + 4] == pattern {
                        count += 1;
                    }
                }
            }
            if count > 0 {
                println!("      {} (0x{:08X}): {} occurrences", name, hash, count);
                has_crypto = true;
            } else {
                println!("      {} — not found", name);
            }
        }

        // Scan for unknown 256-byte permutations in data sections
        println!("\n    Unknown permutation scan (data sections):");
        let mut perm_count = 0;
        for sec in &pe.sections {
            // Skip code-like sections for permutation scan
            if sec.name == ".text" || sec.name == ".pdata" || sec.name == ".reloc" {
                continue;
            }
            let range = sec.file_range();
            let start = range.start.min(data.len());
            let end = range.end.min(data.len());
            if end < start + 256 {
                continue;
            }
            // Scan on 16-byte alignment
            let mut off = (start + 15) & !15;
            while off + 256 <= end {
                let block = &data[off..off + 256];
                let ent = entropy(block);
                if ent > 7.5 && is_permutation(block) {
                    perm_count += 1;
                    let rva = pe.file_offset_to_rva(off).unwrap_or(0);
                    if perm_count <= 10 {
                        println!(
                            "      \x1b[1;32mPermutation in {} at file:0x{:08X} rva:0x{:08X} entropy={:.3}\x1b[0m",
                            sec.name, off, rva, ent
                        );
                        hex_dump(block, off, 64);
                    }
                    has_crypto = true;
                }
                off += 16;
            }
        }
        if perm_count == 0 {
            println!("      No unknown permutations found");
        } else if perm_count > 10 {
            println!("      ... {} total permutations", perm_count);
        }

        // Extract crypto-related strings
        println!("\n    Crypto keyword strings:");
        let mut all_strings = Vec::new();
        for sec in &pe.sections {
            let range = sec.file_range();
            let start = range.start.min(data.len());
            let end = range.end.min(data.len());
            let strings = extract_crypto_strings(&data[start..end], start);
            all_strings.extend(strings);
        }
        // Deduplicate by offset
        all_strings.sort_by_key(|(off, _)| *off);
        all_strings.dedup_by_key(|(off, _)| *off);

        if all_strings.is_empty() {
            println!("      (none)");
        } else {
            has_crypto = has_crypto || !all_strings.is_empty();
            for (off, s) in all_strings.iter().take(30) {
                let rva = pe.file_offset_to_rva(*off).unwrap_or(0);
                let trunc = if s.len() > 100 { &s[..100] } else { s.as_str() };
                println!("      file:0x{:08X} rva:0x{:08X} \"{}\"", off, rva, trunc);
            }
            if all_strings.len() > 30 {
                println!("      ... {} total crypto strings", all_strings.len());
            }
        }

        println!();
        results.push(DllScanResult {
            path: dll_path.to_string(),
            has_crypto,
            has_beyond_exports,
        });
    }

    // Summary
    println!("  === Pass A Summary ===");
    for r in &results {
        let flags = format!(
            "{}{}",
            if r.has_beyond_exports {
                " \x1b[1;33m[BEYOND EXPORTS]\x1b[0m"
            } else {
                ""
            },
            if r.has_crypto {
                " \x1b[1;32m[CRYPTO]\x1b[0m"
            } else {
                ""
            },
        );
        println!("    {}{}", r.path, flags);
    }

    results
}

// ══════════════════════════════════════════════════════════════
// Pass B: Function Pointer Consumers
// ══════════════════════════════════════════════════════════════

fn pass_b_function_pointer_consumers(data: &[u8], pe: &PeInfo) {
    println!("\n╔══════════════════════════════════════════════════════════════╗");
    println!("║  Pass B: Function Pointer Consumers ([0x18F60CD80])        ║");
    println!("╚══════════════════════════════════════════════════════════════╝\n");

    let target_va: u64 = GA_IMAGE_BASE + DECRYPT_FUNC_PTR_RVA as u64;
    println!(
        "  Target: [0x{:016X}] (RVA 0x{:08X})",
        target_va, DECRYPT_FUNC_PTR_RVA
    );
    println!("  Searching for RIP-relative accesses in code sections...\n");

    let code_sections: Vec<&Section> = pe
        .sections
        .iter()
        .filter(|s| s.name == ".text" || s.name == "il2cpp")
        .collect();

    let mut total_hits = 0;

    for sec in &code_sections {
        let range = sec.file_range();
        let sec_start = range.start.min(data.len());
        let sec_end = range.end.min(data.len());
        let sec_data = &data[sec_start..sec_end];

        println!(
            "  Scanning {} ({})...",
            sec.name,
            format_bytes(sec.size_of_raw_data as u64)
        );

        if sec_data.len() < 8 {
            continue;
        }

        // Hybrid approach: fast byte scan with instruction prefix length guesses
        // For RIP-relative addressing, displacement is at instruction[prefix_len..prefix_len+4]
        // and target = RIP + instruction_length + displacement
        // So displacement = target_va - (instruction_va + instruction_length)
        // For prefix lengths of 3 (opcode+modrm, e.g. mov reg, [rip+disp32]):
        //   disp = target_va - (instr_va + 7)  [3 prefix + 4 disp = 7 total]
        // For prefix lengths of 2 (no REX):
        //   disp = target_va - (instr_va + 6)  [2 prefix + 4 disp = 6 total]

        let instr_lengths: &[u32] = &[6, 7]; // total instruction lengths to try

        for i in 0..=(sec_data.len() - 4) {
            let instr_va_base = GA_IMAGE_BASE + (sec.virtual_address + i as u32) as u64;

            for &total_len in instr_lengths {
                // The disp32 starts at i, and the instruction started at i-(total_len-4)
                let prefix_len = total_len - 4;
                if i < prefix_len as usize {
                    continue;
                }
                let instr_start = i - prefix_len as usize;
                let instr_va = GA_IMAGE_BASE + (sec.virtual_address + instr_start as u32) as u64;
                let rip_at_end = instr_va + total_len as u64;
                let expected_disp = target_va.wrapping_sub(rip_at_end) as u32;
                let expected_bytes = expected_disp.to_le_bytes();

                if sec_data[i..i + 4] == expected_bytes {
                    // Verify with iced-x86 decoder
                    let decode_start = instr_start.saturating_sub(4); // give some room
                    let decode_end = (instr_start + 16).min(sec_data.len());
                    let decode_buf = &sec_data[decode_start..decode_end];
                    let decode_rip =
                        GA_IMAGE_BASE + (sec.virtual_address + decode_start as u32) as u64;

                    let mut decoder =
                        Decoder::with_ip(64, decode_buf, decode_rip, DecoderOptions::NONE);
                    let mut found = false;

                    while decoder.can_decode() {
                        let instr = decoder.decode();
                        if instr.is_invalid() {
                            continue;
                        }
                        // Check if this instruction uses RIP-relative addressing to our target
                        if instr.is_ip_rel_memory_operand() {
                            let mem_addr = instr.ip_rel_memory_address();
                            if mem_addr == target_va {
                                // Skip known call sites (we already know about those)
                                let instr_rva = (instr.ip() - GA_IMAGE_BASE) as u32;
                                let is_known = (instr_rva.abs_diff(CALL_SITE_1_RVA) < 32)
                                    || (instr_rva.abs_diff(CALL_SITE_2_RVA) < 32);

                                total_hits += 1;
                                let file_off = sec_start + instr_start;
                                let tag = if is_known { " [KNOWN CALL SITE]" } else { "" };

                                // Categorize the instruction
                                let mut formatter = IntelFormatter::new();
                                formatter.options_mut().set_first_operand_char_index(10);
                                let mut output = String::new();
                                formatter.format(&instr, &mut output);

                                let category = if output.contains("call") {
                                    "INDIRECT CALL"
                                } else if output.contains("mov") || output.contains("lea") {
                                    "LOAD (for call reg)"
                                } else if output.contains("cmp") || output.contains("test") {
                                    "NULL CHECK"
                                } else {
                                    "OTHER"
                                };

                                println!(
                                    "    \x1b[1;33mHIT #{}: {} at rva:0x{:08X} file:0x{:08X}{}\x1b[0m",
                                    total_hits, category, instr_rva, file_off, tag
                                );
                                println!("      {}", output);

                                if !is_known {
                                    // Disassemble wider context
                                    disassemble_around(
                                        data,
                                        pe,
                                        file_off,
                                        &format!(
                                            "Consumer #{} ({}) at rva:0x{:08X}",
                                            total_hits, category, instr_rva
                                        ),
                                        40,
                                    );
                                }

                                found = true;
                                break;
                            }
                        }
                        // Don't scan too far
                        if instr.ip() > instr_va_base + 8 {
                            break;
                        }
                    }

                    if found {
                        break; // Found for this offset, don't try other prefix lengths
                    }
                }
            }
        }
    }

    println!(
        "\n  Total consumers of [0x{:016X}]: {}",
        target_va, total_hits
    );
    if total_hits == 0 {
        println!("  WARNING: No consumers found! The pointer may be accessed indirectly.");
        println!(
            "  Consider: the dispatcher may load the address from a vtable or through another indirection."
        );
    }
}

// ══════════════════════════════════════════════════════════════
// Pass C: Extended Call Site Disassembly
// ══════════════════════════════════════════════════════════════

fn pass_c_extended_call_sites(data: &[u8], pe: &PeInfo) {
    println!("\n╔══════════════════════════════════════════════════════════════╗");
    println!("║  Pass C: Extended Call Site Disassembly                     ║");
    println!("╚══════════════════════════════════════════════════════════════╝\n");

    // Call site 2: function prologue at 0x18C08E4B0 (RVA 0x0C08E4B0)
    println!(
        "  === Call Site 2 (RVA 0x{:08X}) — Function Prologue ===",
        CALL_SITE_2_RVA
    );
    if let Some(file_off) = pe.rva_to_file_offset(CALL_SITE_2_RVA) {
        println!("  File offset: 0x{:08X}", file_off);
        println!("  Disassembling 150 instructions from function prologue...");
        disassemble_at(data, pe, file_off, 150, "Call Site 2 — Full Function");

        // Also annotate what we see: look for call instructions and parameter setup
        println!("\n  Analyzing parameter passing pattern (Win x64: rcx, rdx, r8, r9)...");
        let end = (file_off + 150 * 15).min(data.len());
        let code = &data[file_off..end];
        let rip = GA_IMAGE_BASE + CALL_SITE_2_RVA as u64;
        let mut decoder = Decoder::with_ip(64, code, rip, DecoderOptions::NONE);
        let mut formatter = IntelFormatter::new();
        formatter.options_mut().set_first_operand_char_index(10);
        let mut output = String::new();
        let mut n = 0;

        println!("\n  Key instructions (calls, parameter moves, branches):");
        while decoder.can_decode() && n < 150 {
            let instr = decoder.decode();
            n += 1;
            output.clear();
            formatter.format(&instr, &mut output);

            let interesting = output.contains("call")
                || (output.contains("mov")
                    && (output.contains("rcx")
                        || output.contains("rdx")
                        || output.contains("r8")
                        || output.contains("r9")))
                || output.contains("jmp")
                || output.contains("ret");

            if interesting {
                let rva = (instr.ip() - GA_IMAGE_BASE) as u32;
                println!("    0x{:08X}  {}", rva, output);
            }
        }
    } else {
        println!("    ERROR: Could not resolve RVA to file offset");
    }

    // Call site 1: instruction at 0x18C0376FF (RVA 0x0C0376FF)
    println!(
        "\n\n  === Call Site 1 (RVA 0x{:08X}) — Find Function Start ===",
        CALL_SITE_1_RVA
    );
    if let Some(file_off) = pe.rva_to_file_offset(CALL_SITE_1_RVA) {
        println!("  Instruction file offset: 0x{:08X}", file_off);

        // Find function start by scanning backwards for int3 padding
        let func_start = find_function_start(data, file_off);
        let func_start_rva = pe.file_offset_to_rva(func_start).unwrap_or(0);
        println!(
            "  Function start: file:0x{:08X} rva:0x{:08X} (distance: {} bytes)",
            func_start,
            func_start_rva,
            file_off - func_start
        );

        println!("  Disassembling 150 instructions from function start...");
        disassemble_at(data, pe, func_start, 150, "Call Site 1 — Full Function");

        // Annotate key instructions
        let end = (func_start + 150 * 15).min(data.len());
        let code = &data[func_start..end];
        let rip = GA_IMAGE_BASE + func_start_rva as u64;
        let mut decoder = Decoder::with_ip(64, code, rip, DecoderOptions::NONE);
        let mut formatter = IntelFormatter::new();
        formatter.options_mut().set_first_operand_char_index(10);
        let mut output = String::new();
        let mut n = 0;

        println!("\n  Key instructions (calls, parameter moves, branches):");
        while decoder.can_decode() && n < 150 {
            let instr = decoder.decode();
            n += 1;
            output.clear();
            formatter.format(&instr, &mut output);

            let interesting = output.contains("call")
                || (output.contains("mov")
                    && (output.contains("rcx")
                        || output.contains("rdx")
                        || output.contains("r8")
                        || output.contains("r9")))
                || output.contains("jmp")
                || output.contains("ret");

            if interesting {
                let rva = (instr.ip() - GA_IMAGE_BASE) as u32;
                println!("    0x{:08X}  {}", rva, output);
            }
        }
    } else {
        println!("    ERROR: Could not resolve RVA to file offset");
    }
}

// ══════════════════════════════════════════════════════════════
// Pass D: HGP.dll Deep Dive
// ══════════════════════════════════════════════════════════════

fn pass_d_hgp_deep_dive(dll_results: &[DllScanResult], ga_data: &[u8]) {
    println!("\n╔══════════════════════════════════════════════════════════════╗");
    println!("║  Pass D: HGP.dll Deep Dive                                 ║");
    println!("╚══════════════════════════════════════════════════════════════╝\n");

    // Check if HGP.dll had anything interesting from Pass A
    let hgp_result = dll_results.iter().find(|r| r.path.contains("HGP"));
    let hgp_interesting = hgp_result.is_some_and(|r| r.has_crypto || r.has_beyond_exports);

    if !hgp_interesting {
        println!("  HGP.dll showed no crypto content or beyond exports in Pass A.");
        println!("  Performing minimal investigation anyway...\n");
    } else {
        println!("  HGP.dll flagged as interesting in Pass A — performing deep dive.\n");
    }

    let hgp_data = match fs::read("./output/HGP.dll") {
        Ok(d) => d,
        Err(e) => {
            println!("  ERROR loading HGP.dll: {}", e);
            return;
        }
    };

    let pe = parse_pe(&hgp_data);

    // Disassemble first 20 instructions of each exported function
    if !pe.exports.is_empty() {
        println!("  === Export Function Prologues ===");
        for exp in pe.exports.iter().take(50) {
            if let Some(file_off) = pe.rva_to_file_offset(exp.rva)
                && file_off + 16 <= hgp_data.len()
            {
                disassemble_at(
                    &hgp_data,
                    &pe,
                    file_off,
                    20,
                    &format!("Export: {} (RVA:0x{:08X})", exp.name, exp.rva),
                );
            }
        }
    } else {
        println!("  No exports in HGP.dll");
    }

    // Extended string extraction
    println!("\n  === Extended String Extraction ===");
    let mut all_strings = Vec::new();
    for sec in &pe.sections {
        let range = sec.file_range();
        let start = range.start.min(hgp_data.len());
        let end = range.end.min(hgp_data.len());
        let strings = extract_crypto_strings(&hgp_data[start..end], start);
        all_strings.extend(strings);
    }
    all_strings.sort_by_key(|(off, _)| *off);
    all_strings.dedup_by_key(|(off, _)| *off);

    if all_strings.is_empty() {
        println!("    No crypto-related strings found");
    } else {
        for (off, s) in all_strings.iter().take(50) {
            let rva = pe.file_offset_to_rva(*off).unwrap_or(0);
            let trunc = if s.len() > 100 { &s[..100] } else { s.as_str() };
            println!("    file:0x{:08X} rva:0x{:08X} \"{}\"", off, rva, trunc);
        }
        if all_strings.len() > 50 {
            println!("    ... {} total", all_strings.len());
        }
    }

    // Compare any found permutations against known S-boxes
    println!("\n  === S-box Comparison (if any found) ===");

    // Extract GA S-boxes for comparison
    let mut ga_sboxes: Vec<[u8; 256]> = Vec::new();
    for &off in &SBOX_FILE_OFFSETS {
        if off + 256 <= ga_data.len() {
            let mut sbox = [0u8; 256];
            sbox.copy_from_slice(&ga_data[off..off + 256]);
            ga_sboxes.push(sbox);
        }
    }

    let mut found_perms = Vec::new();
    for sec in &pe.sections {
        if sec.name == ".text" || sec.name == ".pdata" || sec.name == ".reloc" {
            continue;
        }
        let range = sec.file_range();
        let start = range.start.min(hgp_data.len());
        let end = range.end.min(hgp_data.len());
        if end < start + 256 {
            continue;
        }
        let mut off = (start + 15) & !15;
        while off + 256 <= end {
            let block = &hgp_data[off..off + 256];
            if entropy(block) > 7.5 && is_permutation(block) {
                let mut perm = [0u8; 256];
                perm.copy_from_slice(block);
                found_perms.push((off, perm));
            }
            off += 16;
        }
    }

    if found_perms.is_empty() {
        println!("    No permutations found in HGP.dll");
    } else {
        for (off, perm) in &found_perms {
            let rva = pe.file_offset_to_rva(*off).unwrap_or(0);
            println!("    Permutation at file:0x{:08X} rva:0x{:08X}:", off, rva);

            // Compare against each GA S-box
            for (i, ga_sbox) in ga_sboxes.iter().enumerate() {
                let hamming: usize = perm
                    .iter()
                    .zip(ga_sbox.iter())
                    .filter(|&(a, b)| a != b)
                    .count();
                let is_inverse = (0..256).all(|x| ga_sbox[perm[x] as usize] == x as u8);
                let tag = if hamming == 0 {
                    " \x1b[1;32m[IDENTICAL]\x1b[0m"
                } else if is_inverse {
                    " \x1b[1;33m[INVERSE]\x1b[0m"
                } else {
                    ""
                };
                println!("      vs GA S-box {}: Hamming={}{}", i, hamming, tag);
            }

            // Also compare with VFS_AES_SBOX
            let vfs_hamming: usize = perm
                .iter()
                .zip(VFS_AES_SBOX.iter())
                .filter(|&(a, b)| a != b)
                .count();
            let vfs_inverse = (0..256).all(|x| VFS_AES_SBOX[perm[x] as usize] == x as u8);
            let tag = if vfs_hamming == 0 {
                " [IDENTICAL]"
            } else if vfs_inverse {
                " [INVERSE]"
            } else {
                ""
            };
            println!("      vs VFS_AES_SBOX: Hamming={}{}", vfs_hamming, tag);
        }
    }

    // List exports of ALL scanned DLLs for completeness
    println!("\n  === All DLL Export Summary ===");
    for &dll_path in OTHER_DLLS {
        let dll_data = match fs::read(dll_path) {
            Ok(d) => d,
            Err(_) => continue,
        };
        let dll_pe = parse_pe(&dll_data);
        println!("    {} ({} exports):", dll_path, dll_pe.exports.len());
        if dll_pe.exports.is_empty() {
            println!("      (none)");
        } else {
            for exp in dll_pe.exports.iter().take(20) {
                println!("      {} (RVA:0x{:08X})", exp.name, exp.rva);
            }
            if dll_pe.exports.len() > 20 {
                println!("      ... {} total exports", dll_pe.exports.len());
            }
        }
    }
}

// ══════════════════════════════════════════════════════════════
// Main
// ══════════════════════════════════════════════════════════════

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("=== Phase 2c: Decryption Dispatch & Cross-DLL Crypto Analysis ===\n");
    println!("Goal: Find WHO dispatches decryption and WHICH DLL implements it.\n");

    // Load GameAssembly.dll
    let ga_data = fs::read(GA_PATH)?;
    println!(
        "Loaded GameAssembly.dll: {} ({})\n",
        format_bytes(ga_data.len() as u64),
        ga_data.len()
    );

    let ga_pe = parse_pe(&ga_data);
    println!("GA sections: {}", ga_pe.sections.len());
    for s in &ga_pe.sections {
        println!(
            "  {:>10} VA:0x{:08X} raw:0x{:08X} size:{}",
            s.name,
            s.virtual_address,
            s.pointer_to_raw_data,
            format_bytes(s.size_of_raw_data as u64)
        );
    }

    // Pass A: Scan other DLLs (HIGHEST PRIORITY — run first)
    let dll_results = pass_a_other_dll_scan(&ga_data);

    // Pass B: Find consumers of the cached function pointer
    pass_b_function_pointer_consumers(&ga_data, &ga_pe);

    // Pass C: Extended call site disassembly
    pass_c_extended_call_sites(&ga_data, &ga_pe);

    // Pass D: HGP.dll deep dive
    pass_d_hgp_deep_dive(&dll_results, &ga_data);

    // ══════════════════════════════════════════════════════════════
    // Summary
    // ══════════════════════════════════════════════════════════════
    println!("\n╔══════════════════════════════════════════════════════════════╗");
    println!("║  Summary                                                    ║");
    println!("╚══════════════════════════════════════════════════════════════╝\n");

    println!("  DLLs scanned: {}", OTHER_DLLS.len());
    for r in &dll_results {
        let flags = format!(
            "{}{}",
            if r.has_beyond_exports {
                " [BEYOND]"
            } else {
                ""
            },
            if r.has_crypto { " [CRYPTO]" } else { "" },
        );
        println!("    {}{}", r.path, flags);
    }

    let beyond_dll = dll_results.iter().find(|r| r.has_beyond_exports);
    if let Some(dll) = beyond_dll {
        println!("\n  \x1b[1;33mLikely decryption DLL: {}\x1b[0m", dll.path);
    } else {
        println!("\n  No DLL with beyond/decrypt exports found.");
        println!("  The decryption function may be loaded dynamically or embedded in VM code.");
    }

    println!("\n=== Phase 2c Complete ===");
    Ok(())
}
