use crate::extraction::wwise_pck::{
    AKPK_MAGIC, SMILEY_MAGIC, decipher_inplace, decrypt_header, is_smiley_format, parse_header,
};

// ── Migrated from src/extraction/wwise_pck.rs ──

#[test]
fn cipher_key_word_deterministic() {
    // cipher_key_word is private, but we test it indirectly through decipher_inplace
    let mut data_a = [0u8; 8];
    let mut data_b = [0u8; 8];
    decipher_inplace(&mut data_a, 42, 0);
    decipher_inplace(&mut data_b, 42, 0);
    assert_eq!(data_a, data_b);
}

#[test]
fn decipher_roundtrip() {
    let original = [0x41u8, 0x4B, 0x50, 0x4B, 0x01, 0x00, 0x00, 0x00];
    let mut data = original;
    decipher_inplace(&mut data, 100, 0);
    assert_ne!(data, original);
    decipher_inplace(&mut data, 100, 0);
    assert_eq!(data, original);
}

#[test]
fn is_smiley_format_valid() {
    assert!(is_smiley_format(b":)xD\x00\x00\x00\x00"));
    assert!(!is_smiley_format(b"AKPK\x00\x00\x00\x00"));
    assert!(!is_smiley_format(b":)xD")); // too short (< 8 bytes)
}

// ── New tests ──

#[test]
fn is_smiley_format_empty_and_short() {
    assert!(!is_smiley_format(&[]));
    assert!(!is_smiley_format(&[0x3A]));
    assert!(!is_smiley_format(&[
        0x3A, 0x29, 0x78, 0x44, 0x00, 0x00, 0x00
    ])); // 7 bytes
}

#[test]
fn decipher_different_seeds_produce_different_output() {
    let mut data_a = [1u8, 2, 3, 4, 5, 6, 7, 8];
    let mut data_b = [1u8, 2, 3, 4, 5, 6, 7, 8];
    decipher_inplace(&mut data_a, 1, 0);
    decipher_inplace(&mut data_b, 2, 0);
    assert_ne!(data_a, data_b);
}

#[test]
fn decipher_different_offsets_produce_different_output() {
    let mut data_a = [1u8, 2, 3, 4, 5, 6, 7, 8];
    let mut data_b = [1u8, 2, 3, 4, 5, 6, 7, 8];
    decipher_inplace(&mut data_a, 0, 0);
    decipher_inplace(&mut data_b, 0, 1);
    assert_ne!(data_a, data_b);
}

#[test]
fn decipher_trailing_bytes_unchanged() {
    // 6 bytes: only first 4 (1 word) get deciphered, last 2 unchanged
    let mut data = [0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF];
    let trailing_orig = [data[4], data[5]];
    decipher_inplace(&mut data, 0, 0);
    assert_eq!([data[4], data[5]], trailing_orig);
}

#[test]
fn smiley_magic_constant_matches() {
    assert_eq!(&SMILEY_MAGIC, b":)xD");
}

#[test]
fn akpk_magic_constant_matches() {
    assert_eq!(&AKPK_MAGIC, b"AKPK");
}

#[test]
fn decrypt_header_too_short_returns_err() {
    let result = decrypt_header(&[0; 8]);
    assert!(result.is_err());
}

#[test]
fn decrypt_header_wrong_magic_returns_err() {
    let mut data = vec![0u8; 64];
    data[0..4].copy_from_slice(b"AKPK");
    let result = decrypt_header(&data);
    assert!(result.is_err());
}

#[test]
fn decrypt_header_truncated_body_returns_err() {
    let mut data = vec![0u8; 16];
    data[0..4].copy_from_slice(&SMILEY_MAGIC);
    // header_size = 1000 (way beyond data length)
    data[4..8].copy_from_slice(&1000u32.to_le_bytes());
    let result = decrypt_header(&data);
    assert!(result.is_err());
}

#[test]
fn parse_header_too_short_returns_err() {
    let result = parse_header(&[0; 8], 100);
    assert!(result.is_err());
}

#[test]
fn parse_header_section_sizes_exceed_header_returns_err() {
    let mut data = vec![0u8; 20];
    // lang_map_size = 1000 (way too large)
    data[0..4].copy_from_slice(&1000u32.to_le_bytes());
    data[4..8].copy_from_slice(&0u32.to_le_bytes());
    data[8..12].copy_from_slice(&0u32.to_le_bytes());
    data[12..16].copy_from_slice(&0u32.to_le_bytes());
    let result = parse_header(&data, 100);
    assert!(result.is_err());
}

#[test]
fn parse_header_all_empty_sections() {
    let mut data = vec![0u8; 16];
    data[0..4].copy_from_slice(&0u32.to_le_bytes()); // lang_map_size
    data[4..8].copy_from_slice(&0u32.to_le_bytes()); // banks_size
    data[8..12].copy_from_slice(&0u32.to_le_bytes()); // streams_size
    data[12..16].copy_from_slice(&0u32.to_le_bytes()); // externals_size
    let pck = parse_header(&data, 100).unwrap();
    assert!(pck.languages.is_empty());
    assert!(pck.banks.is_empty());
    assert!(pck.streams.is_empty());
    assert!(pck.externals.is_empty());
    assert_eq!(pck.all_files().count(), 0);
}
