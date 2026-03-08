use crate::extraction::crypto_constants::{VFS_AES_IV, VFS_AES_KEY};
use crate::extraction::vfs_aes::decrypt;

#[test]
fn decrypt_modifies_data() {
    let mut data = vec![0u8; 32];
    let original = data.clone();
    decrypt(&mut data, &VFS_AES_KEY, &VFS_AES_IV);
    assert_ne!(data, original);
}

#[test]
fn decrypt_different_keys_produce_different_output() {
    let mut data_a = vec![0xAA; 32];
    let mut data_b = data_a.clone();
    let key_b = [0x01u8; 16];
    decrypt(&mut data_a, &VFS_AES_KEY, &VFS_AES_IV);
    decrypt(&mut data_b, &key_b, &VFS_AES_IV);
    assert_ne!(data_a, data_b);
}

#[test]
fn decrypt_different_ivs_produce_different_output() {
    let mut data_a = vec![0xBB; 32];
    let mut data_b = data_a.clone();
    let iv_b = [0x01u8; 16];
    decrypt(&mut data_a, &VFS_AES_KEY, &VFS_AES_IV);
    decrypt(&mut data_b, &VFS_AES_KEY, &iv_b);
    assert_ne!(data_a, data_b);
}

#[test]
fn decrypt_partial_block_handled() {
    // 20 bytes = 1 full block (16) + 4 remaining
    let mut data = vec![0xCC; 20];
    // Should not panic on partial final block
    decrypt(&mut data, &VFS_AES_KEY, &VFS_AES_IV);
}

#[test]
fn decrypt_single_block() {
    let mut data = vec![0xDD; 16];
    decrypt(&mut data, &VFS_AES_KEY, &VFS_AES_IV);
    // Just verify it runs without panic and modifies data
    assert_ne!(data, vec![0xDD; 16]);
}

#[test]
fn decrypt_empty_data_noop() {
    let mut data = vec![];
    decrypt(&mut data, &VFS_AES_KEY, &VFS_AES_IV);
    assert!(data.is_empty());
}

#[test]
fn decrypt_deterministic() {
    let mut data_a = vec![0xEE; 48];
    let mut data_b = data_a.clone();
    decrypt(&mut data_a, &VFS_AES_KEY, &VFS_AES_IV);
    decrypt(&mut data_b, &VFS_AES_KEY, &VFS_AES_IV);
    assert_eq!(data_a, data_b);
}
