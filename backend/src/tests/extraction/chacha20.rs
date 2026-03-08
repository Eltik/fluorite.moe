use crate::extraction::chacha20::{CHACHA_KEY, chacha20_crypt};

#[test]
fn encrypt_decrypt_roundtrip() {
    let plaintext = b"Hello, ChaCha20!";
    let nonce = [0u8; 12];
    let encrypted = chacha20_crypt(&CHACHA_KEY, &nonce, plaintext, 1);
    assert_ne!(encrypted, plaintext);
    let decrypted = chacha20_crypt(&CHACHA_KEY, &nonce, &encrypted, 1);
    assert_eq!(decrypted, plaintext);
}

#[test]
fn different_nonces_produce_different_output() {
    let plaintext = b"same input data here";
    let nonce_a = [0u8; 12];
    let mut nonce_b = [0u8; 12];
    nonce_b[0] = 1;

    let enc_a = chacha20_crypt(&CHACHA_KEY, &nonce_a, plaintext, 1);
    let enc_b = chacha20_crypt(&CHACHA_KEY, &nonce_b, plaintext, 1);
    assert_ne!(enc_a, enc_b);
}

#[test]
fn different_counters_produce_different_output() {
    let plaintext = b"counter test data";
    let nonce = [0u8; 12];

    let enc_c0 = chacha20_crypt(&CHACHA_KEY, &nonce, plaintext, 0);
    let enc_c1 = chacha20_crypt(&CHACHA_KEY, &nonce, plaintext, 1);
    assert_ne!(enc_c0, enc_c1);
}

#[test]
fn empty_input_returns_empty() {
    let nonce = [0u8; 12];
    let result = chacha20_crypt(&CHACHA_KEY, &nonce, &[], 1);
    assert!(result.is_empty());
}

#[test]
fn output_length_matches_input() {
    let nonce = [0u8; 12];
    for len in [1, 15, 16, 63, 64, 65, 128, 200] {
        let data = vec![0xABu8; len];
        let encrypted = chacha20_crypt(&CHACHA_KEY, &nonce, &data, 1);
        assert_eq!(encrypted.len(), len);
    }
}

#[test]
fn multi_block_roundtrip() {
    // 200 bytes spans 4 ChaCha20 blocks (64 bytes each)
    let plaintext: Vec<u8> = (0..200).map(|i| (i & 0xFF) as u8).collect();
    let nonce = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12];
    let encrypted = chacha20_crypt(&CHACHA_KEY, &nonce, &plaintext, 1);
    let decrypted = chacha20_crypt(&CHACHA_KEY, &nonce, &encrypted, 1);
    assert_eq!(decrypted, plaintext);
}

/// RFC7539 Section 2.4.2 test vector for ChaCha20 encryption.
#[test]
fn rfc7539_test_vector() {
    let key: [u8; 32] = [
        0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x0e,
        0x0f, 0x10, 0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17, 0x18, 0x19, 0x1a, 0x1b, 0x1c, 0x1d,
        0x1e, 0x1f,
    ];
    let nonce: [u8; 12] = [
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x4a, 0x00, 0x00, 0x00, 0x00,
    ];
    let plaintext = b"Ladies and Gentlemen of the class of '99: If I could offer you only one tip for the future, sunscreen would be it.";
    let expected_ciphertext: Vec<u8> = vec![
        0x6e, 0x2e, 0x35, 0x9a, 0x25, 0x68, 0xf9, 0x80, 0x41, 0xba, 0x07, 0x28, 0xdd, 0x0d, 0x69,
        0x81, 0xe9, 0x7e, 0x7a, 0xec, 0x1d, 0x43, 0x60, 0xc2, 0x0a, 0x27, 0xaf, 0xcc, 0xfd, 0x9f,
        0xae, 0x0b, 0xf9, 0x1b, 0x65, 0xc5, 0x52, 0x47, 0x33, 0xab, 0x8f, 0x59, 0x3d, 0xab, 0xcd,
        0x62, 0xb3, 0x57, 0x16, 0x39, 0xd6, 0x24, 0xe6, 0x51, 0x52, 0xab, 0x8f, 0x53, 0x0c, 0x35,
        0x9f, 0x08, 0x61, 0xd8, 0x07, 0xca, 0x0d, 0xbf, 0x50, 0x0d, 0x6a, 0x61, 0x56, 0xa3, 0x8e,
        0x08, 0x8a, 0x22, 0xb6, 0x5e, 0x52, 0xbc, 0x51, 0x4d, 0x16, 0xcc, 0xf8, 0x06, 0x81, 0x8c,
        0xe9, 0x1a, 0xb7, 0x79, 0x37, 0x36, 0x5a, 0xf9, 0x0b, 0xbf, 0x74, 0xa3, 0x5b, 0xe6, 0xb4,
        0x0b, 0x8e, 0xed, 0xf2, 0x78, 0x5e, 0x42, 0x87, 0x4d,
    ];

    let result = chacha20_crypt(&key, &nonce, plaintext, 1);
    assert_eq!(result, expected_ciphertext);
}
