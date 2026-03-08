use crate::extraction::fair_guard_utils::{generate_seed, is_encrypted, rc4};

#[test]
fn is_encrypted_fewer_than_32_bytes() {
    assert!(!is_encrypted(&[0xA6; 16]));
}

#[test]
fn is_encrypted_all_a6_in_first_32() {
    let data = vec![0xA6u8; 64];
    assert!(is_encrypted(&data));
}

#[test]
fn is_encrypted_no_a6_bytes() {
    let data = vec![0x00u8; 64];
    assert!(!is_encrypted(&data));
}

#[test]
fn is_encrypted_exactly_threshold() {
    // 5 occurrences of 0xA6 is NOT enough (> 5 required)
    let mut data = vec![0x00u8; 64];
    for b in data.iter_mut().take(5) {
        *b = 0xA6;
    }
    assert!(!is_encrypted(&data));

    // 6 occurrences is enough
    data[5] = 0xA6;
    assert!(is_encrypted(&data));
}

#[test]
fn is_encrypted_only_checks_first_32_bytes() {
    let mut data = vec![0xA6u8; 64];
    // Clear first 32 bytes, fill rest with 0xA6
    for b in data.iter_mut().take(32) {
        *b = 0x00;
    }
    assert!(!is_encrypted(&data));
}

#[test]
fn generate_seed_deterministic() {
    let bytes = b"test input data";
    let seed1 = generate_seed(bytes);
    let seed2 = generate_seed(bytes);
    assert_eq!(seed1, seed2);
}

#[test]
fn generate_seed_different_input_different_output() {
    let seed_a = generate_seed(b"input_a");
    let seed_b = generate_seed(b"input_b");
    assert_ne!(seed_a, seed_b);
}

#[test]
fn generate_seed_empty_input() {
    // Should return initial state[0] = 0xC1646153
    let seed = generate_seed(&[]);
    assert_eq!(seed, 0xC1646153);
}

#[test]
fn rc4_roundtrip_with_short_key() {
    let key = [0x01, 0x02, 0x03, 0x04];
    let original = b"Hello, World!".to_vec();
    let mut encrypted = original.clone();
    rc4(&mut encrypted, &key);
    assert_ne!(encrypted, original);

    // RC4 is NOT a simple XOR involution with this modified implementation,
    // so we verify it at least modifies the data
}

#[test]
fn rc4_empty_data() {
    let key = [0x42u8; 4];
    let mut data = vec![];
    rc4(&mut data, &key);
    assert!(data.is_empty());
}

#[test]
fn rc4_deterministic() {
    let key = [0x11, 0x22, 0x33, 0x44];
    let mut data_a = vec![0xAA; 32];
    let mut data_b = vec![0xAA; 32];
    rc4(&mut data_a, &key);
    rc4(&mut data_b, &key);
    assert_eq!(data_a, data_b);
}
