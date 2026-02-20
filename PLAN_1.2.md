# PLAN 1.2: ZIP Central Directory Parser & File Extractor

## Goal

Implement the body of every method in `src/extraction/zip_range.rs` so that the module can:

1. Parse the ZIP64 central directory from a remote split archive using HTTP Range requests
2. Build an in-memory lookup table of all 1,049 files
3. Extract individual files by name, decompressing deflate if needed

Total network cost for parsing: ~148 KB. Per-file extraction: local header (~1 KB) + compressed data.

---

## Table of Contents

- [Prerequisites](#prerequisites)
- [Binary Format Reference](#binary-format-reference)
- [Implementation Guide](#implementation-guide)
  - [A. Imports & Utility Functions](#a-imports--utility-functions)
  - [B. HTTP Range Fetching](#b-http-range-fetching)
  - [C. Global-to-Part Offset Mapping](#c-global-to-part-offset-mapping)
  - [D. Cross-Part Range Fetching](#d-cross-part-range-fetching)
  - [E. EOCD Discovery & Parsing](#e-eocd-discovery--parsing)
  - [F. ZIP64 EOCD Locator & Record Parsing](#f-zip64-eocd-locator--record-parsing)
  - [G. Central Directory Entry Parsing](#g-central-directory-entry-parsing)
  - [H. ZIP64 Extra Field Resolution](#h-zip64-extra-field-resolution)
  - [I. `from_split_archive` — The Main Constructor](#i-from_split_archive--the-main-constructor)
  - [J. Accessor Methods](#j-accessor-methods)
  - [K. Local Header Parsing](#k-local-header-parsing)
  - [L. `extract` and `extract_raw`](#l-extract-and-extract_raw)
- [Test Binary](#test-binary)
- [Where Else This Logic Applies](#where-else-this-logic-applies)
- [Verification Checklist](#verification-checklist)

---

## Prerequisites

Make sure these are already in place (they should be from Step 1.1):

```toml
# Cargo.toml
reqwest = { version = "0.13.2", features = ["json", "query"] }
flate2 = "1.1.9"

[[bin]]
name = "zip-range-test"
path = "src/bin/zip_range_test.rs"
```

```rust
// src/extraction/mod.rs
pub mod zip_range;
```

---

## Binary Format Reference

All multi-byte integers in ZIP are **little-endian** unless otherwise noted.

### End of Central Directory (EOCD) — 22 bytes minimum

```
Offset  Size  Field
------  ----  -----
0       4     Signature: 0x06054b50
4       2     Disk number (this disk)
6       2     Disk where CD starts
8       2     Number of CD entries on this disk
10      2     Total CD entries (0xFFFF if ZIP64)
12      4     CD size in bytes (0xFFFFFFFF if ZIP64)
16      4     CD offset from start of archive (0xFFFFFFFF if ZIP64)
20      2     Comment length
22      var   Comment
```

### ZIP64 End of Central Directory Locator — 20 bytes

Located **immediately before** the EOCD record (i.e., at `eocd_offset - 20`).

```
Offset  Size  Field
------  ----  -----
0       4     Signature: 0x07064b50
4       4     Disk number containing ZIP64 EOCD
8       8     Relative offset of ZIP64 EOCD record
16      4     Total number of disks
```

### ZIP64 End of Central Directory Record — 56 bytes minimum

```
Offset  Size  Field
------  ----  -----
0       4     Signature: 0x06064b50
4       8     Size of this record (minus 12)
12      2     Version made by
14      2     Version needed to extract
16      4     Disk number
20      4     Disk where CD starts
24      8     Total CD entries on this disk
32      8     Total CD entries
40      8     CD size in bytes
48      8     CD offset from start of archive
```

### Central Directory File Header — 46 bytes + variable

```
Offset  Size  Field
------  ----  -----
0       4     Signature: 0x02014b50
4       2     Version made by
6       2     Version needed
8       2     General purpose bit flag
10      2     Compression method (0=stored, 8=deflate)
12      2     Last mod time
14      2     Last mod date
16      4     CRC-32
20      4     Compressed size (0xFFFFFFFF -> see ZIP64 extra)
24      4     Uncompressed size (0xFFFFFFFF -> see ZIP64 extra)
28      2     Filename length (n)
30      2     Extra field length (m)
32      2     File comment length (k)
34      2     Disk number start (0xFFFF -> see ZIP64 extra)
36      2     Internal file attributes
38      4     External file attributes
42      4     Relative offset of local header (0xFFFFFFFF -> see ZIP64 extra)
46      n     Filename
46+n    m     Extra field
46+n+m  k     File comment
```

### ZIP64 Extended Information Extra Field (tag `0x0001`)

Found inside the extra field of a CD entry when any of the size/offset fields are set to their sentinel values (0xFFFFFFFF or 0xFFFF). Fields appear **in order**, but only if the corresponding CD field had the sentinel value:

```
Offset  Size  Field                   Present when CD field is...
------  ----  -----                   --------------------------
0       2     Tag: 0x0001
2       2     Data size
4       8     Uncompressed size       CD uncompressed_size == 0xFFFFFFFF
4/12    8     Compressed size         CD compressed_size == 0xFFFFFFFF
4/12/20 8     Local header offset     CD local_header_offset == 0xFFFFFFFF
4-28    4     Disk number start       CD disk_number == 0xFFFF
```

### Local File Header — 30 bytes + variable

```
Offset  Size  Field
------  ----  -----
0       4     Signature: 0x04034b50
4       2     Version needed
6       2     General purpose bit flag
8       2     Compression method
10      2     Last mod time
12      2     Last mod date
14      4     CRC-32
18      4     Compressed size
22      4     Uncompressed size
26      2     Filename length (n)
28      2     Extra field length (m)
30      n     Filename
30+n    m     Extra field
```

Data starts at offset `30 + n + m` from the start of the local header.

---

## Implementation Guide

### A. Imports & Utility Functions

At the top of `zip_range.rs`, add:

```rust
use std::collections::HashMap;
use std::io::Read as _;
use reqwest::Client;
use flate2::read::DeflateDecoder;
```

Then add little-endian binary readers. Follow the same `from_le_bytes` + `try_into().unwrap()` pattern used in `fair_guard_utils.rs:15-17`:

```rust
fn read_u16(buf: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes(buf[offset..offset + 2].try_into().unwrap())
}

fn read_u32(buf: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(buf[offset..offset + 4].try_into().unwrap())
}

fn read_u64(buf: &[u8], offset: usize) -> u64 {
    u64::from_le_bytes(buf[offset..offset + 8].try_into().unwrap())
}
```

**Reference:** `fair_guard_utils.rs:15-17` does the same with a closure:
```rust
let read_u32_le = |data: &[u8], idx: usize| -> u32 {
    u32::from_le_bytes(data[idx * 4..idx * 4 + 4].try_into().unwrap())
};
```

The difference here is that `read_u32` takes a byte offset, not an index of u32s. This is more natural for ZIP parsing where offsets vary.

---

### B. HTTP Range Fetching

A single-URL Range request helper:

```rust
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
            resp.status(), start, end_inclusive, url
        ).into());
    }

    Ok(resp.bytes().await?.to_vec())
}
```

**Key points:**
- The CDN returns HTTP 206 Partial Content for valid Range requests.
- If the server returns 200 instead of 206, it sent the whole file (wrong). If 416, the range is invalid.
- Always validate the status code.

**Reference:** `launcher.rs:78-85` shows the pattern for making HTTP requests with reqwest. The difference here is that we add a `Range` header and check for 206 instead of 200.

---

### C. Global-to-Part Offset Mapping

The split archive is conceptually one big file. A "global offset" is a byte offset into the concatenated stream of all parts. To figure out which part a global offset falls in, use the `cumulative_sizes` prefix sum array.

```rust
/// Returns (part_index, offset_within_part)
fn global_to_part(cumulative_sizes: &[u64], global_offset: u64) -> (usize, u64) {
    // cumulative_sizes[i] = sum of sizes of parts 0..i
    // Example: parts of size [1GB, 1GB, 476MB]
    //   cumulative_sizes = [0, 1GB, 2GB, 2GB+476MB]
    // Binary search: find rightmost i where cumulative_sizes[i] <= global_offset
    let part_idx = cumulative_sizes
        .partition_point(|&cum| cum <= global_offset)
        - 1;
    let offset_in_part = global_offset - cumulative_sizes[part_idx];
    (part_idx, offset_in_part)
}
```

**How to build `cumulative_sizes`:**

```rust
let mut cumulative_sizes = Vec::with_capacity(parts.len() + 1);
cumulative_sizes.push(0);
for part in &parts {
    let last = *cumulative_sizes.last().unwrap();
    cumulative_sizes.push(last + part.size);
}
// cumulative_sizes.len() == parts.len() + 1
```

**Where else this pattern is used:**
- Any split/segmented file format. For example, VMDK split images, RAR multi-volume archives, and game engine PAK files with multiple segments all use the same "prefix sum + binary search" technique.
- In databases, B-tree page location often uses similar offset calculations for memory-mapped segmented files.

---

### D. Cross-Part Range Fetching

When a byte range spans two (or more) parts, split it into per-part sub-ranges:

```rust
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

    for part_idx in start_part..=end_part {
        let range_start = if part_idx == start_part {
            start_offset
        } else {
            0
        };
        let range_end = if part_idx == end_part {
            end_offset
        } else {
            parts[part_idx].size - 1
        };

        let chunk = fetch_range(client, &parts[part_idx].url, range_start, range_end).await?;
        result.extend_from_slice(&chunk);
    }

    Ok(result)
}
```

**Important:** Most files in this archive won't span parts. The parts are 1 GiB each, so only files larger than the remaining space in a part will span. The central directory itself is ~147 KB and sits in the last part, so it never spans.

**Where else this pattern is used:**
- HTTP video streaming players (DASH/HLS) that stitch segments together
- Database storage engines reading records that cross page boundaries
- Memory-mapped files with segment windows

---

### E. EOCD Discovery & Parsing

The EOCD record sits at the very end of the archive (end of the last part). Its signature is `0x06054b50`. To find it, download the last N bytes of the last part and scan backwards.

```rust
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
```

The reason we scan backwards is that the EOCD can be preceded by a variable-length comment. In practice, most ZIP files have no comment and the EOCD starts exactly 22 bytes from the end.

**Parsing the EOCD:**

```rust
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
```

If `cd_entries_total == 0xFFFF` or `cd_size == 0xFFFFFFFF` or `cd_offset == 0xFFFFFFFF`, the archive uses ZIP64 and you must look at the ZIP64 EOCD record instead.

---

### F. ZIP64 EOCD Locator & Record Parsing

The ZIP64 EOCD Locator is a 20-byte record located immediately before the EOCD:

```rust
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
```

The ZIP64 EOCD record itself:

```rust
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
```

**Critical detail:** The `zip64_eocd_offset` from the locator is a **global offset** into the concatenated archive, not an offset within the last part. However, for the Endfield archive, both the ZIP64 EOCD and the EOCD are in the last part, so in practice you can compute the offset within the last part as:

```
offset_in_last_part = zip64_eocd_offset - cumulative_sizes[last_part_index]
```

But to keep things general, fetch it via `fetch_global_range`.

**Where else this pattern is used:**
- Any tool that works with large ZIP archives (> 4 GiB): 7-Zip, WinRAR, Python's `zipfile` module
- The pattern of "find a footer record, then follow a pointer to a larger structure" is extremely common: SQLite databases have a similar structure (the file header points to the root page of the B-tree)

---

### G. Central Directory Entry Parsing

Parse the central directory buffer into a `Vec<ZipEntry>`:

```rust
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

        let filename = String::from_utf8_lossy(
            &buf[pos + 46..pos + 46 + filename_len]
        ).to_string();

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
```

**Key point:** The 32-bit fields in the CD header act as "sentinels." When a value is `0xFFFFFFFF` (for u32) or `0xFFFF` (for u16), the real value is in the ZIP64 extra field. You must check each one.

---

### H. ZIP64 Extra Field Resolution

The ZIP64 extra field (tag `0x0001`) contains the actual u64 values for any field that overflowed its 32-bit slot:

```rust
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
```

**Critical detail:** The fields in the ZIP64 extra appear in a fixed order (`uncompressed`, `compressed`, `offset`, `disk`), but only the ones whose 32-bit counterpart was the sentinel value are present. If you read them out of order, or read a field that isn't present, you'll get corrupted data.

**Where else this pattern is used:**
- PNG chunks have the same "tag + length + data" structure for extensible metadata (tEXt, iTXt, pHYs, etc.)
- TIFF/EXIF IFD entries: a base value that can overflow to an offset pointing to a larger value
- ELF program headers: 32-bit ELF uses 4-byte fields, 64-bit ELF uses 8-byte fields in the same structural positions

---

### I. `from_split_archive` — The Main Constructor

This is the public entry point. It orchestrates everything above:

```rust
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
        client, &parts, &cumulative_sizes,
        tail_start, total_size - 1,
    ).await?;

    // 3. Find EOCD signature in the tail
    let eocd_pos_in_tail = find_eocd_signature(&tail)
        .ok_or("EOCD signature not found in last 1024 bytes")?;
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
            client, &parts, &cumulative_sizes,
            locator.zip64_eocd_offset,
            locator.zip64_eocd_offset + 55,
        ).await?;

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
        client, &parts, &cumulative_sizes,
        cd_offset, cd_offset + cd_size - 1,
    ).await?;

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
```

**Network requests made:**
1. 1 request: last 1024 bytes (EOCD tail)
2. 0 or 1 requests: ZIP64 EOCD record (56 bytes) — only if the initial tail didn't include it, which it almost always does since the ZIP64 EOCD is right before the EOCD locator which is right before the EOCD. In practice for this archive, a 1024-byte tail should capture all three records.
3. 1 request: central directory (~147 KB)

**Optimization note:** If the ZIP64 EOCD is within the 1024-byte tail (which it usually is), you can parse it directly from `tail` instead of making another Range request. To determine this:
```rust
let z64_eocd_global = locator.zip64_eocd_offset;
let tail_global_start = total_size - tail_size;
if z64_eocd_global >= tail_global_start {
    // Parse from tail directly
    let local_offset = (z64_eocd_global - tail_global_start) as usize;
    let z64 = parse_zip64_eocd(&tail[local_offset..]);
}
```

---

### J. Accessor Methods

These are straightforward:

```rust
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
```

---

### K. Local Header Parsing

When extracting a file, you first need to read the local file header to find where the actual data starts. The local header has a different `extra_len` than the central directory entry (they can differ!), so you must parse it from the file itself.

```rust
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
```

**Critical detail:** The local header `extra_len` can differ from the central directory's `extra_len`. In ZIP64, the local header often has a ZIP64 extra field that the central directory entry also has, but their sizes can vary. Always parse the actual local header to get the true data offset.

---

### L. `extract` and `extract_raw`

```rust
pub async fn extract(
    &self,
    client: &Client,
    filename: &str,
) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let entry = self.entries.get(filename)
        .ok_or_else(|| format!("file not found in archive: {}", filename))?;

    // 1. Fetch local header (first 1024 bytes should be enough)
    let header_fetch_size: u64 = 1024;
    let header_buf = fetch_global_range(
        client, &self.parts, &self.cumulative_sizes,
        entry.local_header_offset,
        entry.local_header_offset + header_fetch_size - 1,
    ).await?;

    let header_size = parse_local_header_size(&header_buf)? as u64;

    // 2. Fetch compressed data
    let data_start = entry.local_header_offset + header_size;
    let data_end = data_start + entry.compressed_size - 1;
    let compressed = fetch_global_range(
        client, &self.parts, &self.cumulative_sizes,
        data_start, data_end,
    ).await?;

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

pub async fn extract_raw(
    &self,
    client: &Client,
    filename: &str,
) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let entry = self.entries.get(filename)
        .ok_or_else(|| format!("file not found in archive: {}", filename))?;

    let header_fetch_size: u64 = 1024;
    let header_buf = fetch_global_range(
        client, &self.parts, &self.cumulative_sizes,
        entry.local_header_offset,
        entry.local_header_offset + header_fetch_size - 1,
    ).await?;

    let header_size = parse_local_header_size(&header_buf)? as u64;
    let data_start = entry.local_header_offset + header_size;
    let data_end = data_start + entry.compressed_size - 1;

    fetch_global_range(
        client, &self.parts, &self.cumulative_sizes,
        data_start, data_end,
    ).await
}
```

**Important: `DeflateDecoder`, NOT `GzDecoder` or `ZlibDecoder`.**

ZIP's "deflate" (method 8) uses **raw deflate** — no zlib header, no gzip wrapper. `flate2::read::DeflateDecoder` handles this correctly. Using `ZlibDecoder` or `GzDecoder` will fail because they expect additional framing bytes that ZIP doesn't use.

**Where else this pattern is used:**
- HTTP `Content-Encoding: deflate` also uses raw deflate (in theory, though some servers incorrectly send zlib-wrapped)
- PNG IDAT chunks store zlib-wrapped deflate (different from ZIP!)
- The pattern of "fetch header, parse variable-length prefix, then fetch data at computed offset" is universal in binary formats: ELF section loading, PE import table parsing, PDF cross-reference table resolution

---

## Test Binary

Create `src/bin/zip_range_test.rs`:

```rust
use backend::core::endfield::{
    config::{ResolveParams, resolve_game_config},
    constants::{app_code, channel, sub_channel},
};
use backend::core::hypergryph::launcher::Platform;
use backend::extraction::zip_range::{SplitArchivePart, ZipCentralDirectory};
use reqwest::Client;

fn format_bytes(bytes: u64) -> String {
    const UNITS: &[&str] = &["B", "KB", "MB", "GB", "TB"];
    if bytes == 0 { return "0 B".into(); }
    let mut size = bytes as f64;
    let mut i = 0;
    while size >= 1024.0 && i < UNITS.len() - 1 {
        size /= 1024.0;
        i += 1;
    }
    format!("{:.2} {}", size, UNITS[i])
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let client = Client::new();

    // 1. Resolve game config to get pack URLs
    println!("=== Resolving game config ===");
    let params = ResolveParams {
        app_code: app_code::game::OS_WIN_REL,
        launcher_app_code: app_code::launcher::OS_WIN_REL,
        channel: channel::OS_WIN_REL,
        sub_channel: sub_channel::OS_WIN_REL,
        launcher_sub_channel: sub_channel::OS_WIN_REL,
        platform: Platform::Windows,
    };
    let config = resolve_game_config(&client, &params).await?;
    println!("Version: {}, Packs: {}", config.version, config.packs.len());

    // 2. Build SplitArchivePart list from packs
    let parts: Vec<SplitArchivePart> = config.packs.iter().map(|p| {
        SplitArchivePart {
            url: p.url.clone(),
            size: p.package_size.parse::<u64>().expect("invalid package_size"),
        }
    }).collect();
    println!("Total parts: {}", parts.len());

    // 3. Parse central directory
    println!("\n=== Parsing ZIP central directory ===");
    let cd = ZipCentralDirectory::from_split_archive(&client, parts).await?;
    println!("Total entries: {}", cd.len());

    // 4. List .chk files
    let mut chk_files: Vec<_> = cd.filenames()
        .filter(|f| f.ends_with(".chk"))
        .collect();
    chk_files.sort();
    println!("\n=== .chk files in archive ({}) ===", chk_files.len());
    for name in &chk_files {
        if let Some(entry) = cd.get(name) {
            println!(
                "  {} (compressed: {}, uncompressed: {}, method: {})",
                name,
                format_bytes(entry.compressed_size),
                format_bytes(entry.uncompressed_size),
                entry.compression_method,
            );
        }
    }

    // 5. Extract the smallest .chk as proof of concept
    if let Some(&smallest) = chk_files.iter().min_by_key(|&&name| {
        cd.get(name).map_or(u64::MAX, |e| e.compressed_size)
    }) {
        println!("\n=== Extracting smallest .chk: {} ===", smallest);
        let entry = cd.get(smallest).unwrap();
        println!("Compressed: {}, Uncompressed: {}",
            format_bytes(entry.compressed_size),
            format_bytes(entry.uncompressed_size),
        );

        let data = cd.extract(&client, smallest).await?;
        println!("Extracted {} bytes", data.len());

        // Peek at first few bytes to verify it looks like data
        if data.len() >= 8 {
            println!(
                "First 8 bytes: {:02x?}",
                &data[..8]
            );
        }
    }

    println!("\nDone.");
    Ok(())
}
```

---

## Where Else This Logic Applies

### Binary Parsing Patterns (`read_u16`/`read_u32`/`read_u64`)

Used everywhere that reads binary formats:

| Location | What it parses |
|---|---|
| `extraction/fair_guard_utils.rs:15-17` | FairGuard encrypted block (u32 LE reads) |
| **Future:** `extraction/lz4inv.rs` | LZ4Inv token bytes and big-endian chunk end offsets |
| **Future:** `core/unity/bundle.rs` | UnityFS header (magic, version, sizes) — note: Unity uses **big-endian** for some header fields! |
| **Future:** `core/unity/serialized_file.rs` | SerializedFile metadata (type tree, object table) |

**Tip:** If you find yourself needing both LE and BE readers (Unity headers are mixed-endian), add `read_u32_be` and `read_u64_be` alongside the LE variants.

### HTTP Range Request Pattern (`fetch_range`)

Any future module that needs partial downloads from CDN:

| Location | Use case |
|---|---|
| `extraction/zip_range.rs` | ZIP central directory + file extraction |
| **Future:** selective `.chk` download | If we can identify byte ranges within the ZIP for specific `.chk` files, we can download only those ranges instead of entire parts |
| **Future:** incremental catalog updates | Download only the diff patch files for changed chunks |
| **Future:** GameAssembly.dll partial download | If we ever need IL2CPP headers (unlikely), we could Range-request just the first ~1 MB |

### Prefix Sum + Binary Search (`global_to_part`)

Offset mapping for any segmented/chunked format:

| Location | Use case |
|---|---|
| `extraction/zip_range.rs` | Split archive part lookup |
| **Future:** `.chk` block file parsing | A `.chk` contains multiple concatenated UnityFS bundles. After parsing each bundle's header (which declares its size), you build a prefix sum to find which bundle a given byte offset is in |
| **Future:** UnityFS block decompression | UnityFS bundles have a `block_info` section listing compressed/uncompressed sizes for each storage block. To find which block contains a given uncompressed offset, you'd use the same prefix sum technique |
| **Future:** asset catalog | Mapping asset names to `(chk_file, bundle_index, offset_within_bundle)` |

### Deflate Decompression (`DeflateDecoder`)

| Location | Use case |
|---|---|
| `extraction/zip_range.rs` | ZIP deflate method 8 |
| **Future:** Unity LZMA bundles | Some Unity bundles use LZMA (method 14 in Unity's flags) — same pattern but with `lzma-rs` |
| **Future:** LZ4/LZ4Inv blocks | The primary compression in Endfield's bundles — use custom LZ4Inv or `lz4_flex` |

### Error Pattern (`Box<dyn std::error::Error>`)

Every function in this module uses `Result<T, Box<dyn std::error::Error>>` to match the project convention established in `config.rs` and `launcher.rs`. If you later want structured errors, you could define:

```rust
#[derive(Debug)]
pub enum ZipRangeError {
    Http(reqwest::Error),
    InvalidFormat(String),
    FileNotFound(String),
    UnsupportedCompression(u16),
    Io(std::io::Error),
}
```

But `Box<dyn Error>` is fine for now and consistent with the rest of the codebase.

---

## Verification Checklist

After implementing all the bodies:

1. **`cargo check`** — compiles without errors
2. **`cargo run --bin zip-range-test`** — runs against live CDN:
   - Prints total entries (expect ~1,049)
   - Lists `.chk` files found in the archive
   - Successfully extracts the smallest `.chk` file
   - Prints extracted file size and first bytes
3. **Manual verification:**
   - Central directory download should be ~147 KB (check network logs or add a println)
   - All filenames should be valid UTF-8 paths
   - `.chk` files should match the entries listed in `patch.json`
   - Extracted `.chk` file size should match the uncompressed size from the CD entry
4. **Edge cases to watch:**
   - Files with `compressed_size == 0` and `uncompressed_size == 0` (empty files or directories)
   - Filenames with path separators (e.g., `Endfield_Data/StreamingAssets/VFS/...`)
   - The last entry in the central directory (make sure parsing doesn't overrun the buffer)
