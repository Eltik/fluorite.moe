/// BLC catalog parser and ChaCha20 VFS extraction pipeline.
///
/// BLC files are per-directory metadata catalogs listing all .chk files,
/// their offsets, sizes, encryption flags, and IV seeds.
use super::chacha20::{CHACHA_KEY, chacha20_crypt};
use super::pipeline::{self, ResolvedPipeline};
use super::sparkbuffer;
use reqwest::Client;
use std::io::Write;
use std::path::Path;

/// Parsed BLC catalog metadata.
pub struct BlcInfo {
    pub version: i32,
    pub group_cfg_name: String,
    pub group_cfg_hash: String,
    pub files: Vec<BlcFileEntry>,
}

/// A single file entry within a BLC catalog.
pub struct BlcFileEntry {
    pub file_name: String,
    pub chunk_md5: String,
    pub offset: i64,
    pub len: i64,
    pub use_encrypt: bool,
    pub iv_seed: i64,
}

/// All encrypted VFS directories.
pub const ENCRYPTED_DIRS: &[&str] = &[
    "DAFE52C9", // IFixPatchOut
    "42A8FCA6", // Table (SparkBuffer)
    "775A31D1", // JsonData
    "19E3AE45", // Lua
    "D6E622F7", // ExtendData
    "1CDDBF1F", // BundleManifest
    "3C9D9D2D", // InitialExtendData
    "1EBAF5C6", // Audio English
    "E1E7D7CE", // Audio Chinese
];

fn read_u16_str(data: &[u8], pos: &mut usize) -> String {
    let len = u16::from_le_bytes(data[*pos..*pos + 2].try_into().unwrap()) as usize;
    *pos += 2;
    let s = String::from_utf8_lossy(&data[*pos..*pos + len]).to_string();
    *pos += len;
    s
}

fn read_md5(data: &[u8], pos: &mut usize) -> String {
    let bytes = &data[*pos..*pos + 16];
    *pos += 16;
    bytes.iter().map(|b| format!("{:02x}", b)).collect()
}

/// Decrypt and parse a BLC catalog file.
pub fn parse_blc(raw_blc: &[u8]) -> Result<BlcInfo, String> {
    if raw_blc.len() < 16 {
        return Err("BLC too small".to_string());
    }

    // Decrypt: nonce = first 12 bytes, counter = 1
    let mut nonce = [0u8; 12];
    nonce.copy_from_slice(&raw_blc[..12]);
    let decrypted = chacha20_crypt(&CHACHA_KEY, &nonce, &raw_blc[12..], 1);

    // Reconstruct full data: nonce + decrypted payload
    // FkArkEnd reads from position 0: version = nonce[0:4], skip 12, then string
    let mut full = Vec::with_capacity(raw_blc.len());
    full.extend_from_slice(&raw_blc[..12]);
    full.extend_from_slice(&decrypted);
    let data = &full;

    let mut pos = 0;
    let version = i32::from_le_bytes(data[pos..pos + 4].try_into().unwrap());
    pos += 4;
    pos += 12; // skip remaining nonce + 4 unknown bytes

    let group_cfg_name = read_u16_str(data, &mut pos);

    let hash_val = u32::from_be_bytes(data[pos..pos + 4].try_into().unwrap());
    let group_cfg_hash = format!("{:08X}", hash_val);
    pos += 8;

    let _file_info_num = i32::from_le_bytes(data[pos..pos + 4].try_into().unwrap());
    pos += 4;
    let _chunks_length = i64::from_le_bytes(data[pos..pos + 8].try_into().unwrap());
    pos += 8;
    let _block_type = data[pos];
    pos += 1;
    let chunk_count = i32::from_le_bytes(data[pos..pos + 4].try_into().unwrap());
    pos += 4;

    let mut all_files = Vec::new();
    for _ in 0..chunk_count {
        let _chunk_md5_name = read_md5(data, &mut pos);
        let _content_md5 = read_md5(data, &mut pos);
        let _chunk_length = i64::from_le_bytes(data[pos..pos + 8].try_into().unwrap());
        pos += 8;
        let _chunk_block_type = data[pos];
        pos += 1;
        let file_count = i32::from_le_bytes(data[pos..pos + 4].try_into().unwrap());
        pos += 4;

        for _ in 0..file_count {
            let file_name = read_u16_str(data, &mut pos);
            let _file_name_hash = i64::from_le_bytes(data[pos..pos + 8].try_into().unwrap());
            pos += 8;
            let chunk_md5 = read_md5(data, &mut pos);
            let _file_data_md5 = read_md5(data, &mut pos);
            let offset = i64::from_le_bytes(data[pos..pos + 8].try_into().unwrap());
            pos += 8;
            let len = i64::from_le_bytes(data[pos..pos + 8].try_into().unwrap());
            pos += 8;
            let _file_block_type = data[pos];
            pos += 1;
            let use_encrypt = data[pos] != 0;
            pos += 1;
            let mut iv_seed: i64 = 0;
            if use_encrypt {
                iv_seed = i64::from_le_bytes(data[pos..pos + 8].try_into().unwrap());
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

/// Decrypt a single file from a chunk using its BLC entry metadata.
pub fn decrypt_file(chunk_data: &[u8], entry: &BlcFileEntry, version: i32) -> Option<Vec<u8>> {
    let offset = entry.offset as usize;
    let len = entry.len as usize;
    if offset + len > chunk_data.len() {
        return None;
    }
    let file_data = &chunk_data[offset..offset + len];
    if entry.use_encrypt {
        let mut nonce = [0u8; 12];
        nonce[0..4].copy_from_slice(&version.to_le_bytes());
        nonce[4..12].copy_from_slice(&entry.iv_seed.to_le_bytes());
        Some(chacha20_crypt(&CHACHA_KEY, &nonce, file_data, 1))
    } else {
        Some(file_data.to_vec())
    }
}

/// Result of extracting a single VFS directory.
pub struct DirExtractResult {
    pub dir_hash: String,
    pub group_name: String,
    pub total_files: usize,
    pub extracted: usize,
    pub sparkbuffer_parsed: usize,
    pub errors: Vec<String>,
}

/// Extract all files from a single encrypted VFS directory.
pub async fn extract_directory(
    client: &Client,
    resolved: &ResolvedPipeline,
    dir_hash: &str,
    output_dir: &Path,
) -> Result<DirExtractResult, Box<dyn std::error::Error>> {
    let blc_name = format!(
        "Endfield_Data/StreamingAssets/VFS/{}/{}.blc",
        dir_hash, dir_hash
    );
    let blc_data = pipeline::download_chk(client, &resolved.cd, &blc_name).await?;
    let blc = parse_blc(&blc_data).map_err(|e| format!("BLC parse error: {e}"))?;

    let dir_output = output_dir.join(dir_hash);
    std::fs::create_dir_all(&dir_output)?;

    // Save metadata
    let meta_path = dir_output.join("blc_metadata.txt");
    let mut meta_file = std::fs::File::create(&meta_path)?;
    writeln!(
        meta_file,
        "Group: {} ({})",
        blc.group_cfg_name, blc.group_cfg_hash
    )?;
    writeln!(meta_file, "Version: {}", blc.version)?;
    writeln!(meta_file, "Files: {}", blc.files.len())?;

    let mut result = DirExtractResult {
        dir_hash: dir_hash.to_string(),
        group_name: blc.group_cfg_name.clone(),
        total_files: blc.files.len(),
        extracted: 0,
        sparkbuffer_parsed: 0,
        errors: Vec::new(),
    };

    if blc.files.is_empty() {
        return Ok(result);
    }

    // Group files by chunk MD5
    let mut chunks_needed: std::collections::BTreeMap<String, Vec<usize>> =
        std::collections::BTreeMap::new();
    for (i, f) in blc.files.iter().enumerate() {
        chunks_needed
            .entry(f.chunk_md5.clone())
            .or_default()
            .push(i);
    }

    for (chunk_md5, file_indices) in &chunks_needed {
        let chunk_name = format!(
            "Endfield_Data/StreamingAssets/VFS/{}/{}.chk",
            dir_hash,
            chunk_md5.to_uppercase()
        );

        let chunk_data = match pipeline::download_chk(client, &resolved.cd, &chunk_name).await {
            Ok(data) => data,
            Err(_) => {
                // Try patch
                if let Some(patch) = &resolved.patch_index {
                    let patch_name = format!("{}/{}.chk", dir_hash, chunk_md5.to_uppercase());
                    if let Some(pf) = patch.files.iter().find(|p| p.name == patch_name) {
                        let main_res = resolved
                            .config
                            .resources
                            .iter()
                            .find(|r| r.name == "main")
                            .expect("no main resource");
                        let url = format!("{}/{}", main_res.path, pf.name);
                        match client.get(&url).send().await {
                            Ok(resp) if resp.status().is_success() => resp.bytes().await?.to_vec(),
                            _ => {
                                result.errors.push(format!("chunk {} not found", chunk_md5));
                                continue;
                            }
                        }
                    } else {
                        result.errors.push(format!("chunk {} not found", chunk_md5));
                        continue;
                    }
                } else {
                    result.errors.push(format!("chunk {} not found", chunk_md5));
                    continue;
                }
            }
        };

        for &fi in file_indices {
            let f = &blc.files[fi];
            let Some(decrypted) = decrypt_file(&chunk_data, f, blc.version) else {
                result
                    .errors
                    .push(format!("{}: offset/len out of bounds", f.file_name));
                continue;
            };

            let safe_name = f.file_name.replace(['/', '\\'], "_");
            let out_path = dir_output.join(&safe_name);
            std::fs::write(&out_path, &decrypted)?;
            result.extracted += 1;

            // Parse SparkBuffer .bytes → JSON
            if f.file_name.ends_with(".bytes") {
                match sparkbuffer::parse_sparkbuffer(&decrypted) {
                    Ok(json_val) => {
                        let json_name = format!("{}.json", safe_name.trim_end_matches(".bytes"));
                        let json_path = dir_output.join(&json_name);
                        let json_str = serde_json::to_string_pretty(&json_val).unwrap();
                        std::fs::write(&json_path, &json_str)?;
                        result.sparkbuffer_parsed += 1;
                    }
                    Err(e) => {
                        result
                            .errors
                            .push(format!("{}: SparkBuffer parse error: {}", f.file_name, e));
                    }
                }
            }
        }
    }

    Ok(result)
}
