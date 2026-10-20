//! BLC catalog parser and ChaCha20 VFS extraction pipeline.
//!
//! BLC files are per-directory metadata catalogs listing all .chk files,
//! their offsets, sizes, encryption flags, and IV seeds.
use super::chacha20::{CHACHA_KEY, chacha20_crypt};
use super::lua;
use super::memorypack;
use super::pipeline::{self, ResolvedPipeline, VFS_ROOT};
use super::sparkbuffer;
use crate::extraction::utils::{flat_filename, sanitize_filename, to_hex};
use reqwest::Client;
use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};

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

/// Bounds-checked little-endian reader over a decrypted BLC catalog.
struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, len: usize) -> Result<&'a [u8], String> {
        let end = self
            .pos
            .checked_add(len)
            .filter(|&end| end <= self.data.len())
            .ok_or_else(|| format!("BLC truncated at offset {} (need {len} bytes)", self.pos))?;
        let bytes = &self.data[self.pos..end];
        self.pos = end;
        Ok(bytes)
    }

    fn skip(&mut self, len: usize) -> Result<(), String> {
        self.take(len).map(|_| ())
    }

    fn u8(&mut self) -> Result<u8, String> {
        Ok(self.take(1)?[0])
    }

    fn u16(&mut self) -> Result<u16, String> {
        Ok(u16::from_le_bytes(self.take(2)?.try_into().unwrap()))
    }

    fn i32(&mut self) -> Result<i32, String> {
        Ok(i32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }

    fn i64(&mut self) -> Result<i64, String> {
        Ok(i64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }

    fn string(&mut self) -> Result<String, String> {
        let len = self.u16()? as usize;
        Ok(String::from_utf8_lossy(self.take(len)?).to_string())
    }

    fn md5(&mut self) -> Result<String, String> {
        Ok(to_hex(self.take(16)?))
    }

    fn count(&mut self) -> Result<usize, String> {
        let n = self.i32()?;
        usize::try_from(n).map_err(|_| format!("BLC negative count {n} at offset {}", self.pos - 4))
    }
}

/// Catalog layout revision. `V5` adds a header field before the group name and an extra
/// `i64` after each chunk/file block type.
#[derive(Clone, Copy, PartialEq)]
enum Layout {
    Legacy,
    V5,
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

    // The format revision sits at offset 12; try the layout it names first.
    let revision = i32::from_le_bytes(full[12..16].try_into().unwrap());
    let order = if revision >= 5 {
        [Layout::V5, Layout::Legacy]
    } else {
        [Layout::Legacy, Layout::V5]
    };
    parse_layout(&full, order[0]).or_else(|first| parse_layout(&full, order[1]).map_err(|_| first))
}

fn parse_layout(data: &[u8], layout: Layout) -> Result<BlcInfo, String> {
    let v5 = layout == Layout::V5;
    let mut r = Reader { data, pos: 0 };
    let version = r.i32()?;
    r.skip(if v5 { 16 } else { 12 })?;

    let group_cfg_name = r.string()?;
    let hash_val = u32::from_be_bytes(r.take(4)?.try_into().unwrap());
    let group_cfg_hash = format!("{:08X}", hash_val);
    r.skip(4)?;

    let file_info_num = r.count()?;
    let _chunks_length = r.i64()?;
    let _block_type = r.u8()?;
    let chunk_count = r.count()?;

    let mut all_files = Vec::new();
    for _ in 0..chunk_count {
        let _chunk_md5_name = r.md5()?;
        let _content_md5 = r.md5()?;
        let _chunk_length = r.i64()?;
        let _chunk_block_type = r.u8()?;
        if v5 {
            r.skip(8)?;
        }
        let file_count = r.count()?;

        for _ in 0..file_count {
            let file_name = r.string()?;
            let _file_name_hash = r.i64()?;
            let chunk_md5 = r.md5()?;
            let _file_data_md5 = r.md5()?;
            let offset = r.i64()?;
            let len = r.i64()?;
            let _file_block_type = r.u8()?;
            let use_encrypt = r.u8()? != 0;
            let iv_seed = if use_encrypt { r.i64()? } else { 0 };
            if v5 {
                r.skip(8)?;
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
    if v5 && all_files.len() != file_info_num {
        return Err(format!(
            "BLC lists {file_info_num} files but chunks hold {}",
            all_files.len()
        ));
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
    // The catalog is untrusted: a negative or huge offset/len must not wrap.
    let offset = usize::try_from(entry.offset).ok()?;
    let len = usize::try_from(entry.len).ok()?;
    let end = offset.checked_add(len)?;
    let file_data = chunk_data.get(offset..end)?;
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
    /// Directory the files were written to: `<output_dir>/<group name>`.
    pub output: PathBuf,
    pub total_files: usize,
    pub extracted: usize,
    pub sparkbuffer_parsed: usize,
    /// Lua scripts written as decrypted source.
    pub lua_decrypted: usize,
    /// MemoryPack files rewritten as JSON (originals kept as `.mpk`).
    pub memorypack_decoded: usize,
    /// MemoryPack files no schema type decodes; kept as stored, and not an error.
    pub memorypack_failed: usize,
    /// Decoded MemoryPack files per schema label.
    pub memorypack_by_schema: BTreeMap<String, u64>,
    pub errors: Vec<String>,
}

/// Group whose `.bytes` files are SparkBuffer tables worth converting to JSON.
const SPARKBUFFER_GROUP: &str = "Table";

/// Group name recorded in a `blc_metadata.txt` (its first line is `Group: <name> (<hash>)`).
pub fn metadata_group(metadata: &str) -> Option<&str> {
    metadata
        .lines()
        .next()?
        .strip_prefix("Group: ")?
        .split_once(" (")
        .map(|(group, _)| group)
}

/// Map a catalog file name (`Data/LuaScripts/Foo.lua`) to a safe relative output path.
pub fn relative_file_path(file_name: &str) -> PathBuf {
    let trimmed = file_name.strip_prefix("Data/").unwrap_or(file_name);
    let path: PathBuf = trimmed
        .split(['/', '\\'])
        .filter(|part| !part.is_empty() && *part != "." && *part != "..")
        .map(sanitize_filename)
        .collect();
    if path.as_os_str().is_empty() {
        PathBuf::from("_unnamed")
    } else {
        path
    }
}

/// Fetch a chunk missing from the archive from the patch CDN, by its `<dir>/<md5>.chk` name.
///
/// `Ok(None)` when the patch index does not list it or the request fails.
async fn download_patch_chunk(
    client: &Client,
    resolved: &ResolvedPipeline,
    patch_name: &str,
) -> Result<Option<Vec<u8>>, Box<dyn std::error::Error>> {
    let Some(patch) = &resolved.patch_index else {
        return Ok(None);
    };
    let Some(pf) = patch.files.iter().find(|p| p.name == patch_name) else {
        return Ok(None);
    };
    let main_res = resolved
        .config
        .resources
        .iter()
        .find(|r| r.name == "main")
        .expect("no main resource");
    let url = format!("{}/{}", main_res.path, pf.name);
    match client.get(&url).send().await {
        Ok(resp) if resp.status().is_success() => Ok(Some(resp.bytes().await?.to_vec())),
        _ => Ok(None),
    }
}

/// Extract all files from a single BLC-catalogued VFS directory into `<output_dir>/<group>`,
/// decoding MemoryPack files with the embedded schema.
pub async fn extract_directory(
    client: &Client,
    resolved: &ResolvedPipeline,
    dir_hash: &str,
    output_dir: &Path,
) -> Result<DirExtractResult, Box<dyn std::error::Error>> {
    let schemas = memorypack::SchemaSet::embedded()?;
    extract_directory_with(client, resolved, dir_hash, output_dir, &schemas).await
}

/// [`extract_directory`], decoding MemoryPack files with `schemas`.
pub async fn extract_directory_with(
    client: &Client,
    resolved: &ResolvedPipeline,
    dir_hash: &str,
    output_dir: &Path,
    schemas: &memorypack::SchemaSet,
) -> Result<DirExtractResult, Box<dyn std::error::Error>> {
    let blc_data =
        pipeline::download_chk(client, &resolved.cd, &pipeline::blc_path(dir_hash)).await?;
    let blc = parse_blc(&blc_data).map_err(|e| format!("BLC parse error: {e}"))?;

    let mut group = flat_filename(&blc.group_cfg_name);
    if group.is_empty() {
        group = dir_hash.to_string();
    }
    let dir_output = output_dir.join(&group);
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
        output: dir_output.clone(),
        total_files: blc.files.len(),
        extracted: 0,
        sparkbuffer_parsed: 0,
        lua_decrypted: 0,
        memorypack_decoded: 0,
        memorypack_failed: 0,
        memorypack_by_schema: BTreeMap::new(),
        errors: Vec::new(),
    };

    if blc.files.is_empty() {
        return Ok(result);
    }

    // Group files by chunk MD5
    let memorypack = blc.group_cfg_name == memorypack::JSON_DATA_GROUP;
    let mut chunks_needed: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for (i, f) in blc.files.iter().enumerate() {
        chunks_needed
            .entry(f.chunk_md5.clone())
            .or_default()
            .push(i);
    }

    for (chunk_md5, file_indices) in &chunks_needed {
        let chunk_file = format!("{}/{}.chk", dir_hash, chunk_md5.to_uppercase());
        let chunk_name = format!("{VFS_ROOT}{chunk_file}");

        let chunk_data = match pipeline::download_chk(client, &resolved.cd, &chunk_name).await {
            Ok(data) => data,
            Err(_) => match download_patch_chunk(client, resolved, &chunk_file).await? {
                Some(data) => data,
                None => {
                    result.errors.push(format!("chunk {} not found", chunk_md5));
                    continue;
                }
            },
        };

        for &fi in file_indices {
            let f = &blc.files[fi];
            let Some(decrypted) = decrypt_file(&chunk_data, f, blc.version) else {
                result
                    .errors
                    .push(format!("{}: offset/len out of bounds", f.file_name));
                continue;
            };

            let relative = relative_file_path(&f.file_name);
            let out_path = dir_output.join(&relative);
            if let Some(parent) = out_path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            // Scripts that don't decrypt are kept as stored rather than failing the group.
            let lua = (blc.group_cfg_name == lua::LUA_GROUP && f.file_name.ends_with(".lua"))
                .then(|| lua::decrypt_lua(&decrypted))
                .flatten();
            std::fs::write(&out_path, lua.as_deref().unwrap_or(&decrypted))?;
            // A source left by an earlier run into the same folder would be decoded instead.
            if memorypack && out_path.extension().is_some_and(|ext| ext == "json") {
                match std::fs::remove_file(out_path.with_extension("mpk")) {
                    Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
                        result
                            .errors
                            .push(format!("{}: stale .mpk: {e}", f.file_name));
                    }
                    _ => {}
                }
            }
            result.extracted += 1;
            result.lua_decrypted += usize::from(lua.is_some());

            // Parse SparkBuffer .bytes → JSON
            if blc.group_cfg_name == SPARKBUFFER_GROUP && f.file_name.ends_with(".bytes") {
                match sparkbuffer::parse_sparkbuffer(&decrypted) {
                    Ok(json_val) => {
                        let json_path = out_path.with_extension("json");
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

    if memorypack {
        match memorypack::decode_tree_set(schemas, &dir_output) {
            Ok(decoded) => {
                result.memorypack_decoded = decoded.decoded as usize;
                result.memorypack_failed = decoded.failed as usize;
                result.memorypack_by_schema = decoded.by_schema.clone();
                if decoded.write_errors > 0 {
                    result.errors.push(format!(
                        "MemoryPack: could not write {} decoded files",
                        decoded.write_errors
                    ));
                }
            }
            // An unreadable string pool (e.g. a new client format) leaves the files as stored;
            // blocking the group would only re-download it on every update.
            Err(e) if e.kind() == std::io::ErrorKind::InvalidData => {
                eprintln!("  MemoryPack decode skipped: {e}");
            }
            Err(e) => result.errors.push(format!("MemoryPack decode: {e}")),
        }
    }

    Ok(result)
}
