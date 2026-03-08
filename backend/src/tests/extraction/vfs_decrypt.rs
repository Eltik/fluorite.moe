use crate::extraction::crypto_constants::*;
use crate::extraction::vfs_decrypt::{VfsStorageBlock, decrypt_block, is_valid_header};

#[test]
fn is_valid_header_too_short() {
    assert!(!is_valid_header(&[0; 4]));
}

#[test]
fn is_valid_header_zeroes_invalid() {
    assert!(!is_valid_header(&[0; 8]));
}

#[test]
fn is_valid_header_correct_pair() {
    // Build a valid header checksum pair
    let a_raw: u32 = 0x12345678;
    let a = a_raw ^ HEADER_XOR_A;
    let xored = a_raw;
    let c1 = 4u32.wrapping_mul(xored) & 0xFFFF0000;
    let c2 = xored.rotate_right(14);
    let b = c1 ^ c2 ^ HEADER_XOR_B;

    let mut data = [0u8; 8];
    data[0..4].copy_from_slice(&(a ^ HEADER_XOR_A).to_be_bytes());
    // Wait - a IS a_raw ^ HEADER_XOR_A, so the 8 bytes store a and b in BE
    // is_valid_header reads a = BE(data[0..4]), b = BE(data[4..8])
    // xored = a ^ HEADER_XOR_A should give a_raw
    data[0..4].copy_from_slice(&a.to_be_bytes());
    data[4..8].copy_from_slice(&b.to_be_bytes());

    assert!(is_valid_header(&data));
}

#[test]
fn is_valid_header_corrupted_pair() {
    let a: u32 = 0x12345678;
    let mut data = [0u8; 8];
    data[0..4].copy_from_slice(&a.to_be_bytes());
    data[4..8].copy_from_slice(&0xDEADBEEFu32.to_be_bytes());
    assert!(!is_valid_header(&data));
}

#[test]
fn decrypt_block_small_buffer() {
    // <= 256 bytes: full AES decrypt
    let mut data = vec![0xAA; 128];
    let original = data.clone();
    decrypt_block(&mut data);
    assert_ne!(data, original);
}

#[test]
fn decrypt_block_large_buffer() {
    // > 256 bytes: scatter/gather AES
    let mut data = vec![0xBB; 1024];
    let original = data.clone();
    decrypt_block(&mut data);
    assert_ne!(data, original);
}

#[test]
fn decrypt_block_exactly_256_bytes() {
    let mut data = vec![0xCC; 256];
    let original = data.clone();
    decrypt_block(&mut data);
    assert_ne!(data, original);
}

#[test]
fn decrypt_block_257_bytes_uses_scatter_path() {
    let mut data = vec![0xDD; 257];
    let original = data.clone();
    decrypt_block(&mut data);
    assert_ne!(data, original);
}

#[test]
fn vfs_storage_block_compression() {
    let block = VfsStorageBlock {
        compressed_size: 100,
        uncompressed_size: 200,
        flags: 0x45, // compression bits = 0x05
    };
    assert_eq!(block.compression(), 5);
}

#[test]
fn vfs_storage_block_compression_zero() {
    let block = VfsStorageBlock {
        compressed_size: 100,
        uncompressed_size: 100,
        flags: 0x00,
    };
    assert_eq!(block.compression(), 0);
}
