/// Phase P: Sagittarius VM Reverse Engineering.
///
/// 5-pass analysis of the .Sgxm0 VM dispatcher and .tvm0 bytecode
/// to find the custom cipher protecting 4 encrypted VFS directories.
///
/// Pass 1: .Sgxm0 dispatcher anatomy (dispatch loop, handler table, entry points)
/// Pass 2: VM instruction format (width, opcode field, operand encoding)
/// Pass 3: Opcode handler classification (arithmetic, memory, control, S-box refs)
/// Pass 4: Targeted .tvm0 disassembly from HGP.dll entry points
/// Pass 5: Cipher pattern extraction (round structure, key schedule, S-box usage)
use goblin::pe::PE;
use iced_x86::{
    Decoder, DecoderOptions, FlowControl, Formatter, Instruction, IntelFormatter, Mnemonic, OpKind,
    Register,
};
use std::collections::HashMap;
use std::fs;

// ══════════════════════════════════════════════════════════════
// Constants
// ══════════════════════════════════════════════════════════════

const GA_PATH: &str = "./output/GameAssembly.dll";
const HGP_PATH: &str = "./output/HGP.dll";
const GA_IMAGE_BASE: u64 = 0x180000000;

/// RVAs of the 6 S-boxes in GameAssembly.dll (.Sgxm1 section)
const SBOX_RVAS: [u32; 6] = [
    0x10280160, 0x10280260, 0x10280360, // Group 1: S0↔S2 inverse, S1 involution
    0x10280500, 0x10280600, 0x10280700, // Group 2: S3 involution, S4↔S5 inverse
];

/// File offsets of the S-boxes
const SBOX_FILE_OFFSETS: [usize; 6] = [
    0x0FF6C960, 0x0FF6CA60, 0x0FF6CB60, 0x0FF6CD00, 0x0FF6CE00, 0x0FF6CF00,
];

/// S-box VA range for detecting references in disassembly
const SBOX_VA_LO: u64 = GA_IMAGE_BASE + SBOX_RVAS[0] as u64;
const SBOX_VA_HI: u64 = GA_IMAGE_BASE + SBOX_RVAS[5] as u64 + 256;

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

    fn file_offset_to_rva(&self, offset: usize) -> Option<u32> {
        let off = offset as u32;
        for s in &self.sections {
            if off >= s.pointer_to_raw_data && off < s.pointer_to_raw_data + s.size_of_raw_data {
                return Some(off - s.pointer_to_raw_data + s.virtual_address);
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

/// Check if an address falls within the S-box VA range
fn is_sbox_va(addr: u64) -> Option<usize> {
    if !(SBOX_VA_LO..SBOX_VA_HI).contains(&addr) {
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

/// Format an instruction to string
fn format_instr(instr: &Instruction) -> String {
    let mut formatter = IntelFormatter::new();
    formatter.options_mut().set_first_operand_char_index(10);
    let mut output = String::new();
    formatter.format(instr, &mut output);
    output
}

// ══════════════════════════════════════════════════════════════
// Pass 1: .Sgxm0 Dispatcher Anatomy
// ══════════════════════════════════════════════════════════════

/// Categorized instruction in the dispatcher
#[derive(Debug, Clone)]
struct DispatcherInfo {
    /// Address of the most-targeted basic block (dispatch hub)
    hub_address: Option<u64>,
    /// Indirect jump instructions (handler dispatch)
    indirect_jumps: Vec<IndirectJump>,
    /// Handler table address and size if found
    handler_table: Option<(u64, usize)>,
    /// Cross-references into .Sgxm0 from other sections
    entry_points: Vec<(u64, String)>,
}

#[derive(Debug, Clone)]
struct IndirectJump {
    address: u64,
    file_offset: usize,
    description: String,
}

fn pass_1_dispatcher_anatomy(ga_data: &[u8], pe: &PeInfo) -> DispatcherInfo {
    println!("\n╔══════════════════════════════════════════════════════════════╗");
    println!("║  Pass 1: .Sgxm0 Dispatcher Anatomy                        ║");
    println!("╚══════════════════════════════════════════════════════════════╝\n");

    let sgxm0 = pe.find_section(".Sgxm0").expect(".Sgxm0 section not found");
    let range = sgxm0.file_range();
    let va_range = sgxm0.va_range(pe.image_base);
    println!(
        "  .Sgxm0: file 0x{:08X}..0x{:08X} ({}) VA 0x{:X}..0x{:X}",
        range.start,
        range.end,
        format_bytes((range.end - range.start) as u64),
        va_range.start,
        va_range.end,
    );

    let code = &ga_data[range.start..range.end.min(ga_data.len())];
    let base_rva = sgxm0.virtual_address;
    let base_rip = pe.image_base + base_rva as u64;

    // Step 1: Full disassembly of .Sgxm0 — build jump target histogram
    println!("\n  Step 1: Full disassembly + jump target histogram...");

    let mut jump_targets: HashMap<u64, u32> = HashMap::new();
    let mut indirect_jumps: Vec<IndirectJump> = Vec::new();
    let mut total_instrs = 0u64;
    let mut lea_rip_targets: Vec<(u64, u64)> = Vec::new(); // (instr_addr, target_addr)

    let mut decoder = Decoder::with_ip(64, code, base_rip, DecoderOptions::NONE);
    let mut instr = Instruction::default();

    while decoder.can_decode() {
        decoder.decode_out(&mut instr);
        if instr.is_invalid() {
            continue;
        }
        total_instrs += 1;

        // Track branch targets
        match instr.flow_control() {
            FlowControl::ConditionalBranch | FlowControl::UnconditionalBranch => {
                if instr.op0_kind() == OpKind::NearBranch64 {
                    let target = instr.near_branch64();
                    *jump_targets.entry(target).or_insert(0) += 1;
                }
            }
            FlowControl::IndirectBranch => {
                // This is a handler dispatch candidate
                let desc = format_instr(&instr);
                indirect_jumps.push(IndirectJump {
                    address: instr.ip(),
                    file_offset: range.start + (instr.ip() - base_rip) as usize,
                    description: desc,
                });
            }
            _ => {}
        }

        // Track LEA with RIP-relative addressing (table pointer loads)
        if instr.mnemonic() == Mnemonic::Lea {
            for op_idx in 0..instr.op_count() {
                if instr.op_kind(op_idx) == OpKind::Memory && instr.memory_base() == Register::RIP {
                    let target = instr
                        .ip()
                        .wrapping_add(instr.len() as u64)
                        .wrapping_add(instr.memory_displacement64());
                    lea_rip_targets.push((instr.ip(), target));
                }
            }
        }
    }

    println!("  Total instructions decoded: {}", total_instrs);
    println!("  Unique branch targets: {}", jump_targets.len());
    println!("  Indirect jumps found: {}", indirect_jumps.len());
    println!("  LEA RIP-relative loads: {}", lea_rip_targets.len());

    // Step 2: Find dispatch hub (most-targeted address)
    let mut targets_sorted: Vec<_> = jump_targets.iter().collect();
    targets_sorted.sort_by(|a, b| b.1.cmp(a.1));

    println!("\n  Step 2: Top 20 branch targets (dispatch hub candidates):");
    let hub_address = targets_sorted.first().map(|(addr, _)| **addr);
    for (i, (addr, count)) in targets_sorted.iter().take(20).enumerate() {
        let addr = **addr;
        let count = **count;
        let in_sgxm0 = addr >= va_range.start && addr < va_range.end;
        let marker = if i == 0 { " <<<< DISPATCH HUB" } else { "" };
        let section = if in_sgxm0 { ".Sgxm0" } else { "other" };
        println!(
            "    #{:2}: VA 0x{:016X} hits={:4} [{}]{}",
            i + 1,
            addr,
            count,
            section,
            marker
        );
    }

    // Step 3: Analyze indirect jumps — find handler table
    println!("\n  Step 3: Indirect jump analysis:");
    let mut handler_table: Option<(u64, usize)> = None;

    for ij in &indirect_jumps {
        println!("    VA 0x{:016X}: {}", ij.address, ij.description);

        // Disassemble 10 instructions before the indirect jump to find the table base
        let ij_offset = ij.file_offset;
        if ij_offset >= 150 {
            println!("      Context (10 instructions before):");
            let ctx_start = ij_offset.saturating_sub(150);
            let ctx_code = &ga_data[ctx_start..ij_offset + 15];
            let ctx_rva = pe.file_offset_to_rva(ctx_start).unwrap_or(0);
            let ctx_rip = pe.image_base + ctx_rva as u64;

            let mut ctx_decoder = Decoder::with_ip(64, ctx_code, ctx_rip, DecoderOptions::NONE);
            let mut ctx_instr = Instruction::default();
            let mut ctx_instrs = Vec::new();

            while ctx_decoder.can_decode() {
                ctx_decoder.decode_out(&mut ctx_instr);
                if !ctx_instr.is_invalid() {
                    ctx_instrs.push(ctx_instr);
                }
            }

            // Show last 15 instructions before the indirect jump
            let show_start = ctx_instrs.len().saturating_sub(15);
            for ci in &ctx_instrs[show_start..] {
                let desc = format_instr(ci);
                let marker = if ci.ip() == ij.address { " <<<<" } else { "" };
                println!("        {:016X}  {}{}", ci.ip(), desc, marker);

                // Look for LEA that loads a jump table base
                if ci.mnemonic() == Mnemonic::Lea
                    && ci.op1_kind() == OpKind::Memory
                    && ci.memory_base() == Register::RIP
                {
                    let target = ci
                        .ip()
                        .wrapping_add(ci.len() as u64)
                        .wrapping_add(ci.memory_displacement64());
                    println!(
                        "        ^^^ LEA target: 0x{:016X} — possible HANDLER_TABLE",
                        target
                    );
                    // Try to read the table at that address
                    let target_rva = (target - pe.image_base) as u32;
                    if let Some(table_off) = pe.rva_to_file_offset(target_rva) {
                        // Read potential 8-byte entries
                        let mut count = 0;
                        let mut off = table_off;
                        while off + 8 <= ga_data.len() && count < 512 {
                            let val = u64::from_le_bytes(ga_data[off..off + 8].try_into().unwrap());
                            // Handler entries should be VAs within .Sgxm0
                            if val >= va_range.start && val < va_range.end {
                                count += 1;
                            } else {
                                break;
                            }
                            off += 8;
                        }
                        if count > 5 {
                            println!(
                                "        JUMP_TABLE at 0x{:016X}: {} entries (8-byte VAs)",
                                target, count
                            );
                            handler_table = Some((target, count));
                        } else {
                            // Try 4-byte relative offsets
                            let mut count4 = 0;
                            off = table_off;
                            while off + 4 <= ga_data.len() && count4 < 512 {
                                let val =
                                    i32::from_le_bytes(ga_data[off..off + 4].try_into().unwrap());
                                let handler_va = (target as i64 + val as i64) as u64;
                                if handler_va >= va_range.start && handler_va < va_range.end {
                                    count4 += 1;
                                } else {
                                    break;
                                }
                                off += 4;
                            }
                            if count4 > 5 {
                                println!(
                                    "        JUMP_TABLE at 0x{:016X}: {} entries (4-byte rel offsets)",
                                    target, count4
                                );
                                handler_table = Some((target, count4));
                            }
                        }
                    }
                }
            }
        }
    }

    // Step 4: Cross-references INTO .Sgxm0 from other sections
    println!("\n  Step 4: Entry points into .Sgxm0 from other sections:");
    let mut entry_points: Vec<(u64, String)> = Vec::new();

    // Scan .text and il2cpp sections for calls/jumps targeting .Sgxm0
    let scan_sections = [".text", "il2cpp"];
    for sec_name in &scan_sections {
        let Some(sec) = pe.find_section(sec_name) else {
            continue;
        };
        let sec_range = sec.file_range();
        let sec_start = sec_range.start.min(ga_data.len());
        let sec_end = sec_range.end.min(ga_data.len());
        if sec_start >= sec_end {
            continue;
        }

        let sec_code = &ga_data[sec_start..sec_end];
        let sec_rip = pe.image_base + sec.virtual_address as u64;

        let mut dec = Decoder::with_ip(64, sec_code, sec_rip, DecoderOptions::NONE);
        let mut ins = Instruction::default();
        let mut found = 0u32;

        while dec.can_decode() {
            dec.decode_out(&mut ins);
            if ins.is_invalid() {
                continue;
            }

            match ins.flow_control() {
                FlowControl::Call | FlowControl::UnconditionalBranch => {
                    if ins.op0_kind() == OpKind::NearBranch64 {
                        let target = ins.near_branch64();
                        if target >= va_range.start && target < va_range.end {
                            found += 1;
                            if found <= 30 {
                                let desc = format_instr(&ins);
                                entry_points.push((ins.ip(), format!("{} → {}", desc, sec_name)));
                                println!(
                                    "    {:016X} [{}]: {} → .Sgxm0 target 0x{:016X}",
                                    ins.ip(),
                                    sec_name,
                                    desc,
                                    target
                                );
                            }
                        }
                    }
                }
                _ => {}
            }
        }

        if found > 30 {
            println!("    ... {} total cross-references from {}", found, sec_name);
        } else if found == 0 {
            println!("    No cross-references from {}", sec_name);
        }
    }

    // Step 5: Look for switch table patterns (movsxd + add-based dispatch)
    println!("\n  Step 5: Switch dispatch pattern scan in .Sgxm0:");
    let mut movsxd_count = 0u32;
    let mut switch_candidates: Vec<(u64, String)> = Vec::new();

    let mut decoder2 = Decoder::with_ip(64, code, base_rip, DecoderOptions::NONE);
    while decoder2.can_decode() {
        decoder2.decode_out(&mut instr);
        if instr.is_invalid() {
            continue;
        }

        // Pattern: movsxd reg, [reg + reg*4 + base] — reading switch table offsets
        if instr.mnemonic() == Mnemonic::Movsxd {
            movsxd_count += 1;
            if instr.op1_kind() == OpKind::Memory && instr.memory_index() != Register::None {
                let desc = format_instr(&instr);
                switch_candidates.push((instr.ip(), desc.clone()));
                if switch_candidates.len() <= 20 {
                    println!("    SWITCH_TABLE_READ at {:016X}: {}", instr.ip(), desc);
                }
            }
        }
    }
    println!(
        "  Total MOVSXD: {}, switch-table reads: {}",
        movsxd_count,
        switch_candidates.len()
    );

    let info = DispatcherInfo {
        hub_address,
        indirect_jumps,
        handler_table,
        entry_points,
    };

    // Summary
    println!("\n  ═══ Pass 1 Summary ═══");
    if let Some(hub) = info.hub_address {
        println!("  DISPATCH HUB: 0x{:016X}", hub);
    }
    if let Some((table_addr, count)) = &info.handler_table {
        println!("  HANDLER_TABLE: 0x{:016X}, {} entries", table_addr, count);
    }
    println!("  Entry points found: {}", info.entry_points.len());
    println!("  Indirect jumps: {}", info.indirect_jumps.len());

    info
}

// ══════════════════════════════════════════════════════════════
// Pass 2: VM Instruction Format
// ══════════════════════════════════════════════════════════════

#[derive(Debug, Clone)]
struct VmFormatInfo {
    /// Suspected instruction width (0 = variable/unknown)
    instruction_width: u32,
    /// PC advance pattern
    pc_advance_pattern: String,
    /// Detected register file base
    register_file_info: String,
}

fn pass_2_instruction_format(
    ga_data: &[u8],
    pe: &PeInfo,
    dispatcher: &DispatcherInfo,
) -> VmFormatInfo {
    println!("\n╔══════════════════════════════════════════════════════════════╗");
    println!("║  Pass 2: VM Instruction Format                             ║");
    println!("╚══════════════════════════════════════════════════════════════╝\n");

    let sgxm0 = pe.find_section(".Sgxm0").expect(".Sgxm0 not found");
    let range = sgxm0.file_range();
    let code = &ga_data[range.start..range.end.min(ga_data.len())];
    let base_rip = pe.image_base + sgxm0.virtual_address as u64;

    let mut format_info = VmFormatInfo {
        instruction_width: 0,
        pc_advance_pattern: "unknown".into(),
        register_file_info: "unknown".into(),
    };

    // If we found the dispatch hub, disassemble around it to trace the fetch loop
    if let Some(hub) = dispatcher.hub_address {
        println!("  Analyzing dispatch hub at 0x{:016X}...\n", hub);

        let hub_off_in_section = (hub - base_rip) as usize;
        if hub_off_in_section < code.len() {
            // Disassemble from 128 bytes before the hub through 256 bytes after
            let ctx_start = hub_off_in_section.saturating_sub(128);
            let ctx_end = (hub_off_in_section + 256).min(code.len());
            let ctx_code = &code[ctx_start..ctx_end];
            let ctx_rip = base_rip + ctx_start as u64;

            let mut decoder = Decoder::with_ip(64, ctx_code, ctx_rip, DecoderOptions::NONE);
            let mut instr = Instruction::default();
            let mut instrs_around_hub = Vec::new();

            while decoder.can_decode() {
                decoder.decode_out(&mut instr);
                if !instr.is_invalid() {
                    instrs_around_hub.push(instr);
                }
            }

            println!(
                "  Dispatch hub context ({} instructions):",
                instrs_around_hub.len()
            );

            // Track ADD/SUB patterns for PC advance and base+index for register file
            let mut add_patterns: HashMap<String, u32> = HashMap::new();
            let mut base_index_patterns: HashMap<String, u32> = HashMap::new();

            for ci in &instrs_around_hub {
                let desc = format_instr(ci);
                let marker = if ci.ip() == hub { " <<<< HUB" } else { "" };
                println!("    {:016X}  {}{}", ci.ip(), desc, marker);

                // Look for ADD reg, imm (PC advance)
                if ci.mnemonic() == Mnemonic::Add && ci.op1_kind() == OpKind::Immediate8to64 {
                    let imm = ci.immediate(1) as i64;
                    let reg = ci.op0_register();
                    let key = format!("add {}, {}", reg_name(reg), imm);
                    *add_patterns.entry(key).or_insert(0) += 1;
                }
                if ci.mnemonic() == Mnemonic::Sub && ci.op1_kind() == OpKind::Immediate8to64 {
                    let imm = ci.immediate(1) as i64;
                    let reg = ci.op0_register();
                    let key = format!("sub {}, {}", reg_name(reg), imm);
                    *add_patterns.entry(key).or_insert(0) += 1;
                }

                // Look for base+index memory patterns (register file access)
                if ci.op_count() > 0 {
                    for op_idx in 0..ci.op_count() {
                        if ci.op_kind(op_idx) == OpKind::Memory
                            && ci.memory_base() != Register::None
                            && ci.memory_index() != Register::None
                        {
                            let base = reg_name(ci.memory_base());
                            let idx = reg_name(ci.memory_index());
                            let scale = ci.memory_index_scale();
                            let key = format!("[{} + {}*{}]", base, idx, scale);
                            *base_index_patterns.entry(key).or_insert(0) += 1;
                        }
                    }
                }
            }

            if !add_patterns.is_empty() {
                println!("\n  PC advance candidates (ADD/SUB reg, imm near hub):");
                let mut sorted: Vec<_> = add_patterns.iter().collect();
                sorted.sort_by(|a, b| b.1.cmp(a.1));
                for (pattern, count) in sorted.iter().take(10) {
                    println!("    {} — {} occurrences", pattern, count);
                }
                if let Some((best, _)) = sorted.first() {
                    format_info.pc_advance_pattern = (*best).clone();
                    // Extract width from the pattern
                    if let Some(imm_str) = best.split(", ").nth(1)
                        && let Ok(w) = imm_str.parse::<u32>()
                    {
                        format_info.instruction_width = w;
                    }
                }
            }

            if !base_index_patterns.is_empty() {
                println!("\n  Register file access patterns (base+index near hub):");
                let mut sorted: Vec<_> = base_index_patterns.iter().collect();
                sorted.sort_by(|a, b| b.1.cmp(a.1));
                for (pattern, count) in sorted.iter().take(10) {
                    println!("    {} — {} occurrences", pattern, count);
                }
                if let Some((best, _)) = sorted.first() {
                    format_info.register_file_info = (*best).clone();
                }
            }
        }
    }

    // Also look for opcode fetch patterns (movzx for byte fetches)
    println!("\n  Opcode fetch scan (MOVZX patterns near dispatch):");
    let mut movzx_byte_count = 0u32;
    let mut movzx_word_count = 0u32;
    let mut movzx_examples: Vec<(u64, String)> = Vec::new();

    let mut decoder = Decoder::with_ip(64, code, base_rip, DecoderOptions::NONE);
    let mut instr = Instruction::default();

    while decoder.can_decode() {
        decoder.decode_out(&mut instr);
        if instr.is_invalid() {
            continue;
        }

        if instr.mnemonic() == Mnemonic::Movzx {
            let size = instr.memory_size().size();
            if size == 1 {
                movzx_byte_count += 1;
                if movzx_byte_count <= 5 {
                    let desc = format_instr(&instr);
                    movzx_examples.push((instr.ip(), desc));
                }
            } else if size == 2 {
                movzx_word_count += 1;
            }
        }
    }

    println!("  MOVZX byte fetches: {}", movzx_byte_count);
    println!("  MOVZX word fetches: {}", movzx_word_count);
    for (addr, desc) in &movzx_examples {
        println!("    Example: {:016X}  {}", addr, desc);
    }

    // Summary
    println!("\n  ═══ Pass 2 Summary ═══");
    println!(
        "  Instruction width: {}",
        if format_info.instruction_width > 0 {
            format!("{} bytes", format_info.instruction_width)
        } else {
            "variable/unknown".into()
        }
    );
    println!("  PC advance: {}", format_info.pc_advance_pattern);
    println!("  Register file: {}", format_info.register_file_info);

    format_info
}

fn reg_name(reg: Register) -> &'static str {
    match reg {
        Register::RAX => "rax",
        Register::RBX => "rbx",
        Register::RCX => "rcx",
        Register::RDX => "rdx",
        Register::RSI => "rsi",
        Register::RDI => "rdi",
        Register::RBP => "rbp",
        Register::RSP => "rsp",
        Register::R8 => "r8",
        Register::R9 => "r9",
        Register::R10 => "r10",
        Register::R11 => "r11",
        Register::R12 => "r12",
        Register::R13 => "r13",
        Register::R14 => "r14",
        Register::R15 => "r15",
        Register::EAX => "eax",
        Register::EBX => "ebx",
        Register::ECX => "ecx",
        Register::EDX => "edx",
        Register::ESI => "esi",
        Register::EDI => "edi",
        _ => "?",
    }
}

// ══════════════════════════════════════════════════════════════
// Pass 3: Opcode Handler Classification
// ══════════════════════════════════════════════════════════════

#[derive(Debug, Clone)]
struct HandlerInfo {
    va: u64,
    index: usize,
    category: String,
    sbox_refs: Vec<usize>, // S-box indices referenced
    instruction_count: usize,
    has_xor: bool,
    has_shift: bool,
    has_table_lookup: bool,
    has_loop: bool,
}

fn pass_3_handler_classification(
    ga_data: &[u8],
    pe: &PeInfo,
    dispatcher: &DispatcherInfo,
) -> Vec<HandlerInfo> {
    println!("\n╔══════════════════════════════════════════════════════════════╗");
    println!("║  Pass 3: Opcode Handler Classification                     ║");
    println!("╚══════════════════════════════════════════════════════════════╝\n");

    let sgxm0 = pe.find_section(".Sgxm0").expect(".Sgxm0 not found");
    let va_range = sgxm0.va_range(pe.image_base);

    let mut handlers: Vec<HandlerInfo> = Vec::new();

    if let Some((table_va, count)) = dispatcher.handler_table {
        println!(
            "  Classifying {} handler table entries at 0x{:016X}...",
            count, table_va
        );

        let table_rva = (table_va - pe.image_base) as u32;
        let Some(table_off) = pe.rva_to_file_offset(table_rva) else {
            println!("  ERROR: Cannot resolve handler table file offset");
            return handlers;
        };

        // Read handler addresses from table
        let mut handler_vas: Vec<u64> = Vec::new();
        for i in 0..count {
            let entry_off = table_off + i * 8;
            if entry_off + 8 > ga_data.len() {
                break;
            }
            let va = u64::from_le_bytes(ga_data[entry_off..entry_off + 8].try_into().unwrap());
            if va >= va_range.start && va < va_range.end {
                handler_vas.push(va);
            } else {
                // Try 4-byte relative offset
                let rel = i32::from_le_bytes(
                    ga_data[table_off + i * 4..table_off + i * 4 + 4]
                        .try_into()
                        .unwrap_or([0; 4]),
                );
                let abs_va = (table_va as i64 + rel as i64) as u64;
                if abs_va >= va_range.start && abs_va < va_range.end {
                    handler_vas.push(abs_va);
                }
            }
        }

        println!("  Resolved {} handler VAs\n", handler_vas.len());

        // Classify each handler by disassembling its first N instructions
        let max_handler_instrs = 50;
        let mut sbox_referencing: Vec<usize> = Vec::new();

        for (idx, &hva) in handler_vas.iter().enumerate() {
            let handler_rva = (hva - pe.image_base) as u32;
            let Some(handler_off) = pe.rva_to_file_offset(handler_rva) else {
                continue;
            };

            let end = (handler_off + max_handler_instrs * 15).min(ga_data.len());
            let handler_code = &ga_data[handler_off..end];
            let mut decoder = Decoder::with_ip(64, handler_code, hva, DecoderOptions::NONE);
            let mut instr = Instruction::default();

            let mut info = HandlerInfo {
                va: hva,
                index: idx,
                category: String::new(),
                sbox_refs: Vec::new(),
                instruction_count: 0,
                has_xor: false,
                has_shift: false,
                has_table_lookup: false,
                has_loop: false,
            };

            let mut has_add = false;
            let mut has_sub = false;
            let mut has_mul = false;
            let mut has_cmp = false;
            let mut has_call = false;
            let mut has_ret = false;
            let mut has_mem_load = false;
            let mut has_mem_store = false;
            let mut branches_back = false;

            let mut n = 0;
            while decoder.can_decode() && n < max_handler_instrs {
                decoder.decode_out(&mut instr);
                if instr.is_invalid() {
                    break;
                }
                n += 1;

                match instr.mnemonic() {
                    Mnemonic::Xor => info.has_xor = true,
                    Mnemonic::Shl
                    | Mnemonic::Shr
                    | Mnemonic::Sar
                    | Mnemonic::Rol
                    | Mnemonic::Ror => {
                        info.has_shift = true;
                    }
                    Mnemonic::Add => has_add = true,
                    Mnemonic::Sub => has_sub = true,
                    Mnemonic::Imul | Mnemonic::Mul => has_mul = true,
                    Mnemonic::Cmp | Mnemonic::Test => has_cmp = true,
                    Mnemonic::Call => has_call = true,
                    Mnemonic::Ret => has_ret = true,
                    Mnemonic::Mov | Mnemonic::Movzx | Mnemonic::Movsx | Mnemonic::Movsxd => {
                        // Check for memory operands
                        if instr.op0_kind() == OpKind::Memory {
                            has_mem_store = true;
                        }
                        if instr.op_count() > 1 && instr.op1_kind() == OpKind::Memory {
                            has_mem_load = true;
                        }
                    }
                    _ => {}
                }

                // Check for backward branches (loops)
                if instr.flow_control() == FlowControl::ConditionalBranch
                    && instr.op0_kind() == OpKind::NearBranch64
                    && instr.near_branch64() < instr.ip()
                {
                    branches_back = true;
                    info.has_loop = true;
                }

                // Check memory accesses for S-box references
                if instr.op_count() > 0 {
                    for op_idx in 0..instr.op_count() {
                        if instr.op_kind(op_idx) == OpKind::Memory {
                            let disp = instr.memory_displacement64();
                            if let Some(sbox_idx) = is_sbox_va(disp) {
                                if !info.sbox_refs.contains(&sbox_idx) {
                                    info.sbox_refs.push(sbox_idx);
                                }
                                info.has_table_lookup = true;
                            }
                        }
                        // Also check immediate values for encoded S-box addresses
                        if instr.op_kind(op_idx) == OpKind::Immediate64 {
                            let imm = instr.immediate(op_idx);
                            if let Some(sbox_idx) = is_sbox_va(imm) {
                                if !info.sbox_refs.contains(&sbox_idx) {
                                    info.sbox_refs.push(sbox_idx);
                                }
                                info.has_table_lookup = true;
                            }
                        }
                    }

                    // Check LEA targets for S-box references
                    if instr.mnemonic() == Mnemonic::Lea
                        && instr.op1_kind() == OpKind::Memory
                        && instr.memory_base() == Register::RIP
                    {
                        let target = instr
                            .ip()
                            .wrapping_add(instr.len() as u64)
                            .wrapping_add(instr.memory_displacement64());
                        if let Some(sbox_idx) = is_sbox_va(target) {
                            if !info.sbox_refs.contains(&sbox_idx) {
                                info.sbox_refs.push(sbox_idx);
                            }
                            info.has_table_lookup = true;
                        }
                    }
                }

                // Stop at return
                if has_ret && n > 5 {
                    break;
                }
            }

            info.instruction_count = n;

            // Classify
            if !info.sbox_refs.is_empty() {
                info.category = format!("CIPHER(S-box {:?})", info.sbox_refs);
                sbox_referencing.push(idx);
            } else if info.has_xor && info.has_shift && info.has_loop {
                info.category = "CRYPTO_CANDIDATE".into();
            } else if info.has_xor && info.has_shift {
                info.category = "bitwise".into();
            } else if has_mul || (has_add && has_sub && !has_mem_store) {
                info.category = "arithmetic".into();
            } else if has_mem_store && has_mem_load {
                info.category = "memory".into();
            } else if has_cmp && branches_back {
                info.category = "control_flow".into();
            } else if has_call {
                info.category = "call".into();
            } else if has_ret && n < 10 {
                info.category = "simple_return".into();
            } else {
                info.category = "other".into();
            }

            handlers.push(info);
        }

        // Summary
        let mut category_counts: HashMap<&str, u32> = HashMap::new();
        for h in &handlers {
            *category_counts.entry(&h.category).or_insert(0) += 1;
        }

        println!("  ═══ Handler Classification Summary ═══");
        let mut sorted_cats: Vec<_> = category_counts.iter().collect();
        sorted_cats.sort_by(|a, b| b.1.cmp(a.1));
        for (cat, count) in &sorted_cats {
            println!("    {:25} : {}", cat, count);
        }

        if !sbox_referencing.is_empty() {
            println!("\n  SBOX_REF handlers (cipher operations):");
            for &idx in &sbox_referencing {
                let h = &handlers[idx];
                println!(
                    "    Handler #{:3} at 0x{:016X}: {} — S-boxes {:?}, xor={}, shift={}, loop={}",
                    idx, h.va, h.category, h.sbox_refs, h.has_xor, h.has_shift, h.has_loop
                );

                // Dump the handler instructions for manual analysis
                let handler_rva = (h.va - pe.image_base) as u32;
                if let Some(handler_off) = pe.rva_to_file_offset(handler_rva) {
                    let end = (handler_off + 50 * 15).min(ga_data.len());
                    let hcode = &ga_data[handler_off..end];
                    let mut dec = Decoder::with_ip(64, hcode, h.va, DecoderOptions::NONE);
                    let mut ins = Instruction::default();
                    let mut n = 0;
                    while dec.can_decode() && n < h.instruction_count {
                        dec.decode_out(&mut ins);
                        if ins.is_invalid() {
                            break;
                        }
                        let desc = format_instr(&ins);
                        println!("      {:016X}  {}", ins.ip(), desc);
                        n += 1;
                    }
                    println!();
                }
            }
        } else {
            println!("\n  No handlers directly reference S-box RVAs.");
            println!("  Fallback: scanning for indirect S-box loading patterns...");
        }
    } else {
        println!("  No handler table found in Pass 1.");
        println!("  Fallback: S-box beacon scan in .Sgxm0...\n");

        // Scan all of .Sgxm0 for any reference to S-box VAs
        pass_3_sbox_beacon_scan(ga_data, pe);
    }

    handlers
}

/// Fallback: Scan .Sgxm0 for any instruction referencing S-box addresses
fn pass_3_sbox_beacon_scan(ga_data: &[u8], pe: &PeInfo) {
    let sgxm0 = pe.find_section(".Sgxm0").expect(".Sgxm0 not found");
    let range = sgxm0.file_range();
    let code = &ga_data[range.start..range.end.min(ga_data.len())];
    let base_rip = pe.image_base + sgxm0.virtual_address as u64;

    let mut decoder = Decoder::with_ip(64, code, base_rip, DecoderOptions::NONE);
    let mut instr = Instruction::default();
    let mut found = 0u32;

    println!("  Beacon scan: searching all .Sgxm0 instructions for S-box VAs...");

    while decoder.can_decode() {
        decoder.decode_out(&mut instr);
        if instr.is_invalid() {
            continue;
        }

        let mut hit = false;
        let mut sbox_idx = 0;

        // Check displacement in memory operands
        for op_idx in 0..instr.op_count() {
            if instr.op_kind(op_idx) == OpKind::Memory {
                let disp = instr.memory_displacement64();
                if let Some(idx) = is_sbox_va(disp) {
                    hit = true;
                    sbox_idx = idx;
                }
            }
            if instr.op_kind(op_idx) == OpKind::Immediate64
                && let Some(idx) = is_sbox_va(instr.immediate(op_idx))
            {
                hit = true;
                sbox_idx = idx;
            }
        }

        // Check LEA RIP-relative targets
        if instr.mnemonic() == Mnemonic::Lea
            && instr.op1_kind() == OpKind::Memory
            && instr.memory_base() == Register::RIP
        {
            let target = instr
                .ip()
                .wrapping_add(instr.len() as u64)
                .wrapping_add(instr.memory_displacement64());
            if let Some(idx) = is_sbox_va(target) {
                hit = true;
                sbox_idx = idx;
            }
        }

        if hit {
            found += 1;
            let desc = format_instr(&instr);
            println!(
                "    SBOX_REF S-box {} at {:016X}: {}",
                sbox_idx,
                instr.ip(),
                desc
            );
        }
    }

    println!("  Total S-box references in .Sgxm0: {}", found);

    // Also scan for S-box RVA bytes in raw .Sgxm0 data
    println!("\n  Raw byte scan for S-box RVA patterns...");
    for (i, &rva) in SBOX_RVAS.iter().enumerate() {
        let va = GA_IMAGE_BASE + rva as u64;
        let va_bytes = va.to_le_bytes();
        let rva_bytes = rva.to_le_bytes();

        let mut va_hits = 0u32;
        let mut rva_hits = 0u32;

        // Search for 8-byte VA
        for off in 0..code.len().saturating_sub(8) {
            if code[off..off + 8] == va_bytes {
                va_hits += 1;
                println!(
                    "    S-box {} VA found at .Sgxm0+0x{:X} (file 0x{:X})",
                    i,
                    off,
                    range.start + off
                );
            }
        }
        // Search for 4-byte RVA
        for off in 0..code.len().saturating_sub(4) {
            if code[off..off + 4] == rva_bytes {
                rva_hits += 1;
                if rva_hits <= 3 {
                    println!(
                        "    S-box {} RVA found at .Sgxm0+0x{:X} (file 0x{:X})",
                        i,
                        off,
                        range.start + off
                    );
                }
            }
        }
        if va_hits > 0 || rva_hits > 0 {
            println!(
                "    S-box {}: {} VA hits, {} RVA hits",
                i, va_hits, rva_hits
            );
        }
    }
}

// ══════════════════════════════════════════════════════════════
// Pass 4: Targeted .tvm0 Disassembly
// ══════════════════════════════════════════════════════════════

fn pass_4_tvm0_disassembly(ga_data: &[u8], pe: &PeInfo, vm_format: &VmFormatInfo) {
    println!("\n╔══════════════════════════════════════════════════════════════╗");
    println!("║  Pass 4: Targeted .tvm0 Disassembly (HGP.dll Entry Points) ║");
    println!("╚══════════════════════════════════════════════════════════════╝\n");

    // Find .tvm0 section in GameAssembly.dll
    let tvm0 = match pe.find_section(".tvm0") {
        Some(s) => s,
        None => {
            println!("  .tvm0 section not found in GameAssembly.dll");
            return;
        }
    };

    let tvm0_range = tvm0.file_range();
    let tvm0_va_range = tvm0.va_range(pe.image_base);
    println!(
        "  .tvm0: file 0x{:08X}..0x{:08X} ({}) VA 0x{:X}..0x{:X}",
        tvm0_range.start,
        tvm0_range.end,
        format_bytes((tvm0_range.end - tvm0_range.start) as u64),
        tvm0_va_range.start,
        tvm0_va_range.end,
    );

    // Parse HGP.dll to get entry point targets
    let hgp_data = match fs::read(HGP_PATH) {
        Ok(d) => d,
        Err(e) => {
            println!("  Cannot read {}: {}", HGP_PATH, e);
            println!("  Skipping HGP.dll entry point analysis.");
            // Still do raw .tvm0 analysis
            pass_4_tvm0_raw_analysis(ga_data, pe, tvm0);
            return;
        }
    };

    let hgp_pe = parse_pe(&hgp_data);
    println!(
        "\n  HGP.dll: {} exports, {} sections",
        hgp_pe.exports.len(),
        hgp_pe.sections.len()
    );

    // For each HGP export, disassemble to find the .tvm0 jump target
    let mut tvm0_targets: Vec<(String, u64)> = Vec::new();

    for exp in &hgp_pe.exports {
        let Some(file_off) = hgp_pe.rva_to_file_offset(exp.rva) else {
            continue;
        };
        if file_off + 15 > hgp_data.len() {
            continue;
        }

        // Disassemble first few instructions — expect jmp into .tvm0
        let code = &hgp_data[file_off..(file_off + 64).min(hgp_data.len())];
        let rip = hgp_pe.image_base + exp.rva as u64;
        let mut decoder = Decoder::with_ip(64, code, rip, DecoderOptions::NONE);
        let mut instr = Instruction::default();

        println!("\n  Export: {} (RVA 0x{:08X})", exp.name, exp.rva);

        let mut n = 0;
        while decoder.can_decode() && n < 10 {
            decoder.decode_out(&mut instr);
            if instr.is_invalid() {
                break;
            }
            let desc = format_instr(&instr);
            println!("    {:016X}  {}", instr.ip(), desc);
            n += 1;

            // Check if this jumps into .tvm0 range
            // The pattern is: LEA rsp,[rsp+XX]; ... then eventually a target in .tvm0
            // Or directly: jmp [rip+XX] pointing at .tvm0 VA

            // For HGP exports that jump into GA's .tvm0, we need to look for
            // the VM entry pattern described in Phase 2d memory:
            // lea rsp,[rsp+XX]; mov [rsp+10],rbp; mov rbp,rsp; pushfq; pop [rbp]; mov [rbp+78],r14
            if instr.mnemonic() == Mnemonic::Lea
                && instr.op0_register() == Register::RSP
                && instr.op1_kind() == OpKind::Memory
                && instr.memory_base() == Register::RSP
            {
                println!("      ^^^ VM ENTRY PATTERN: lea rsp,[rsp+XX]");
            }
        }

        // HGP.dll is small and jumps into GA's .tvm0 at known offsets
        // Look for immediate/displacement values that fall in .tvm0 VA range
        let mut decoder2 = Decoder::with_ip(64, code, rip, DecoderOptions::NONE);
        while decoder2.can_decode() {
            decoder2.decode_out(&mut instr);
            if instr.is_invalid() {
                break;
            }

            // Check branch targets
            if instr.op0_kind() == OpKind::NearBranch64 {
                let target = instr.near_branch64();
                if target >= tvm0_va_range.start && target < tvm0_va_range.end {
                    println!(
                        "      JUMP_TABLE: {} → .tvm0 offset 0x{:X}",
                        exp.name,
                        target - tvm0_va_range.start
                    );
                    tvm0_targets.push((exp.name.clone(), target));
                }
            }

            // Check memory displacements
            for op_idx in 0..instr.op_count() {
                if instr.op_kind(op_idx) == OpKind::Memory {
                    let disp = instr.memory_displacement64();
                    if disp >= tvm0_va_range.start && disp < tvm0_va_range.end {
                        println!(
                            "      MEM_TARGET: {} → .tvm0 offset 0x{:X}",
                            exp.name,
                            disp - tvm0_va_range.start
                        );
                        tvm0_targets.push((exp.name.clone(), disp));
                    }
                }
            }
        }
    }

    println!(
        "\n  Found {} .tvm0 entry points from HGP.dll",
        tvm0_targets.len()
    );

    // Analyze .tvm0 at each entry point
    if !tvm0_targets.is_empty() {
        for (name, target_va) in &tvm0_targets {
            let offset_in_tvm0 = (*target_va - tvm0_va_range.start) as usize;
            let file_off = tvm0_range.start + offset_in_tvm0;

            println!("\n  ═══ .tvm0 at 0x{:016X} (from {}) ═══", target_va, name);

            // If we know the VM instruction width, parse bytecode
            if vm_format.instruction_width > 0 {
                println!(
                    "  Parsing as {}-byte VM instructions:",
                    vm_format.instruction_width
                );
                let width = vm_format.instruction_width as usize;
                let tvm0_data = &ga_data[tvm0_range.start..tvm0_range.end.min(ga_data.len())];
                let max_instrs = 100;

                for i in 0..max_instrs {
                    let off = offset_in_tvm0 + i * width;
                    if off + width > tvm0_data.len() {
                        break;
                    }
                    let bytes = &tvm0_data[off..off + width];
                    print!("    {:08X}: ", off);
                    for b in bytes {
                        print!("{:02x} ", b);
                    }
                    // Interpret first byte as opcode
                    let opcode = bytes[0];
                    println!("  [opcode=0x{:02X}]", opcode);
                }
            } else {
                // Raw hex dump of the .tvm0 region
                println!("  Raw bytecode (first 256 bytes):");
                if file_off + 256 <= ga_data.len() {
                    hex_dump(&ga_data[file_off..file_off + 256], file_off, 256);
                }
            }
        }
    }

    // Also do raw .tvm0 analysis regardless
    pass_4_tvm0_raw_analysis(ga_data, pe, tvm0);
}

/// Raw analysis of .tvm0: look for S-box address encodings and structure
fn pass_4_tvm0_raw_analysis(ga_data: &[u8], _pe: &PeInfo, tvm0: &Section) {
    let range = tvm0.file_range();
    let data = &ga_data[range.start..range.end.min(ga_data.len())];

    println!("\n  ═══ .tvm0 Raw Analysis ═══");
    println!("  Section size: {}", format_bytes(data.len() as u64));

    // Scan for S-box VA/RVA byte patterns in .tvm0 bytecode
    println!("\n  S-box address encoding scan:");
    for (i, &rva) in SBOX_RVAS.iter().enumerate() {
        let va = GA_IMAGE_BASE + rva as u64;
        let va_bytes = va.to_le_bytes();
        let rva_bytes = rva.to_le_bytes();

        let mut va_hits = Vec::new();
        let mut rva_hits = Vec::new();

        for off in 0..data.len().saturating_sub(8) {
            if data[off..off + 8] == va_bytes {
                va_hits.push(off);
            }
        }
        for off in 0..data.len().saturating_sub(4) {
            if data[off..off + 4] == rva_bytes {
                rva_hits.push(off);
            }
        }

        if !va_hits.is_empty() || !rva_hits.is_empty() {
            println!(
                "    S-box {} (RVA 0x{:08X}): {} VA hits, {} RVA hits",
                i,
                rva,
                va_hits.len(),
                rva_hits.len()
            );
            for &off in va_hits.iter().take(5) {
                println!(
                    "      VA at .tvm0+0x{:08X} (file 0x{:08X})",
                    off,
                    range.start + off
                );
                hex_dump(
                    &data[off.saturating_sub(16)..(off + 32).min(data.len())],
                    range.start + off.saturating_sub(16),
                    48,
                );
            }
            for &off in rva_hits.iter().take(5) {
                println!(
                    "      RVA at .tvm0+0x{:08X} (file 0x{:08X})",
                    off,
                    range.start + off
                );
            }
        }
    }

    // Byte frequency analysis of .tvm0 (detect if some "opcodes" are more common)
    println!("\n  .tvm0 byte frequency analysis (top 20):");
    let mut freq = [0u32; 256];
    for &b in data {
        freq[b as usize] += 1;
    }
    let mut byte_freqs: Vec<(u8, u32)> = freq
        .iter()
        .enumerate()
        .map(|(b, &c)| (b as u8, c))
        .collect();
    byte_freqs.sort_by(|a, b| b.1.cmp(&a.1));

    for (b, count) in byte_freqs.iter().take(20) {
        let pct = *count as f64 / data.len() as f64 * 100.0;
        println!("    byte 0x{:02X}: {:6} ({:.2}%)", b, count, pct);
    }

    // Look for repeating patterns that might indicate instruction boundaries
    println!("\n  Stride analysis (looking for instruction alignment patterns):");
    for stride in [2, 4, 6, 8, 10, 12, 16] {
        // Check if first byte at each stride offset has low entropy (opcode field)
        let mut first_byte_freq = [0u32; 256];
        let count = data.len() / stride;
        for i in 0..count {
            first_byte_freq[data[i * stride] as usize] += 1;
        }
        let unique_first = first_byte_freq.iter().filter(|&&c| c > 0).count();
        let max_first = first_byte_freq.iter().max().copied().unwrap_or(0);
        let ent = {
            let len_f = count as f64;
            first_byte_freq
                .iter()
                .filter(|&&c| c > 0)
                .map(|&c| {
                    let p = c as f64 / len_f;
                    -p * p.log2()
                })
                .sum::<f64>()
        };
        println!(
            "    stride={:2}: unique_first_bytes={:3}, max_freq={:5}, entropy={:.2}",
            stride, unique_first, max_first, ent
        );
    }

    // Search for "WUKONG_EXPAND_DATA" strings in .tvm0
    println!("\n  String anchor scan in .tvm0:");
    let anchors: &[&[u8]] = &[
        b"WUKONG_EXPAND_DATA_BEGIN",
        b"WUKONG_EXPAND_DATA_END",
        b"global-metadata.dat",
    ];
    for anchor in anchors {
        for off in 0..data.len().saturating_sub(anchor.len()) {
            if data[off..off + anchor.len()] == **anchor {
                println!(
                    "    \"{}\" at .tvm0+0x{:08X}",
                    String::from_utf8_lossy(anchor),
                    off
                );
            }
        }
    }
}

// ══════════════════════════════════════════════════════════════
// Pass 5: Cipher Pattern Extraction
// ══════════════════════════════════════════════════════════════

fn pass_5_cipher_patterns(ga_data: &[u8], pe: &PeInfo, handlers: &[HandlerInfo]) {
    println!("\n╔══════════════════════════════════════════════════════════════╗");
    println!("║  Pass 5: Cipher Pattern Extraction                         ║");
    println!("╚══════════════════════════════════════════════════════════════╝\n");

    // Extract actual S-boxes for reference
    let mut sboxes = Vec::new();
    for &off in &SBOX_FILE_OFFSETS {
        if off + 256 <= ga_data.len() {
            let mut sbox = [0u8; 256];
            sbox.copy_from_slice(&ga_data[off..off + 256]);
            sboxes.push(sbox);
        }
    }
    println!("  Loaded {} S-boxes from GameAssembly.dll", sboxes.len());

    // Analyze cipher-flagged handlers in detail
    let cipher_handlers: Vec<_> = handlers
        .iter()
        .filter(|h| !h.sbox_refs.is_empty())
        .collect();

    if cipher_handlers.is_empty() {
        println!("  No S-box-referencing handlers found.");
        println!("  Attempting cipher structure recovery from .tvm0 patterns...\n");

        // Fallback: look for cipher-like patterns in .tvm0
        pass_5_tvm0_cipher_scan(ga_data, pe, &sboxes);
        return;
    }

    println!(
        "  {} handlers reference S-boxes directly.\n",
        cipher_handlers.len()
    );

    for h in &cipher_handlers {
        println!("  ═══ Handler #{} (S-boxes {:?}) ═══", h.index, h.sbox_refs);
        println!(
            "  Properties: xor={}, shift={}, loop={}, table_lookup={}, instrs={}",
            h.has_xor, h.has_shift, h.has_loop, h.has_table_lookup, h.instruction_count
        );

        // Identify cipher structure
        let uses_encrypt_pair = h.sbox_refs.contains(&0) || h.sbox_refs.contains(&4);
        let uses_decrypt_pair = h.sbox_refs.contains(&2) || h.sbox_refs.contains(&5);
        let uses_involution = h.sbox_refs.contains(&1) || h.sbox_refs.contains(&3);

        if uses_encrypt_pair {
            println!("    → Uses ENCRYPT S-box pair");
        }
        if uses_decrypt_pair {
            println!("    → Uses DECRYPT S-box pair");
        }
        if uses_involution {
            println!("    → Uses INVOLUTION S-box");
        }

        // Deep disassembly of the handler for round structure
        let handler_rva = (h.va - pe.image_base) as u32;
        if let Some(handler_off) = pe.rva_to_file_offset(handler_rva) {
            let end = (handler_off + 200 * 15).min(ga_data.len());
            let hcode = &ga_data[handler_off..end];
            let mut dec = Decoder::with_ip(64, hcode, h.va, DecoderOptions::NONE);
            let mut ins = Instruction::default();

            let mut loop_back_targets = Vec::new();
            let mut xor_count = 0;
            let mut shift_count = 0;
            let mut n = 0;

            while dec.can_decode() && n < 200 {
                dec.decode_out(&mut ins);
                if ins.is_invalid() {
                    break;
                }
                n += 1;

                if ins.mnemonic() == Mnemonic::Xor {
                    xor_count += 1;
                }
                if matches!(
                    ins.mnemonic(),
                    Mnemonic::Shl | Mnemonic::Shr | Mnemonic::Rol | Mnemonic::Ror
                ) {
                    shift_count += 1;
                }

                // Track loop back-edges
                if ins.flow_control() == FlowControl::ConditionalBranch
                    && ins.op0_kind() == OpKind::NearBranch64
                    && ins.near_branch64() < ins.ip()
                {
                    loop_back_targets.push((ins.ip(), ins.near_branch64()));
                }

                // Look for CMP reg, imm (round count)
                if ins.mnemonic() == Mnemonic::Cmp && ins.op1_kind() == OpKind::Immediate8to64 {
                    let imm = ins.immediate(1);
                    if (1..=32).contains(&imm) {
                        let desc = format_instr(&ins);
                        println!("    ROUND_COUNT candidate: {} (imm={})", desc, imm);
                    }
                }
            }

            println!(
                "    Loop back-edges: {}, XOR ops: {}, shift ops: {}",
                loop_back_targets.len(),
                xor_count,
                shift_count
            );

            for (from, to) in &loop_back_targets {
                let loop_size = from - to;
                println!(
                    "    Loop: 0x{:016X} → 0x{:016X} (body = {} bytes)",
                    from, to, loop_size
                );
            }
        }
    }
}

/// Fallback: scan .tvm0 for cipher-like patterns
fn pass_5_tvm0_cipher_scan(ga_data: &[u8], pe: &PeInfo, sboxes: &[[u8; 256]]) {
    let Some(tvm0) = pe.find_section(".tvm0") else {
        println!("  .tvm0 section not found");
        return;
    };

    let range = tvm0.file_range();
    let data = &ga_data[range.start..range.end.min(ga_data.len())];

    // Look for S-box content embedded in .tvm0 bytecode (not as addresses, but as data)
    println!("  Scanning .tvm0 for embedded S-box data (256-byte exact match)...");
    for (si, sbox) in sboxes.iter().enumerate() {
        for off in 0..data.len().saturating_sub(256) {
            if &data[off..off + 256] == sbox {
                println!(
                    "    S-box {} EMBEDDED at .tvm0+0x{:08X} (file 0x{:08X})",
                    si,
                    off,
                    range.start + off
                );
            }
        }
    }

    // Look for S-box partial matches (first 16 bytes)
    println!("\n  Scanning for partial S-box matches (first 16 bytes)...");
    for (si, sbox) in sboxes.iter().enumerate() {
        let prefix = &sbox[..16];
        for off in 0..data.len().saturating_sub(16) {
            if data[off..off + 16] == *prefix {
                println!(
                    "    S-box {} prefix at .tvm0+0x{:08X} (file 0x{:08X})",
                    si,
                    off,
                    range.start + off
                );
            }
        }
    }

    // Look for key material patterns: 16 or 32 byte sequences near S-box refs
    println!(
        "\n  Scanning for potential key material (16/32 byte low-entropy blocks near S-box addresses)..."
    );
    for &rva in &SBOX_RVAS {
        let rva_bytes = rva.to_le_bytes();
        for off in 0..data.len().saturating_sub(4) {
            if data[off..off + 4] == rva_bytes {
                // Check surrounding 128 bytes for key material
                let ctx_start = off.saturating_sub(64);
                let ctx_end = (off + 64).min(data.len());
                let ctx = &data[ctx_start..ctx_end];

                // Look for 16-byte blocks with entropy between 3.0 and 6.0 (key-like)
                for block_off in (0..ctx.len().saturating_sub(16)).step_by(4) {
                    let block = &ctx[block_off..block_off + 16];
                    let ent = {
                        let mut freq = [0u32; 256];
                        for &b in block {
                            freq[b as usize] += 1;
                        }
                        let len_f = 16.0;
                        freq.iter()
                            .filter(|&&c| c > 0)
                            .map(|&c| {
                                let p = c as f64 / len_f;
                                -p * p.log2()
                            })
                            .sum::<f64>()
                    };
                    if ent > 3.0 && ent < 6.0 {
                        let abs_off = ctx_start + block_off;
                        println!(
                            "    KEY_CANDIDATE at .tvm0+0x{:08X} (ent={:.2}): {:02x?}",
                            abs_off, ent, block
                        );
                    }
                }
            }
        }
    }

    // Summary of .Sgxm1 section analysis (where S-boxes live)
    if let Some(sgxm1) = pe.find_section(".Sgxm1") {
        let sgxm1_range = sgxm1.file_range();
        let sgxm1_data = &ga_data[sgxm1_range.start..sgxm1_range.end.min(ga_data.len())];
        println!(
            "\n  .Sgxm1 analysis ({}):",
            format_bytes(sgxm1_data.len() as u64)
        );

        // Find what's between S-box groups
        let gap_start = SBOX_FILE_OFFSETS[2] + 256; // After S-box 2
        let gap_end = SBOX_FILE_OFFSETS[3]; // Before S-box 3
        if gap_start < gap_end && gap_end <= ga_data.len() {
            let gap = &ga_data[gap_start..gap_end];
            println!(
                "    Gap between S-box groups: {} bytes at file 0x{:08X}",
                gap.len(),
                gap_start
            );
            hex_dump(gap, gap_start, gap.len().min(128));
        }
    }
}

// ══════════════════════════════════════════════════════════════
// Main
// ══════════════════════════════════════════════════════════════

fn main() {
    println!("╔══════════════════════════════════════════════════════════════╗");
    println!("║  Phase P: Sagittarius VM Reverse Engineering               ║");
    println!("║  5-Pass .Sgxm0 Dispatcher + .tvm0 Bytecode Analysis        ║");
    println!("╚══════════════════════════════════════════════════════════════╝\n");

    // Load GameAssembly.dll
    let ga_data = match fs::read(GA_PATH) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("ERROR: Cannot read {}: {}", GA_PATH, e);
            eprintln!("Place GameAssembly.dll in ./output/");
            std::process::exit(1);
        }
    };

    println!(
        "  GameAssembly.dll: {} loaded",
        format_bytes(ga_data.len() as u64)
    );

    let pe = parse_pe(&ga_data);
    println!("  Sections:");
    for s in &pe.sections {
        println!(
            "    {:10} VA 0x{:08X} size {} (file 0x{:08X}..0x{:08X})",
            s.name,
            s.virtual_address,
            format_bytes(s.virtual_size as u64),
            s.pointer_to_raw_data,
            s.pointer_to_raw_data + s.size_of_raw_data
        );
    }

    // Pass 1: Dispatcher anatomy
    let dispatcher = pass_1_dispatcher_anatomy(&ga_data, &pe);

    // Pass 2: Instruction format
    let vm_format = pass_2_instruction_format(&ga_data, &pe, &dispatcher);

    // Pass 3: Handler classification
    let handlers = pass_3_handler_classification(&ga_data, &pe, &dispatcher);

    // Pass 4: Targeted .tvm0 disassembly
    pass_4_tvm0_disassembly(&ga_data, &pe, &vm_format);

    // Pass 5: Cipher pattern extraction
    pass_5_cipher_patterns(&ga_data, &pe, &handlers);

    println!("\n╔══════════════════════════════════════════════════════════════╗");
    println!("║  Phase P Complete                                          ║");
    println!("╚══════════════════════════════════════════════════════════════╝");
}
