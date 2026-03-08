/// Test decryption of :)xD format (encrypted Wwise PCK) files.
use backend::extraction::utils::{format_bytes, hex_dump};
use backend::extraction::wwise_pck;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let files = [
        "07A1BB91_smallest.chk",
        "1EBAF5C6_smallest.chk",
        "A31457D0_smallest.chk",
        "E1E7D7CE_smallest.chk",
        "E9D31017_smallest.chk",
        "F668D4EE_smallest.chk",
        "24ED34CF_smallest.chk",
    ];

    println!("=== Wwise PCK (:)xD) Decryption Test ===\n");

    let mut success_count = 0;
    let mut total_extracted = 0;

    for fname in &files {
        let path = format!("./output/{}", fname);
        let data = match std::fs::read(&path) {
            Ok(d) => d,
            Err(e) => {
                println!("SKIP {}: {}", fname, e);
                continue;
            }
        };

        if !wwise_pck::is_smiley_format(&data) {
            println!("SKIP {}: not :)xD format", fname);
            continue;
        }

        let header_size = u32::from_le_bytes([data[4], data[5], data[6], data[7]]);
        println!(
            "--- {} ({}, header_size={}) ---",
            fname,
            format_bytes(data.len() as u64),
            header_size
        );

        let (decrypted, _) = match wwise_pck::decrypt_header(&data) {
            Ok(r) => r,
            Err(e) => {
                println!("  Decrypt error: {}", e);
                continue;
            }
        };

        let pck = match wwise_pck::parse_header(&decrypted, header_size) {
            Ok(p) => p,
            Err(e) => {
                println!("  Parse error: {}", e);
                continue;
            }
        };

        println!("  Languages: {:?}", pck.languages);
        println!(
            "  Banks: {}, Streams: {}, Externals: {}",
            pck.banks.len(),
            pck.streams.len(),
            pck.externals.len()
        );

        // Extract and verify first few files from each table
        let mut file_valid = 0;
        let mut file_total = 0;
        for (table_name, entries) in [
            ("Bank", &pck.banks),
            ("Stream", &pck.streams),
            ("Ext", &pck.externals),
        ] {
            for (i, entry) in entries.iter().enumerate().take(3) {
                file_total += 1;
                total_extracted += 1;
                match wwise_pck::extract_audio(&data, entry) {
                    Ok(audio) => {
                        let magic = if audio.len() >= 4 {
                            format!("{}", String::from_utf8_lossy(&audio[0..4]))
                        } else {
                            "???".into()
                        };
                        let valid = audio.len() >= 4
                            && (&audio[0..4] == b"BKHD" || &audio[0..4] == b"RIFF");
                        if valid {
                            file_valid += 1;
                            success_count += 1;
                        }
                        println!(
                            "  {}[{}] id=0x{:X} off={} sz={} → {} {}",
                            table_name,
                            i,
                            entry.id,
                            entry.offset,
                            entry.size,
                            magic,
                            if valid { "OK" } else { "???" }
                        );
                        if i == 0 {
                            hex_dump(&audio, 64);
                        }
                    }
                    Err(e) => println!("  {}[{}] error: {}", table_name, i, e),
                }
            }
        }
        println!("  Verified: {}/{} valid", file_valid, file_total);
        println!();
    }

    println!(
        "=== Done: {}/{} extracted files had valid headers ===",
        success_count, total_extracted
    );
    Ok(())
}
