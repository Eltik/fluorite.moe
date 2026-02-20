use std::collections::HashMap;
use std::io::Read;

use flate2::read::DeflateDecoder;
use reqwest::Client;

/// Describes one part of a split archive
pub struct SplitArchivePart {
    pub url: String,
    pub size: u64,
}

/// Parsed ZIP64 central directory with file lookups
pub struct ZipCentralDirectory {
    parts: Vec<SplitArchivePart>,
    entries: HashMap<String, ZipEntry>,
    cumulative_sizes: Vec<u64>, // prefix sums for global offset -> part mapping
}

impl ZipCentralDirectory {
    /// Fetch & parse central directory via 2-3 Range requests against last part.
    /// Downloads: ~1KB tail (EOCD) + ~147KB central directory.
    pub async fn from_split_archive(
        client: &Client,
        parts: Vec<SplitArchivePart>,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        // 1. Build cumulative size prefix sums
        let mut cumulative_sizes = Vec::with_capacity(parts.len() + 1);
        cumulative_sizes.push(0u64);
        for part in &parts {
            let last = *cumulative_sizes.last().unwrap();
            cumulative_sizes.push(last + part.size);
        }
        let total_size = *cumulative_sizes.last().unwrap();

        // 2. Fetch last 1024 bytes of the archive (from the last part)
        let tail_size: u64 = 1024;
        let tail_start = total_size - tail_size;
        let tail = fetch_global_range(
            client,
            &parts,
            &cumulative_sizes,
            tail_start,
            total_size - 1,
        )
        .await?;

        // 3. Find EOCD signature in the tail
        let eocd_pos_in_tail =
            find_eocd_signature(&tail).ok_or("EOCD signature not found in last 1024 bytes")?;
        let eocd = parse_eocd(&tail, eocd_pos_in_tail);

        // 4. Determine if ZIP64; get CD offset + size
        let (cd_offset, cd_size, total_entries) = if eocd.cd_offset == 0xFFFFFFFF
            || eocd.cd_entries_total == 0xFFFF
            || eocd.cd_size == 0xFFFFFFFF
        {
            // ZIP64 path
            let locator = parse_zip64_eocd_locator(&tail, eocd_pos_in_tail)
                .ok_or("ZIP64 EOCD Locator not found")?;

            // 5. Fetch the ZIP64 EOCD record (56 bytes)
            let z64_eocd_buf = fetch_global_range(
                client,
                &parts,
                &cumulative_sizes,
                locator.zip64_eocd_offset,
                locator.zip64_eocd_offset + 55,
            )
            .await?;

            let z64 = parse_zip64_eocd(&z64_eocd_buf);
            (z64.cd_offset, z64.cd_size, z64.cd_entries_total)
        } else {
            (
                eocd.cd_offset as u64,
                eocd.cd_size as u64,
                eocd.cd_entries_total as u64,
            )
        };

        // 6. Fetch the entire central directory
        let cd_buf = fetch_global_range(
            client,
            &parts,
            &cumulative_sizes,
            cd_offset,
            cd_offset + cd_size - 1,
        )
        .await?;

        // 7. Parse all entries
        let entry_list = parse_cd_entries(&cd_buf, total_entries);
        let mut entries = HashMap::with_capacity(entry_list.len());
        for entry in entry_list {
            entries.insert(entry.filename.clone(), entry);
        }

        Ok(ZipCentralDirectory {
            parts,
            entries,
            cumulative_sizes,
        })
    }

    pub fn get(&self, filename: &str) -> Option<&ZipEntry> {
        self.entries.get(filename)
    }

    pub fn filenames(&self) -> impl Iterator<Item = &str> {
        self.entries.keys().map(|s| s.as_str())
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Extract a file's contents, decompressing deflate if needed.
    /// Downloads: local header (~1KB) + compressed data.
    pub async fn extract(
        &self,
        client: &Client,
        filename: &str,
    ) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
        let entry = self
            .get(filename)
            .ok_or_else(|| format!("file not found in archive: {}", filename))?;

        // 1. Fetch local header (first 1024 bytes should be enough)
        let header_fetch_size: u64 = 1024;
        let header_buf = fetch_global_range(
            client,
            &self.parts,
            &self.cumulative_sizes,
            entry.local_header_offset,
            entry.local_header_offset + header_fetch_size - 1,
        )
        .await?;

        let header_size = parse_local_header_size(&header_buf)? as u64;

        // 2. Fetch compressed data
        let data_start = entry.local_header_offset + header_size;
        let data_end = data_start + entry.compressed_size - 1;
        let compressed = fetch_global_range(
            client,
            &self.parts,
            &self.cumulative_sizes,
            data_start,
            data_end,
        )
        .await?;

        // 3. Decompress if needed
        match entry.compression_method {
            0 => {
                // Stored — no compression
                Ok(compressed)
            }
            8 => {
                // Deflate — use raw deflate (NOT zlib, NOT gzip)
                let mut decoder = DeflateDecoder::new(&compressed[..]);
                let mut decompressed = Vec::with_capacity(entry.uncompressed_size as usize);
                decoder.read_to_end(&mut decompressed)?;
                Ok(decompressed)
            }
            other => Err(format!("unsupported compression method: {}", other).into()),
        }
    }

    /// Extract raw (possibly compressed) data without decompressing.
    pub async fn extract_raw(
        &self,
        client: &Client,
        filename: &str,
    ) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
        let entry = self
            .get(filename)
            .ok_or_else(|| format!("file not found in archive: {}", filename))?;

        let header_fetch_size: u64 = 1024;
        let header_buf = fetch_global_range(
            client,
            &self.parts,
            &self.cumulative_sizes,
            entry.local_header_offset,
            entry.local_header_offset + header_fetch_size - 1,
        )
        .await?;

        let header_size = parse_local_header_size(&header_buf)? as u64;
        let data_start = entry.local_header_offset + header_size;
        let data_end = data_start + entry.compressed_size - 1;

        fetch_global_range(
            client,
            &self.parts,
            &self.cumulative_sizes,
            data_start,
            data_end,
        )
        .await
    }
}

/// One file's metadata from the central directory
pub struct ZipEntry {
    pub filename: String,
    pub compression_method: u16, // 0=stored, 8=deflate
    pub crc32: u32,
    pub compressed_size: u64,
    pub uncompressed_size: u64,
    pub local_header_offset: u64, // global offset in concatenated archive
    pub disk_number: u32,
}

fn read_u16(buf: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes(buf[offset..offset + 2].try_into().unwrap())
}

fn read_u32(buf: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(buf[offset..offset + 4].try_into().unwrap())
}

fn read_u64(buf: &[u8], offset: usize) -> u64 {
    u64::from_le_bytes(buf[offset..offset + 8].try_into().unwrap())
}

async fn fetch_global_range(
    client: &Client,
    parts: &[SplitArchivePart],
    cumulative_sizes: &[u64],
    global_start: u64,
    global_end: u64, // inclusive
) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let (start_part, start_offset) = global_to_part(cumulative_sizes, global_start);
    let (end_part, end_offset) = global_to_part(cumulative_sizes, global_end);

    if start_part == end_part {
        // Common case: entirely within one part
        return fetch_range(client, &parts[start_part].url, start_offset, end_offset).await;
    }

    // Rare case: spans multiple parts
    let mut result = Vec::new();

    for (i, part) in parts[start_part..=end_part].iter().enumerate() {
        let part_idx = start_part + i;
        let range_start = if part_idx == start_part {
            start_offset
        } else {
            0
        };
        let range_end = if part_idx == end_part {
            end_offset
        } else {
            part.size - 1
        };
        let chunk = fetch_range(client, &part.url, range_start, range_end).await?;
        result.extend_from_slice(&chunk);
    }

    Ok(result)
}

async fn fetch_range(
    client: &Client,
    url: &str,
    start: u64,
    end_inclusive: u64,
) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let range_header = format!("bytes={}-{}", start, end_inclusive);
    let resp = client
        .get(url)
        .header("Range", &range_header)
        .send()
        .await?;

    if resp.status() != 206 {
        return Err(format!(
            "expected HTTP 206, got {} for range {}-{} on {}",
            resp.status(),
            start,
            end_inclusive,
            url
        )
        .into());
    }

    Ok(resp.bytes().await?.to_vec())
}

/// Returns (part_index, offset_within_part)
fn global_to_part(cumulative_sizes: &[u64], global_offset: u64) -> (usize, u64) {
    // cumulative_sizes[i] = sum of sizes of parts 0..i
    // Example: parts of size [1GB, 1GB, 476MB]
    //   cumulative_sizes = [0, 1GB, 2GB, 2GB+476MB]
    // Binary search: find rightmost i where cumulative_sizes[i] <= global_offset
    let part_idx = cumulative_sizes.partition_point(|&cumalative| cumalative <= global_offset) - 1;
    let offset_in_part = global_offset - cumulative_sizes[part_idx];
    (part_idx, offset_in_part)
}

/// Scan backwards through buf for the EOCD signature 0x06054b50.
/// Returns the offset within buf where the signature starts.
fn find_eocd_signature(buf: &[u8]) -> Option<usize> {
    // EOCD is at least 22 bytes. Scan backwards from buf.len() - 22.
    if buf.len() < 22 {
        return None;
    }
    let sig: [u8; 4] = [0x50, 0x4b, 0x05, 0x06]; // little-endian 0x06054b50
    for i in (0..=buf.len() - 22).rev() {
        if buf[i..i + 4] == sig {
            return Some(i);
        }
    }
    None
}

struct EocdRecord {
    cd_entries_total: u16,
    cd_size: u32,
    cd_offset: u32,
}

fn parse_eocd(buf: &[u8], offset: usize) -> EocdRecord {
    EocdRecord {
        cd_entries_total: read_u16(buf, offset + 10),
        cd_size: read_u32(buf, offset + 12),
        cd_offset: read_u32(buf, offset + 16),
    }
}

struct Zip64EocdLocator {
    zip64_eocd_offset: u64, // global offset of the ZIP64 EOCD record
}

fn parse_zip64_eocd_locator(buf: &[u8], eocd_offset: usize) -> Option<Zip64EocdLocator> {
    // Locator is 20 bytes immediately before EOCD
    if eocd_offset < 20 {
        return None;
    }
    let loc_offset = eocd_offset - 20;
    let sig = read_u32(buf, loc_offset);
    if sig != 0x07064b50 {
        return None;
    }
    Some(Zip64EocdLocator {
        zip64_eocd_offset: read_u64(buf, loc_offset + 8),
    })
}

struct Zip64EocdRecord {
    cd_entries_total: u64,
    cd_size: u64,
    cd_offset: u64,
}

fn parse_zip64_eocd(buf: &[u8]) -> Zip64EocdRecord {
    // buf starts at the ZIP64 EOCD record
    assert_eq!(read_u32(buf, 0), 0x06064b50, "bad ZIP64 EOCD sig");
    Zip64EocdRecord {
        cd_entries_total: read_u64(buf, 32),
        cd_size: read_u64(buf, 40),
        cd_offset: read_u64(buf, 48),
    }
}

fn parse_cd_entries(buf: &[u8], expected_count: u64) -> Vec<ZipEntry> {
    let mut entries = Vec::with_capacity(expected_count as usize);
    let mut pos = 0;

    while pos + 46 <= buf.len() {
        let sig = read_u32(buf, pos);
        if sig != 0x02014b50 {
            break; // no more CD entries
        }

        let compression_method = read_u16(buf, pos + 10);
        let crc32 = read_u32(buf, pos + 16);
        let compressed_size_32 = read_u32(buf, pos + 20);
        let uncompressed_size_32 = read_u32(buf, pos + 24);
        let filename_len = read_u16(buf, pos + 28) as usize;
        let extra_len = read_u16(buf, pos + 30) as usize;
        let comment_len = read_u16(buf, pos + 32) as usize;
        let disk_number_32 = read_u16(buf, pos + 34) as u32;
        let local_header_offset_32 = read_u32(buf, pos + 42);

        let filename = String::from_utf8_lossy(&buf[pos + 46..pos + 46 + filename_len]).to_string();

        // Start with 32-bit values; resolve_zip64_extra will upgrade them
        let mut entry = ZipEntry {
            filename,
            compression_method,
            crc32,
            compressed_size: compressed_size_32 as u64,
            uncompressed_size: uncompressed_size_32 as u64,
            local_header_offset: local_header_offset_32 as u64,
            disk_number: disk_number_32,
        };

        // Parse extra field for ZIP64 extensions
        let extra_start = pos + 46 + filename_len;
        let extra_data = &buf[extra_start..extra_start + extra_len];
        resolve_zip64_extra(
            &mut entry,
            extra_data,
            compressed_size_32,
            uncompressed_size_32,
            local_header_offset_32,
            disk_number_32 as u16,
        );

        entries.push(entry);

        pos += 46 + filename_len + extra_len + comment_len;
    }

    entries
}

fn resolve_zip64_extra(
    entry: &mut ZipEntry,
    extra: &[u8],
    compressed_32: u32,
    uncompressed_32: u32,
    offset_32: u32,
    disk_32: u16,
) {
    // Scan extra field for tag 0x0001
    let mut pos = 0;
    while pos + 4 <= extra.len() {
        let tag = read_u16(extra, pos);
        let data_size = read_u16(extra, pos + 2) as usize;

        if tag == 0x0001 {
            // ZIP64 extended info — fields appear IN ORDER but only
            // if the corresponding 32-bit field was the sentinel value
            let mut field_pos = pos + 4;

            if uncompressed_32 == 0xFFFFFFFF {
                entry.uncompressed_size = read_u64(extra, field_pos);
                field_pos += 8;
            }
            if compressed_32 == 0xFFFFFFFF {
                entry.compressed_size = read_u64(extra, field_pos);
                field_pos += 8;
            }
            if offset_32 == 0xFFFFFFFF {
                entry.local_header_offset = read_u64(extra, field_pos);
                field_pos += 8;
            }
            if disk_32 == 0xFFFF {
                entry.disk_number = read_u32(extra, field_pos);
                // field_pos += 4;  // not needed, last field
            }
            return;
        }

        pos += 4 + data_size;
    }
}

/// Returns the total size of the local header (30 + filename_len + extra_len).
/// The file data starts at local_header_offset + this value.
fn parse_local_header_size(buf: &[u8]) -> Result<usize, Box<dyn std::error::Error>> {
    let sig = read_u32(buf, 0);
    if sig != 0x04034b50 {
        return Err(format!("bad local header signature: 0x{:08x}", sig).into());
    }
    let filename_len = read_u16(buf, 26) as usize;
    let extra_len = read_u16(buf, 28) as usize;
    Ok(30 + filename_len + extra_len)
}
