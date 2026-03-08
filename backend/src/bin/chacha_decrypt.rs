/// Download BLC catalogs and decrypt encrypted VFS directories using ChaCha20.
use backend::extraction::{pipeline, sparkbuffer};
use reqwest::Client;
use std::io::Write;

// ChaCha20 constants
const CHACHA_KEY_B64: &str = "6VsxesT4KFadI6hr8nHctT6Eb6dckk1nHbqOOPTKUuE=";

fn rotl32(v: u32, n: u32) -> u32 {
    (v << n) | (v >> (32 - n))
}

fn quarter_round(s: &mut [u32; 16], a: usize, b: usize, c: usize, d: usize) {
    s[a] = s[a].wrapping_add(s[b]); s[d] ^= s[a]; s[d] = rotl32(s[d], 16);
    s[c] = s[c].wrapping_add(s[d]); s[b] ^= s[c]; s[b] = rotl32(s[b], 12);
    s[a] = s[a].wrapping_add(s[b]); s[d] ^= s[a]; s[d] = rotl32(s[d], 8);
    s[c] = s[c].wrapping_add(s[d]); s[b] ^= s[c]; s[b] = rotl32(s[b], 7);
}

fn chacha20_block(key: &[u8; 32], counter: u32, nonce: &[u8; 12]) -> [u8; 64] {
    let mut state: [u32; 16] = [0; 16];
    state[0] = 0x61707865; state[1] = 0x3320646e;
    state[2] = 0x79622d32; state[3] = 0x6b206574;
    for i in 0..8 {
        state[4 + i] = u32::from_le_bytes(key[i*4..i*4+4].try_into().unwrap());
    }
    state[12] = counter;
    for i in 0..3 {
        state[13 + i] = u32::from_le_bytes(nonce[i*4..i*4+4].try_into().unwrap());
    }
    let orig = state;
    for _ in 0..10 {
        quarter_round(&mut state, 0, 4, 8, 12);
        quarter_round(&mut state, 1, 5, 9, 13);
        quarter_round(&mut state, 2, 6, 10, 14);
        quarter_round(&mut state, 3, 7, 11, 15);
        quarter_round(&mut state, 0, 5, 10, 15);
        quarter_round(&mut state, 1, 6, 11, 12);
        quarter_round(&mut state, 2, 7, 8, 13);
        quarter_round(&mut state, 3, 4, 9, 14);
    }
    let mut out = [0u8; 64];
    for i in 0..16 {
        let val = state[i].wrapping_add(orig[i]);
        out[i*4..i*4+4].copy_from_slice(&val.to_le_bytes());
    }
    out
}

fn chacha20_crypt(key: &[u8; 32], nonce: &[u8; 12], data: &[u8], counter: u32) -> Vec<u8> {
    let mut result = Vec::with_capacity(data.len());
    let blocks = (data.len() + 63) / 64;
    for i in 0..blocks {
        let ks = chacha20_block(key, counter + i as u32, nonce);
        let start = i * 64;
        let end = std::cmp::min(start + 64, data.len());
        for j in start..end {
            result.push(data[j] ^ ks[j - start]);
        }
    }
    result
}

/// Parse BLC metadata after decryption
struct BlcInfo {
    version: i32,
    group_cfg_name: String,
    group_cfg_hash: String,
    files: Vec<BlcFileEntry>,
}

struct BlcFileEntry {
    file_name: String,
    chunk_md5: String,
    offset: i64,
    len: i64,
    use_encrypt: bool,
    iv_seed: i64,
}

fn read_u16_str(data: &[u8], pos: &mut usize) -> String {
    let len = u16::from_le_bytes(data[*pos..*pos+2].try_into().unwrap()) as usize;
    *pos += 2;
    let s = String::from_utf8_lossy(&data[*pos..*pos+len]).to_string();
    *pos += len;
    s
}

fn read_md5(data: &[u8], pos: &mut usize) -> String {
    let bytes = &data[*pos..*pos+16];
    *pos += 16;
    bytes.iter().map(|b| format!("{:02x}", b)).collect()
}

fn parse_blc(raw: &[u8]) -> Result<BlcInfo, String> {
    // raw = nonce (12 bytes) + decrypted payload
    // FkArkEnd reads from position 0 of full file:
    //   pos 0-3: version = nonce first 4 bytes (e.g. 3)
    //   Seek(12) skips remaining nonce + first 4 decrypted bytes
    //   pos 16: groupCfgName
    let data = raw;
    let mut pos = 0;

    // version from nonce bytes (4 bytes)
    let version = i32::from_le_bytes(data[pos..pos+4].try_into().unwrap());
    pos += 4;

    // skip 12 bytes (remaining 8 nonce bytes + 4 unknown decrypted bytes)
    pos += 12;

    // groupCfgName (u16 len + utf8)
    let group_cfg_name = read_u16_str(data, &mut pos);

    // groupCfgHashName (8 bytes -> first 4 as u32 BE -> hex)
    let hash_bytes = &data[pos..pos+8];
    let hash_val = u32::from_be_bytes(hash_bytes[0..4].try_into().unwrap());
    let group_cfg_hash = format!("{:08X}", hash_val);
    pos += 8;

    // groupFileInfoNum (4 bytes)
    let _file_info_num = i32::from_le_bytes(data[pos..pos+4].try_into().unwrap());
    pos += 4;

    // groupChunksLength (8 bytes)
    let _chunks_length = i64::from_le_bytes(data[pos..pos+8].try_into().unwrap());
    pos += 8;

    // blockType (1 byte)
    let _block_type = data[pos];
    pos += 1;

    // chunkCount (4 bytes)
    let chunk_count = i32::from_le_bytes(data[pos..pos+4].try_into().unwrap());
    pos += 4;

    let mut all_files = Vec::new();

    for _ in 0..chunk_count {
        // md5Name (16 bytes)
        let _chunk_md5_name = read_md5(data, &mut pos);

        // contentMD5 (16 bytes)
        let _content_md5 = read_md5(data, &mut pos);

        // length (8 bytes)
        let _chunk_length = i64::from_le_bytes(data[pos..pos+8].try_into().unwrap());
        pos += 8;

        // blockType (1 byte)
        let _chunk_block_type = data[pos];
        pos += 1;

        // fileCount (4 bytes)
        let file_count = i32::from_le_bytes(data[pos..pos+4].try_into().unwrap());
        pos += 4;

        for _ in 0..file_count {
            // fileName (u16 len + utf8)
            let file_name = read_u16_str(data, &mut pos);

            // fileNameHash (8 bytes)
            let _file_name_hash = i64::from_le_bytes(data[pos..pos+8].try_into().unwrap());
            pos += 8;

            // fileChunkMD5Name (16 bytes)
            let chunk_md5 = read_md5(data, &mut pos);

            // fileDataMD5 (16 bytes)
            let _file_data_md5 = read_md5(data, &mut pos);

            // offset (8 bytes)
            let offset = i64::from_le_bytes(data[pos..pos+8].try_into().unwrap());
            pos += 8;

            // len (8 bytes)
            let len = i64::from_le_bytes(data[pos..pos+8].try_into().unwrap());
            pos += 8;

            // blockType (1 byte)
            let _file_block_type = data[pos];
            pos += 1;

            // bUseEncrypt (1 byte)
            let use_encrypt = data[pos] != 0;
            pos += 1;

            let mut iv_seed: i64 = 0;
            if use_encrypt {
                iv_seed = i64::from_le_bytes(data[pos..pos+8].try_into().unwrap());
                pos += 8;
            }

            all_files.push(BlcFileEntry {
                file_name,
                chunk_md5,
                offset,
                len,
                use_encrypt,
                iv_seed,
            });
        }
    }

    Ok(BlcInfo {
        version,
        group_cfg_name,
        group_cfg_hash,
        files: all_files,
    })
}

// VFS directories to decrypt
const ENCRYPTED_DIRS: &[&str] = &[
    "DAFE52C9", // IFixPatchOut
    "42A8FCA6", // Table (SparkBuffer)
    "775A31D1", // JsonData
    "19E3AE45", // Lua
    "D6E622F7", // ExtendData
    "1CDDBF1F", // BundleManifest
    "3C9D9D2D", // InitialExtendData
    "1EBAF5C6", // ?
    "E1E7D7CE", // AudioChinese
];

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    use base64::Engine;
    let key_bytes = base64::engine::general_purpose::STANDARD.decode(CHACHA_KEY_B64)?;
    let key: [u8; 32] = key_bytes.try_into().expect("key must be 32 bytes");
    let key_hex: String = key.iter().map(|b| format!("{:02x}", b)).collect();
    println!("ChaCha20 key: {}", key_hex);

    let client = Client::new();
    let resolved = pipeline::resolve_pipeline(&client, true).await?;
    println!("Version: {}", resolved.config.version);

    let output_dir = std::path::Path::new("./output/chacha_decrypted");
    std::fs::create_dir_all(output_dir)?;

    // Process each encrypted directory
    for dir_hash in ENCRYPTED_DIRS {
        println!("\n{}", "=".repeat(60));
        println!("=== Directory: {} ===", dir_hash);

        // Download BLC catalog
        let blc_name = format!("Endfield_Data/StreamingAssets/VFS/{}/{}.blc", dir_hash, dir_hash);
        println!("Downloading BLC: {}", blc_name);

        let blc_data = match pipeline::download_chk(&client, &resolved.cd, &blc_name).await {
            Ok(data) => data,
            Err(e) => {
                println!("  Failed to download BLC: {}", e);
                continue;
            }
        };
        println!("  BLC size: {} bytes", blc_data.len());

        // Debug: dump raw BLC
        print!("  Raw BLC hex: ");
        for b in blc_data.iter().take(73) {
            print!("{:02x}", b);
        }
        println!();

        // Decrypt BLC: nonce = first 12 bytes, counter = 1
        let mut nonce = [0u8; 12];
        nonce.copy_from_slice(&blc_data[..12]);
        let decrypted_blc_payload = chacha20_crypt(&key, &nonce, &blc_data[12..], 1);

        // Debug: dump decrypted payload
        print!("  Decrypted payload: ");
        for b in decrypted_blc_payload.iter().take(61) {
            print!("{:02x}", b);
        }
        println!();
        let ascii: String = decrypted_blc_payload.iter().take(61)
            .map(|&b| if (32..127).contains(&b) { b as char } else { '.' })
            .collect();
        println!("  ASCII: {}", ascii);

        // Reconstruct full BLC (nonce + decrypted payload)
        let mut full_blc = Vec::with_capacity(blc_data.len());
        full_blc.extend_from_slice(&blc_data[..12]);
        full_blc.extend_from_slice(&decrypted_blc_payload);

        // Parse BLC
        let blc = match parse_blc(&full_blc) {
            Ok(b) => b,
            Err(e) => {
                println!("  BLC parse error: {}", e);
                // Hex dump first 64 bytes of decrypted payload
                print!("  Decrypted: ");
                for b in decrypted_blc_payload.iter().take(64) {
                    print!("{:02x}", b);
                }
                println!();
                continue;
            }
        };

        println!("  Group: {} (hash: {})", blc.group_cfg_name, blc.group_cfg_hash);
        println!("  Version: {}", blc.version);
        println!("  Files: {}", blc.files.len());

        if blc.group_cfg_hash != *dir_hash {
            println!("  WARNING: hash mismatch! Expected {}, got {}", dir_hash, blc.group_cfg_hash);
        }

        // Create dir-specific output
        let dir_output = output_dir.join(dir_hash);
        std::fs::create_dir_all(&dir_output)?;

        // Save BLC metadata
        let meta_path = dir_output.join("blc_metadata.txt");
        let mut meta_file = std::fs::File::create(&meta_path)?;
        writeln!(meta_file, "Group: {} ({})", blc.group_cfg_name, blc.group_cfg_hash)?;
        writeln!(meta_file, "Version: {}", blc.version)?;
        writeln!(meta_file, "Files: {}", blc.files.len())?;
        for (i, f) in blc.files.iter().enumerate() {
            writeln!(meta_file, "\n[{}] {}", i, f.file_name)?;
            writeln!(meta_file, "  chunk: {}", f.chunk_md5)?;
            writeln!(meta_file, "  offset: {}, len: {}", f.offset, f.len)?;
            writeln!(meta_file, "  encrypt: {}, ivSeed: 0x{:016X}", f.use_encrypt, f.iv_seed)?;
        }

        // For each file, download its chunk and decrypt
        // Group files by chunk MD5 to avoid re-downloading
        let mut chunks_needed: std::collections::BTreeMap<String, Vec<usize>> = std::collections::BTreeMap::new();
        for (i, f) in blc.files.iter().enumerate() {
            chunks_needed.entry(f.chunk_md5.clone()).or_default().push(i);
        }

        for (chunk_md5, file_indices) in &chunks_needed {
            let chunk_name = format!(
                "Endfield_Data/StreamingAssets/VFS/{}/{}.chk",
                dir_hash,
                chunk_md5.to_uppercase()
            );

            // Try main ZIP first, then patch
            println!("\n  Downloading chunk: {}...", chunk_md5);

            let chunk_data = match pipeline::download_chk(&client, &resolved.cd, &chunk_name).await {
                Ok(data) => {
                    println!("    Downloaded {} bytes from main ZIP", data.len());
                    data
                }
                Err(_) => {
                    // Try patch
                    if let Some(patch) = &resolved.patch_index {
                        let patch_name = format!("{}/{}.chk", dir_hash, chunk_md5.to_uppercase());
                        if let Some(pf) = patch.files.iter().find(|p| p.name == patch_name) {
                            let main_res = resolved.config.resources.iter()
                                .find(|r| r.name == "main")
                                .expect("no main resource");
                            let url = format!("{}/{}", main_res.path, pf.name);
                            println!("    Downloading from patch: {}", url);
                            match client.get(&url).send().await {
                                Ok(resp) if resp.status().is_success() => {
                                    let data = resp.bytes().await?.to_vec();
                                    println!("    Downloaded {} bytes from patch", data.len());
                                    data
                                }
                                Ok(resp) => {
                                    println!("    Patch HTTP {}", resp.status());
                                    continue;
                                }
                                Err(e) => {
                                    println!("    Patch error: {}", e);
                                    continue;
                                }
                            }
                        } else {
                            println!("    Not found in patch either");
                            continue;
                        }
                    } else {
                        println!("    Not found");
                        continue;
                    }
                }
            };

            // Extract and decrypt each file from this chunk
            for &fi in file_indices {
                let f = &blc.files[fi];
                let offset = f.offset as usize;
                let len = f.len as usize;

                if offset + len > chunk_data.len() {
                    println!("    [{}] {} - offset/len out of bounds ({} + {} > {})",
                        fi, f.file_name, offset, len, chunk_data.len());
                    continue;
                }

                let file_data = &chunk_data[offset..offset + len];

                let decrypted = if f.use_encrypt {
                    // Construct nonce: [version as i32 LE (4 bytes)][ivSeed as i64 LE (8 bytes)]
                    let mut file_nonce = [0u8; 12];
                    file_nonce[0..4].copy_from_slice(&(blc.version as i32).to_le_bytes());
                    file_nonce[4..12].copy_from_slice(&f.iv_seed.to_le_bytes());

                    chacha20_crypt(&key, &file_nonce, file_data, 1)
                } else {
                    file_data.to_vec()
                };

                // Save decrypted file
                let safe_name = f.file_name.replace('/', "_").replace('\\', "_");
                let out_path = dir_output.join(&safe_name);
                std::fs::write(&out_path, &decrypted)?;

                // Analyze first bytes
                let first32: String = decrypted.iter().take(32)
                    .map(|b| format!("{:02x}", b))
                    .collect();
                let ascii: String = decrypted.iter().take(32)
                    .map(|&b| if (32..127).contains(&b) { b as char } else { '.' })
                    .collect();

                let enc_str = if f.use_encrypt { "ENCRYPTED" } else { "plain" };
                println!("    [{}] {} ({} bytes, {}) -> {}",
                    fi, f.file_name, len, enc_str, safe_name);
                println!("      hex: {}", first32);
                println!("      asc: {}", ascii);

                // Check for known formats
                if decrypted.len() >= 7 && &decrypted[..7] == b"UnityFS" {
                    println!("      *** UnityFS bundle detected! ***");
                }
                if decrypted.len() >= 1 && decrypted[0] == b'{' {
                    println!("      *** JSON detected! ***");
                }
                if decrypted.len() >= 4 && &decrypted[..4] == b":)xD" {
                    println!("      *** Wwise PCK detected! ***");
                }

                // Parse SparkBuffer .bytes files to JSON
                if f.file_name.ends_with(".bytes") {
                    match sparkbuffer::parse_sparkbuffer(&decrypted) {
                        Ok(json_val) => {
                            let json_name = format!("{}.json", safe_name.trim_end_matches(".bytes"));
                            let json_path = dir_output.join(&json_name);
                            let json_str = serde_json::to_string_pretty(&json_val).unwrap();
                            std::fs::write(&json_path, &json_str)?;
                            println!("      -> parsed to {}", json_name);
                        }
                        Err(e) => {
                            println!("      SparkBuffer parse error: {}", e);
                        }
                    }
                }
                // Save .json files as-is (they're already JSON)
                if f.file_name.ends_with(".json") && decrypted.len() >= 1 && decrypted[0] == b'{' {
                    // Already saved as raw file above, just note it
                    println!("      -> JSON file (already saved)");
                }
            }
        }
    }

    println!("\n\nDone! Output in {}", output_dir.display());
    Ok(())
}
