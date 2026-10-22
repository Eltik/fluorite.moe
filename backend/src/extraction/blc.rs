//! BLC catalog parser and ChaCha20 VFS extraction pipeline.
//!
//! BLC files are per-directory metadata catalogs listing all .chk files,
//! their offsets, sizes, encryption flags, and IV seeds.
use super::chacha20::{CHACHA_KEY, chacha20_crypt};
use super::lua;
use super::memorypack;
use super::pipeline::{self, ResolvedPipeline, VFS_ROOT};
use super::sparkbuffer;
use crate::extraction::utils::{flat_filename, format_bytes, sanitize_filename, to_hex};
use futures::StreamExt;
use reqwest::Client;
use std::collections::BTreeMap;
use std::io::{Read, Seek, SeekFrom, Write};
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

/// Byte range `(offset, len)` of an entry inside a chunk of `chunk_len` bytes, if in bounds.
///
/// The catalog is untrusted: a negative or huge offset/len must not wrap.
pub fn entry_range(entry: &BlcFileEntry, chunk_len: u64) -> Option<(u64, u64)> {
    let offset = u64::try_from(entry.offset).ok()?;
    let len = u64::try_from(entry.len).ok()?;
    (offset.checked_add(len)? <= chunk_len).then_some((offset, len))
}

/// Decrypt a single file from a chunk using its BLC entry metadata.
pub fn decrypt_file(chunk_data: &[u8], entry: &BlcFileEntry, version: i32) -> Option<Vec<u8>> {
    let (offset, len) = entry_range(entry, chunk_data.len() as u64)?;
    let file_data = &chunk_data[offset as usize..(offset + len) as usize];
    Some(decrypt_entry_bytes(file_data, entry, version))
}

/// Decipher an entry's bytes (already cut out of its chunk) if the catalog marks it encrypted.
pub fn decrypt_entry_bytes(file_data: &[u8], entry: &BlcFileEntry, version: i32) -> Vec<u8> {
    if entry.use_encrypt {
        let mut nonce = [0u8; 12];
        nonce[0..4].copy_from_slice(&version.to_le_bytes());
        nonce[4..12].copy_from_slice(&entry.iv_seed.to_le_bytes());
        chacha20_crypt(&CHACHA_KEY, &nonce, file_data, 1)
    } else {
        file_data.to_vec()
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

/// Whether a directory's catalog is read from the patch CDN: the patch index lists files under
/// it (a hot update, whose chunks only the CDN catalog names) or the archive lacks it.
pub fn uses_cdn_catalog(resolved: &ResolvedPipeline, dir_hash: &str) -> bool {
    resolved.cd.get(&pipeline::blc_path(dir_hash)).is_none()
        || resolved
            .patch_index
            .as_ref()
            .is_some_and(|patch| pipeline::patch_files(patch, dir_hash).next().is_some())
}

/// The raw `.blc` catalog of a directory from the patch CDN, or `None` when the CDN has none
/// (HTTP 403/404): some hot-updated directories ship patch chunks without a CDN catalog.
pub async fn fetch_cdn_catalog(
    client: &Client,
    resolved: &ResolvedPipeline,
    dir_hash: &str,
) -> Result<Option<Vec<u8>>, Box<dyn std::error::Error>> {
    let main =
        pipeline::main_resource_path(&resolved.config).ok_or("launcher lists no main resource")?;
    let url = pipeline::cdn_blc_url(main, dir_hash);
    let resp = client
        .get(&url)
        .send()
        .await
        .map_err(|e| format!("{url}: {e}"))?;
    let status = resp.status();
    if status == reqwest::StatusCode::FORBIDDEN || status == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }
    if !status.is_success() {
        return Err(format!("{url}: HTTP {status}").into());
    }
    Ok(Some(
        resp.bytes()
            .await
            .map_err(|e| format!("{url}: {e}"))?
            .to_vec(),
    ))
}

/// The raw `.blc` catalog of a directory as the archive ships it, if it ships one.
pub async fn fetch_archive_catalog(
    client: &Client,
    resolved: &ResolvedPipeline,
    dir_hash: &str,
) -> Result<Option<Vec<u8>>, Box<dyn std::error::Error>> {
    let path = pipeline::blc_path(dir_hash);
    if resolved.cd.get(&path).is_none() {
        return Ok(None);
    }
    Ok(Some(
        pipeline::download_chk(client, &resolved.cd, &path).await?,
    ))
}

/// The raw `.blc` catalog of a directory: from the CDN when [`uses_cdn_catalog`], falling back
/// to the archive copy only when the CDN has none.
pub async fn fetch_catalog(
    client: &Client,
    resolved: &ResolvedPipeline,
    dir_hash: &str,
) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    if uses_cdn_catalog(resolved, dir_hash)
        && let Some(catalog) = fetch_cdn_catalog(client, resolved, dir_hash).await?
    {
        return Ok(catalog);
    }
    fetch_archive_catalog(client, resolved, dir_hash)
        .await?
        .ok_or_else(|| format!("{dir_hash}: no catalog on the CDN or in the archive").into())
}

/// Attempts per download segment. Within one run a retry continues from the segment's last
/// written byte; nothing carries over between runs (a new run downloads the chunk again).
const DOWNLOAD_ATTEMPTS: u32 = 4;
/// Patch chunks are fetched as ranges of this size, [`DOWNLOAD_PARALLEL`] at a time: the CDN
/// throttles single connections well below the link speed.
const DOWNLOAD_SEGMENT: u64 = 32 << 20;
const DOWNLOAD_PARALLEL: usize = 6;

/// Stream `url` (`expected` bytes, `0` if unknown) into `dest` without holding it in memory.
///
/// A known size is fetched as parallel range requests written straight to their place in the
/// file; an unknown one as a single request. Returns the size written.
pub async fn download_to_file(
    client: &Client,
    url: &str,
    dest: &Path,
    expected: u64,
) -> Result<u64, String> {
    let io = |e: std::io::Error| format!("{}: {e}", dest.display());
    let file = std::fs::File::create(dest).map_err(io)?;
    if expected == 0 {
        let written = download_range(client, url, dest, 0, None).await?;
        return Ok(written);
    }
    file.set_len(expected).map_err(io)?;
    drop(file);
    let segments = (0..expected.div_ceil(DOWNLOAD_SEGMENT)).map(|i| {
        let start = i * DOWNLOAD_SEGMENT;
        let end = (start + DOWNLOAD_SEGMENT).min(expected);
        download_range(client, url, dest, start, Some(end))
    });
    let mut results = futures::stream::iter(segments).buffer_unordered(DOWNLOAD_PARALLEL);
    while let Some(result) = results.next().await {
        result?;
    }
    Ok(expected)
}

/// Download bytes `start..end` (to the end of the file when `end` is `None`) of `url` into the
/// same offsets of the existing file `dest`, retrying from where a dropped request stopped.
async fn download_range(
    client: &Client,
    url: &str,
    dest: &Path,
    start: u64,
    end: Option<u64>,
) -> Result<u64, String> {
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .open(dest)
        .map_err(|e| format!("{}: {e}", dest.display()))?;
    let mut pos = start;
    let mut attempt = 0;
    loop {
        attempt += 1;
        let outcome: Result<(), String> = async {
            let mut request = client.get(url);
            let ranged = pos > 0 || end.is_some();
            if ranged {
                let last = end.map_or(String::new(), |e| (e - 1).to_string());
                request = request.header(reqwest::header::RANGE, format!("bytes={pos}-{last}"));
            }
            let mut resp = request.send().await.map_err(|e| e.to_string())?;
            let status = resp.status();
            if !status.is_success() {
                return Err(format!("HTTP {status}"));
            }
            if ranged {
                if status != reqwest::StatusCode::PARTIAL_CONTENT {
                    return Err(format!("HTTP {status} to a range request"));
                }
                let start = resp
                    .headers()
                    .get(reqwest::header::CONTENT_RANGE)
                    .and_then(|v| v.to_str().ok())
                    .and_then(content_range_start);
                if start != Some(pos) {
                    return Err(format!("range from byte {pos} answered with {start:?}"));
                }
            }
            file.seek(SeekFrom::Start(pos)).map_err(|e| e.to_string())?;
            while let Some(bytes) = resp.chunk().await.map_err(|e| e.to_string())? {
                let take = end.map_or(bytes.len() as u64, |e| (e - pos).min(bytes.len() as u64));
                file.write_all(&bytes[..take as usize])
                    .map_err(|e| e.to_string())?;
                pos += take;
            }
            Ok(())
        }
        .await;
        let done = end.is_none_or(|e| pos >= e);
        match outcome {
            Ok(()) if done => return Ok(pos - start),
            Ok(()) if attempt >= DOWNLOAD_ATTEMPTS => {
                return Err(format!("{url}: short read at byte {pos}"));
            }
            Err(e) if attempt >= DOWNLOAD_ATTEMPTS => return Err(format!("{url}: {e}")),
            _ => tokio::time::sleep(std::time::Duration::from_secs(2 * u64::from(attempt))).await,
        }
    }
}

/// A chunk's bytes: in memory (archive chunks) or in a scratch file (patch CDN chunks, which
/// reach 1.5 GB for the voice packages).
enum Chunk {
    Memory(Vec<u8>),
    File { path: PathBuf, len: u64 },
}

impl Chunk {
    fn len(&self) -> u64 {
        match self {
            Chunk::Memory(data) => data.len() as u64,
            Chunk::File { len, .. } => *len,
        }
    }

    /// Delete the scratch file, if any (it may already have been moved into place).
    fn discard(self) {
        if let Chunk::File { path, .. } = self {
            let _ = std::fs::remove_file(path);
        }
    }
}

/// Fetch a chunk: from the archive when it holds it, else from the patch CDN into `scratch`.
/// `Ok(None)` when neither lists it.
async fn fetch_chunk(
    client: &Client,
    resolved: &ResolvedPipeline,
    dir_hash: &str,
    chunk_md5: &str,
    scratch: &Path,
) -> Result<Option<Chunk>, String> {
    let relative = format!("{dir_hash}/{}.chk", chunk_md5.to_uppercase());
    let archive_name = format!("{VFS_ROOT}{relative}");
    if resolved.cd.get(&archive_name).is_some() {
        return pipeline::download_chk(client, &resolved.cd, &archive_name)
            .await
            .map(|data| Some(Chunk::Memory(data)))
            .map_err(|e| format!("{archive_name}: {e}"));
    }
    let listed = resolved.patch_index.as_ref().and_then(|patch| {
        patch
            .files
            .iter()
            .find(|p| p.name.eq_ignore_ascii_case(&relative))
    });
    // A CDN catalog can name chunks the patch index leaves out (AudioEnglish's second stream
    // package); they sit on the CDN all the same.
    if listed.is_none() && !uses_cdn_catalog(resolved, dir_hash) {
        return Ok(None);
    }
    let main =
        pipeline::main_resource_path(&resolved.config).ok_or("launcher lists no main resource")?;
    let url = pipeline::cdn_vfs_url(main, &relative);
    let size = match listed {
        Some(pf) => pf.size,
        None => match content_length(client, &url).await? {
            Some(size) => size,
            None => return Ok(None),
        },
    };
    if size >= LARGE_CHUNK {
        println!("  downloading {relative} ({})", format_bytes(size));
    }
    match download_to_file(client, &url, scratch, size).await {
        Ok(len) => Ok(Some(Chunk::File {
            path: scratch.to_path_buf(),
            len,
        })),
        Err(e) => {
            let _ = std::fs::remove_file(scratch);
            Err(e)
        }
    }
}

/// Size of `url` from a HEAD request; `None` when the CDN does not have it.
async fn content_length(client: &Client, url: &str) -> Result<Option<u64>, String> {
    let resp = client
        .head(url)
        .send()
        .await
        .map_err(|e| format!("{url}: {e}"))?;
    if !resp.status().is_success() {
        return Ok(None);
    }
    Ok(resp
        .headers()
        .get(reqwest::header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok()?.parse().ok())
        .or(Some(0)))
}

/// Chunks at least this big get a progress line.
const LARGE_CHUNK: u64 = 64 << 20;

/// Read `len` bytes at `offset` of a file.
fn read_range(path: &Path, offset: u64, len: u64) -> std::io::Result<Vec<u8>> {
    let mut file = std::fs::File::open(path)?;
    file.seek(SeekFrom::Start(offset))?;
    let mut buf = Vec::with_capacity(len as usize);
    file.take(len).read_to_end(&mut buf)?;
    if (buf.len() as u64) < len {
        return Err(std::io::ErrorKind::UnexpectedEof.into());
    }
    Ok(buf)
}

/// Copy `len` bytes at `offset` of `src` into a new file `dest`, through a small buffer.
fn copy_range(src: &Path, offset: u64, len: u64, dest: &Path) -> std::io::Result<()> {
    let mut input = std::fs::File::open(src)?;
    input.seek(SeekFrom::Start(offset))?;
    let mut output = std::io::BufWriter::new(std::fs::File::create(dest)?);
    let copied = std::io::copy(&mut input.take(len), &mut output)?;
    if copied < len {
        return Err(std::io::ErrorKind::UnexpectedEof.into());
    }
    output.flush()
}

/// Write the files `file_indices` of `blc` held by `chunk` into `dir_output`.
///
/// Plain files of a file-backed chunk are copied without loading them; a chunk that is
/// exactly one plain file (the voice `.pck` packages) is moved into place.
fn extract_chunk_files(
    blc: &BlcInfo,
    chunk: &Chunk,
    file_indices: &[usize],
    dir_output: &Path,
    result: &mut DirExtractResult,
) -> std::io::Result<()> {
    let memorypack = blc.group_cfg_name == memorypack::JSON_DATA_GROUP;
    let lua_group = blc.group_cfg_name == lua::LUA_GROUP;
    let sparkbuffer_group = blc.group_cfg_name == SPARKBUFFER_GROUP;
    for &fi in file_indices {
        let f = &blc.files[fi];
        let Some((offset, len)) = entry_range(f, chunk.len()) else {
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
        let is_lua = lua_group && f.file_name.ends_with(".lua");
        let is_sparkbuffer = sparkbuffer_group && f.file_name.ends_with(".bytes");
        let mut lua_done = false;
        match chunk {
            Chunk::File {
                path,
                len: chunk_len,
            } if !(f.use_encrypt || is_lua || is_sparkbuffer) => {
                if offset == 0 && len == *chunk_len && file_indices.len() == 1 {
                    std::fs::rename(path, &out_path)?;
                } else {
                    copy_range(path, offset, len, &out_path)?;
                }
            }
            _ => {
                let decrypted = match chunk {
                    Chunk::Memory(data) => decrypt_entry_bytes(
                        &data[offset as usize..(offset + len) as usize],
                        f,
                        blc.version,
                    ),
                    Chunk::File { path, .. } => {
                        decrypt_entry_bytes(&read_range(path, offset, len)?, f, blc.version)
                    }
                };
                // Scripts that don't decrypt are kept as stored rather than failing the group.
                let lua = is_lua.then(|| lua::decrypt_lua(&decrypted)).flatten();
                std::fs::write(&out_path, lua.as_deref().unwrap_or(&decrypted))?;
                lua_done = lua.is_some();

                if is_sparkbuffer {
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
        result.lua_decrypted += usize::from(lua_done);
    }
    Ok(())
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
    let blc_data = fetch_catalog(client, resolved, dir_hash).await?;
    extract_directory_from(client, resolved, dir_hash, &blc_data, output_dir, schemas).await
}

/// [`extract_directory_with`] from an already fetched raw catalog (see [`fetch_catalog`]).
pub async fn extract_directory_from(
    client: &Client,
    resolved: &ResolvedPipeline,
    dir_hash: &str,
    blc_data: &[u8],
    output_dir: &Path,
    schemas: &memorypack::SchemaSet,
) -> Result<DirExtractResult, Box<dyn std::error::Error>> {
    let blc = parse_blc(blc_data).map_err(|e| format!("BLC parse error: {e}"))?;

    let mut group = flat_filename(&blc.group_cfg_name);
    if group.is_empty() {
        group = dir_hash.to_string();
    }
    let dir_output = output_dir.join(&group);
    std::fs::create_dir_all(&dir_output)?;
    remove_stale_parts(output_dir, dir_hash);

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
        write_metadata(&dir_output, &blc)?;
        return Ok(result);
    }

    let memorypack = blc.group_cfg_name == memorypack::JSON_DATA_GROUP;
    let mut chunks_needed: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for (i, f) in blc.files.iter().enumerate() {
        chunks_needed
            .entry(f.chunk_md5.clone())
            .or_default()
            .push(i);
    }

    for (chunk_md5, file_indices) in &chunks_needed {
        let scratch = output_dir.join(format!(".{dir_hash}-{chunk_md5}.chk.part"));
        let chunk = match fetch_chunk(client, resolved, dir_hash, chunk_md5, &scratch).await {
            Ok(Some(chunk)) => chunk,
            Ok(None) => {
                result.errors.push(format!("chunk {} not found", chunk_md5));
                continue;
            }
            Err(e) => {
                result.errors.push(format!("chunk {chunk_md5}: {e}"));
                continue;
            }
        };
        let outcome = extract_chunk_files(&blc, &chunk, file_indices, &dir_output, &mut result);
        chunk.discard();
        outcome?;
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

    // Written last: its presence marks the group as installed (the audio API stops streaming
    // a voice language from the CDN once it sees one), so a failed run must not leave it.
    if result.errors.is_empty() {
        write_metadata(&dir_output, &blc)?;
    } else {
        match std::fs::remove_file(dir_output.join(METADATA_FILE)) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e.into()),
            _ => {}
        }
    }
    Ok(result)
}

/// Catalog summary written into each extracted group folder.
const METADATA_FILE: &str = "blc_metadata.txt";

/// Write `<group>/blc_metadata.txt` (`Group: <name> (<hash>)`, version, file count).
fn write_metadata(dir_output: &Path, blc: &BlcInfo) -> std::io::Result<()> {
    let text = format!(
        "Group: {} ({})\nVersion: {}\nFiles: {}\n",
        blc.group_cfg_name,
        blc.group_cfg_hash,
        blc.version,
        blc.files.len()
    );
    std::fs::write(dir_output.join(METADATA_FILE), text)
}

/// Delete scratch chunk files (`.{dir}-{md5}.chk.part`) a crashed run left in `output_dir`;
/// downloads always start over.
fn remove_stale_parts(output_dir: &Path, dir_hash: &str) {
    let Ok(entries) = std::fs::read_dir(output_dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        if is_stale_part(&name.to_string_lossy(), dir_hash) {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// Whether `name` is a scratch chunk file of directory `dir_hash`.
pub fn is_stale_part(name: &str, dir_hash: &str) -> bool {
    name.strip_prefix('.')
        .and_then(|rest| {
            rest.get(..dir_hash.len())
                .map(|d| (d, &rest[dir_hash.len()..]))
        })
        .is_some_and(|(dir, rest)| {
            dir.eq_ignore_ascii_case(dir_hash)
                && rest.starts_with('-')
                && rest.ends_with(".chk.part")
        })
}

/// First byte of a `Content-Range: bytes S-E/T` (or `bytes S-E/*`) header value.
pub fn content_range_start(value: &str) -> Option<u64> {
    let range = value.trim().strip_prefix("bytes ")?;
    let (span, _total) = range.split_once('/')?;
    let (start, end) = span.split_once('-')?;
    let start: u64 = start.trim().parse().ok()?;
    let end: u64 = end.trim().parse().ok()?;
    (start <= end).then_some(start)
}
