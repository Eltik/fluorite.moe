/// Phase R2: HGP.dll Standalone Probe.
///
/// Static analysis of HGP.dll to determine if standalone loading is feasible,
/// infer export signatures, and generate a Windows Python prober script.
///
/// 6 passes:
///   1. Import table analysis (go/no-go for standalone loading)
///   2. Section map + .tvm0 self-containment check
///   3. Export signature inference via disassembly
///   4. PInvoke descriptor search in GameAssembly.dll
///   5. String & constant survey
///   6. Auto-generate Python prober script
use goblin::pe::PE;
use iced_x86::{
    Decoder, DecoderOptions, Formatter, Instruction, IntelFormatter, Mnemonic, OpKind, Register,
};
use std::collections::{BTreeMap, HashSet};
use std::fs;

// ══════════════════════════════════════════════════════════════
// Constants
// ══════════════════════════════════════════════════════════════

const HGP_PATH: &str = "./output/HGP.dll";
const GA_PATH: &str = "./output/GameAssembly.dll";
const GA_IMAGE_BASE: u64 = 0x180000000;

/// Known S-box file offsets in GameAssembly.dll (.Sgxm1 section)
const SBOX_FILE_OFFSETS: [usize; 6] = [
    0x0FF6C960, 0x0FF6CA60, 0x0FF6CB60, 0x0FF6CD00, 0x0FF6CE00, 0x0FF6CF00,
];

/// Known encrypted directory hashes
const DIR_HASHES: &[(&str, u32)] = &[
    ("19E3AE45", 0x19E3AE45),
    ("1CDDBF1F", 0x1CDDBF1F),
    ("42A8FCA6", 0x42A8FCA6),
    ("775A31D1", 0x775A31D1),
];

/// VM entry pattern: lea rsp,[rsp+XX]; mov [rsp+10],rbp
const VM_ENTRY_PREFIX: &[u8] = &[0x48, 0x8D, 0x64, 0x24];

/// Import categories
const SYSTEM_DLLS: &[&str] = &[
    "kernel32.dll",
    "kernelbase.dll",
    "ntdll.dll",
    "advapi32.dll",
    "user32.dll",
    "gdi32.dll",
    "ole32.dll",
    "oleaut32.dll",
    "shell32.dll",
    "ws2_32.dll",
    "crypt32.dll",
    "bcrypt.dll",
    "secur32.dll",
    "comctl32.dll",
    "shlwapi.dll",
    "version.dll",
    "winmm.dll",
    "dbghelp.dll",
    "psapi.dll",
    "iphlpapi.dll",
];

const CRT_DLLS: &[&str] = &[
    "msvcrt.dll",
    "ucrtbase.dll",
    "vcruntime140.dll",
    "vcruntime140_1.dll",
    "msvcp140.dll",
    "msvcp140_1.dll",
    "concrt140.dll",
    "api-ms-win-crt-",
    "api-ms-win-core-",
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
    characteristics: u32,
}

impl Section {
    fn file_range(&self) -> std::ops::Range<usize> {
        let start = self.pointer_to_raw_data as usize;
        start..start + self.size_of_raw_data as usize
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

    fn permissions_str(&self) -> String {
        let r = if self.characteristics & 0x40000000 != 0 {
            "R"
        } else {
            "-"
        };
        let w = if self.characteristics & 0x80000000 != 0 {
            "W"
        } else {
            "-"
        };
        let x = if self.characteristics & 0x20000000 != 0 {
            "X"
        } else {
            "-"
        };
        format!("{}{}{}", r, w, x)
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
            if let Some(off) = s.rva_to_file_offset(rva) {
                return Some(off);
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

    fn find_section_for_rva(&self, rva: u32) -> Option<&Section> {
        self.sections.iter().find(|s| s.contains_rva(rva))
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
                characteristics: s.characteristics,
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

fn read_at(data: &[u8], offset: usize, len: usize) -> Option<&[u8]> {
    if offset + len <= data.len() {
        Some(&data[offset..offset + len])
    } else {
        None
    }
}

fn read_u32_at(data: &[u8], offset: usize) -> Option<u32> {
    read_at(data, offset, 4).map(|b| u32::from_le_bytes(b.try_into().unwrap()))
}

fn read_u64_at(data: &[u8], offset: usize) -> Option<u64> {
    read_at(data, offset, 8).map(|b| u64::from_le_bytes(b.try_into().unwrap()))
}

fn format_instr(instr: &Instruction) -> String {
    let mut formatter = IntelFormatter::new();
    formatter.options_mut().set_first_operand_char_index(10);
    let mut output = String::new();
    formatter.format(instr, &mut output);
    output
}

// ══════════════════════════════════════════════════════════════
// Import Classification
// ══════════════════════════════════════════════════════════════

#[derive(Debug, Clone, PartialEq)]
enum ImportCategory {
    System,
    Crt,
    GameSpecific,
}

fn classify_import(dll_name: &str) -> ImportCategory {
    let lower = dll_name.to_lowercase();
    for &sys in SYSTEM_DLLS {
        if lower == sys {
            return ImportCategory::System;
        }
    }
    for &crt in CRT_DLLS {
        if lower.starts_with(crt) || lower == crt {
            return ImportCategory::Crt;
        }
    }
    ImportCategory::GameSpecific
}

// ══════════════════════════════════════════════════════════════
// Export Signature Inference
// ══════════════════════════════════════════════════════════════

#[derive(Debug, Clone)]
struct InferredSignature {
    export_name: String,
    param_count: usize,
    param_hints: Vec<String>,
    return_hint: String,
    jmp_target_rva: Option<u32>,
    jmp_target_section: Option<String>,
    is_vm_entry: bool,
    stack_frame_size: Option<u32>,
}

fn infer_signature(data: &[u8], pe: &PeInfo, export: &ExportInfo) -> InferredSignature {
    let mut sig = InferredSignature {
        export_name: export.name.clone(),
        param_count: 0,
        param_hints: Vec::new(),
        return_hint: "unknown".into(),
        jmp_target_rva: None,
        jmp_target_section: None,
        is_vm_entry: false,
        stack_frame_size: None,
    };

    let Some(file_off) = pe.rva_to_file_offset(export.rva) else {
        return sig;
    };

    // Check for JMP trampoline first
    if let Some(code) = read_at(data, file_off, 16)
        && code[0] == 0xE9
    {
        let disp = i32::from_le_bytes(code[1..5].try_into().unwrap());
        let target_va = (pe.image_base + export.rva as u64 + 5).wrapping_add_signed(disp as i64);
        let target_rva = (target_va - pe.image_base) as u32;
        sig.jmp_target_rva = Some(target_rva);

        if let Some(sec) = pe.find_section_for_rva(target_rva) {
            sig.jmp_target_section = Some(sec.name.clone());

            // Check if target is VM bytecode
            if let Some(target_off) = pe.rva_to_file_offset(target_rva)
                && let Some(vm_bytes) = read_at(data, target_off, 4)
                && vm_bytes == VM_ENTRY_PREFIX
            {
                sig.is_vm_entry = true;
            }
        }
    }

    // Disassemble first 50 instructions to analyze register usage
    let decode_size = 50 * 15; // max instruction size
    let end = (file_off + decode_size).min(data.len());
    let code = &data[file_off..end];
    let rip = pe.image_base + export.rva as u64;
    let mut decoder = Decoder::with_ip(64, code, rip, DecoderOptions::NONE);
    let mut instr = Instruction::default();

    // Windows x64 ABI: rcx, rdx, r8, r9 for first 4 params
    let param_regs = [Register::RCX, Register::RDX, Register::R8, Register::R9];
    let param_regs_32 = [Register::ECX, Register::EDX, Register::R8D, Register::R9D];
    let mut regs_read_before_written: HashSet<Register> = HashSet::new();
    let mut regs_written: HashSet<Register> = HashSet::new();
    let mut param_type_hints: BTreeMap<Register, Vec<String>> = BTreeMap::new();

    let mut count = 0;
    while decoder.can_decode() && count < 50 {
        decoder.decode_out(&mut instr);
        if instr.is_invalid() {
            continue;
        }
        count += 1;

        // Stop at first ret or unconditional jump (trampoline already handled)
        if instr.mnemonic() == Mnemonic::Ret {
            break;
        }
        // For JMP trampolines, we've already handled them — keep analyzing past
        // the jmp target if we're looking at the target code

        // Track register reads/writes for parameter inference
        for (&reg64, &reg32) in param_regs.iter().zip(param_regs_32.iter()) {
            // Check if this instruction reads the register
            let reads_reg = (0..instr.op_count()).any(|idx| {
                let kind = instr.op_kind(idx);
                match kind {
                    OpKind::Register => {
                        let r = instr.op_register(idx);
                        r == reg64 || r == reg32
                    }
                    OpKind::Memory => instr.memory_base() == reg64 || instr.memory_index() == reg64,
                    _ => false,
                }
            });

            // Check if instruction WRITES the register (destination operand)
            let writes_reg = instr.op_count() > 0 && {
                let kind = instr.op_kind(0);
                kind == OpKind::Register && {
                    let r = instr.op_register(0);
                    r == reg64 || r == reg32
                }
            };

            if reads_reg && !regs_written.contains(&reg64) {
                regs_read_before_written.insert(reg64);

                // Collect type hints
                let hints = param_type_hints.entry(reg64).or_default();
                match instr.mnemonic() {
                    Mnemonic::Test => {
                        // test reg, reg → null-check → pointer
                        if instr.op_count() >= 2
                            && instr.op_kind(0) == OpKind::Register
                            && instr.op_kind(1) == OpKind::Register
                        {
                            hints.push("null-check → pointer".into());
                        }
                    }
                    Mnemonic::Cmp => {
                        if instr.op_count() >= 2 && instr.op_kind(1) == OpKind::Immediate32 {
                            let imm = instr.immediate(1) as u32;
                            hints.push(format!("cmp vs 0x{:X} → range check", imm));
                        }
                    }
                    Mnemonic::Mov => {
                        // mov [reg], ... or mov reg, [reg] → dereference → pointer
                        if instr.op_kind(0) == OpKind::Memory
                            && (instr.memory_base() == reg64 || instr.memory_index() == reg64)
                        {
                            hints.push("store via pointer".into());
                        }
                        if instr.op_count() >= 2
                            && instr.op_kind(1) == OpKind::Memory
                            && (instr.memory_base() == reg64 || instr.memory_index() == reg64)
                        {
                            hints.push("dereference → pointer".into());
                        }
                    }
                    Mnemonic::Movzx | Mnemonic::Movsx => {
                        hints.push("width extension → integer".into());
                    }
                    _ => {}
                }
            }

            if writes_reg {
                regs_written.insert(reg64);
            }
        }

        // Track stack frame setup (sub rsp, imm)
        if instr.mnemonic() == Mnemonic::Sub
            && instr.op_count() >= 2
            && instr.op_kind(0) == OpKind::Register
            && instr.op_register(0) == Register::RSP
            && instr.op_kind(1) == OpKind::Immediate32
        {
            sig.stack_frame_size = Some(instr.immediate(1) as u32);
        }
    }

    // Count params based on ABI register reads
    for (i, &reg) in param_regs.iter().enumerate() {
        if regs_read_before_written.contains(&reg) {
            sig.param_count = i + 1;
            if let Some(hints) = param_type_hints.get(&reg) {
                sig.param_hints
                    .push(format!("arg{}: {}", i, hints.join(", ")));
            } else {
                sig.param_hints.push(format!("arg{}: unknown type", i));
            }
        } else if i < sig.param_count {
            // Gap in register usage — unusual but record it
            sig.param_hints.push(format!("arg{}: unused?", i));
        }
    }

    // Return hint: check if EAX/RAX is written before ret
    if regs_written.contains(&Register::RAX) || regs_written.contains(&Register::EAX) {
        sig.return_hint = "non-void (sets RAX)".into();
    }

    sig
}

// ══════════════════════════════════════════════════════════════
// Pass 1: Import Table Analysis
// ══════════════════════════════════════════════════════════════

struct ImportVerdict {
    can_load_standalone: bool,
    system_imports: Vec<(String, usize)>, // (dll_name, import_count)
    crt_imports: Vec<(String, usize)>,
    game_imports: Vec<(String, usize)>,
    has_tls_callbacks: bool,
}

fn pass1_import_analysis(data: &[u8]) -> ImportVerdict {
    println!("\n╔══════════════════════════════════════════════════════════════╗");
    println!("║  Pass 1: Import Table Analysis (CRITICAL GATE)             ║");
    println!("╚══════════════════════════════════════════════════════════════╝\n");

    let pe = PE::parse(data).expect("Failed to parse PE");

    let mut system_imports: BTreeMap<String, usize> = BTreeMap::new();
    let mut crt_imports: BTreeMap<String, usize> = BTreeMap::new();
    let mut game_imports: BTreeMap<String, usize> = BTreeMap::new();

    // Regular imports
    for import in &pe.imports {
        let dll = import.dll.to_lowercase();
        let cat = classify_import(&dll);
        match cat {
            ImportCategory::System => *system_imports.entry(dll).or_default() += 1,
            ImportCategory::Crt => *crt_imports.entry(dll).or_default() += 1,
            ImportCategory::GameSpecific => *game_imports.entry(dll).or_default() += 1,
        }
    }

    // Delay-load imports
    println!(
        "  Regular imports: {} functions from {} DLLs",
        pe.imports.len(),
        {
            let mut dlls: HashSet<&str> = HashSet::new();
            for imp in &pe.imports {
                dlls.insert(imp.dll);
            }
            dlls.len()
        }
    );

    // Print all imports by category
    println!("\n  === System DLLs ===");
    for (dll, count) in &system_imports {
        println!("    {} ({} functions)", dll, count);
    }

    println!("\n  === CRT DLLs ===");
    for (dll, count) in &crt_imports {
        println!("    {} ({} functions)", dll, count);
    }

    if !game_imports.is_empty() {
        println!("\n  \x1b[1;31m=== GAME-SPECIFIC DLLs (BLOCKING) ===\x1b[0m");
        for (dll, count) in &game_imports {
            println!("    \x1b[1;31m{} ({} functions)\x1b[0m", dll, count);
        }
    } else {
        println!("\n  === Game-Specific DLLs ===");
        println!("    (none)");
    }

    // Check for TLS callbacks
    // Check for TLS directory
    let has_tls = pe
        .header
        .optional_header
        .is_some_and(|oh| oh.data_directories.get_tls_table().is_some());
    println!(
        "\n  TLS directory: {}",
        if has_tls {
            "PRESENT (may have anti-tamper callbacks)"
        } else {
            "not present"
        }
    );

    // Print detail on interesting imports
    println!("\n  === Notable Import Functions ===");
    for import in &pe.imports {
        let name = &import.name;
        let lower = name.to_lowercase();
        if lower.contains("crypt")
            || lower.contains("virtual")
            || lower.contains("loadlib")
            || lower.contains("getproc")
            || lower.contains("createfile")
            || lower.contains("thread")
            || lower.contains("mutex")
            || lower.contains("heap")
            || lower.contains("exception")
        {
            println!("    {}!{}", import.dll, name);
        }
    }

    // Verdict
    let can_load = game_imports.is_empty();
    println!("\n  ╔════════════════════════════════════════════╗");
    if can_load {
        println!("  ║  \x1b[1;32mVERDICT: GO — Standalone loading feasible\x1b[0m  ║");
        println!("  ║  Only system/CRT imports detected.          ║");
    } else {
        println!("  ║  \x1b[1;31mVERDICT: NO-GO — Game imports required\x1b[0m     ║");
        println!("  ║  Need stub DLL approach or game directory.   ║");
        println!(
            "  ║  Game DLLs: {}                               ║",
            game_imports
                .keys()
                .map(|s| s.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    println!("  ╚════════════════════════════════════════════╝");

    ImportVerdict {
        can_load_standalone: can_load,
        system_imports: system_imports.into_iter().collect(),
        crt_imports: crt_imports.into_iter().collect(),
        game_imports: game_imports.into_iter().collect(),
        has_tls_callbacks: has_tls,
    }
}

// ══════════════════════════════════════════════════════════════
// Pass 2: Section Map + .tvm0 Self-Containment
// ══════════════════════════════════════════════════════════════

struct SectionAnalysis {
    pe: PeInfo,
    has_sgxm0: bool,
    has_sgxm1: bool,
    has_tvm0: bool,
    is_self_contained: bool,
}

fn pass2_section_map(data: &[u8]) -> SectionAnalysis {
    println!("\n╔══════════════════════════════════════════════════════════════╗");
    println!("║  Pass 2: Section Map + .tvm0 Self-Containment              ║");
    println!("╚══════════════════════════════════════════════════════════════╝\n");

    let pe = parse_pe(data);

    println!("  Image base: 0x{:016X}", pe.image_base);
    println!(
        "  {:>10} {:>12} {:>12} {:>8} {:>12} {:>7} {:>8}",
        "Name", "VirtAddr", "VirtSize", "RawOff", "RawSize", "Perms", "Entropy"
    );
    println!("  {}", "-".repeat(78));

    let mut has_sgxm0 = false;
    let mut has_sgxm1 = false;
    let mut has_tvm0 = false;

    for sec in &pe.sections {
        let range = sec.file_range();
        let end = range.end.min(data.len());
        let start = range.start.min(end);
        let sec_data = &data[start..end];
        let ent = entropy(sec_data);

        println!(
            "  {:>10} 0x{:08X}   {:>10} {:>8X} {:>10}   {:>5}   {:.3}",
            sec.name,
            sec.virtual_address,
            format_bytes(sec.virtual_size as u64),
            sec.pointer_to_raw_data,
            format_bytes(sec.size_of_raw_data as u64),
            sec.permissions_str(),
            ent
        );

        match sec.name.as_str() {
            ".Sgxm0" => has_sgxm0 = true,
            ".Sgxm1" => has_sgxm1 = true,
            ".tvm0" => has_tvm0 = true,
            _ => {}
        }
    }

    println!();
    println!("  Sagittarius VM sections:");
    println!(
        "    .Sgxm0 (VM dispatcher): {}",
        if has_sgxm0 {
            "\x1b[1;32mPRESENT\x1b[0m"
        } else {
            "\x1b[1;31mMISSING\x1b[0m"
        }
    );
    println!(
        "    .Sgxm1 (VM data):       {}",
        if has_sgxm1 {
            "\x1b[1;32mPRESENT\x1b[0m"
        } else {
            "\x1b[1;31mMISSING\x1b[0m"
        }
    );
    println!(
        "    .tvm0 (bytecode):       {}",
        if has_tvm0 {
            "\x1b[1;32mPRESENT\x1b[0m"
        } else {
            "\x1b[1;31mMISSING\x1b[0m"
        }
    );

    let is_self_contained = has_tvm0 && (has_sgxm0 || has_sgxm1);
    if is_self_contained {
        println!(
            "\n  \x1b[1;32mSelf-contained: HGP.dll has its own VM dispatcher + bytecode\x1b[0m"
        );
    } else if has_tvm0 && !has_sgxm0 {
        println!(
            "\n  \x1b[1;33mPartially contained: has .tvm0 but missing .Sgxm0 dispatcher\x1b[0m"
        );
        println!("  → May need GameAssembly.dll's .Sgxm0 section to execute VM code");
    } else if !has_tvm0 {
        println!("\n  Native code only — no VM protection");
    }

    // Scan .tvm0 entry points for native code vs bytecode
    if has_tvm0 && let Some(tvm0) = pe.sections.iter().find(|s| s.name == ".tvm0") {
        let range = tvm0.file_range();
        let end = range.end.min(data.len());
        let start = range.start.min(end);
        let tvm_data = &data[start..end];

        println!("\n  === .tvm0 Entry Point Analysis ===");
        println!("  Section size: {}", format_bytes(tvm_data.len() as u64));

        // Check first 64 bytes for VM entry pattern
        if tvm_data.len() >= 4 {
            let has_vm_pattern = tvm_data.windows(4).take(64).any(|w| w == VM_ENTRY_PREFIX);
            println!("  VM entry pattern at start: {}", has_vm_pattern);
        }

        // Sample entropy at different offsets
        let sample_size = 256;
        let samples = [
            0,
            tvm_data.len() / 4,
            tvm_data.len() / 2,
            3 * tvm_data.len() / 4,
        ];
        println!("  Entropy samples (256-byte windows):");
        for &off in &samples {
            if off + sample_size <= tvm_data.len() {
                let ent = entropy(&tvm_data[off..off + sample_size]);
                println!("    offset 0x{:06X}: {:.3}", off, ent);
            }
        }
    }

    SectionAnalysis {
        pe,
        has_sgxm0,
        has_sgxm1,
        has_tvm0,
        is_self_contained,
    }
}

// ══════════════════════════════════════════════════════════════
// Pass 3: Export Signature Inference
// ══════════════════════════════════════════════════════════════

fn pass3_export_signatures(data: &[u8], pe: &PeInfo) -> Vec<InferredSignature> {
    println!("\n╔══════════════════════════════════════════════════════════════╗");
    println!("║  Pass 3: Export Signature Inference                        ║");
    println!("╚══════════════════════════════════════════════════════════════╝\n");

    let mut signatures = Vec::new();

    for export in &pe.exports {
        println!("  === {} (RVA 0x{:08X}) ===\n", export.name, export.rva);

        // Disassemble first 30 instructions
        if let Some(file_off) = pe.rva_to_file_offset(export.rva) {
            let end = (file_off + 30 * 15).min(data.len());
            let code = &data[file_off..end];
            let rip = pe.image_base + export.rva as u64;
            let mut decoder = Decoder::with_ip(64, code, rip, DecoderOptions::NONE);
            let mut instr = Instruction::default();
            let mut n = 0;

            while decoder.can_decode() && n < 20 {
                decoder.decode_out(&mut instr);
                if instr.is_invalid() {
                    continue;
                }
                println!("    {:016X}  {}", instr.ip(), format_instr(&instr));
                n += 1;

                // Stop at ret or unconditional jmp for display
                if instr.mnemonic() == Mnemonic::Ret {
                    break;
                }
            }
        }

        let sig = infer_signature(data, pe, export);

        println!("\n    Inferred signature:");
        println!(
            "      Parameters: {} (from ABI register analysis)",
            sig.param_count
        );
        for hint in &sig.param_hints {
            println!("        {}", hint);
        }
        println!("      Return: {}", sig.return_hint);
        if let Some(frame) = sig.stack_frame_size {
            println!(
                "      Stack frame: 0x{:X} bytes ({} locals)",
                frame,
                frame / 8
            );
        }
        if let Some(target) = sig.jmp_target_rva {
            println!(
                "      JMP target: RVA 0x{:08X} → section {}",
                target,
                sig.jmp_target_section.as_deref().unwrap_or("???")
            );
        }
        if sig.is_vm_entry {
            println!("      \x1b[1;33mVM entry point detected\x1b[0m");
        }
        println!();

        signatures.push(sig);
    }

    // Summary table
    println!("  === Signature Summary ===\n");
    println!(
        "  {:>15} {:>6} {:>8} {:>12} {:>8}",
        "Export", "Params", "Returns", "Target Sec", "VM?"
    );
    println!("  {}", "-".repeat(55));
    for sig in &signatures {
        println!(
            "  {:>15} {:>6} {:>8} {:>12} {:>8}",
            sig.export_name,
            sig.param_count,
            sig.return_hint,
            sig.jmp_target_section.as_deref().unwrap_or("native"),
            if sig.is_vm_entry { "YES" } else { "no" }
        );
    }

    signatures
}

// ══════════════════════════════════════════════════════════════
// Pass 4: PInvoke Descriptor Search in GameAssembly.dll
// ══════════════════════════════════════════════════════════════

#[derive(Debug)]
struct PInvokeMatch {
    export_name: String,
    arg_count: Option<u32>,
    file_offset: usize,
    context: String,
}

fn pass4_pinvoke_descriptors(ga_data: &[u8], ga_pe: &PeInfo, hgp_pe: &PeInfo) -> Vec<PInvokeMatch> {
    println!("\n╔══════════════════════════════════════════════════════════════╗");
    println!("║  Pass 4: PInvoke Descriptor Search in GameAssembly.dll     ║");
    println!("╚══════════════════════════════════════════════════════════════╝\n");

    let mut matches = Vec::new();

    // Search for UTF-16 "HGP" or "HGP.dll" in .rdata
    println!("  === UTF-16 Module Name Search ===\n");
    let hgp_utf16: Vec<u8> = "HGP".encode_utf16().flat_map(|c| c.to_le_bytes()).collect();
    let hgp_dll_utf16: Vec<u8> = "HGP.dll"
        .encode_utf16()
        .flat_map(|c| c.to_le_bytes())
        .collect();

    for pattern in [&hgp_utf16, &hgp_dll_utf16] {
        let pattern_str = if pattern.len() == hgp_utf16.len() {
            "HGP"
        } else {
            "HGP.dll"
        };
        let mut found = 0;
        for i in 0..ga_data.len().saturating_sub(pattern.len()) {
            if &ga_data[i..i + pattern.len()] == pattern.as_slice() {
                found += 1;
                if found <= 10 {
                    let sec_name = ga_pe
                        .sections
                        .iter()
                        .find(|s| {
                            let start = s.pointer_to_raw_data as usize;
                            i >= start && i < start + s.size_of_raw_data as usize
                        })
                        .map(|s| s.name.as_str())
                        .unwrap_or("???");

                    // Try to read full UTF-16 string
                    let full = read_utf16_string(ga_data, i).unwrap_or_default();
                    println!(
                        "    UTF-16 \"{}\" at file:0x{:08X} section:{} => \"{}\"",
                        pattern_str, i, sec_name, full
                    );

                    // Dump context around match
                    let ctx_start = i.saturating_sub(32);
                    let ctx_end = (i + 64).min(ga_data.len());
                    hex_dump(&ga_data[ctx_start..ctx_end], ctx_start, ctx_end - ctx_start);
                    println!();
                }
            }
        }
        println!("    Total \"{}\" (UTF-16): {}\n", pattern_str, found);
    }

    // Search for ASCII export names in .rdata
    println!("  === ASCII Export Name Search ===\n");
    let export_names: Vec<&str> = hgp_pe.exports.iter().map(|e| e.name.as_str()).collect();

    for &name in &export_names {
        let pattern = name.as_bytes();
        let mut found_offsets = Vec::new();

        for i in 0..ga_data.len().saturating_sub(pattern.len()) {
            if &ga_data[i..i + pattern.len()] == pattern {
                // Verify null-terminated or at end of region
                if i + pattern.len() < ga_data.len() && ga_data[i + pattern.len()] == 0 {
                    found_offsets.push(i);
                }
            }
        }

        if found_offsets.is_empty() {
            println!("    \"{}\" — not found as null-terminated string", name);
        } else {
            println!("    \"{}\" — {} occurrences:", name, found_offsets.len());
            for &off in found_offsets.iter().take(5) {
                let sec_name = ga_pe
                    .sections
                    .iter()
                    .find(|s| {
                        let start = s.pointer_to_raw_data as usize;
                        off >= start && off < start + s.size_of_raw_data as usize
                    })
                    .map(|s| s.name.as_str())
                    .unwrap_or("???");

                // Try to read the full string for context
                let full = read_cstring(ga_data, off).unwrap_or_default();
                println!(
                    "      file:0x{:08X} section:{} => \"{}\"",
                    off, sec_name, full
                );

                // Look for PInvoke descriptor struct nearby
                // Format: { utf16_module_name, 4, ascii_func_name, func_name_len, 1, arg_count, 0x10, flag }
                // The ascii_func_name field would be a VA pointing to this string
                let string_rva = ga_pe.file_offset_to_rva(off);
                if let Some(rva) = string_rva {
                    let va = GA_IMAGE_BASE + rva as u64;
                    let va_bytes = va.to_le_bytes();

                    // Search for VA reference to this string in .rdata/.data
                    for sec in &ga_pe.sections {
                        if !matches!(sec.name.as_str(), ".rdata" | ".data" | "il2cpp") {
                            continue;
                        }
                        let range = sec.file_range();
                        let end = range.end.min(ga_data.len());
                        let start = range.start;
                        let sec_data = &ga_data[start..end];

                        for j in 0..sec_data.len().saturating_sub(8) {
                            if sec_data[j..j + 8] == va_bytes {
                                let ref_off = start + j;
                                println!(
                                    "        → VA ref at file:0x{:08X} section:{}",
                                    ref_off, sec.name
                                );

                                // Try reading surrounding fields for descriptor struct
                                // Field before: module name VA (should be UTF-16 string)
                                // Field at j-16: utf16_module_name VA
                                // Field at j-8: literal 4
                                // Field at j: ascii_func_name VA (this match)
                                // Field at j+8: func_name_len
                                // Field at j+12: literal 1
                                // Field at j+16: arg_count
                                // Field at j+20: 0x10
                                // Field at j+24: flag
                                if j >= 16
                                    && let Some(pre_val) =
                                        read_u64_at(ga_data, ref_off.saturating_sub(8))
                                    && pre_val == 4
                                {
                                    // Matches! Field before our VA should be module name
                                    if let Some(mod_va) =
                                        read_u64_at(ga_data, ref_off.saturating_sub(16))
                                    {
                                        let mod_rva = (mod_va.wrapping_sub(GA_IMAGE_BASE)) as u32;
                                        if let Some(mod_off) = ga_pe.rva_to_file_offset(mod_rva)
                                            && let Some(mod_str) =
                                                read_utf16_string(ga_data, mod_off)
                                        {
                                            println!("        → Module: \"{}\"", mod_str);
                                        }
                                    }
                                    // Read arg_count field
                                    if let Some(name_len) = read_u32_at(ga_data, ref_off + 8)
                                        && name_len == name.len() as u32
                                    {
                                        println!("        → name_len: {} (matches!)", name_len);
                                        if let Some(one) = read_u32_at(ga_data, ref_off + 12)
                                            && one == 1
                                            && let Some(argc) = read_u32_at(ga_data, ref_off + 16)
                                        {
                                            println!(
                                                "        → \x1b[1;32marg_count: {}\x1b[0m",
                                                argc
                                            );
                                            matches.push(PInvokeMatch {
                                                export_name: name.to_string(),
                                                arg_count: Some(argc),
                                                file_offset: ref_off,
                                                context: "PInvoke descriptor, module via VA"
                                                    .to_string(),
                                            });
                                        }
                                    }
                                }

                                // Also dump context
                                let ctx = ref_off.saturating_sub(32);
                                let ctx_end = (ref_off + 48).min(ga_data.len());
                                hex_dump(&ga_data[ctx..ctx_end], ctx, ctx_end - ctx);
                                println!();
                            }
                        }
                    }
                }
            }
        }
        println!();
    }

    // Summary
    println!("  === PInvoke Summary ===\n");
    if matches.is_empty() {
        println!("  No PInvoke descriptors with arg_count found.");
        println!("  HGP exports may be called via indirect resolution (__InternalDynamic)");
    } else {
        for m in &matches {
            println!(
                "  {} → {} args (at file:0x{:08X}, {})",
                m.export_name,
                m.arg_count.unwrap_or(0),
                m.file_offset,
                m.context
            );
        }
    }

    matches
}

// ══════════════════════════════════════════════════════════════
// Pass 5: String & Constant Survey
// ══════════════════════════════════════════════════════════════

fn pass5_strings_and_constants(hgp_data: &[u8], hgp_pe: &PeInfo, ga_data: Option<&[u8]>) {
    println!("\n╔══════════════════════════════════════════════════════════════╗");
    println!("║  Pass 5: String & Constant Survey                          ║");
    println!("╚══════════════════════════════════════════════════════════════╝\n");

    // Extract all ASCII strings >= 4 chars
    println!("  === ASCII Strings ===\n");
    let mut strings: Vec<(usize, String)> = Vec::new();
    let mut i = 0;
    while i < hgp_data.len() {
        if hgp_data[i].is_ascii_graphic() || hgp_data[i] == b' ' {
            let start = i;
            while i < hgp_data.len() && (hgp_data[i].is_ascii_graphic() || hgp_data[i] == b' ') {
                i += 1;
            }
            let len = i - start;
            if len >= 4 {
                let s = String::from_utf8_lossy(&hgp_data[start..i]).to_string();
                strings.push((start, s));
            }
        } else {
            i += 1;
        }
    }

    // Categorize interesting strings
    let mut error_strings = Vec::new();
    let mut format_strings = Vec::new();
    let mut crypto_strings = Vec::new();
    let mut misc_interesting = Vec::new();

    for (off, s) in &strings {
        let lower = s.to_lowercase();
        if lower.contains("error")
            || lower.contains("fail")
            || lower.contains("invalid")
            || lower.contains("exception")
            || lower.contains("assert")
        {
            error_strings.push((*off, s.as_str()));
        } else if s.contains("%s")
            || s.contains("%d")
            || s.contains("%x")
            || s.contains("%p")
            || s.contains("%u")
            || s.contains("%02x")
        {
            format_strings.push((*off, s.as_str()));
        } else if lower.contains("crypt")
            || lower.contains("cipher")
            || lower.contains("key")
            || lower.contains("aes")
            || lower.contains("hash")
            || lower.contains("hmac")
            || lower.contains("decrypt")
            || lower.contains("encrypt")
        {
            crypto_strings.push((*off, s.as_str()));
        } else if lower.contains("hgp")
            || lower.contains("vfs")
            || lower.contains("beyond")
            || lower.contains("store")
            || lower.contains("cache")
            || lower.contains("const")
            || lower.contains("reverse")
            || lower.contains("note")
        {
            misc_interesting.push((*off, s.as_str()));
        }
    }

    println!("  Total strings (>= 4 chars): {}", strings.len());

    println!("\n  Error/assertion strings ({}):", error_strings.len());
    for (off, s) in &error_strings {
        let sec_name = hgp_pe
            .sections
            .iter()
            .find(|sec| {
                let start = sec.pointer_to_raw_data as usize;
                *off >= start && *off < start + sec.size_of_raw_data as usize
            })
            .map(|sec| sec.name.as_str())
            .unwrap_or("???");
        println!(
            "    [0x{:08X}] {} \"{}\"",
            off,
            sec_name,
            truncate_str(s, 80)
        );
    }

    println!("\n  Format strings ({}):", format_strings.len());
    for (off, s) in &format_strings {
        println!("    [0x{:08X}] \"{}\"", off, truncate_str(s, 80));
    }

    println!("\n  Crypto-related strings ({}):", crypto_strings.len());
    for (off, s) in &crypto_strings {
        println!("    [0x{:08X}] \"{}\"", off, truncate_str(s, 80));
    }

    println!(
        "\n  HGP/export-related strings ({}):",
        misc_interesting.len()
    );
    for (off, s) in misc_interesting.iter().take(30) {
        println!("    [0x{:08X}] \"{}\"", off, truncate_str(s, 80));
    }
    if misc_interesting.len() > 30 {
        println!("    ... and {} more", misc_interesting.len() - 30);
    }

    // S-box fingerprint search
    println!("\n  === S-Box Fingerprint Search ===\n");
    if let Some(ga) = ga_data {
        // Extract the 6 known S-boxes from GameAssembly.dll
        let mut sbox_count = 0;
        for (idx, &off) in SBOX_FILE_OFFSETS.iter().enumerate() {
            if off + 256 <= ga.len() {
                let sbox = &ga[off..off + 256];
                // Search for this S-box in HGP.dll
                let mut found = false;
                for j in 0..hgp_data.len().saturating_sub(256) {
                    if &hgp_data[j..j + 256] == sbox {
                        let sec_name = hgp_pe
                            .sections
                            .iter()
                            .find(|s| {
                                let start = s.pointer_to_raw_data as usize;
                                j >= start && j < start + s.size_of_raw_data as usize
                            })
                            .map(|s| s.name.as_str())
                            .unwrap_or("???");
                        println!(
                            "    \x1b[1;32mS-box {} FOUND at HGP offset 0x{:08X} section:{}\x1b[0m",
                            idx, j, sec_name
                        );
                        found = true;
                        sbox_count += 1;
                    }
                }
                if !found {
                    println!("    S-box {} not found in HGP.dll", idx);
                }
            }
        }
        println!("\n  S-box matches: {}/6", sbox_count);
    } else {
        println!("  (GameAssembly.dll not loaded — skipping S-box search)");
    }

    // Search for 256-byte permutations (potential S-boxes native to HGP)
    println!("\n  === Native Permutation Search (potential S-boxes) ===\n");
    let mut native_sboxes = Vec::new();
    for off in (0..hgp_data.len().saturating_sub(256)).step_by(1) {
        if is_permutation(&hgp_data[off..off + 256]) {
            // Skip identity permutation
            let is_identity = (0..=255u8).all(|i| hgp_data[off + i as usize] == i);
            if !is_identity {
                let sec_name = hgp_pe
                    .sections
                    .iter()
                    .find(|s| {
                        let start = s.pointer_to_raw_data as usize;
                        off >= start && off < start + s.size_of_raw_data as usize
                    })
                    .map(|s| s.name.as_str())
                    .unwrap_or("???");
                let ent = entropy(&hgp_data[off..off + 256]);
                println!(
                    "    Permutation at offset 0x{:08X} section:{} entropy:{:.3}",
                    off, sec_name, ent
                );
                println!("    First 16 bytes: {:02x?}", &hgp_data[off..off + 16]);
                native_sboxes.push(off);
            }
        }
    }
    println!("\n  Native S-boxes found: {}", native_sboxes.len());

    // Search for directory hashes
    println!("\n  === Directory Hash Search ===\n");
    for &(name, hash) in DIR_HASHES {
        let pattern = hash.to_le_bytes();
        let mut found = 0;
        for j in 0..hgp_data.len().saturating_sub(4) {
            if hgp_data[j..j + 4] == pattern {
                found += 1;
                if found <= 3 {
                    let sec_name = hgp_pe
                        .sections
                        .iter()
                        .find(|s| {
                            let start = s.pointer_to_raw_data as usize;
                            j >= start && j < start + s.size_of_raw_data as usize
                        })
                        .map(|s| s.name.as_str())
                        .unwrap_or("???");
                    println!(
                        "    Hash {} (0x{:08X}) at offset 0x{:08X} section:{}",
                        name, hash, j, sec_name
                    );
                }
            }
        }
        if found == 0 {
            println!("    Hash {} (0x{:08X}) — not found", name, hash);
        } else if found > 3 {
            println!("    Hash {} — {} total matches", name, found);
        }
    }
}

fn truncate_str(s: &str, max: usize) -> String {
    if s.len() > max {
        format!("{}...", &s[..max])
    } else {
        s.to_string()
    }
}

// ══════════════════════════════════════════════════════════════
// Pass 6: Generate Python Prober Script
// ══════════════════════════════════════════════════════════════

fn pass6_generate_prober(
    verdict: &ImportVerdict,
    _section_info: &SectionAnalysis,
    signatures: &[InferredSignature],
    pinvoke_matches: &[PInvokeMatch],
) {
    println!("\n╔══════════════════════════════════════════════════════════════╗");
    println!("║  Pass 6: Generate Python Prober Script                     ║");
    println!("╚══════════════════════════════════════════════════════════════╝\n");

    let output_path = "./output/hgp_prober_generated.py";

    let mut py = String::new();
    py.push_str(
        r#"#!/usr/bin/env python3
"""
HGP.dll Standalone Prober — Auto-generated by hgp_probe.rs (Phase R2)

Loads HGP.dll in a standalone Python process and probes its exports.
Run on Windows with HGP.dll in the same directory.

Usage:
    python hgp_prober.py [--dll PATH] [--capture capture.bin]

Zero ban risk — loads DLL in isolated process, not the game.
"""
import ctypes
import ctypes.wintypes as wt
import struct
import sys
import os
import time
from pathlib import Path

# ═══════════════════════════════════════════════════════════
# Configuration
# ═══════════════════════════════════════════════════════════

DLL_NAME = "HGP.dll"
EVENT_FORMAT = True  # Output in frida_companion.rs event format

"#,
    );

    // Generate inferred prototypes as comments
    py.push_str("# Inferred export signatures (from static analysis):\n");
    for sig in signatures {
        let argc = if let Some(pm) = pinvoke_matches
            .iter()
            .find(|m| m.export_name == sig.export_name)
        {
            pm.arg_count.unwrap_or(sig.param_count as u32) as usize
        } else {
            sig.param_count
        };
        py.push_str(&format!(
            "#   {} — {} params, {}{}\n",
            sig.export_name,
            argc,
            sig.return_hint,
            if sig.is_vm_entry {
                ", VM-protected"
            } else {
                ""
            }
        ));
    }

    py.push_str(
        r#"
# ═══════════════════════════════════════════════════════════
# Safety Layer
# ═══════════════════════════════════════════════════════════

def safe_call(func, *args, desc=""):
    """Call a DLL function with SEH protection."""
    try:
        result = func(*args)
        return (True, result)
    except OSError as e:
        return (False, f"SEH: {e}")
    except Exception as e:
        return (False, f"Exception: {e}")

def emit_event(event_type, **fields):
    """Output event in frida_companion.rs format."""
    if EVENT_FORMAT:
        print(f"=== HGP_PROBE type={event_type} ===")
        for k, v in fields.items():
            if isinstance(v, int):
                print(f"{k}=0x{v:X}")
            elif isinstance(v, bytes):
                print(f"{k}={v.hex()}")
            else:
                print(f"{k}={v}")
        print("=== END_EVENT ===")
        print()
    else:
        print(f"[{event_type}] {fields}")

# ═══════════════════════════════════════════════════════════
# DLL Loading
# ═══════════════════════════════════════════════════════════

def load_dll(dll_path=None):
    """Load HGP.dll and resolve exports."""
    if dll_path is None:
        dll_path = DLL_NAME

    if not os.path.exists(dll_path):
        print(f"ERROR: {dll_path} not found")
        print(f"Copy HGP.dll to: {os.path.abspath('.')}")
        sys.exit(1)

    print(f"Loading {dll_path}...")
    print(f"  Size: {os.path.getsize(dll_path):,} bytes")
"#,
    );

    // Generate loading strategy based on Pass 1 verdict
    if verdict.can_load_standalone {
        py.push_str(
            r#"
    # Pass 1 verdict: standalone loading is feasible
    try:
        dll = ctypes.CDLL(str(Path(dll_path).resolve()))
        print(f"  Loaded successfully (standalone)")
    except OSError as e:
        print(f"  LoadLibrary failed: {e}")
        print(f"  Trying with altered search path...")
        try:
            dll_dir = str(Path(dll_path).resolve().parent)
            os.add_dll_directory(dll_dir)
            dll = ctypes.CDLL(str(Path(dll_path).resolve()))
            print(f"  Loaded with DLL directory: {dll_dir}")
        except OSError as e2:
            print(f"  Still failed: {e2}")
            sys.exit(1)
"#,
        );
    } else {
        py.push_str(
            r#"
    # Pass 1 verdict: game-specific imports detected
    # Must load from game directory or provide stub DLLs
    game_dir = r"C:\Program Files\ArknightsEndfield"  # Adjust as needed
    print(f"  NOTE: Game-specific imports detected. Trying game directory: {game_dir}")

    try:
        if os.path.isdir(game_dir):
            os.add_dll_directory(game_dir)
        dll = ctypes.CDLL(str(Path(dll_path).resolve()))
        print(f"  Loaded successfully (with game directory)")
    except OSError as e:
        print(f"  LoadLibrary failed: {e}")
        print(f"  Game-specific imports required:")
"#,
        );
        for (dll_name, count) in &verdict.game_imports {
            py.push_str(&format!(
                "        print(f\"    {} ({} functions)\")\n",
                dll_name, count
            ));
        }
        py.push_str("        sys.exit(1)\n");
    }

    py.push_str(
        r#"
    emit_event("DLL_LOADED", path=str(Path(dll_path).resolve()),
               size=os.path.getsize(dll_path))
    return dll

# ═══════════════════════════════════════════════════════════
# Export Probing
# ═══════════════════════════════════════════════════════════

"#,
    );

    // Generate typed function setup for each export
    for sig in signatures {
        let argc = if let Some(pm) = pinvoke_matches
            .iter()
            .find(|m| m.export_name == sig.export_name)
        {
            pm.arg_count.unwrap_or(sig.param_count as u32) as usize
        } else {
            sig.param_count
        };

        py.push_str(&format!(
            r#"def probe_{name_lower}(dll):
    """Probe HGP.{name} ({argc} params inferred)."""
    print(f"\n=== Probing {name} ===")
    try:
        func = getattr(dll, "{name}")
    except AttributeError:
        print(f"  Export '{name}' not found!")
        return

    # Set up prototype — conservative: all args as c_uint64, return c_uint64
    func.restype = ctypes.c_uint64
    func.argtypes = [{argtypes}]

"#,
            name = sig.export_name,
            name_lower = sig.export_name.to_lowercase(),
            argc = argc,
            argtypes = (0..argc)
                .map(|_| "ctypes.c_uint64")
                .collect::<Vec<_>>()
                .join(", "),
        ));

        // Generate test cases based on export name semantics
        match sig.export_name.as_str() {
            "GetConst" => {
                py.push_str(
                    r#"    # GetConst — likely returns a constant by index
    for idx in range(16):
        ok, result = safe_call(func, idx, desc=f"GetConst({idx})")
        if ok:
            emit_event("EXPORT_CALL", export_name="GetConst",
                       arg0=idx, retval=result, success="true")
            if result != 0:
                print(f"  GetConst({idx}) = 0x{result:X}")
                # Try interpreting as pointer to string
                try:
                    ptr = ctypes.c_char_p(result)
                    if ptr.value and len(ptr.value) < 256:
                        print(f"    As string: {ptr.value}")
                except:
                    pass
        else:
            emit_event("EXPORT_CALL", export_name="GetConst",
                       arg0=idx, error=str(result), success="false")
            print(f"  GetConst({idx}) failed: {result}")
            break
"#,
                );
            }
            "ReverseStr" => {
                py.push_str(r#"    # ReverseStr — likely reverses/transforms a string or buffer
    test_inputs = [b"ABCDEF\x00", b"Hello World\x00", b"\x01\x02\x03\x04\x05\x06\x07\x08"]
    for test_data in test_inputs:
        buf = ctypes.create_string_buffer(test_data)
        buf_ptr = ctypes.cast(buf, ctypes.c_uint64)
        size = len(test_data)
        args = [buf_ptr.value, size] if func.argtypes and len(func.argtypes) >= 2 else [buf_ptr.value]
        ok, result = safe_call(func, *args, desc=f"ReverseStr({test_data!r})")
        if ok:
            emit_event("EXPORT_CALL", export_name="ReverseStr",
                       input=test_data.hex(), retval=result, success="true",
                       output=bytes(buf).hex())
            print(f"  ReverseStr({test_data!r}) = 0x{result:X}")
            print(f"    Buffer after: {bytes(buf)[:32].hex()}")
        else:
            emit_event("EXPORT_CALL", export_name="ReverseStr",
                       input=test_data.hex(), error=str(result), success="false")
            print(f"  ReverseStr failed: {result}")
            break
"#);
            }
            "TestFrom" => {
                py.push_str(r#"    # TestFrom — likely validates/tests data
    # Try with a small buffer
    test_buf = ctypes.create_string_buffer(256)
    buf_ptr = ctypes.cast(test_buf, ctypes.c_uint64)
    for size in [0, 16, 64, 256]:
        args = [buf_ptr.value, size] if func.argtypes and len(func.argtypes) >= 2 else [buf_ptr.value]
        ok, result = safe_call(func, *args, desc=f"TestFrom(buf, {size})")
        if ok:
            emit_event("EXPORT_CALL", export_name="TestFrom",
                       arg0=buf_ptr.value, arg1=size, retval=result, success="true")
            print(f"  TestFrom(buf, {size}) = 0x{result:X}")
        else:
            emit_event("EXPORT_CALL", export_name="TestFrom",
                       arg0=buf_ptr.value, arg1=size, error=str(result), success="false")
            print(f"  TestFrom(buf, {size}) failed: {result}")
            break
"#);
            }
            "NoteCache" => {
                py.push_str(
                    r#"    # NoteCache — likely caches some data
    # Probe with small integer args first (safe)
    for val in [0, 1, 0xDEAD]:
        args = [val]
        if func.argtypes and len(func.argtypes) >= 2:
            args = [val, 0]
        if func.argtypes and len(func.argtypes) >= 3:
            args = [val, 0, 0]
        ok, result = safe_call(func, *args, desc=f"NoteCache({val})")
        if ok:
            emit_event("EXPORT_CALL", export_name="NoteCache",
                       arg0=val, retval=result, success="true")
            print(f"  NoteCache({val}) = 0x{result:X}")
        else:
            emit_event("EXPORT_CALL", export_name="NoteCache",
                       arg0=val, error=str(result), success="false")
            print(f"  NoteCache({val}) failed: {result}")
            break
"#,
                );
            }
            "Store" => {
                py.push_str(
                    r#"    # Store — likely stores data (most dangerous, probe last)
    # Try read-only probing with null/zero args
    args = [0]
    if func.argtypes and len(func.argtypes) >= 2:
        args = [0, 0]
    if func.argtypes and len(func.argtypes) >= 3:
        args = [0, 0, 0]
    ok, result = safe_call(func, *args, desc="Store(0, ...)")
    if ok:
        emit_event("EXPORT_CALL", export_name="Store",
                   args=str(args), retval=result, success="true")
        print(f"  Store({args}) = 0x{result:X}")
    else:
        emit_event("EXPORT_CALL", export_name="Store",
                   args=str(args), error=str(result), success="false")
        print(f"  Store({args}) failed: {result}")
"#,
                );
            }
            _ => {
                // Generic probe
                py.push_str(&format!(
                    r#"    # Generic probe for {name}
    ok, result = safe_call(func, {zeros}, desc="{name}(0, ...)")
    if ok:
        emit_event("EXPORT_CALL", export_name="{name}",
                   retval=result, success="true")
        print(f"  {name}() = 0x{{result:X}}")
    else:
        emit_event("EXPORT_CALL", export_name="{name}",
                   error=str(result), success="false")
        print(f"  {name}() failed: {{result}}")
"#,
                    name = sig.export_name,
                    zeros = (0..argc).map(|_| "0").collect::<Vec<_>>().join(", "),
                ));
            }
        }
        py.push('\n');
    }

    // Generate captured traffic feeding function
    py.push_str(
        r#"
def probe_with_capture(dll, capture_path):
    """Feed captured TCP data through HGP exports."""
    print(f"\n=== Feeding captured traffic: {capture_path} ===")

    if not os.path.exists(capture_path):
        print(f"  Capture file not found: {capture_path}")
        return

    with open(capture_path, "rb") as f:
        data = f.read()

    print(f"  Loaded {len(data)} bytes")

    # Try each export that might be a decrypt function
    for export_name in ["ReverseStr", "TestFrom", "GetConst"]:
        try:
            func = getattr(dll, export_name)
        except AttributeError:
            continue

        func.restype = ctypes.c_uint64
        func.argtypes = [ctypes.c_uint64, ctypes.c_uint64]

        # Allocate buffers
        in_buf = ctypes.create_string_buffer(data)
        out_buf = ctypes.create_string_buffer(len(data))
        in_ptr = ctypes.cast(in_buf, ctypes.c_uint64).value
        out_ptr = ctypes.cast(out_buf, ctypes.c_uint64).value

        # Try as decrypt(input, size)
        ok, result = safe_call(func, in_ptr, len(data), desc=f"{export_name}(capture, {len(data)})")
        if ok:
            output_data = bytes(out_buf)
            # Check if output looks like decrypted data (lower entropy or has structure)
            nonzero = sum(1 for b in output_data if b != 0)
            emit_event("CAPTURE_FEED", export_name=export_name,
                       input_size=len(data), retval=result,
                       output_nonzero=nonzero, success="true")
            print(f"  {export_name}(capture, {len(data)}) = 0x{result:X}")
            print(f"  Output: {nonzero} non-zero bytes")
            if nonzero > 0:
                print(f"  First 64 bytes: {output_data[:64].hex()}")
        else:
            print(f"  {export_name} failed: {result}")

# ═══════════════════════════════════════════════════════════
# Main
# ═══════════════════════════════════════════════════════════

def main():
    import argparse
    parser = argparse.ArgumentParser(description="HGP.dll Standalone Prober")
    parser.add_argument("--dll", default=DLL_NAME, help="Path to HGP.dll")
    parser.add_argument("--capture", help="Path to captured TCP data file")
    parser.add_argument("--no-events", action="store_true", help="Disable event format output")
    args = parser.parse_args()

    global EVENT_FORMAT
    if args.no_events:
        EVENT_FORMAT = False

    print("=" * 60)
    print("  HGP.dll Standalone Prober (Phase R2)")
    print("  Auto-generated by hgp_probe.rs")
    print("=" * 60)
    print()

    dll = load_dll(args.dll)
    print()

    # Probe exports in order of safety
"#,
    );

    // Generate main probe calls in safety order
    let safety_order = ["GetConst", "ReverseStr", "TestFrom", "NoteCache", "Store"];
    for name in &safety_order {
        let has_export = signatures.iter().any(|s| s.export_name == *name);
        if has_export {
            py.push_str(&format!("    probe_{}(dll)\n", name.to_lowercase()));
        }
    }

    py.push_str(
        r#"
    # Feed captured traffic if provided
    if args.capture:
        probe_with_capture(dll, args.capture)

    print("\n" + "=" * 60)
    print("  Probing complete")
    print("=" * 60)

if __name__ == "__main__":
    main()
"#,
    );

    // Write the file
    fs::create_dir_all("./output").ok();
    fs::write(output_path, &py).expect("Failed to write hgp_prober.py");
    println!("  Generated: {} ({} bytes)", output_path, py.len());

    // Also print summary of what was generated
    println!("\n  Generated probes:");
    for sig in signatures {
        let argc = if let Some(pm) = pinvoke_matches
            .iter()
            .find(|m| m.export_name == sig.export_name)
        {
            pm.arg_count.unwrap_or(sig.param_count as u32) as usize
        } else {
            sig.param_count
        };
        println!(
            "    probe_{}() — {} args, {}",
            sig.export_name.to_lowercase(),
            argc,
            if sig.is_vm_entry {
                "VM-protected"
            } else {
                "native"
            }
        );
    }
}

// ══════════════════════════════════════════════════════════════
// Pass 7: .tvm0 Bytecode Disassembly at Export JMP Targets
// ══════════════════════════════════════════════════════════════

fn pass7_tvm0_disassembly(data: &[u8], pe: &PeInfo, signatures: &[InferredSignature]) {
    println!("\n╔══════════════════════════════════════════════════════════════╗");
    println!("║  Pass 7: .tvm0 Bytecode Disassembly at Export JMP Targets  ║");
    println!("╚══════════════════════════════════════════════════════════════╝\n");

    if !pe.sections.iter().any(|s| s.name == ".tvm0") {
        println!("  .tvm0 section not found — skipping");
        return;
    }

    for sig in signatures {
        let Some(target_rva) = sig.jmp_target_rva else {
            println!("  {} — no JMP target, skipping\n", sig.export_name);
            continue;
        };

        let Some(file_off) = pe.rva_to_file_offset(target_rva) else {
            println!(
                "  {} — cannot resolve RVA 0x{:08X} to file offset\n",
                sig.export_name, target_rva
            );
            continue;
        };

        println!(
            "  === {} → .tvm0 RVA 0x{:08X} (file:0x{:08X}) ===\n",
            sig.export_name, target_rva, file_off
        );

        // Raw hex dump of first 128 bytes
        if let Some(block) = read_at(data, file_off, 128) {
            println!("    Raw bytes (128):");
            hex_dump(block, file_off, 128);
        }

        // Try disassembling as x86-64
        let decode_len = 512.min(data.len().saturating_sub(file_off));
        let code = &data[file_off..file_off + decode_len];
        let rip = pe.image_base + target_rva as u64;
        let mut decoder = Decoder::with_ip(64, code, rip, DecoderOptions::NONE);
        let mut instr = Instruction::default();

        println!("\n    x86-64 disassembly (first 60 instructions):");
        let mut valid_count = 0;
        let mut invalid_count = 0;
        let mut first_invalid_ip = None;
        let mut _last_valid_ip = 0u64;
        let mut has_stack_setup = false;
        let mut has_pushfq = false;
        let mut calls = Vec::new();
        let mut n = 0;

        while decoder.can_decode() && n < 60 {
            decoder.decode_out(&mut instr);
            n += 1;

            if instr.is_invalid() {
                invalid_count += 1;
                if first_invalid_ip.is_none() {
                    first_invalid_ip = Some(instr.ip());
                }
                if n <= 40 {
                    println!("    {:016X}  <invalid>", instr.ip());
                }
                continue;
            }

            valid_count += 1;
            _last_valid_ip = instr.ip();

            if n <= 40 {
                println!("    {:016X}  {}", instr.ip(), format_instr(&instr));
            }

            // Track patterns
            match instr.mnemonic() {
                Mnemonic::Lea
                    if instr.op_count() >= 2
                        && instr.op_kind(0) == OpKind::Register
                        && instr.op_register(0) == Register::RSP =>
                {
                    has_stack_setup = true;
                }
                Mnemonic::Pushfq => {
                    has_pushfq = true;
                }
                Mnemonic::Call => {
                    if instr.op_kind(0) == OpKind::NearBranch64 {
                        let target = instr.near_branch64();
                        let target_rva = (target - pe.image_base) as u32;
                        let target_sec = pe
                            .find_section_for_rva(target_rva)
                            .map(|s| s.name.as_str())
                            .unwrap_or("???");
                        calls.push((target, target_sec.to_string()));
                    }
                }
                _ => {}
            }

            // Stop at ret if we've decoded enough
            if instr.mnemonic() == Mnemonic::Ret && valid_count > 10 {
                break;
            }
        }

        println!(
            "\n    Decode stats: {} valid, {} invalid instructions",
            valid_count, invalid_count
        );
        if valid_count > 0 && invalid_count == 0 {
            println!("    \x1b[1;32mPure x86-64 code — NOT raw VM bytecode!\x1b[0m");
        } else if valid_count > invalid_count * 2 {
            println!("    \x1b[1;33mMostly valid x86-64 with some data interleaved\x1b[0m");
        } else if invalid_count > valid_count * 2 {
            println!("    \x1b[1;31mMostly invalid — likely raw VM bytecode\x1b[0m");
        }

        if has_stack_setup {
            println!("    Stack frame setup: YES (lea rsp pattern)");
        }
        if has_pushfq {
            println!("    PUSHFQ detected: VM entry signature");
        }
        if let Some(inv_ip) = first_invalid_ip {
            println!(
                "    First invalid at: 0x{:016X} ({} bytes in)",
                inv_ip,
                inv_ip - rip
            );
        }
        if !calls.is_empty() {
            println!("    Call targets:");
            for (target, sec) in &calls {
                println!("      0x{:016X} → {}", target, sec);
            }
        }
        println!();
    }
}

// ══════════════════════════════════════════════════════════════
// Pass 8: Byte Frequency & Stride Analysis
// ══════════════════════════════════════════════════════════════

fn pass8_frequency_analysis(data: &[u8], pe: &PeInfo, signatures: &[InferredSignature]) {
    println!("\n╔══════════════════════════════════════════════════════════════╗");
    println!("║  Pass 8: Byte Frequency & Stride Analysis                  ║");
    println!("╚══════════════════════════════════════════════════════════════╝\n");

    if !pe.sections.iter().any(|s| s.name == ".tvm0") {
        println!("  .tvm0 section not found — skipping");
        return;
    }

    for sig in signatures {
        let Some(target_rva) = sig.jmp_target_rva else {
            continue;
        };
        let Some(file_off) = pe.rva_to_file_offset(target_rva) else {
            continue;
        };

        // Analyze 4KB window at each entry point
        let window_size = 4096;
        let end = (file_off + window_size).min(data.len());
        let window = &data[file_off..end];

        println!(
            "  === {} (4KB window at file:0x{:08X}) ===\n",
            sig.export_name, file_off
        );

        // Byte frequency distribution
        let mut freq = [0u32; 256];
        for &b in window {
            freq[b as usize] += 1;
        }

        // Top 20 most common bytes
        let mut ranked: Vec<(u8, u32)> = freq
            .iter()
            .enumerate()
            .map(|(i, &c)| (i as u8, c))
            .collect();
        ranked.sort_by(|a, b| b.1.cmp(&a.1));

        println!("    Top 15 bytes:");
        for &(byte, count) in ranked.iter().take(15) {
            let pct = count as f64 / window.len() as f64 * 100.0;
            let bar_len = (pct * 2.0) as usize;
            println!(
                "      0x{:02X}: {:4} ({:5.2}%) {}",
                byte,
                count,
                pct,
                "█".repeat(bar_len.min(40))
            );
        }

        let unique_bytes = freq.iter().filter(|&&c| c > 0).count();
        let ent = entropy(window);
        println!("    Unique bytes: {}/256", unique_bytes);
        println!("    Shannon entropy: {:.3}", ent);

        // Stride analysis: for each candidate instruction width,
        // check entropy of the first byte at each stride offset
        println!("\n    Stride analysis (opcode field entropy):");
        println!(
            "    {:>6} {:>8} {:>10} {:>10}",
            "Width", "Unique", "MaxFreq%", "Entropy"
        );
        println!("    {}", "-".repeat(40));

        for width in [1, 2, 3, 4, 5, 6, 8, 10, 12, 16] {
            let mut stride_freq = [0u32; 256];
            let mut count = 0u32;
            let mut i = 0;
            while i < window.len() {
                stride_freq[window[i] as usize] += 1;
                count += 1;
                i += width;
            }
            if count == 0 {
                continue;
            }

            let stride_unique = stride_freq.iter().filter(|&&c| c > 0).count();
            let max_freq = *stride_freq.iter().max().unwrap_or(&0);
            let max_pct = max_freq as f64 / count as f64 * 100.0;
            let stride_ent = {
                let cf = count as f64;
                stride_freq
                    .iter()
                    .filter(|&&c| c > 0)
                    .map(|&c| {
                        let p = c as f64 / cf;
                        -p * p.log2()
                    })
                    .sum::<f64>()
            };

            let marker = if stride_ent < 6.0 && stride_unique < 200 {
                " ← candidate"
            } else {
                ""
            };
            println!(
                "    {:>6} {:>8} {:>9.1}% {:>10.3}{}",
                width, stride_unique, max_pct, stride_ent, marker
            );
        }

        // Check for repeated patterns (fixed-size instructions)
        println!("\n    N-gram analysis (2-4 byte patterns):");
        for n in [2, 3, 4] {
            let mut ngram_freq: BTreeMap<Vec<u8>, u32> = BTreeMap::new();
            for chunk in window.windows(n) {
                *ngram_freq.entry(chunk.to_vec()).or_default() += 1;
            }
            let mut top: Vec<_> = ngram_freq.into_iter().collect();
            top.sort_by(|a, b| b.1.cmp(&a.1));
            let total = window.len().saturating_sub(n - 1);
            print!("      {}-byte: top=", n);
            for (pattern, count) in top.iter().take(5) {
                let hex: String = pattern
                    .iter()
                    .map(|b| format!("{:02x}", b))
                    .collect::<Vec<_>>()
                    .join("");
                print!(
                    " {}×{} ({:.1}%)",
                    hex,
                    count,
                    *count as f64 / total as f64 * 100.0
                );
            }
            println!();
        }
        println!();
    }
}

// ══════════════════════════════════════════════════════════════
// Pass 9: S-Box & Constant Scanning in .tvm0 Bytecode
// ══════════════════════════════════════════════════════════════

fn pass9_tvm0_constants(
    data: &[u8],
    pe: &PeInfo,
    ga_data: Option<&[u8]>,
    signatures: &[InferredSignature],
) {
    println!("\n╔══════════════════════════════════════════════════════════════╗");
    println!("║  Pass 9: S-Box & Constant Scanning in .tvm0 Bytecode      ║");
    println!("╚══════════════════════════════════════════════════════════════╝\n");

    let Some(tvm0) = pe.sections.iter().find(|s| s.name == ".tvm0") else {
        println!("  .tvm0 section not found — skipping");
        return;
    };

    let tvm0_range = tvm0.file_range();
    let tvm0_end = tvm0_range.end.min(data.len());
    let tvm0_start = tvm0_range.start.min(tvm0_end);
    let tvm0_data = &data[tvm0_start..tvm0_end];

    println!(
        "  .tvm0: {} at file offset 0x{:08X}\n",
        format_bytes(tvm0_data.len() as u64),
        tvm0_start
    );

    // Scan for S-box references (VAs from GameAssembly.dll)
    if let Some(ga) = ga_data {
        println!("  === S-Box VA References in .tvm0 ===\n");

        let ga_pe = parse_pe(ga);
        let ga_base = ga_pe.image_base;

        // Check each S-box as a VA reference embedded in .tvm0
        for (idx, &sbox_off) in SBOX_FILE_OFFSETS.iter().enumerate() {
            let sbox_rva = ga_pe.file_offset_to_rva(sbox_off).unwrap_or(0);
            let sbox_va = ga_base + sbox_rva as u64;
            let va_bytes = sbox_va.to_le_bytes();
            let rva_bytes = sbox_rva.to_le_bytes();

            let mut va_hits = 0;
            let mut rva_hits = 0;

            // Search for 8-byte VA
            for i in 0..tvm0_data.len().saturating_sub(8) {
                if tvm0_data[i..i + 8] == va_bytes {
                    va_hits += 1;
                    if va_hits <= 3 {
                        println!(
                            "    S-box {} VA 0x{:016X} at .tvm0+0x{:06X}",
                            idx, sbox_va, i
                        );
                    }
                }
            }

            // Search for 4-byte RVA
            for i in 0..tvm0_data.len().saturating_sub(4) {
                if tvm0_data[i..i + 4] == rva_bytes {
                    rva_hits += 1;
                    if rva_hits <= 3 {
                        println!(
                            "    S-box {} RVA 0x{:08X} at .tvm0+0x{:06X}",
                            idx, sbox_rva, i
                        );
                    }
                }
            }

            if va_hits == 0 && rva_hits == 0 {
                println!("    S-box {} — not referenced", idx);
            } else if va_hits > 3 || rva_hits > 3 {
                println!(
                    "    S-box {} — {} VA hits, {} RVA hits total",
                    idx, va_hits, rva_hits
                );
            }
        }
    }

    // Scan for HGP.dll's own section VAs referenced in .tvm0
    // (self-references reveal data layout)
    println!("\n  === Self-References (.tvm0 → HGP sections) ===\n");
    let hgp_base = pe.image_base;
    for sec in &pe.sections {
        if sec.name == ".tvm0" {
            continue;
        }
        let sec_va = hgp_base + sec.virtual_address as u64;
        let va_bytes = sec_va.to_le_bytes();
        let mut hits = 0;
        for i in 0..tvm0_data.len().saturating_sub(8) {
            if tvm0_data[i..i + 8] == va_bytes {
                hits += 1;
                if hits <= 3 {
                    println!(
                        "    .tvm0+0x{:06X} → {} base VA 0x{:016X}",
                        i, sec.name, sec_va
                    );
                }
            }
        }
        if hits > 3 {
            println!("    .tvm0 → {} base: {} total refs", sec.name, hits);
        }
    }

    // Scan for interesting constant patterns near each export's entry point
    println!("\n  === Constants Near Export Entry Points ===\n");
    for sig in signatures {
        let Some(target_rva) = sig.jmp_target_rva else {
            continue;
        };
        let Some(file_off) = pe.rva_to_file_offset(target_rva) else {
            continue;
        };

        // Check 8KB window around entry
        let window_start = file_off.saturating_sub(tvm0_start);
        let window_end = (window_start + 8192).min(tvm0_data.len());
        let window = &tvm0_data[window_start..window_end];

        println!(
            "    {} (8KB window at .tvm0+0x{:06X}):",
            sig.export_name, window_start
        );

        // Look for 8-byte values that look like VAs (0x180xxxxxxx)
        let mut va_refs = Vec::new();
        for i in 0..window.len().saturating_sub(8) {
            if let Some(val) = read_u64_at(window, i) {
                // HGP VA range
                if (hgp_base..hgp_base + 0x1000000).contains(&val) {
                    let ref_rva = (val - hgp_base) as u32;
                    let target_sec = pe
                        .find_section_for_rva(ref_rva)
                        .map(|s| s.name.as_str())
                        .unwrap_or("???");
                    va_refs.push((i, val, target_sec.to_string()));
                }
            }
        }

        if va_refs.is_empty() {
            println!("      No HGP VA references found");
        } else {
            println!("      {} HGP VA references:", va_refs.len());
            for (i, va, sec) in va_refs.iter().take(10) {
                println!("        +0x{:04X}: 0x{:016X} → {}", i, va, sec);
            }
            if va_refs.len() > 10 {
                println!("        ... and {} more", va_refs.len() - 10);
            }
        }

        // Look for potential inline keys: 16/32-byte blocks with entropy 3.0-7.0
        let mut key_candidates = Vec::new();
        for key_len in [16, 32] {
            for i in (0..window.len().saturating_sub(key_len)).step_by(8) {
                let block = &window[i..i + key_len];
                let ent = entropy(block);
                let unique = {
                    let mut seen = [false; 256];
                    let mut u = 0;
                    for &b in block {
                        if !seen[b as usize] {
                            u += 1;
                            seen[b as usize] = true;
                        }
                    }
                    u
                };
                // Key-like: moderate entropy, not all same byte, not sequential
                if ent > 3.0 && ent < 7.5 && unique > key_len / 2 {
                    let is_zero = block.iter().all(|&b| b == 0);
                    let is_sequential = block.windows(2).all(|w| w[1] == w[0].wrapping_add(1));
                    if !is_zero && !is_sequential {
                        key_candidates.push((i, key_len, ent, block.to_vec()));
                    }
                }
            }
        }

        // Deduplicate nearby candidates
        let mut deduped: Vec<(usize, usize, f64, Vec<u8>)> = Vec::new();
        for cand in &key_candidates {
            if !deduped.iter().any(|d| d.0.abs_diff(cand.0) < 16) {
                deduped.push(cand.clone());
            }
        }

        if !deduped.is_empty() {
            println!(
                "      Potential key material ({} candidates after dedup):",
                deduped.len()
            );
            for (i, len, ent, block) in deduped.iter().take(5) {
                let hex: String = block
                    .iter()
                    .take(16)
                    .map(|b| format!("{:02x}", b))
                    .collect::<Vec<_>>()
                    .join(" ");
                println!(
                    "        +0x{:04X} ({}B, ent={:.2}): {}{}",
                    i,
                    len,
                    ent,
                    hex,
                    if *len > 16 { " ..." } else { "" }
                );
            }
        }
        println!();
    }

    // Function boundary detection: scan for common prologue patterns
    println!("  === Function Boundary Detection in .tvm0 ===\n");

    // Pattern 1: lea rsp,[rsp+XX] (VM entry)
    let mut vm_entries = Vec::new();
    for i in 0..tvm0_data.len().saturating_sub(4) {
        if tvm0_data[i..i + 4] == *VM_ENTRY_PREFIX {
            vm_entries.push(i);
        }
    }
    println!(
        "    VM entry pattern (lea rsp,[rsp+XX]): {} occurrences",
        vm_entries.len()
    );
    if !vm_entries.is_empty() {
        println!("    First 10:");
        for &off in vm_entries.iter().take(10) {
            let rva = tvm0.virtual_address + off as u32;
            let is_export = signatures.iter().any(|s| s.jmp_target_rva == Some(rva));
            println!(
                "      .tvm0+0x{:06X} (RVA 0x{:08X}){}",
                off,
                rva,
                if is_export { " ← EXPORT TARGET" } else { "" }
            );
        }
    }

    // Pattern 2: pushfq (flags save, common in VM prologues)
    let pushfq_count = tvm0_data
        .iter()
        .enumerate()
        .filter(|&(i, &b)| b == 0x9C && i > 0 && tvm0_data[i - 1] != 0x0F) // 0x9C = pushfq (not pushfd prefix)
        .count();
    println!("    PUSHFQ (0x9C): {} occurrences", pushfq_count);

    // Pattern 3: INT3 runs (function padding)
    let mut int3_runs = 0;
    let mut i = 0;
    while i < tvm0_data.len() {
        if tvm0_data[i] == 0xCC {
            let start = i;
            while i < tvm0_data.len() && tvm0_data[i] == 0xCC {
                i += 1;
            }
            if i - start >= 4 {
                int3_runs += 1;
            }
        } else {
            i += 1;
        }
    }
    println!("    INT3 padding runs (≥4): {}", int3_runs);

    // Pattern 4: RET instructions
    let ret_count = tvm0_data.iter().filter(|&&b| b == 0xC3).count();
    println!("    RET (0xC3): {} occurrences", ret_count);

    if int3_runs > 100 && ret_count > 100 {
        println!(
            "\n    \x1b[1;32mConclusion: .tvm0 contains NATIVE x86-64 code with function boundaries\x1b[0m"
        );
        println!("    Not raw VM bytecode — obfuscated native code in a custom section.");
        println!("    Standalone execution should work IF DllMain initializes correctly.");
    } else if vm_entries.is_empty() && int3_runs < 10 {
        println!("\n    \x1b[1;31mConclusion: .tvm0 appears to be raw VM bytecode\x1b[0m");
        println!("    Needs external VM dispatcher (.Sgxm0) to execute.");
    }
}

// ══════════════════════════════════════════════════════════════
// Main
// ══════════════════════════════════════════════════════════════

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("=== Phase R2: HGP.dll Standalone Probe ===\n");

    // Load HGP.dll
    println!("Loading {}...", HGP_PATH);
    let hgp_data = fs::read(HGP_PATH)?;
    println!(
        "Loaded {} ({})\n",
        format_bytes(hgp_data.len() as u64),
        hgp_data.len()
    );

    // Pass 1: Import table analysis (go/no-go)
    let verdict = pass1_import_analysis(&hgp_data);

    // Pass 2: Section map + self-containment
    let section_info = pass2_section_map(&hgp_data);

    // Pass 3: Export signature inference
    let signatures = pass3_export_signatures(&hgp_data, &section_info.pe);

    // Pass 4: PInvoke descriptor search in GameAssembly.dll
    let pinvoke_matches = if let Ok(ga_data) = fs::read(GA_PATH) {
        println!(
            "\nLoaded GameAssembly.dll: {} ({})",
            format_bytes(ga_data.len() as u64),
            ga_data.len()
        );
        let ga_pe = parse_pe(&ga_data);
        pass4_pinvoke_descriptors(&ga_data, &ga_pe, &section_info.pe)
    } else {
        println!("\n  WARNING: {} not found — skipping Pass 4", GA_PATH);
        Vec::new()
    };

    // Pass 5: String & constant survey
    let ga_data_opt = fs::read(GA_PATH).ok();
    pass5_strings_and_constants(&hgp_data, &section_info.pe, ga_data_opt.as_deref());

    // Pass 6: Generate Python prober
    pass6_generate_prober(&verdict, &section_info, &signatures, &pinvoke_matches);

    // Pass 7: .tvm0 bytecode disassembly at JMP targets
    pass7_tvm0_disassembly(&hgp_data, &section_info.pe, &signatures);

    // Pass 8: Byte frequency & stride analysis
    pass8_frequency_analysis(&hgp_data, &section_info.pe, &signatures);

    // Pass 9: S-box & constant scanning in .tvm0
    pass9_tvm0_constants(
        &hgp_data,
        &section_info.pe,
        ga_data_opt.as_deref(),
        &signatures,
    );

    // ── Summary ──
    println!("\n╔══════════════════════════════════════════════════════════════╗");
    println!("║  Summary                                                    ║");
    println!("╚══════════════════════════════════════════════════════════════╝\n");

    println!(
        "  Standalone loading: {}",
        if verdict.can_load_standalone {
            "\x1b[1;32mFEASIBLE\x1b[0m"
        } else {
            "\x1b[1;31mBLOCKED\x1b[0m"
        }
    );
    println!(
        "  Imports: {} system, {} CRT, {} game-specific",
        verdict.system_imports.len(),
        verdict.crt_imports.len(),
        verdict.game_imports.len()
    );
    if !verdict.game_imports.is_empty() {
        println!(
            "  Blocking DLLs: {}",
            verdict
                .game_imports
                .iter()
                .map(|(n, _)| n.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    println!("  TLS callbacks: {}", verdict.has_tls_callbacks);
    println!(
        "  VM sections: .Sgxm0={} .Sgxm1={} .tvm0={}",
        section_info.has_sgxm0, section_info.has_sgxm1, section_info.has_tvm0
    );
    println!("  Self-contained VM: {}", section_info.is_self_contained);
    println!("  Exports analyzed: {}", signatures.len());
    for sig in &signatures {
        let argc = pinvoke_matches
            .iter()
            .find(|m| m.export_name == sig.export_name)
            .and_then(|m| m.arg_count)
            .unwrap_or(sig.param_count as u32);
        println!(
            "    {} — {} params, {}, target: {}",
            sig.export_name,
            argc,
            sig.return_hint,
            sig.jmp_target_section.as_deref().unwrap_or("native")
        );
    }
    println!("  PInvoke descriptors: {}", pinvoke_matches.len());
    println!("  Python prober: ./output/hgp_prober.py");

    println!("\n  Next steps:");
    if verdict.can_load_standalone {
        println!("    1. Copy hgp_prober.py + HGP.dll to Windows");
        println!("    2. python hgp_prober.py");
        println!("    3. Review output for cipher behavior");
    } else {
        println!("    1. Copy hgp_prober.py + HGP.dll + game DLLs to Windows");
        println!("    2. Set game_dir path in hgp_prober.py");
        println!("    3. python hgp_prober.py --dll HGP.dll");
    }

    println!("\n=== Analysis Complete ===");
    Ok(())
}
