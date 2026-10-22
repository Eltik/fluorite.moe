fn read_u32_be(buf: &[u8], offset: usize) -> u32 {
    u32::from_be_bytes(buf[offset..offset + 4].try_into().unwrap())
}

fn read_i64_be(buf: &[u8], offset: usize) -> i64 {
    i64::from_be_bytes(buf[offset..offset + 8].try_into().unwrap())
}

/// Read a null-terminated string from buf starting at offset.
/// Returns (string, bytes_consumed_including_null).
fn read_cstring(buf: &[u8], offset: usize) -> (String, usize) {
    let start = offset;
    let mut end = offset;
    while end < buf.len() && buf[end] != 0 {
        end += 1;
    }
    let s = String::from_utf8_lossy(&buf[start..end]).to_string();
    (s, end - start + 1) // +1 for the null terminator
}

pub struct UnityFsHeader {
    pub format_version: u32,
    pub player_version: String,
    pub engine_version: String,
    pub bundle_size: i64,
    pub compressed_block_info_size: u32,
    pub uncompressed_block_info_size: u32,
    pub flags: u32,
    pub header_size: usize,
}

pub fn parse_header(buf: &[u8]) -> Result<UnityFsHeader, Box<dyn std::error::Error>> {
    if buf.len() < 8 || &buf[0..8] != b"UnityFS\0" {
        return Err("not a UnityFS bundle".into());
    }

    let mut offset = 8;

    let format_version = read_u32_be(buf, offset);
    offset += 4;

    let (player_version, consumed) = read_cstring(buf, offset);
    offset += consumed;

    let (engine_version, consumed) = read_cstring(buf, offset);
    offset += consumed;

    let bundle_size = read_i64_be(buf, offset);
    offset += 8;

    let compressed_block_info_size = read_u32_be(buf, offset);
    offset += 4;

    let uncompressed_block_info_size = read_u32_be(buf, offset);
    offset += 4;

    let flags = read_u32_be(buf, offset);
    offset += 4;

    if format_version >= 7 {
        offset = (offset + 15) & !15;
    }

    Ok(UnityFsHeader {
        format_version,
        player_version,
        engine_version,
        bundle_size,
        compressed_block_info_size,
        uncompressed_block_info_size,
        flags,
        header_size: offset,
    })
}

impl UnityFsHeader {
    /// Compression method for the block info section (NOT for storage blocks).
    /// 0=none, 1=LZMA, 2=LZ4, 3=LZ4HC
    pub fn block_info_compression(&self) -> u32 {
        self.flags & 0x3F
    }

    /// If true, block info is at the END of the file, not right after the header.
    pub fn block_info_at_end(&self) -> bool {
        (self.flags & 0x80) != 0
    }
}

/// Decompress the block info section based on the header's compression flag.
pub fn decompress_block_info(
    compressed: &[u8],
    uncompressed_size: u32,
    compression: u32,
) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    match compression {
        0 => Ok(compressed.to_vec()), // none
        2 | 3 => {
            // LZ4 or LZ4HC — both decompress the same way
            let decompressed = lz4_flex::decompress(compressed, uncompressed_size as usize)
                .map_err(|e| format!("LZ4 decompress error: {}", e))?;
            Ok(decompressed)
        }
        1 => Err("LZMA block info compression not yet implemented".into()),
        other => Err(format!("unknown block info compression: {}", other).into()),
    }
}

pub struct StorageBlock {
    pub uncompressed_size: u32,
    pub compressed_size: u32,
    pub flags: u16,
}

impl StorageBlock {
    /// Compression type for this block: 0=none, 1=LZMA, 2=LZ4, 3=LZ4HC
    pub fn compression(&self) -> u16 {
        self.flags & 0x3F
    }
}

/// Returns (blocks, offset_after_blocks) so directory entries can be parsed from the remainder.
pub fn parse_storage_blocks(block_info: &[u8]) -> (Vec<StorageBlock>, usize) {
    let mut offset = 16; // skip hash

    let block_count = read_u32_be(block_info, offset) as usize;
    offset += 4;

    let mut blocks = Vec::with_capacity(block_count);
    for _ in 0..block_count {
        let uncompressed_size = read_u32_be(block_info, offset);
        offset += 4;
        let compressed_size = read_u32_be(block_info, offset);
        offset += 4;
        let flags = u16::from_be_bytes(block_info[offset..offset + 2].try_into().unwrap());
        offset += 2;

        blocks.push(StorageBlock {
            uncompressed_size,
            compressed_size,
            flags,
        });
    }

    (blocks, offset)
}

#[derive(Debug)]
pub struct DirectoryEntry {
    pub offset: i64,
    pub size: i64,
    pub flags: u32,
    pub path: String,
}

/// Parse directory entries from block_info starting at the given offset (after storage blocks).
pub fn parse_directory_entries(block_info: &[u8], start_offset: usize) -> Vec<DirectoryEntry> {
    let mut offset = start_offset;

    let entry_count = read_u32_be(block_info, offset) as usize;
    offset += 4;

    let mut entries = Vec::with_capacity(entry_count);
    for _ in 0..entry_count {
        let entry_offset = read_i64_be(block_info, offset);
        offset += 8;
        let size = read_i64_be(block_info, offset);
        offset += 8;
        let flags = read_u32_be(block_info, offset);
        offset += 4;
        let (path, consumed) = read_cstring(block_info, offset);
        offset += consumed;

        entries.push(DirectoryEntry {
            offset: entry_offset,
            size,
            flags,
            path,
        });
    }

    entries
}

/// Calculate the byte offset where storage block data begins within the bundle.
pub fn storage_blocks_offset(header: &UnityFsHeader) -> usize {
    if header.block_info_at_end() {
        // Block info is at end of file; storage blocks start right after header
        header.header_size
    } else {
        // Block info follows header; storage blocks come after block info
        header.header_size + header.compressed_block_info_size as usize
    }
}

/// Iterate over concatenated UnityFS bundles.
/// Calls the callback with (bundle_index, bundle_data_slice) for each bundle found.
pub fn iter_bundles(data: &[u8], mut callback: impl FnMut(usize, &[u8])) {
    let mut offset = 0;
    let mut index = 0;

    while offset + 8 <= data.len() {
        // Check for UnityFS magic
        if &data[offset..offset + 8] != b"UnityFS\0" {
            break;
        }

        match parse_header(&data[offset..]) {
            Ok(header) => {
                let bundle_size = header.bundle_size as usize;
                let end = (offset + bundle_size).min(data.len());
                callback(index, &data[offset..end]);
                offset = end;
                index += 1;
            }
            Err(_) => break,
        }
    }
}

/// Process a single UnityFS bundle: parse header, decompress block info,
/// FairGuard-decrypt block 0 if needed, decompress storage blocks.
pub fn process_bundle(
    bundle_data: &[u8],
) -> Result<(Vec<u8>, Vec<DirectoryEntry>), Box<dyn std::error::Error>> {
    let header = parse_header(bundle_data)?;

    let block_info_data = if header.block_info_at_end() {
        let start = header.bundle_size as usize - header.compressed_block_info_size as usize;
        &bundle_data[start..header.bundle_size as usize]
    } else {
        let start = header.header_size;
        let end = start + header.compressed_block_info_size as usize;
        &bundle_data[start..end]
    };

    let block_info = decompress_block_info(
        block_info_data,
        header.uncompressed_block_info_size,
        header.block_info_compression(),
    )?;

    let (blocks, dir_offset) = parse_storage_blocks(&block_info);
    let entries = parse_directory_entries(&block_info, dir_offset);

    let data_start = storage_blocks_offset(&header);
    let mut output = Vec::new();
    let mut current_offset = data_start;

    for (i, block) in blocks.iter().enumerate() {
        let block_end = current_offset + block.compressed_size as usize;
        if block_end > bundle_data.len() {
            return Err(format!(
                "Storage block {} extends beyond bundle: offset={}, size={}, bundle_len={}",
                i,
                current_offset,
                block.compressed_size,
                bundle_data.len()
            )
            .into());
        }

        let mut block_data = bundle_data[current_offset..block_end].to_vec();

        if i == 0 && super::fair_guard_utils::is_encrypted(&block_data) {
            super::fair_guard_utils::decrypt(&mut block_data);
        }

        match block.compression() {
            0 => output.extend_from_slice(&block_data),
            2 | 3 => {
                let decompressed =
                    lz4_flex::decompress(&block_data, block.uncompressed_size as usize)
                        .map_err(|e| format!("LZ4 decompress block {}: {}", i, e))?;
                output.extend_from_slice(&decompressed);
            }
            1 => return Err(format!("LZMA compression not implemented (block {})", i).into()),
            other => {
                return Err(format!("Unknown compression type {} in block {}", other, i).into());
            }
        }

        current_offset = block_end;
    }

    Ok((output, entries))
}

/// An extracted asset from the VFS → UnityFS pipeline.
#[derive(Debug)]
pub struct ExtractedAsset {
    pub vfs_name: String,
    pub is_unityfs: bool,
    pub bundle_entries: Vec<DirectoryEntry>,
    pub data: Vec<u8>,
}

/// Process a VFS-encrypted .chk file end-to-end: VFS decrypt → extract files → UnityFS process.
pub fn process_chk_file(
    chk_data: &[u8],
) -> Result<Vec<ExtractedAsset>, Box<dyn std::error::Error>> {
    let containers = super::vfs_decrypt::process_all_vfs(chk_data)?;
    let mut assets = Vec::new();

    for result in &containers {
        let files = super::vfs_decrypt::extract_files(&result.vfs, &result.data);

        for (name, file_data) in files {
            if file_data.len() >= 8 && &file_data[..8] == b"UnityFS\0" {
                iter_bundles(&file_data, |_idx, bundle_slice| {
                    match process_bundle(bundle_slice) {
                        Ok((output, entries)) => {
                            assets.push(ExtractedAsset {
                                vfs_name: name.clone(),
                                is_unityfs: true,
                                bundle_entries: entries,
                                data: output,
                            });
                        }
                        Err(e) => {
                            eprintln!(
                                "Warning: failed to process UnityFS bundle in {}: {}",
                                name, e
                            );
                        }
                    }
                });
            } else {
                assets.push(ExtractedAsset {
                    vfs_name: name.clone(),
                    is_unityfs: false,
                    bundle_entries: Vec::new(),
                    data: file_data,
                });
            }
        }
    }

    Ok(assets)
}
