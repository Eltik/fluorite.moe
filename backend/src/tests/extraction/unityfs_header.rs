use crate::extraction::unityfs_header::{
    decompress_block_info, parse_directory_entries, parse_header, parse_storage_blocks,
    storage_blocks_offset,
};

#[test]
fn parse_header_not_unityfs() {
    let result = parse_header(b"NotUnity");
    assert!(result.is_err());
}

#[test]
fn parse_header_too_short() {
    let result = parse_header(b"Unity");
    assert!(result.is_err());
}

#[test]
fn parse_header_valid_minimal() {
    // Build a minimal UnityFS header
    let mut buf = Vec::new();
    buf.extend_from_slice(b"UnityFS\0"); // magic (8 bytes)
    buf.extend_from_slice(&6u32.to_be_bytes()); // format_version = 6
    buf.push(b'5'); // player_version = "5"
    buf.push(0); // null terminator
    buf.extend_from_slice(b"2021.3.1f1"); // engine_version
    buf.push(0); // null terminator
    buf.extend_from_slice(&1000i64.to_be_bytes()); // bundle_size
    buf.extend_from_slice(&100u32.to_be_bytes()); // compressed_block_info_size
    buf.extend_from_slice(&200u32.to_be_bytes()); // uncompressed_block_info_size
    buf.extend_from_slice(&0x43u32.to_be_bytes()); // flags (LZ4HC + block_info at end)

    let header = parse_header(&buf).unwrap();
    assert_eq!(header.format_version, 6);
    assert_eq!(header.player_version, "5");
    assert_eq!(header.engine_version, "2021.3.1f1");
    assert_eq!(header.bundle_size, 1000);
    assert_eq!(header.compressed_block_info_size, 100);
    assert_eq!(header.uncompressed_block_info_size, 200);
    assert_eq!(header.block_info_compression(), 3); // 0x43 & 0x3F = 3
    assert!(!header.block_info_at_end()); // 0x43 & 0x80 = 0
}

#[test]
fn parse_header_format_v7_aligns_to_16() {
    let mut buf = Vec::new();
    buf.extend_from_slice(b"UnityFS\0");
    buf.extend_from_slice(&7u32.to_be_bytes()); // format_version = 7
    buf.push(b'5');
    buf.push(0);
    buf.push(b'e');
    buf.push(0);
    buf.extend_from_slice(&1000i64.to_be_bytes());
    buf.extend_from_slice(&100u32.to_be_bytes());
    buf.extend_from_slice(&200u32.to_be_bytes());
    buf.extend_from_slice(&0u32.to_be_bytes());

    let header = parse_header(&buf).unwrap();
    assert_eq!(header.format_version, 7);
    // header_size should be aligned to 16
    assert_eq!(header.header_size % 16, 0);
}

#[test]
fn block_info_at_end_flag() {
    let mut buf = Vec::new();
    buf.extend_from_slice(b"UnityFS\0");
    buf.extend_from_slice(&6u32.to_be_bytes());
    buf.push(b'5');
    buf.push(0);
    buf.push(b'e');
    buf.push(0);
    buf.extend_from_slice(&1000i64.to_be_bytes());
    buf.extend_from_slice(&100u32.to_be_bytes());
    buf.extend_from_slice(&200u32.to_be_bytes());
    buf.extend_from_slice(&0x80u32.to_be_bytes()); // block_info_at_end flag

    let header = parse_header(&buf).unwrap();
    assert!(header.block_info_at_end());
}

#[test]
fn decompress_block_info_none() {
    let data = vec![1, 2, 3, 4];
    let result = decompress_block_info(&data, 4, 0).unwrap();
    assert_eq!(result, data);
}

#[test]
fn decompress_block_info_lzma_not_implemented() {
    let result = decompress_block_info(&[0; 16], 32, 1);
    assert!(result.is_err());
}

#[test]
fn decompress_block_info_unknown_compression() {
    let result = decompress_block_info(&[0; 16], 32, 99);
    assert!(result.is_err());
}

#[test]
fn storage_blocks_offset_at_end() {
    let mut buf = Vec::new();
    buf.extend_from_slice(b"UnityFS\0");
    buf.extend_from_slice(&6u32.to_be_bytes());
    buf.push(b'5');
    buf.push(0);
    buf.push(b'e');
    buf.push(0);
    buf.extend_from_slice(&1000i64.to_be_bytes());
    buf.extend_from_slice(&100u32.to_be_bytes());
    buf.extend_from_slice(&200u32.to_be_bytes());
    buf.extend_from_slice(&0x80u32.to_be_bytes()); // at end

    let header = parse_header(&buf).unwrap();
    // When block info is at end, storage blocks start right after header
    assert_eq!(storage_blocks_offset(&header), header.header_size);
}

#[test]
fn storage_blocks_offset_inline() {
    let mut buf = Vec::new();
    buf.extend_from_slice(b"UnityFS\0");
    buf.extend_from_slice(&6u32.to_be_bytes());
    buf.push(b'5');
    buf.push(0);
    buf.push(b'e');
    buf.push(0);
    buf.extend_from_slice(&1000i64.to_be_bytes());
    buf.extend_from_slice(&100u32.to_be_bytes()); // compressed_block_info_size = 100
    buf.extend_from_slice(&200u32.to_be_bytes());
    buf.extend_from_slice(&0u32.to_be_bytes()); // inline (not at end)

    let header = parse_header(&buf).unwrap();
    assert_eq!(
        storage_blocks_offset(&header),
        header.header_size + header.compressed_block_info_size as usize
    );
}

#[test]
fn parse_storage_blocks_minimal() {
    // 16 bytes hash + u32 count=0
    let mut data = vec![0u8; 20];
    data[16..20].copy_from_slice(&0u32.to_be_bytes());
    let (blocks, offset) = parse_storage_blocks(&data);
    assert!(blocks.is_empty());
    assert_eq!(offset, 20);
}

#[test]
fn parse_storage_blocks_one_block() {
    let mut data = vec![0u8; 30]; // 16 hash + 4 count + 10 block
    data[16..20].copy_from_slice(&1u32.to_be_bytes()); // count = 1
    data[20..24].copy_from_slice(&512u32.to_be_bytes()); // uncompressed_size
    data[24..28].copy_from_slice(&256u32.to_be_bytes()); // compressed_size
    data[28..30].copy_from_slice(&0x02u16.to_be_bytes()); // flags (LZ4)

    let (blocks, offset) = parse_storage_blocks(&data);
    assert_eq!(blocks.len(), 1);
    assert_eq!(blocks[0].uncompressed_size, 512);
    assert_eq!(blocks[0].compressed_size, 256);
    assert_eq!(blocks[0].compression(), 2);
    assert_eq!(offset, 30);
}

#[test]
fn parse_directory_entries_empty() {
    let mut data = vec![0u8; 4];
    data[0..4].copy_from_slice(&0u32.to_be_bytes()); // count = 0
    let entries = parse_directory_entries(&data, 0);
    assert!(entries.is_empty());
}

#[test]
fn parse_directory_entries_one_entry() {
    let mut data = Vec::new();
    data.extend_from_slice(&1u32.to_be_bytes()); // count = 1
    data.extend_from_slice(&100i64.to_be_bytes()); // offset
    data.extend_from_slice(&50i64.to_be_bytes()); // size
    data.extend_from_slice(&0u32.to_be_bytes()); // flags
    data.extend_from_slice(b"CAB-test\0"); // path with null terminator

    let entries = parse_directory_entries(&data, 0);
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].offset, 100);
    assert_eq!(entries[0].size, 50);
    assert_eq!(entries[0].path, "CAB-test");
}
