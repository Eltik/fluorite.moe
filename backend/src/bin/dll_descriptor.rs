/// Phase 2d: IL2CPP Method Descriptor Decoding & Resolver Disassembly.
///
/// Follow-up to dll_dispatch.rs Phase 2c findings:
/// - No satellite DLL has "beyond" or "decrypt" exports
/// - Only 4 accesses to [0x18F60CD80] — 2 reads + 2 writes, all at the 2 known call sites
/// - Call Site 1 resolves 3 sequential cached function pointers via resolver 0x180E5DE90
/// - HGP.dll exports all trampoline via jmp into .tvm0 VM bytecode
/// - UnityPlayer.dll has "Failed to Decrypt data when creating archive" and "hgvfs:/"
///
/// Goal: Decode the IL2CPP method descriptors to find METHOD NAMES and DLL NAMES,
///       disassemble the resolver, and trace beyond_setDecryptionFunc to its source module.
use goblin::pe::PE;
use iced_x86::{Decoder, DecoderOptions, Formatter, IntelFormatter};
use std::fs;

// ══════════════════════════════════════════════════════════════
// Constants
// ══════════════════════════════════════════════════════════════

const GA_PATH: &str = "./output/GameAssembly.dll";
const GA_IMAGE_BASE: u64 = 0x180000000;

/// Three descriptor table RVAs (from lea instructions at call sites).
/// Format: (RVA, label, descriptor_payload_size, cached_ptr_rva)
const DESCRIPTOR_ENTRIES: [(u32, &str, u32, u32); 3] = [
    (0x0C538D60, "decrypt_func", 0x18, 0x0F60CD80),
    (0x0C538D10, "second_func", 0x0C, 0x0F60C930),
    (0x0C5388E8, "third_func", 0x0E, 0x0F60CB90),
];

/// Resolver function RVA
const RESOLVER_RVA: u32 = 0x00E5DE90;

/// String pointer loaded at call site 2 (first field of descriptor struct)
const CALL_SITE2_STRING_RVA: u32 = 0x0C16F6C8;

/// Init function called before resolution
const INIT_FUNC_RVA: u32 = 0x00E045C0;

/// Class/metadata pointers used around the call sites
const CONTEXT_POINTER_RVAS: [(u32, &str); 4] = [
    (0x0EEB3640, "init_class_1 (Call Site 1)"),
    (0x0EE5EB20, "init_class_2 (Call Site 1)"),
    (0x0EE5EB08, "class_ptr (Call Site 1)"),
    (0x0EC693D8, "post_decrypt_ptr (Call Site 1)"),
];

/// HGP.dll path
const HGP_PATH: &str = "./output/HGP.dll";

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

/// Extract a null-terminated UTF-8 string from data at the given offset.
fn read_cstring(data: &[u8], offset: usize) -> Option<String> {
    if offset >= data.len() {
        return None;
    }
    let end = data[offset..]
        .iter()
        .position(|&b| b == 0)
        .map(|p| offset + p)
        .unwrap_or(data.len().min(offset + 512));
    let s = String::from_utf8_lossy(&data[offset..end]).to_string();
    if s.chars().all(|c| c.is_ascii_graphic() || c == ' ') && !s.is_empty() {
        Some(s)
    } else {
        None
    }
}

/// Extract a null-terminated UTF-16LE string from data at the given offset.
fn read_utf16_string(data: &[u8], offset: usize) -> Option<String> {
    if offset + 2 > data.len() {
        return None;
    }
    let mut chars = Vec::new();
    let mut i = offset;
    while i + 1 < data.len() && chars.len() < 256 {
        let c = u16::from_le_bytes([data[i], data[i + 1]]);
        if c == 0 {
            break;
        }
        chars.push(c);
        i += 2;
    }
    if chars.is_empty() {
        return None;
    }
    let s = String::from_utf16_lossy(&chars);
    if s.chars().all(|c| c.is_ascii_graphic() || c == ' ') {
        Some(s)
    } else {
        None
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
    fn file_offset_to_rva(&self, offset: usize) -> Option<u32> {
        let off = offset as u32;
        for s in &self.sections {
            if off >= s.pointer_to_raw_data && off < s.pointer_to_raw_data + s.size_of_raw_data {
                return Some(off - s.pointer_to_raw_data + s.virtual_address);
            }
        }
        None
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
    let end = (file_offset + count * 15).min(data.len());
    let code = &data[file_offset..end];
    let containing_section = pe.sections.iter().find(|s| {
        let start = s.pointer_to_raw_data as usize;
        file_offset >= start && file_offset < start + s.size_of_raw_data as usize
    });
    let rva = (file_offset as u32).saturating_sub(
        containing_section.map(|s| s.pointer_to_raw_data).unwrap_or(0),
    ) + containing_section.map(|s| s.virtual_address).unwrap_or(0);
    let rip = pe.image_base + rva as u64;

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

/// Read bytes safely at a file offset.
fn read_at(data: &[u8], offset: usize, len: usize) -> Option<&[u8]> {
    if offset + len <= data.len() {
        Some(&data[offset..offset + len])
    } else {
        None
    }
}

/// Read a u64 LE at a file offset.
fn read_u64_at(data: &[u8], offset: usize) -> Option<u64> {
    read_at(data, offset, 8).map(|b| u64::from_le_bytes(b.try_into().unwrap()))
}

/// Read a u32 LE at a file offset.
fn read_u32_at(data: &[u8], offset: usize) -> Option<u32> {
    read_at(data, offset, 4).map(|b| u32::from_le_bytes(b.try_into().unwrap()))
}

/// Check if a value looks like a VA in the GameAssembly.dll image (0x180xxxxxxx).
fn is_ga_va(val: u64) -> bool {
    (GA_IMAGE_BASE..GA_IMAGE_BASE + 0x20000000).contains(&val)
}

// ══════════════════════════════════════════════════════════════
// Pass A: Raw Descriptor Data Dump
// ══════════════════════════════════════════════════════════════

fn pass_a_descriptor_dump(data: &[u8], pe: &PeInfo) {
    println!("\n╔══════════════════════════════════════════════════════════════╗");
    println!("║  Pass A: Raw Descriptor Data Dump                          ║");
    println!("╚══════════════════════════════════════════════════════════════╝\n");

    for &(rva, label, payload_size, cached_rva) in &DESCRIPTOR_ENTRIES {
        println!(
            "  ━━━ Descriptor: {} (RVA 0x{:08X}, size 0x{:02X}, cached at [0x{:08X}]) ━━━",
            label,
            rva,
            payload_size,
            GA_IMAGE_BASE as u32 + cached_rva
        );

        let Some(file_off) = pe.rva_to_file_offset(rva) else {
            println!("    ERROR: Cannot resolve RVA to file offset\n");
            continue;
        };

        // Dump the descriptor payload bytes
        println!(
            "\n    Payload ({} bytes) at file:0x{:08X}:",
            payload_size, file_off
        );
        if let Some(payload) = read_at(data, file_off, payload_size as usize) {
            hex_dump(payload, file_off, payload_size as usize);
        }

        // Dump extended context (256 bytes)
        println!("\n    Extended dump (256 bytes):");
        if let Some(block) = read_at(data, file_off, 256) {
            hex_dump(block, file_off, 256);
        }

        // Try to interpret 8-byte aligned fields as pointers
        println!("\n    Pointer analysis (8-byte fields):");
        let field_count = (payload_size as usize) / 8;
        for i in 0..field_count {
            let field_off = file_off + i * 8;
            if let Some(val) = read_u64_at(data, field_off) {
                let tag = if is_ga_va(val) {
                    let target_rva = (val - GA_IMAGE_BASE) as u32;
                    format!(
                        " -> RVA 0x{:08X}{}",
                        target_rva,
                        pe.rva_to_file_offset(target_rva)
                            .map(|fo| format!(" (file:0x{:08X})", fo))
                            .unwrap_or_default()
                    )
                } else {
                    String::new()
                };
                println!(
                    "      field[{}] @ +0x{:02X}: 0x{:016X}{}",
                    i,
                    i * 8,
                    val,
                    tag
                );
            }
        }

        // Also try 4-byte fields for small descriptors
        if payload_size < 0x18 {
            println!("\n    4-byte field analysis:");
            let field_count_32 = (payload_size as usize) / 4;
            for i in 0..field_count_32 {
                let field_off = file_off + i * 4;
                if let Some(val) = read_u32_at(data, field_off) {
                    println!("      field[{}] @ +0x{:02X}: 0x{:08X} ({})", i, i * 4, val, val);
                }
            }
        }

        println!();
    }

    // Also dump the string at CALL_SITE2_STRING_RVA
    println!("  ━━━ Call Site 2 String Data (RVA 0x{:08X}) ━━━", CALL_SITE2_STRING_RVA);
    if let Some(file_off) = pe.rva_to_file_offset(CALL_SITE2_STRING_RVA) {
        println!("    File offset: 0x{:08X}", file_off);
        if let Some(block) = read_at(data, file_off, 256) {
            hex_dump(block, file_off, 256);
        }

        // Try UTF-8
        if let Some(s) = read_cstring(data, file_off) {
            println!("    UTF-8: \"{}\"", s);
        }
        // Try UTF-16
        if let Some(s) = read_utf16_string(data, file_off) {
            println!("    UTF-16LE: \"{}\"", s);
        }
    } else {
        println!("    ERROR: Cannot resolve RVA");
    }
}

// ══════════════════════════════════════════════════════════════
// Pass B: Pointer Chain Following
// ══════════════════════════════════════════════════════════════

fn follow_pointer(data: &[u8], pe: &PeInfo, va: u64, depth: usize, prefix: &str) {
    if depth > 3 {
        return;
    }
    let rva = (va - GA_IMAGE_BASE) as u32;
    let Some(file_off) = pe.rva_to_file_offset(rva) else {
        println!(
            "{}VA 0x{:016X} (RVA 0x{:08X}) — cannot resolve",
            prefix, va, rva
        );
        return;
    };

    println!(
        "{}VA 0x{:016X} (RVA 0x{:08X}, file:0x{:08X}):",
        prefix, va, rva, file_off
    );

    // Try to read as string first
    if let Some(s) = read_cstring(data, file_off) {
        println!("{}  STRING: \"{}\"", prefix, s);
        return;
    }
    if let Some(s) = read_utf16_string(data, file_off) {
        println!("{}  UTF16: \"{}\"", prefix, s);
        return;
    }

    // Dump raw data
    if let Some(block) = read_at(data, file_off, 64) {
        hex_dump(block, file_off, 64);
    }

    // Follow embedded pointers (first 8 u64 fields)
    for i in 0..8 {
        if let Some(val) = read_u64_at(data, file_off + i * 8)
            && is_ga_va(val)
            && val != va
        {
            follow_pointer(data, pe, val, depth + 1, &format!("{}    ", prefix));
        }
    }
}

fn pass_b_pointer_chains(data: &[u8], pe: &PeInfo) {
    println!("\n╔══════════════════════════════════════════════════════════════╗");
    println!("║  Pass B: Pointer Chain Following                           ║");
    println!("╚══════════════════════════════════════════════════════════════╝\n");

    // Follow descriptor pointer chains
    for &(rva, label, payload_size, _) in &DESCRIPTOR_ENTRIES {
        println!("  ━━━ Following pointers in {} (RVA 0x{:08X}) ━━━\n", label, rva);

        let Some(file_off) = pe.rva_to_file_offset(rva) else {
            continue;
        };

        // Read each 8-byte field and follow pointers
        let field_count = (payload_size as usize) / 8;
        for i in 0..field_count {
            if let Some(val) = read_u64_at(data, file_off + i * 8)
                && is_ga_va(val)
            {
                println!("    field[{}] @ +0x{:02X}:", i, i * 8);
                follow_pointer(data, pe, val, 0, "      ");
                println!();
            }
        }

        // Also check 4-byte fields as potential RVAs or small values
        let field_count_32 = (payload_size as usize) / 4;
        for i in 0..field_count_32 {
            if let Some(val) = read_u32_at(data, file_off + i * 4) {
                // Check if it looks like an RVA (in valid section range)
                if val > 0x1000 && val < 0x20000000 && pe.rva_to_file_offset(val).is_some() {
                    // Only print if not already covered by 8-byte analysis
                    if i * 4 % 8 != 0 || !is_ga_va(read_u64_at(data, file_off + (i * 4 / 8) * 8).unwrap_or(0)) {
                        let target_off = pe.rva_to_file_offset(val).unwrap();
                        println!("    u32_field[{}] @ +0x{:02X}: RVA 0x{:08X} (file:0x{:08X})", i, i * 4, val, target_off);
                        if let Some(s) = read_cstring(data, target_off) {
                            println!("      STRING: \"{}\"", s);
                        } else if let Some(block) = read_at(data, target_off, 32) {
                            hex_dump(block, target_off, 32);
                        }
                        println!();
                    }
                }
            }
        }
    }

    // Follow context pointers from call sites
    println!("\n  ━━━ Context Pointers ━━━\n");
    for &(rva, label) in &CONTEXT_POINTER_RVAS {
        println!("  {} (RVA 0x{:08X}):", label, rva);
        let Some(file_off) = pe.rva_to_file_offset(rva) else {
            println!("    Cannot resolve\n");
            continue;
        };

        // Read the value stored at this location
        if let Some(stored_val) = read_u64_at(data, file_off) {
            println!(
                "    Stored value: 0x{:016X} (file:0x{:08X})",
                stored_val, file_off
            );
            if is_ga_va(stored_val) {
                follow_pointer(data, pe, stored_val, 0, "      ");
            } else if stored_val == 0 {
                println!("    (NULL — runtime-initialized)");
            }
        }

        // Also dump surrounding bytes for context
        if let Some(block) = read_at(data, file_off, 64) {
            println!("    Raw data at location:");
            hex_dump(block, file_off, 64);
        }
        println!();
    }
}

// ══════════════════════════════════════════════════════════════
// Pass C: Resolver Function Disassembly
// ══════════════════════════════════════════════════════════════

fn pass_c_resolver_disassembly(data: &[u8], pe: &PeInfo) {
    println!("\n╔══════════════════════════════════════════════════════════════╗");
    println!("║  Pass C: Resolver Function Disassembly                     ║");
    println!("╚══════════════════════════════════════════════════════════════╝\n");

    // Disassemble the resolver
    println!("  === Resolver at RVA 0x{:08X} ===", RESOLVER_RVA);
    if let Some(file_off) = pe.rva_to_file_offset(RESOLVER_RVA) {
        println!("  File offset: 0x{:08X}", file_off);
        disassemble_at(
            data,
            pe,
            file_off,
            100,
            &format!("Resolver (0x{:016X})", GA_IMAGE_BASE + RESOLVER_RVA as u64),
        );

        // Annotate: extract call targets and memory references
        println!("\n  Key instructions analysis:");
        let end = (file_off + 100 * 15).min(data.len());
        let code = &data[file_off..end];
        let rip = GA_IMAGE_BASE + RESOLVER_RVA as u64;
        let mut decoder = Decoder::with_ip(64, code, rip, DecoderOptions::NONE);
        let mut formatter = IntelFormatter::new();
        formatter.options_mut().set_first_operand_char_index(10);
        let mut output = String::new();
        let mut n = 0;

        while decoder.can_decode() && n < 100 {
            let instr = decoder.decode();
            n += 1;
            output.clear();
            formatter.format(&instr, &mut output);

            let rva = (instr.ip() - GA_IMAGE_BASE) as u32;

            // Highlight calls, memory accesses, and branches
            let interesting = output.contains("call")
                || output.contains("jmp")
                || output.contains("ret")
                || (instr.is_ip_rel_memory_operand());

            if interesting {
                let mut annotation = String::new();

                if instr.is_ip_rel_memory_operand() {
                    let mem_addr = instr.ip_rel_memory_address();
                    let mem_rva = (mem_addr - GA_IMAGE_BASE) as u32;
                    annotation = format!(" ; mem=0x{:016X} (RVA 0x{:08X})", mem_addr, mem_rva);

                    // Try to read string at the target
                    if let Some(mem_off) = pe.rva_to_file_offset(mem_rva) {
                        if let Some(s) = read_cstring(data, mem_off) {
                            annotation = format!("{} -> \"{}\"", annotation, s);
                        } else if let Some(ptr_val) = read_u64_at(data, mem_off)
                            && is_ga_va(ptr_val)
                        {
                            let ptr_rva = (ptr_val - GA_IMAGE_BASE) as u32;
                            annotation = format!("{} -> ptr 0x{:016X}", annotation, ptr_val);
                            if let Some(ptr_off) = pe.rva_to_file_offset(ptr_rva)
                                && let Some(s) = read_cstring(data, ptr_off)
                            {
                                annotation = format!("{} -> \"{}\"", annotation, s);
                            }
                        }
                    }
                }

                // For call instructions, note the target
                if output.contains("call")
                    && let Some(target) = output.split_whitespace().last()
                    && let Some(addr_str) = target.strip_suffix('h')
                    && let Ok(addr) = u64::from_str_radix(addr_str, 16)
                {
                    let target_rva = (addr - GA_IMAGE_BASE) as u32;
                    annotation =
                        format!("{} ; call target RVA 0x{:08X}", annotation, target_rva);
                }

                println!("    0x{:08X}  {}{}", rva, output, annotation);
            }
        }
    } else {
        println!("    ERROR: Cannot resolve resolver RVA");
    }

    // Disassemble the init function
    println!("\n\n  === Init Function at RVA 0x{:08X} ===", INIT_FUNC_RVA);
    if let Some(file_off) = pe.rva_to_file_offset(INIT_FUNC_RVA) {
        println!("  File offset: 0x{:08X}", file_off);
        disassemble_at(
            data,
            pe,
            file_off,
            30,
            &format!(
                "Init Function (0x{:016X})",
                GA_IMAGE_BASE + INIT_FUNC_RVA as u64
            ),
        );
    }
}

// ══════════════════════════════════════════════════════════════
// Pass D: "beyond_setDecryptionFunc" Cross-Reference Deep Dive
// ══════════════════════════════════════════════════════════════

fn pass_d_beyond_xref(data: &[u8], pe: &PeInfo) {
    println!("\n╔══════════════════════════════════════════════════════════════╗");
    println!("║  Pass D: \"beyond_setDecryptionFunc\" Cross-Reference        ║");
    println!("╚══════════════════════════════════════════════════════════════╝\n");

    let target = b"beyond_setDecryptionFunc";

    // Find all occurrences of the string in the DLL
    println!("  Searching for \"beyond_setDecryptionFunc\" (ASCII)...\n");
    let mut occurrences = Vec::new();
    if data.len() >= target.len() {
        for i in 0..=(data.len() - target.len()) {
            if &data[i..i + target.len()] == target {
                occurrences.push(i);
            }
        }
    }

    println!("  Found {} occurrences:\n", occurrences.len());

    for (idx, &file_off) in occurrences.iter().enumerate() {
        // Find which section
        let section_name = pe
            .sections
            .iter()
            .find(|s| {
                let start = s.pointer_to_raw_data as usize;
                file_off >= start && file_off < start + s.size_of_raw_data as usize
            })
            .map(|s| s.name.as_str())
            .unwrap_or("???");

        let rva = pe
            .sections
            .iter()
            .find(|s| {
                let start = s.pointer_to_raw_data as usize;
                file_off >= start && file_off < start + s.size_of_raw_data as usize
            })
            .map(|s| (file_off as u32 - s.pointer_to_raw_data) + s.virtual_address)
            .unwrap_or(0);

        println!(
            "  [{}/{}] file:0x{:08X} rva:0x{:08X} section:{}",
            idx + 1,
            occurrences.len(),
            file_off,
            rva,
            section_name
        );

        // Dump surrounding 256 bytes
        let ctx_start = file_off.saturating_sub(64);
        let ctx_end = (file_off + target.len() + 192).min(data.len());
        hex_dump(&data[ctx_start..ctx_end], ctx_start, ctx_end - ctx_start);

        // Extract neighboring strings
        println!("\n    Neighboring strings:");
        let neighbor_start = file_off.saturating_sub(256);
        let neighbor_end = (file_off + 256).min(data.len());
        let neighbor_data = &data[neighbor_start..neighbor_end];

        let mut str_start = 0;
        let mut current = String::new();
        for (i, &b) in neighbor_data.iter().enumerate() {
            if b.is_ascii_graphic() || b == b' ' {
                if current.is_empty() {
                    str_start = i;
                }
                current.push(b as char);
            } else {
                if current.len() >= 4 {
                    let abs_off = neighbor_start + str_start;
                    println!("      0x{:08X} \"{}\"", abs_off, &current[..current.len().min(120)]);
                }
                current.clear();
            }
        }
        if current.len() >= 4 {
            let abs_off = neighbor_start + str_start;
            println!("      0x{:08X} \"{}\"", abs_off, &current[..current.len().min(120)]);
        }

        // Find RIP-relative code references to this string
        println!("\n    RIP-relative code references:");
        let string_va = GA_IMAGE_BASE + rva as u64;

        let code_sections: Vec<&Section> = pe
            .sections
            .iter()
            .filter(|s| s.name == ".text" || s.name == "il2cpp")
            .collect();

        let mut xref_count = 0;
        for sec in &code_sections {
            let range = sec.file_range();
            let sec_start = range.start.min(data.len());
            let sec_end = range.end.min(data.len());
            let sec_data = &data[sec_start..sec_end];

            if sec_data.len() < 7 {
                continue;
            }

            // Search for RIP-relative references
            for total_len in [6u32, 7] {
                let prefix_len = total_len - 4;
                if sec_data.len() < prefix_len as usize + 4 {
                    continue;
                }
                for i in prefix_len as usize..=(sec_data.len() - 4) {
                    let instr_start = i - prefix_len as usize;
                    let instr_va =
                        GA_IMAGE_BASE + (sec.virtual_address + instr_start as u32) as u64;
                    let rip_at_end = instr_va + total_len as u64;
                    let expected_disp = string_va.wrapping_sub(rip_at_end) as u32;
                    let expected_bytes = expected_disp.to_le_bytes();

                    if sec_data[i..i + 4] == expected_bytes {
                        // Verify with decoder
                        let decode_start = instr_start.saturating_sub(4);
                        let decode_end = (instr_start + 16).min(sec_data.len());
                        let decode_buf = &sec_data[decode_start..decode_end];
                        let decode_rip =
                            GA_IMAGE_BASE + (sec.virtual_address + decode_start as u32) as u64;
                        let mut decoder =
                            Decoder::with_ip(64, decode_buf, decode_rip, DecoderOptions::NONE);

                        while decoder.can_decode() {
                            let instr = decoder.decode();
                            if instr.is_invalid() {
                                continue;
                            }
                            if instr.is_ip_rel_memory_operand()
                                && instr.ip_rel_memory_address() == string_va
                            {
                                xref_count += 1;
                                let xref_rva = (instr.ip() - GA_IMAGE_BASE) as u32;
                                let xref_file = sec_start + instr_start;
                                let mut fmt = IntelFormatter::new();
                                fmt.options_mut().set_first_operand_char_index(10);
                                let mut out = String::new();
                                fmt.format(&instr, &mut out);

                                println!(
                                    "      XREF #{} in {} at rva:0x{:08X} file:0x{:08X}: {}",
                                    xref_count, sec.name, xref_rva, xref_file, out
                                );

                                if xref_count <= 5 {
                                    disassemble_at(
                                        data,
                                        pe,
                                        xref_file.saturating_sub(32),
                                        20,
                                        &format!("beyond XREF #{} context", xref_count),
                                    );
                                }
                                break;
                            }
                            if instr.ip() > instr_va + 8 {
                                break;
                            }
                        }
                    }
                }
            }
        }

        if xref_count == 0 {
            println!("      No direct RIP-relative code references found");
            println!("      (String may be referenced via pointer table or IL2CPP metadata)");

            // Search for the string's VA as an 8-byte pointer in data sections
            let string_va_bytes = string_va.to_le_bytes();
            println!("\n    Searching for VA pointer (0x{:016X}) in data sections:", string_va);
            for sec in &pe.sections {
                let range = sec.file_range();
                let sec_start = range.start.min(data.len());
                let sec_end = range.end.min(data.len());
                if sec_end < sec_start + 8 {
                    continue;
                }
                let sec_data = &data[sec_start..sec_end];
                for j in 0..=(sec_data.len() - 8) {
                    if sec_data[j..j + 8] == string_va_bytes {
                        let ptr_file = sec_start + j;
                        let ptr_rva = pe
                            .sections
                            .iter()
                            .find(|s| {
                                let start = s.pointer_to_raw_data as usize;
                                ptr_file >= start
                                    && ptr_file < start + s.size_of_raw_data as usize
                            })
                            .map(|s| {
                                (ptr_file as u32 - s.pointer_to_raw_data) + s.virtual_address
                            })
                            .unwrap_or(0);
                        println!(
                            "      Found in {} at file:0x{:08X} rva:0x{:08X}",
                            sec.name, ptr_file, ptr_rva
                        );
                        // Dump context around the pointer
                        let ctx_s = ptr_file.saturating_sub(32);
                        let ctx_e = (ptr_file + 48).min(data.len());
                        hex_dump(&data[ctx_s..ctx_e], ctx_s, ctx_e - ctx_s);
                    }
                }
            }
        }
        println!();
    }

    // Also search for related PInvoke strings
    println!("  ━━━ Related PInvoke String Search ━━━\n");
    let search_terms: &[&[u8]] = &[
        b"__Internal",
        b"HGP.dll",
        b"HGP",
        b"HyperGryph",
        b"libhgp",
        b"beyond.dll",
        b"beyond_",
        b"hgvfs",
    ];

    for &term in search_terms {
        let term_str = String::from_utf8_lossy(term);
        let mut count = 0;
        if data.len() >= term.len() {
            for i in 0..=(data.len() - term.len()) {
                if &data[i..i + term.len()] == term {
                    count += 1;
                    if count <= 3 {
                        let sec_name = pe
                            .sections
                            .iter()
                            .find(|s| {
                                let start = s.pointer_to_raw_data as usize;
                                i >= start && i < start + s.size_of_raw_data as usize
                            })
                            .map(|s| s.name.as_str())
                            .unwrap_or("???");

                        // Read the full null-terminated string at this location
                        let full_str = read_cstring(data, i).unwrap_or_default();
                        println!(
                            "    \"{}\" at file:0x{:08X} section:{} => \"{}\"",
                            term_str, i, sec_name, full_str
                        );
                    }
                }
            }
        }
        if count > 3 {
            println!("    \"{}\" — {} total occurrences", term_str, count);
        } else if count == 0 {
            println!("    \"{}\" — not found", term_str);
        }
    }
}

// ══════════════════════════════════════════════════════════════
// Pass E: HGP.dll Export Target Analysis
// ══════════════════════════════════════════════════════════════

fn pass_e_hgp_exports(ga_data: &[u8], ga_pe: &PeInfo) {
    println!("\n╔══════════════════════════════════════════════════════════════╗");
    println!("║  Pass E: HGP.dll Export Target Analysis                    ║");
    println!("╚══════════════════════════════════════════════════════════════╝\n");

    let hgp_data = match fs::read(HGP_PATH) {
        Ok(d) => d,
        Err(e) => {
            println!("  ERROR loading HGP.dll: {}", e);
            return;
        }
    };

    let hgp_pe = parse_pe(&hgp_data);
    println!("  HGP.dll: {}", format_bytes(hgp_data.len() as u64));
    println!("  Image base: 0x{:016X}", hgp_pe.image_base);
    println!("  Sections:");
    for s in &hgp_pe.sections {
        println!(
            "    {:>10} VA:0x{:08X} raw:0x{:08X} vsize:{} rsize:{}",
            s.name,
            s.virtual_address,
            s.pointer_to_raw_data,
            format_bytes(s.virtual_size as u64),
            format_bytes(s.size_of_raw_data as u64)
        );
    }

    println!("\n  === Export Analysis ===\n");
    for exp in &hgp_pe.exports {
        println!("  Export: {} (RVA 0x{:08X})", exp.name, exp.rva);

        let Some(file_off) = hgp_pe.rva_to_file_offset(exp.rva) else {
            println!("    Cannot resolve to file offset\n");
            continue;
        };

        // Disassemble first 10 instructions to see the jmp target
        disassemble_at(
            &hgp_data,
            &hgp_pe,
            file_off,
            10,
            &format!("HGP::{}", exp.name),
        );

        // Check first bytes for jmp pattern
        if let Some(code) = read_at(&hgp_data, file_off, 16) {
            // Look for jmp patterns
            if code[0] == 0xE9 {
                // rel32 jmp
                let disp = i32::from_le_bytes(code[1..5].try_into().unwrap());
                let target =
                    (hgp_pe.image_base + exp.rva as u64 + 5).wrapping_add_signed(disp as i64);
                let target_rva = (target - hgp_pe.image_base) as u32;
                println!("    JMP target: 0x{:016X} (RVA 0x{:08X})", target, target_rva);

                // What section does it land in?
                if let Some(target_sec) = hgp_pe
                    .sections
                    .iter()
                    .find(|s| target_rva >= s.virtual_address && target_rva < s.virtual_address + s.virtual_size)
                {
                    println!("    Lands in section: {}", target_sec.name);

                    // Dump 64 bytes at the VM entry point
                    if let Some(vm_off) = hgp_pe.rva_to_file_offset(target_rva) {
                        println!("    VM entry bytecode (64 bytes):");
                        if let Some(block) = read_at(&hgp_data, vm_off, 64) {
                            hex_dump(block, vm_off, 64);
                        }
                    }
                }
            } else if code[0] == 0xFF && (code[1] & 0x38) == 0x20 {
                println!("    Indirect jmp (FF /4)");
            }
        }
        println!();
    }

    // Cross-reference: check if any GA descriptor pointers reference HGP's image base range
    println!("  === Cross-Reference: GA Descriptors → HGP Image Range ===\n");
    let hgp_base = hgp_pe.image_base;
    let hgp_end = hgp_base + hgp_pe.sections.iter().map(|s| (s.virtual_address + s.virtual_size) as u64).max().unwrap_or(0);
    println!(
        "  HGP range: 0x{:016X} .. 0x{:016X}",
        hgp_base, hgp_end
    );

    for &(rva, label, payload_size, _) in &DESCRIPTOR_ENTRIES {
        let Some(file_off) = ga_pe.rva_to_file_offset(rva) else {
            continue;
        };
        let field_count = (payload_size as usize) / 8;
        for i in 0..field_count {
            if let Some(val) = read_u64_at(ga_data, file_off + i * 8)
                && (hgp_base..hgp_end).contains(&val)
            {
                println!(
                    "  \x1b[1;33m{} field[{}] points into HGP.dll: 0x{:016X}\x1b[0m",
                    label, i, val
                );
            }
        }
    }
    println!("  (Check complete — any HGP references would be highlighted above)");
}

// ══════════════════════════════════════════════════════════════
// Pass F: PInvoke DLL Name Discovery
// ══════════════════════════════════════════════════════════════

fn pass_f_pinvoke_discovery(data: &[u8], pe: &PeInfo) {
    println!("\n╔══════════════════════════════════════════════════════════════╗");
    println!("║  Pass F: PInvoke DLL Name Discovery                        ║");
    println!("╚══════════════════════════════════════════════════════════════╝\n");

    // Search for __Internal near beyond_setDecryptionFunc
    // If __Internal, the function is compiled into GameAssembly.dll itself
    let beyond_bytes = b"beyond_setDecryptionFunc";
    let internal_bytes = b"__Internal";

    // Find the beyond string location first
    let mut beyond_offsets = Vec::new();
    if data.len() >= beyond_bytes.len() {
        for i in 0..=(data.len() - beyond_bytes.len()) {
            if &data[i..i + beyond_bytes.len()] == beyond_bytes {
                beyond_offsets.push(i);
            }
        }
    }

    println!("  === \"__Internal\" Proximity Search ===\n");
    for &boff in &beyond_offsets {
        // Search in ±4096 bytes around each beyond string
        let search_start = boff.saturating_sub(4096);
        let search_end = (boff + 4096).min(data.len());
        let region = &data[search_start..search_end];

        let mut found_internal = false;
        if region.len() >= internal_bytes.len() {
            for i in 0..=(region.len() - internal_bytes.len()) {
                if &region[i..i + internal_bytes.len()] == internal_bytes {
                    let abs_off = search_start + i;
                    let full_str = read_cstring(data, abs_off).unwrap_or_default();
                    let distance = abs_off.abs_diff(boff);
                    println!(
                        "    \"__Internal\" at file:0x{:08X} (distance {} bytes from beyond string)",
                        abs_off, distance
                    );
                    println!("    Full string: \"{}\"", full_str);
                    found_internal = true;
                }
            }
        }
        if !found_internal {
            println!(
                "    No \"__Internal\" found near beyond string at file:0x{:08X}",
                boff
            );
        }
    }

    // Search for DLL name patterns
    println!("\n  === DLL Name Pattern Search ===\n");
    let dll_patterns: &[&[u8]] = &[
        b"HGP.dll",
        b"HGP\x00",
        b"hgp.dll",
        b"hgp\x00",
        b"beyond.dll",
        b"libhgp",
        b"libhgp.so",
        b"libhgp.dylib",
        b"Unsafe.VFS.dll",
        b"beyond\x00",
    ];

    for &pattern in dll_patterns {
        let pattern_str = String::from_utf8_lossy(pattern)
            .replace('\0', "\\0");
        if data.len() < pattern.len() {
            continue;
        }
        let mut count = 0;
        for i in 0..=(data.len() - pattern.len()) {
            if &data[i..i + pattern.len()] == pattern {
                count += 1;
                if count <= 5 {
                    let sec_name = pe
                        .sections
                        .iter()
                        .find(|s| {
                            let start = s.pointer_to_raw_data as usize;
                            i >= start && i < start + s.size_of_raw_data as usize
                        })
                        .map(|s| s.name.as_str())
                        .unwrap_or("???");

                    let full = read_cstring(data, i).unwrap_or_default();
                    println!(
                        "    \"{}\" at file:0x{:08X} section:{} => full: \"{}\"",
                        pattern_str, i, sec_name, full
                    );

                    // Dump context
                    let ctx_s = i.saturating_sub(32);
                    let ctx_e = (i + 64).min(data.len());
                    hex_dump(&data[ctx_s..ctx_e], ctx_s, ctx_e - ctx_s);
                }
            }
        }
        if count > 5 {
            println!("    \"{}\" — {} total", pattern_str, count);
        } else if count == 0 {
            println!("    \"{}\" — not found", pattern_str);
        }
    }

    // IL2CPP PInvoke metadata structure search
    // In IL2CPP metadata, PInvoke entries are stored in Il2CppPInvokeInfo structures:
    //   uint32_t index;        // method index
    //   uint32_t moduleIndex;  // index into Il2CppModule table
    //   uint32_t nameIndex;    // index into string literal table
    // The module table has: (uint32_t nameIndex, uint32_t exportIndex)
    //
    // We can't parse the metadata blob (it's encrypted), but we can look for the
    // descriptor structures themselves. The 3 descriptors at their known RVAs
    // should contain the PInvoke dispatch info directly.

    println!("\n  === Descriptor Structure Interpretation ===\n");
    println!("  IL2CPP uses Il2CppMethodPointer* il2cpp_resolve_icall(const char* name)");
    println!("  The resolver at 0x{:08X} likely does:", RESOLVER_RVA);
    println!("    1. Read method descriptor → extract function name string");
    println!("    2. Look up in icall/PInvoke table");
    println!("    3. If PInvoke: LoadLibrary + GetProcAddress");
    println!("    4. Cache result at [cached_ptr]\n");

    // For each descriptor, try to interpret the first field as a string pointer
    for &(rva, label, payload_size, _) in &DESCRIPTOR_ENTRIES {
        let Some(file_off) = pe.rva_to_file_offset(rva) else {
            continue;
        };
        println!("  {} (0x{:08X}, {} bytes):", label, rva, payload_size);

        // Try first field as a VA to a string
        if let Some(first_val) = read_u64_at(data, file_off) {
            if is_ga_va(first_val) {
                let str_rva = (first_val - GA_IMAGE_BASE) as u32;
                if let Some(str_off) = pe.rva_to_file_offset(str_rva) {
                    if let Some(s) = read_cstring(data, str_off) {
                        println!("    field[0] → STRING: \"{}\"", s);
                    } else if let Some(ptr2) = read_u64_at(data, str_off)
                        && is_ga_va(ptr2)
                    {
                        let str2_rva = (ptr2 - GA_IMAGE_BASE) as u32;
                        if let Some(str2_off) = pe.rva_to_file_offset(str2_rva)
                            && let Some(s) = read_cstring(data, str2_off)
                        {
                            println!("    field[0] → ptr → STRING: \"{}\"", s);
                        }
                    }
                }
            } else {
                println!("    field[0] = 0x{:016X} (not a GA VA)", first_val);
            }
        }

        // Try second field
        if payload_size >= 16
            && let Some(second_val) = read_u64_at(data, file_off + 8)
        {
            if is_ga_va(second_val) {
                let str_rva = (second_val - GA_IMAGE_BASE) as u32;
                if let Some(str_off) = pe.rva_to_file_offset(str_rva)
                    && let Some(s) = read_cstring(data, str_off)
                {
                    println!("    field[1] → STRING: \"{}\"", s);
                }
            } else {
                println!("    field[1] = 0x{:016X}", second_val);
            }
        }

        // Try third field (for the 0x18 descriptor)
        if payload_size >= 24
            && let Some(third_val) = read_u64_at(data, file_off + 16)
        {
            if is_ga_va(third_val) {
                let str_rva = (third_val - GA_IMAGE_BASE) as u32;
                if let Some(str_off) = pe.rva_to_file_offset(str_rva)
                    && let Some(s) = read_cstring(data, str_off)
                {
                    println!("    field[2] → STRING: \"{}\"", s);
                }
            } else {
                println!("    field[2] = 0x{:016X}", third_val);
            }
        }

        // For smaller descriptors, also try reading the remaining bytes as raw values
        if payload_size <= 0x10 {
            let remaining_start = file_off + 8;
            let remaining = (payload_size as usize).saturating_sub(8);
            if remaining >= 4
                && let Some(val) = read_u32_at(data, remaining_start)
            {
                println!("    bytes[8..12] as u32: 0x{:08X} ({})", val, val);
            }
            if remaining >= 6 {
                // Read the remaining bytes (2 extra for 0x0E, 4 extra for 0x0C)
                if let Some(block) = read_at(data, remaining_start, remaining) {
                    print!("    bytes[8..{}] raw: ", 8 + remaining);
                    for b in block {
                        print!("{:02x} ", b);
                    }
                    println!();
                }
            }
        }
        println!();
    }
}

// ══════════════════════════════════════════════════════════════
// Main
// ══════════════════════════════════════════════════════════════

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("=== Phase 2d: IL2CPP Method Descriptor Decoding & Resolver Disassembly ===\n");
    println!("Goal: Decode descriptors to find METHOD NAMES and DLL NAMES.\n");

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

    // Pass A: Raw descriptor data dump
    pass_a_descriptor_dump(&ga_data, &ga_pe);

    // Pass B: Pointer chain following
    pass_b_pointer_chains(&ga_data, &ga_pe);

    // Pass C: Resolver function disassembly
    pass_c_resolver_disassembly(&ga_data, &ga_pe);

    // Pass D: "beyond_setDecryptionFunc" cross-reference deep dive
    pass_d_beyond_xref(&ga_data, &ga_pe);

    // Pass E: HGP.dll export target analysis
    pass_e_hgp_exports(&ga_data, &ga_pe);

    // Pass F: PInvoke DLL name discovery
    pass_f_pinvoke_discovery(&ga_data, &ga_pe);

    // ══════════════════════════════════════════════════════════════
    // Summary
    // ══════════════════════════════════════════════════════════════
    println!("\n╔══════════════════════════════════════════════════════════════╗");
    println!("║  Summary                                                    ║");
    println!("╚══════════════════════════════════════════════════════════════╝\n");

    println!("  Descriptors analyzed: {}", DESCRIPTOR_ENTRIES.len());
    for &(rva, label, size, cached) in &DESCRIPTOR_ENTRIES {
        println!(
            "    {} — RVA 0x{:08X}, {} bytes, cached at [0x{:X}]",
            label,
            rva,
            size,
            GA_IMAGE_BASE as u32 + cached
        );
    }

    println!("\n  Key questions answered:");
    println!("    1. What are the method/DLL names in the descriptors?");
    println!("    2. How does the resolver (0x{:08X}) map descriptors → function pointers?", RESOLVER_RVA);
    println!("    3. Which DLL/module hosts beyond_setDecryptionFunc?");
    println!("    4. Do HGP.dll VM entry points match any descriptor targets?");

    println!("\n=== Phase 2d Complete ===");
    Ok(())
}
