/// Search GameAssembly.dll for the :)xD magic constant (0x3A297844).
/// Analyzes surrounding bytes for x86_64 instruction context.
use std::fs;

const SMILEY_MAGIC_LE: [u8; 4] = [0x44, 0x78, 0x29, 0x3A]; // little-endian 0x3A297844
const SMILEY_MAGIC_BE: [u8; 4] = [0x3A, 0x29, 0x78, 0x44]; // big-endian / raw byte order

fn hex_dump(data: &[u8], offset: usize, highlight_start: usize, highlight_len: usize) {
    let dump_len = data.len();
    for row in 0..dump_len.div_ceil(16) {
        let off = row * 16;
        if off >= dump_len {
            break;
        }
        print!("  {:08x}: ", offset + off);
        for i in 0..16 {
            if off + i < dump_len {
                let is_highlight =
                    (off + i) >= highlight_start && (off + i) < highlight_start + highlight_len;
                if is_highlight {
                    print!("\x1b[1;31m{:02x}\x1b[0m ", data[off + i]);
                } else {
                    print!("{:02x} ", data[off + i]);
                }
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

/// Analyze bytes preceding a magic match for x86 instruction context.
fn analyze_instruction_context(data: &[u8], match_offset: usize) -> String {
    let mut hints = Vec::new();

    // Check for CMP EAX, imm32 (opcode 3D)
    if match_offset >= 1 && data[match_offset - 1] == 0x3D {
        hints.push(format!(
            "CMP EAX, 0x3A297844 at offset {:08x}",
            match_offset - 1
        ));
    }

    // Check for MOV r32, imm32 (opcodes B8-BF)
    if match_offset >= 1 {
        let prev = data[match_offset - 1];
        if (0xB8..=0xBF).contains(&prev) {
            let reg =
                ["EAX", "ECX", "EDX", "EBX", "ESP", "EBP", "ESI", "EDI"][(prev - 0xB8) as usize];
            hints.push(format!(
                "MOV {}, 0x3A297844 at offset {:08x}",
                reg,
                match_offset - 1
            ));
        }
    }

    // Check for CMP r/m32, imm32 (opcode 81 /7 = 81 F8+reg)
    if match_offset >= 2 {
        let op1 = data[match_offset - 2];
        let op2 = data[match_offset - 1];
        if op1 == 0x81 {
            let modrm_reg = (op2 >> 3) & 7;
            let modrm_rm = op2 & 7;
            let modrm_mod = (op2 >> 6) & 3;
            let op_name = match modrm_reg {
                0 => "ADD",
                1 => "OR",
                4 => "AND",
                5 => "SUB",
                6 => "XOR",
                7 => "CMP",
                _ => "ALU",
            };
            if modrm_mod == 3 {
                let reg =
                    ["EAX", "ECX", "EDX", "EBX", "ESP", "EBP", "ESI", "EDI"][modrm_rm as usize];
                hints.push(format!(
                    "{} {}, 0x3A297844 at offset {:08x}",
                    op_name,
                    reg,
                    match_offset - 2
                ));
            } else {
                hints.push(format!(
                    "{} [mem], 0x3A297844 at offset {:08x} (ModR/M: {:02x})",
                    op_name,
                    match_offset - 2,
                    op2
                ));
            }
        }
    }

    // Check for REX.W prefix variants (64-bit)
    if match_offset >= 3 {
        let op0 = data[match_offset - 3];
        let op1 = data[match_offset - 2];
        let op2 = data[match_offset - 1];
        // REX prefix (0x40-0x4F) + 81 + ModRM
        if (0x40..=0x4F).contains(&op0) && op1 == 0x81 {
            let rex_w = (op0 & 0x08) != 0;
            let rex_b = (op0 & 0x01) != 0;
            let modrm_reg = (op2 >> 3) & 7;
            let modrm_rm = op2 & 7;
            let modrm_mod = (op2 >> 6) & 3;
            let op_name = match modrm_reg {
                7 => "CMP",
                0 => "ADD",
                5 => "SUB",
                6 => "XOR",
                _ => "ALU",
            };
            let size = if rex_w { "64" } else { "32" };
            let rm_idx = if rex_b { modrm_rm + 8 } else { modrm_rm };
            if modrm_mod == 3 {
                let regs64 = [
                    "RAX", "RCX", "RDX", "RBX", "RSP", "RBP", "RSI", "RDI", "R8", "R9", "R10",
                    "R11", "R12", "R13", "R14", "R15",
                ];
                let reg_name = regs64.get(rm_idx as usize).unwrap_or(&"R??");
                hints.push(format!(
                    "REX {} {}{}, 0x3A297844 at offset {:08x}",
                    op_name,
                    reg_name,
                    if rex_w {
                        format!(" ({}bit)", size)
                    } else {
                        String::new()
                    },
                    match_offset - 3
                ));
            }
        }
    }

    if hints.is_empty() {
        // Show preceding bytes for manual analysis
        let start = match_offset.saturating_sub(4);
        let prefix: Vec<String> = data[start..match_offset]
            .iter()
            .map(|b| format!("{:02x}", b))
            .collect();
        format!(
            "No recognized instruction pattern. Preceding bytes: {}",
            prefix.join(" ")
        )
    } else {
        hints.join("; ")
    }
}

fn search_pattern(data: &[u8], pattern: &[u8], label: &str) -> Vec<usize> {
    let mut matches = Vec::new();
    if data.len() < pattern.len() {
        return matches;
    }
    for i in 0..=(data.len() - pattern.len()) {
        if data[i..i + pattern.len()] == *pattern {
            matches.push(i);
        }
    }
    println!("\n=== {} matches: {} ===", label, matches.len());
    for &offset in &matches {
        println!("\n  Match at offset 0x{:08x} ({}):", offset, offset);

        // Instruction analysis
        let analysis = analyze_instruction_context(data, offset);
        println!("  Instruction context: {}", analysis);

        // Dump ±64 bytes
        let ctx_start = offset.saturating_sub(64);
        let ctx_end = (offset + pattern.len() + 64).min(data.len());
        let highlight_in_ctx = offset - ctx_start;
        hex_dump(
            &data[ctx_start..ctx_end],
            ctx_start,
            highlight_in_ctx,
            pattern.len(),
        );
    }
    matches
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = "./output/GameAssembly.dll";
    println!("=== Magic Constant Search (0x3A297844) ===\n");
    println!("Loading {}...", path);

    let data = fs::read(path)?;
    println!(
        "Loaded {} bytes ({:.1} MB)\n",
        data.len(),
        data.len() as f64 / (1024.0 * 1024.0)
    );

    // Search for little-endian encoding (how x86 stores 32-bit immediates)
    let le_matches = search_pattern(&data, &SMILEY_MAGIC_LE, "Little-endian (44 78 29 3A)");

    // Search for big-endian / raw byte sequence (in data sections)
    let be_matches = search_pattern(&data, &SMILEY_MAGIC_BE, "Big-endian (3A 29 78 44)");

    println!("\n=== Summary ===");
    println!(
        "  LE matches (instruction immediates): {}",
        le_matches.len()
    );
    println!("  BE matches (data sections): {}", be_matches.len());
    println!("  Total: {}", le_matches.len() + be_matches.len());

    // Categorize LE matches by instruction type
    if !le_matches.is_empty() {
        println!("\n=== Instruction Classification ===");
        for &offset in &le_matches {
            let analysis = analyze_instruction_context(&data, offset);
            println!("  0x{:08x}: {}", offset, analysis);
        }
    }

    println!("\n=== Done ===");
    Ok(())
}
