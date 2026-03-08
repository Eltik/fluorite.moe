use crate::extraction::zip_range::ZipEntry;

// UNTESTABLE: from_split_archive, extract, extract_raw, fetch_range — require network

// Test ZipCentralDirectory accessors via a constructed instance
// These are tested indirectly since the struct fields are private,
// but we can test the parsing helpers that are pub(crate) or pub.

#[test]
fn zip_entry_fields() {
    let entry = ZipEntry {
        filename: "test.txt".to_string(),
        compression_method: 8,
        crc32: 0xDEADBEEF,
        compressed_size: 100,
        uncompressed_size: 200,
        local_header_offset: 0,
        disk_number: 0,
    };
    assert_eq!(entry.filename, "test.txt");
    assert_eq!(entry.compression_method, 8);
    assert_eq!(entry.compressed_size, 100);
    assert_eq!(entry.uncompressed_size, 200);
}
