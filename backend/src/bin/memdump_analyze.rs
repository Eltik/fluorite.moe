//! Windows Minidump Analyzer for IL2CPP Game Memory
//!
//! Parses a `.dmp` file created by Windows Task Manager and scans for:
//! - IL2CPP `chr_XXXX` strings (UTF-16LE) → CharInfo objects
//! - CharInfo → Attributes → m_rawData pointer chain traversal
//! - JSON objects, UnityFS headers, known class names
//!
//! Outputs `il2cpp_char_events.jsonl` compatible with `char_data_probe.rs`.
//!
//! Usage:
//!   cargo run --bin memdump-analyze --release -- <dump.dmp> [--output-dir ./output]

use clap::Parser;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{BufWriter, Read, Seek, SeekFrom, Write};
use std::path::PathBuf;

// ── CLI ─────────────────────────────────────────────────────────────────

#[derive(Parser)]
#[command(name = "memdump-analyze")]
#[command(about = "Analyze Windows minidump for IL2CPP character data")]
struct Cli {
    /// Path to .dmp file (from Task Manager "Create dump file")
    dump_path: PathBuf,

    /// Output directory
    #[arg(long, default_value = "./output/memdump")]
    output_dir: PathBuf,

    /// Also scan for JSON objects
    #[arg(long)]
    scan_json: bool,

    /// Also scan for UnityFS headers
    #[arg(long)]
    scan_unity: bool,

    /// Verbose output
    #[arg(short, long)]
    verbose: bool,
}

// ── Minidump Format Constants ───────────────────────────────────────────

const MDMP_MAGIC: u32 = 0x504D444D; // 'MDMP'

// Stream types we care about
const MEMORY_64_LIST_STREAM: u32 = 9;
const MEMORY_LIST_STREAM: u32 = 5;
const _MODULE_LIST_STREAM: u32 = 4;

// ── IL2CPP Layout Constants (from proxy.c) ──────────────────────────────

const CI_TEMPLATE_ID: usize = 0x18; // +24: Il2CppString* templateId
const CI_LEVEL: usize = 0x44;       // +68: int level
const CI_BREAK_STAGE: usize = 0x4C; // +76: int breakStage
const CI_POTENTIAL: usize = 0x60;    // +96: int potentialLevel
const CI_ATTRS: usize = 0xC8;       // +200: Attributes*
const CI_ATTRS_PACK: usize = 0xB8;  // +184: attributesPack (fallback)
const AT_RAW_DATA: usize = 0x60;    // +96: Attributes.m_rawData

// Il2CppString layout: [klass 8][monitor 8][length_i32 4][utf16_chars...]
const IL2CPP_STR_LEN_OFFSET: usize = 16;
const IL2CPP_STR_DATA_OFFSET: usize = 20;

// Il2CppArray<Double> layout: [klass 8][monitor 8][bounds 8][max_length_i32 4][pad 4][doubles...]
const IL2CPP_ARR_MAXLEN_OFFSET: usize = 24;
const IL2CPP_ARR_DATA_OFFSET: usize = 32;

// Attribute names (index into m_rawData double array)
const ATTR_NAMES: &[&str] = &[
    "hp", "str", "agi", "wisd", "will",                              // 0-4
    "atk", "def", "maxHp",                                            // 5-7
    "maxUltimateSp", "maxPoise", "poiseRecTime", "poiseRecVal",       // 8-11
    "staminaMax", "staminaRecTime", "energyMax", "energyRecSpeed",     // 12-15
    "criticalRate", "criticalDamage", "attackSpeed", "attackRate",      // 16-19
    "skillCooldownScalar",                                             // 20
    "unknown_21", "unknown_22", "unknown_23",                          // 21-23
    "normalAttackRange", "hpRecoveryPerSec",                           // 24-25
    "physDmgIncrease", "physDmgResist",                                // 26-27
    "igniteDmgIncrease", "igniteDmgResist",                            // 28-29
    "freezeDmgIncrease", "freezeDmgResist",                            // 30-31
    "voltageDmgIncrease", "voltageDmgResist",                          // 32-33
    "erosionDmgIncrease", "erosionDmgResist",                          // 34-35
    "binaryDmgIncrease", "binaryDmgResist",                            // 36-37
    "physPenetrate", "elementPenetrate",                                // 38-39
    "unknown_40", "unknown_41", "unknown_42", "unknown_43",            // 40-43
    "healIncrease", "healReceive",                                     // 44-45
    "shieldIncrease",                                                  // 46
    "etherDmgIncrease",                                                // 47
    "unknown_48", "unknown_49",                                        // 48-49
];

// ── Memory Region Map ───────────────────────────────────────────────────

/// A contiguous memory region from the dump.
struct MemRegion {
    /// Virtual address in the original process
    va_start: u64,
    /// Size of region in bytes
    size: u64,
    /// Offset in our loaded data buffer
    file_offset: u64,
}

/// Complete memory image from the dump.
struct MemoryImage {
    /// Sorted list of memory regions
    regions: Vec<MemRegion>,
    /// Raw dump data (all regions concatenated, or mmap'd)
    data: Vec<u8>,
    /// Base RVA offset for the data blob within the file
    data_base_rva: u64,
}

impl MemoryImage {
    /// Read bytes at a virtual address. Returns None if address not mapped.
    fn read_bytes(&self, va: u64, len: usize) -> Option<&[u8]> {
        for r in &self.regions {
            if va >= r.va_start && va < r.va_start + r.size {
                let offset_in_region = (va - r.va_start) as usize;
                if offset_in_region + len > r.size as usize {
                    return None;
                }
                let abs_offset = (r.file_offset - self.data_base_rva) as usize + offset_in_region;
                if abs_offset + len > self.data.len() {
                    return None;
                }
                return Some(&self.data[abs_offset..abs_offset + len]);
            }
        }
        None
    }

    /// Read a u32 at virtual address.
    #[allow(dead_code)]
    fn read_u32(&self, va: u64) -> Option<u32> {
        let b = self.read_bytes(va, 4)?;
        Some(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    /// Read a u64 (pointer) at virtual address.
    fn read_u64(&self, va: u64) -> Option<u64> {
        let b = self.read_bytes(va, 8)?;
        Some(u64::from_le_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]]))
    }

    /// Read an i32 at virtual address.
    fn read_i32(&self, va: u64) -> Option<i32> {
        let b = self.read_bytes(va, 4)?;
        Some(i32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    /// Read a f64 at virtual address.
    #[allow(dead_code)]
    fn read_f64(&self, va: u64) -> Option<f64> {
        let b = self.read_bytes(va, 8)?;
        Some(f64::from_le_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]]))
    }

    /// Read a UTF-16LE Il2CppString at pointer `va`.
    fn read_il2cpp_string(&self, va: u64) -> Option<String> {
        let len = self.read_i32(va + IL2CPP_STR_LEN_OFFSET as u64)?;
        if len <= 0 || len > 512 {
            return None;
        }
        let len = len as usize;
        let data = self.read_bytes(va + IL2CPP_STR_DATA_OFFSET as u64, len * 2)?;
        let utf16: Vec<u16> = data
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();
        String::from_utf16(&utf16).ok()
    }

    /// Read an Il2CppArray<double> at pointer `va`. Returns the doubles.
    fn read_double_array(&self, va: u64) -> Option<Vec<f64>> {
        let max_len = self.read_i32(va + IL2CPP_ARR_MAXLEN_OFFSET as u64)?;
        if max_len <= 0 || max_len > 200 {
            return None;
        }
        let max_len = max_len as usize;
        let data = self.read_bytes(va + IL2CPP_ARR_DATA_OFFSET as u64, max_len * 8)?;
        let doubles: Vec<f64> = data
            .chunks_exact(8)
            .map(|c| f64::from_le_bytes([c[0], c[1], c[2], c[3], c[4], c[5], c[6], c[7]]))
            .collect();
        Some(doubles)
    }

    /// Check if a virtual address is mapped.
    fn is_mapped(&self, va: u64) -> bool {
        self.regions
            .iter()
            .any(|r| va >= r.va_start && va < r.va_start + r.size)
    }

    /// Iterate over all regions, yielding (va_start, region_data).
    fn iter_regions(&self) -> impl Iterator<Item = (u64, &[u8])> {
        self.regions.iter().map(move |r| {
            let start = (r.file_offset - self.data_base_rva) as usize;
            let end = start + r.size as usize;
            (r.va_start, &self.data[start..end.min(self.data.len())])
        })
    }
}

// ── Minidump Parser ─────────────────────────────────────────────────────

fn parse_minidump(path: &PathBuf) -> Result<MemoryImage, String> {
    let mut file = File::open(path).map_err(|e| format!("Cannot open dump: {e}"))?;

    // Read header
    let mut hdr = [0u8; 32];
    file.read_exact(&mut hdr)
        .map_err(|e| format!("Cannot read header: {e}"))?;

    let magic = u32::from_le_bytes([hdr[0], hdr[1], hdr[2], hdr[3]]);
    if magic != MDMP_MAGIC {
        return Err(format!(
            "Not a minidump file (magic: {magic:#010X}, expected {MDMP_MAGIC:#010X})"
        ));
    }

    let _version = u16::from_le_bytes([hdr[4], hdr[5]]);
    let stream_count = u32::from_le_bytes([hdr[8], hdr[9], hdr[10], hdr[11]]);
    let stream_dir_rva = u32::from_le_bytes([hdr[12], hdr[13], hdr[14], hdr[15]]);

    println!("  MDMP header OK, {stream_count} streams, directory at {stream_dir_rva:#X}");

    // Read stream directory
    file.seek(SeekFrom::Start(stream_dir_rva as u64))
        .map_err(|e| format!("Cannot seek to stream directory: {e}"))?;

    let mut streams = Vec::new();
    for _ in 0..stream_count {
        let mut entry = [0u8; 12];
        file.read_exact(&mut entry)
            .map_err(|e| format!("Cannot read stream entry: {e}"))?;
        let stream_type = u32::from_le_bytes([entry[0], entry[1], entry[2], entry[3]]);
        let data_size = u32::from_le_bytes([entry[4], entry[5], entry[6], entry[7]]);
        let rva = u32::from_le_bytes([entry[8], entry[9], entry[10], entry[11]]);
        streams.push((stream_type, data_size, rva));
    }

    // Find Memory64ListStream (type 9) — full dumps from Task Manager
    if let Some(&(_, _, rva)) = streams.iter().find(|(t, _, _)| *t == MEMORY_64_LIST_STREAM) {
        return parse_memory64_list(&mut file, rva as u64);
    }

    // Fallback: MemoryListStream (type 5) — partial dumps
    if let Some(&(_, _, rva)) = streams.iter().find(|(t, _, _)| *t == MEMORY_LIST_STREAM) {
        return parse_memory_list(&mut file, rva as u64);
    }

    Err("No memory stream found in dump".to_string())
}

fn parse_memory64_list(file: &mut File, rva: u64) -> Result<MemoryImage, String> {
    file.seek(SeekFrom::Start(rva))
        .map_err(|e| format!("Cannot seek to Memory64ListStream: {e}"))?;

    let mut header = [0u8; 16];
    file.read_exact(&mut header)
        .map_err(|e| format!("Cannot read Memory64List header: {e}"))?;

    let num_entries = u64::from_le_bytes(header[0..8].try_into().unwrap());
    let base_rva = u64::from_le_bytes(header[8..16].try_into().unwrap());

    println!("  Memory64List: {num_entries} regions, data starts at {base_rva:#X}");

    // Read the descriptor array
    let mut regions = Vec::with_capacity(num_entries as usize);
    let mut current_offset = base_rva;
    let mut total_size: u64 = 0;

    for _ in 0..num_entries {
        let mut desc = [0u8; 16];
        file.read_exact(&mut desc)
            .map_err(|e| format!("Cannot read region descriptor: {e}"))?;
        let start_va = u64::from_le_bytes(desc[0..8].try_into().unwrap());
        let data_size = u64::from_le_bytes(desc[8..16].try_into().unwrap());

        regions.push(MemRegion {
            va_start: start_va,
            size: data_size,
            file_offset: current_offset,
        });
        current_offset += data_size;
        total_size += data_size;
    }

    println!(
        "  Total memory: {} ({} regions)",
        format_bytes(total_size),
        regions.len()
    );

    // Read all memory data
    println!("  Loading memory data...");
    file.seek(SeekFrom::Start(base_rva))
        .map_err(|e| format!("Cannot seek to memory data: {e}"))?;

    let mut data = vec![0u8; total_size as usize];
    file.read_exact(&mut data)
        .map_err(|e| format!("Cannot read memory data: {e}"))?;

    println!("  Loaded {} into memory", format_bytes(total_size));

    Ok(MemoryImage {
        regions,
        data,
        data_base_rva: base_rva,
    })
}

fn parse_memory_list(file: &mut File, rva: u64) -> Result<MemoryImage, String> {
    file.seek(SeekFrom::Start(rva))
        .map_err(|e| format!("Cannot seek to MemoryListStream: {e}"))?;

    let mut header = [0u8; 4];
    file.read_exact(&mut header)
        .map_err(|e| format!("Cannot read MemoryList header: {e}"))?;
    let num_entries = u32::from_le_bytes(header) as usize;

    println!("  MemoryList: {num_entries} regions (partial dump)");

    // Each entry: StartOfMemoryRange (u64) + DataSize (u32) + RVA (u32)
    struct MemDescriptor {
        va_start: u64,
        data_size: u32,
        rva: u32,
    }

    let mut descs = Vec::with_capacity(num_entries);
    for _ in 0..num_entries {
        let mut entry = [0u8; 16]; // MINIDUMP_MEMORY_DESCRIPTOR: range(16) but actually it's different
        // Actually: MINIDUMP_MEMORY_DESCRIPTOR = { StartOfMemoryRange: u64, Memory: MINIDUMP_LOCATION_DESCRIPTOR { DataSize: u32, Rva: u32 } }
        file.read_exact(&mut entry)
            .map_err(|e| format!("Cannot read MemoryList entry: {e}"))?;
        let va_start = u64::from_le_bytes(entry[0..8].try_into().unwrap());
        let data_size = u32::from_le_bytes(entry[8..12].try_into().unwrap());
        let entry_rva = u32::from_le_bytes(entry[12..16].try_into().unwrap());
        descs.push(MemDescriptor {
            va_start,
            data_size,
            rva: entry_rva,
        });
    }

    // Sort by RVA and read contiguously
    descs.sort_by_key(|d| d.rva);

    let total_size: u64 = descs.iter().map(|d| d.data_size as u64).sum();

    // For MemoryList, regions aren't contiguous — we need to read each individually
    let mut data = Vec::with_capacity(total_size as usize);
    let mut regions = Vec::with_capacity(num_entries);
    let mut running_offset = 0u64;

    for d in &descs {
        file.seek(SeekFrom::Start(d.rva as u64))
            .map_err(|e| format!("Cannot seek to region: {e}"))?;
        let mut buf = vec![0u8; d.data_size as usize];
        file.read_exact(&mut buf)
            .map_err(|e| format!("Cannot read region: {e}"))?;

        regions.push(MemRegion {
            va_start: d.va_start,
            size: d.data_size as u64,
            file_offset: running_offset, // relative to our data vec
        });
        data.extend_from_slice(&buf);
        running_offset += d.data_size as u64;
    }

    println!(
        "  Total memory: {} ({} regions)",
        format_bytes(total_size),
        regions.len()
    );

    Ok(MemoryImage {
        regions,
        data,
        data_base_rva: 0, // our data vec starts at 0
    })
}

// ── Scanners ────────────────────────────────────────────────────────────

/// UTF-16LE pattern for "chr_" = [0x63, 0x00, 0x68, 0x00, 0x72, 0x00, 0x5F, 0x00]
const CHR_PATTERN: [u8; 8] = [0x63, 0x00, 0x68, 0x00, 0x72, 0x00, 0x5F, 0x00];

/// Scan for `chr_XXXX` UTF-16LE strings and resolve CharInfo objects.
fn scan_chr_strings(mem: &MemoryImage, verbose: bool) -> Vec<CharCapture> {
    println!("\n═══ Pass 1: Scanning for chr_XXXX IL2CPP strings ═══");

    let mut found_strings: Vec<(u64, String)> = Vec::new();

    for (region_va, region_data) in mem.iter_regions() {
        if region_data.len() < CHR_PATTERN.len() + 8 {
            continue;
        }
        // Scan for the UTF-16LE "chr_" pattern
        let mut i = 0;
        while i + CHR_PATTERN.len() + 8 <= region_data.len() {
            if region_data[i..i + CHR_PATTERN.len()] == CHR_PATTERN {
                // Read the rest of the chr_XXXX (4 digits)
                let str_va = region_va + i as u64;
                // Check if this looks like it's inside an Il2CppString
                // Il2CppString data starts at +20 from the object base
                // So the object base would be at str_va - 20
                let possible_str_obj = str_va.wrapping_sub(IL2CPP_STR_DATA_OFFSET as u64);

                if let Some(s) = mem.read_il2cpp_string(possible_str_obj) {
                    if s.starts_with("chr_") && s.len() >= 8 && s.len() <= 16 {
                        // Verify it looks like chr_NNNN
                        let suffix = &s[4..];
                        if suffix.chars().all(|c| c.is_ascii_digit()) {
                            found_strings.push((possible_str_obj, s));
                        }
                    }
                }
                i += 2; // UTF-16 aligned
            } else {
                i += 2; // scan in 2-byte steps for UTF-16
            }
        }
    }

    // Deduplicate
    found_strings.sort_by_key(|(va, _)| *va);
    found_strings.dedup_by_key(|(va, _)| *va);

    println!("  Found {} unique chr_XXXX Il2CppString objects", found_strings.len());

    if verbose {
        for (va, s) in &found_strings {
            println!("    {va:#018X}: {s}");
        }
    }

    // Now find all CharInfo objects that reference these strings
    println!("\n═══ Pass 2: Resolving CharInfo objects ═══");

    let mut captures = Vec::new();
    let mut processed_templates: HashMap<String, u64> = HashMap::new();

    for (str_va, template_id) in &found_strings {
        // Scan memory for pointers to this string object
        // CharInfo.templateId is at offset 0x18 (CI_TEMPLATE_ID)
        // So we need to find an 8-byte value == str_va at some location,
        // and that location - 0x18 is the CharInfo base.
        let str_va_bytes = str_va.to_le_bytes();

        for (region_va, region_data) in mem.iter_regions() {
            if region_data.len() < 8 {
                continue;
            }
            let mut j = 0;
            while j + 8 <= region_data.len() {
                if region_data[j..j + 8] == str_va_bytes {
                    let ptr_va = region_va + j as u64;
                    // This pointer is at some offset within a CharInfo object
                    // Try CI_TEMPLATE_ID (0x18) first
                    let charinfo_base = ptr_va.wrapping_sub(CI_TEMPLATE_ID as u64);

                    if let Some(cap) = try_resolve_charinfo(mem, charinfo_base, template_id, verbose) {
                        if !processed_templates.contains_key(template_id) {
                            processed_templates.insert(template_id.clone(), charinfo_base);
                            if verbose {
                                println!(
                                    "    CharInfo for {} at {:#018X}",
                                    template_id, charinfo_base
                                );
                            }
                            captures.push(cap);
                        }
                    }
                }
                j += 8; // pointer-aligned
            }
        }
    }

    println!(
        "  Resolved {} CharInfo objects with stats",
        captures.len()
    );

    captures
}

/// A captured character's data.
struct CharCapture {
    template_id: String,
    level: i32,
    break_stage: i32,
    potential_level: i32,
    raw_data: Vec<f64>,
    charinfo_va: u64,
    attrs_va: u64,
}

/// Try to resolve CharInfo → Attributes → m_rawData at a given base address.
fn try_resolve_charinfo(
    mem: &MemoryImage,
    base: u64,
    expected_template: &str,
    verbose: bool,
) -> Option<CharCapture> {
    // Validate: the object at `base` should have a valid klass pointer (first 8 bytes)
    let klass_ptr = mem.read_u64(base)?;
    if klass_ptr == 0 || klass_ptr < 0x10000 {
        return None;
    }
    // Klass should be in a reasonable memory range
    if !mem.is_mapped(klass_ptr) {
        return None;
    }

    // Verify template_id string at +0x18
    let str_ptr = mem.read_u64(base + CI_TEMPLATE_ID as u64)?;
    if str_ptr == 0 {
        return None;
    }
    let template_id = mem.read_il2cpp_string(str_ptr)?;
    if template_id != expected_template {
        return None;
    }

    // Read metadata
    let level = mem.read_i32(base + CI_LEVEL as u64).unwrap_or(0);
    let break_stage = mem.read_i32(base + CI_BREAK_STAGE as u64).unwrap_or(0);
    let potential = mem.read_i32(base + CI_POTENTIAL as u64).unwrap_or(0);

    // Navigate to Attributes
    let attrs_ptr = mem.read_u64(base + CI_ATTRS as u64)?;
    if attrs_ptr == 0 || !mem.is_mapped(attrs_ptr) {
        // Try fallback offset
        let attrs_ptr2 = mem.read_u64(base + CI_ATTRS_PACK as u64)?;
        if attrs_ptr2 == 0 || !mem.is_mapped(attrs_ptr2) {
            if verbose {
                println!(
                    "    {expected_template}: CharInfo at {base:#X} — no valid Attributes pointer"
                );
            }
            return None;
        }
        return try_read_attrs(mem, base, attrs_ptr2, &template_id, level, break_stage, potential, verbose);
    }

    try_read_attrs(mem, base, attrs_ptr, &template_id, level, break_stage, potential, verbose)
}

fn try_read_attrs(
    mem: &MemoryImage,
    charinfo_va: u64,
    attrs_ptr: u64,
    template_id: &str,
    level: i32,
    break_stage: i32,
    potential: i32,
    verbose: bool,
) -> Option<CharCapture> {
    // Read m_rawData pointer from Attributes
    let raw_data_ptr = mem.read_u64(attrs_ptr + AT_RAW_DATA as u64)?;
    if raw_data_ptr == 0 || !mem.is_mapped(raw_data_ptr) {
        if verbose {
            println!(
                "    {template_id}: Attributes at {attrs_ptr:#X} — no valid m_rawData pointer"
            );
        }
        return None;
    }

    // Read the double array
    let doubles = mem.read_double_array(raw_data_ptr)?;
    if doubles.len() < 5 {
        return None;
    }

    // Sanity check: first few values should be in reasonable ranges
    // hp (index 0): 0-50000, str (index 1): 0-10000
    let hp = doubles[0];
    let str_val = doubles[1];
    if !(0.0..=100_000.0).contains(&hp) || !(0.0..=50_000.0).contains(&str_val) {
        return None;
    }

    // Additional sanity: atk/def/maxHp multipliers (indices 5-7) should be ~1.0 or small
    if doubles.len() > 7 {
        let atk = doubles[5];
        let def = doubles[6];
        let max_hp = doubles[7];
        // These are multipliers typically 1.0, but can vary
        if atk.abs() > 100_000.0 || def.abs() > 100_000.0 || max_hp.abs() > 100_000.0 {
            return None;
        }
    }

    Some(CharCapture {
        template_id: template_id.to_string(),
        level,
        break_stage,
        potential_level: potential,
        raw_data: doubles,
        charinfo_va,
        attrs_va: attrs_ptr,
    })
}

/// Scan for JSON objects in memory (optional).
fn scan_json_objects(mem: &MemoryImage) -> Vec<(u64, String)> {
    println!("\n═══ Scanning for JSON objects ═══");

    let _pattern = b"{\"";
    let mut results = Vec::new();
    let mut count = 0;

    let keywords = [
        "templateId",
        "characterAttribute",
        "baseValue",
        "CharacterAttributeData",
        "characterId",
        "attributeType",
    ];

    for (region_va, region_data) in mem.iter_regions() {
        let mut i = 0;
        while i + 2 < region_data.len() {
            if region_data[i] == b'{' && region_data[i + 1] == b'"' {
                // Try to extract a JSON-like string
                let start = i;
                let mut depth = 0i32;
                let mut end = i;
                let max_len = (region_data.len() - i).min(8192);

                for k in i..i + max_len {
                    let b = region_data[k];
                    if b == b'{' {
                        depth += 1;
                    } else if b == b'}' {
                        depth -= 1;
                        if depth == 0 {
                            end = k + 1;
                            break;
                        }
                    } else if !b.is_ascii() && b != 0xC2 && b != 0xC3 {
                        // Non-ASCII, non-UTF8 continuation → not JSON
                        break;
                    }
                }

                if end > start && depth == 0 {
                    if let Ok(s) = std::str::from_utf8(&region_data[start..end]) {
                        // Check if it contains any of our keywords
                        if keywords.iter().any(|kw| s.contains(kw)) {
                            let va = region_va + start as u64;
                            results.push((va, s.to_string()));
                            count += 1;
                        }
                    }
                }
                i = if end > start { end } else { i + 1 };
            } else {
                i += 1;
            }
        }
    }

    println!("  Found {count} JSON objects with stat-related keywords");
    results
}

/// Scan for UnityFS headers (optional).
fn scan_unityfs_headers(mem: &MemoryImage) -> Vec<u64> {
    println!("\n═══ Scanning for UnityFS headers ═══");

    let pattern = b"UnityFS\0";
    let mut results = Vec::new();

    for (region_va, region_data) in mem.iter_regions() {
        let mut i = 0;
        while i + pattern.len() <= region_data.len() {
            if region_data[i..i + pattern.len()] == *pattern {
                results.push(region_va + i as u64);
                i += pattern.len();
            } else {
                i += 1;
            }
        }
    }

    println!("  Found {} UnityFS headers in memory", results.len());
    results
}

/// Scan for known class name strings (for context).
fn scan_class_names(mem: &MemoryImage) {
    println!("\n═══ Pass 3: Scanning for IL2CPP class name strings ═══");

    let class_names = [
        "CharBagSystem",
        "CharacterAttributeTable",
        "CharacterAttributeData",
        "AttributesCalculator",
        "AbilitySystem",
        "CharacterFactory",
        "RemoteFactory",
        "CharInfo",
        "Attributes",
        "DataManager",
    ];

    for name in &class_names {
        let pattern = name.as_bytes();
        let mut count = 0;

        for (_region_va, region_data) in mem.iter_regions() {
            let mut i = 0;
            while i + pattern.len() <= region_data.len() {
                if region_data[i..i + pattern.len()] == *pattern {
                    // Verify it's null-terminated or followed by non-alpha
                    let after = if i + pattern.len() < region_data.len() {
                        region_data[i + pattern.len()]
                    } else {
                        0
                    };
                    if after == 0 || !after.is_ascii_alphanumeric() {
                        count += 1;
                    }
                }
                i += 1;
            }
        }

        if count > 0 {
            println!("    {name}: {count} occurrences");
        }
    }
}

// ── Output ──────────────────────────────────────────────────────────────

fn write_jsonl_output(captures: &[CharCapture], output_dir: &PathBuf) -> std::io::Result<()> {
    let path = output_dir.join("il2cpp_char_events.jsonl");
    let file = File::create(&path)?;
    let mut writer = BufWriter::new(file);

    for cap in captures {
        // Build rawData as named map
        let mut named_attrs: serde_json::Map<String, Value> = serde_json::Map::new();
        for (i, &val) in cap.raw_data.iter().enumerate() {
            let name = if i < ATTR_NAMES.len() {
                ATTR_NAMES[i].to_string()
            } else {
                format!("unknown_{i}")
            };
            named_attrs.insert(name, json!(val));
        }

        let event = json!({
            "type": "FULL_CHARACTER_CAPTURE",
            "templateId": cap.template_id,
            "level": cap.level,
            "breakStage": cap.break_stage,
            "potentialLevel": cap.potential_level,
            "source": "memdump_analyze",
            "rawData": cap.raw_data,
            "namedAttrs": named_attrs,
            "charInfo": {
                "templateId": cap.template_id,
                "level": cap.level,
                "breakStage": cap.break_stage,
                "potentialLevel": cap.potential_level,
            },
            "attributes": {
                "rawData": cap.raw_data,
                "rawDataLength": cap.raw_data.len(),
            },
            "_meta": {
                "charinfo_va": format!("{:#018X}", cap.charinfo_va),
                "attrs_va": format!("{:#018X}", cap.attrs_va),
            }
        });

        serde_json::to_writer(&mut writer, &event)?;
        writer.write_all(b"\n")?;
    }

    writer.flush()?;
    println!("  Wrote {} events to {}", captures.len(), path.display());
    Ok(())
}

fn write_summary(captures: &[CharCapture], output_dir: &PathBuf) -> std::io::Result<()> {
    let path = output_dir.join("memdump_summary.json");
    let summary = json!({
        "total_characters": captures.len(),
        "characters": captures.iter().map(|c| {
            let mut stats: serde_json::Map<String, Value> = serde_json::Map::new();
            for (i, &val) in c.raw_data.iter().enumerate().take(8) {
                let name = if i < ATTR_NAMES.len() { ATTR_NAMES[i] } else { "unknown" };
                stats.insert(name.to_string(), json!(val));
            }
            json!({
                "templateId": c.template_id,
                "level": c.level,
                "breakStage": c.break_stage,
                "potentialLevel": c.potential_level,
                "rawDataLength": c.raw_data.len(),
                "coreStats": stats,
                "charinfo_va": format!("{:#018X}", c.charinfo_va),
            })
        }).collect::<Vec<_>>(),
    });

    let file = File::create(&path)?;
    serde_json::to_writer_pretty(file, &summary)?;
    println!("  Wrote summary to {}", path.display());
    Ok(())
}

fn print_stat_table(captures: &[CharCapture]) {
    use comfy_table::{Table, ContentArrangement};

    if captures.is_empty() {
        println!("\n  No character data captured.");
        return;
    }

    println!("\n═══ Character Stats ═══\n");

    let mut table = Table::new();
    table.set_content_arrangement(ContentArrangement::Dynamic);
    table.set_header(vec![
        "Character", "Lv", "E", "HP", "STR", "AGI", "WISD", "WILL",
        "ATK", "DEF", "MaxHP", "rawData#",
    ]);

    let mut sorted = captures.iter().collect::<Vec<_>>();
    sorted.sort_by(|a, b| a.template_id.cmp(&b.template_id));

    for cap in sorted {
        let rd = &cap.raw_data;
        let get = |i: usize| {
            rd.get(i)
                .map(|v| {
                    if v.fract() == 0.0 && v.abs() < 100_000.0 {
                        format!("{}", *v as i64)
                    } else {
                        format!("{:.2}", v)
                    }
                })
                .unwrap_or_else(|| "-".to_string())
        };

        table.add_row(vec![
            cap.template_id.clone(),
            cap.level.to_string(),
            cap.break_stage.to_string(),
            get(0),  // hp
            get(1),  // str
            get(2),  // agi
            get(3),  // wisd
            get(4),  // will
            get(5),  // atk
            get(6),  // def
            get(7),  // maxHp
            rd.len().to_string(),
        ]);
    }

    println!("{table}");
}

// ── Utility ─────────────────────────────────────────────────────────────

fn format_bytes(bytes: u64) -> String {
    if bytes >= 1 << 30 {
        format!("{:.2} GB", bytes as f64 / (1u64 << 30) as f64)
    } else if bytes >= 1 << 20 {
        format!("{:.1} MB", bytes as f64 / (1u64 << 20) as f64)
    } else if bytes >= 1 << 10 {
        format!("{:.1} KB", bytes as f64 / (1u64 << 10) as f64)
    } else {
        format!("{bytes} B")
    }
}

// ── Main ────────────────────────────────────────────────────────────────

fn main() {
    let cli = Cli::parse();

    println!("╔══════════════════════════════════════════════════════════════╗");
    println!("║  Windows Minidump IL2CPP Analyzer                          ║");
    println!("╚══════════════════════════════════════════════════════════════╝");
    println!();

    // Parse dump
    println!("Loading dump: {}", cli.dump_path.display());
    let mem = match parse_minidump(&cli.dump_path) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("ERROR: {e}");
            std::process::exit(1);
        }
    };

    // Create output directory
    fs::create_dir_all(&cli.output_dir).expect("Cannot create output directory");

    // Scan for class names (context)
    scan_class_names(&mem);

    // Main scan: chr_XXXX strings → CharInfo → Attributes → rawData
    let captures = scan_chr_strings(&mem, cli.verbose);

    // Print stat table
    print_stat_table(&captures);

    // Write JSONL output (compatible with char_data_probe.rs)
    if let Err(e) = write_jsonl_output(&captures, &cli.output_dir) {
        eprintln!("ERROR writing JSONL: {e}");
    }

    // Write summary
    if let Err(e) = write_summary(&captures, &cli.output_dir) {
        eprintln!("ERROR writing summary: {e}");
    }

    // Optional scans
    if cli.scan_json {
        let jsons = scan_json_objects(&mem);
        if !jsons.is_empty() {
            let json_path = cli.output_dir.join("json_objects.jsonl");
            if let Ok(f) = File::create(&json_path) {
                let mut w = BufWriter::new(f);
                for (va, content) in &jsons {
                    let _ = writeln!(w, "// VA: {va:#018X}");
                    let _ = writeln!(w, "{content}");
                }
                println!("  Wrote JSON objects to {}", json_path.display());
            }
        }
    }

    if cli.scan_unity {
        let headers = scan_unityfs_headers(&mem);
        if !headers.is_empty() {
            println!("  UnityFS header addresses:");
            for (i, va) in headers.iter().enumerate().take(20) {
                println!("    [{i}] {va:#018X}");
            }
            if headers.len() > 20 {
                println!("    ... and {} more", headers.len() - 20);
            }
        }
    }

    // Summary
    println!("\n════════════════════════════════════════════════════════════════");
    println!("  Characters found: {}", captures.len());
    println!("  Output: {}", cli.output_dir.display());
    if !captures.is_empty() {
        println!("  Next: cargo run --bin char-data-probe --release -- \\");
        println!(
            "          --capture {}/il2cpp_char_events.jsonl",
            cli.output_dir.display()
        );
    }
    println!("════════════════════════════════════════════════════════════════");
}
