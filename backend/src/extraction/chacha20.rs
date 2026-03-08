/// ChaCha20 (RFC7539) cipher implementation for VFS decryption.
fn rotl32(v: u32, n: u32) -> u32 {
    v.rotate_left(n)
}

fn quarter_round(s: &mut [u32; 16], a: usize, b: usize, c: usize, d: usize) {
    s[a] = s[a].wrapping_add(s[b]);
    s[d] ^= s[a];
    s[d] = rotl32(s[d], 16);
    s[c] = s[c].wrapping_add(s[d]);
    s[b] ^= s[c];
    s[b] = rotl32(s[b], 12);
    s[a] = s[a].wrapping_add(s[b]);
    s[d] ^= s[a];
    s[d] = rotl32(s[d], 8);
    s[c] = s[c].wrapping_add(s[d]);
    s[b] ^= s[c];
    s[b] = rotl32(s[b], 7);
}

fn chacha20_block(key: &[u8; 32], counter: u32, nonce: &[u8; 12]) -> [u8; 64] {
    let mut state: [u32; 16] = [0; 16];
    state[0] = 0x61707865;
    state[1] = 0x3320646e;
    state[2] = 0x79622d32;
    state[3] = 0x6b206574;
    for i in 0..8 {
        state[4 + i] = u32::from_le_bytes(key[i * 4..i * 4 + 4].try_into().unwrap());
    }
    state[12] = counter;
    for i in 0..3 {
        state[13 + i] = u32::from_le_bytes(nonce[i * 4..i * 4 + 4].try_into().unwrap());
    }
    let orig = state;
    for _ in 0..10 {
        quarter_round(&mut state, 0, 4, 8, 12);
        quarter_round(&mut state, 1, 5, 9, 13);
        quarter_round(&mut state, 2, 6, 10, 14);
        quarter_round(&mut state, 3, 7, 11, 15);
        quarter_round(&mut state, 0, 5, 10, 15);
        quarter_round(&mut state, 1, 6, 11, 12);
        quarter_round(&mut state, 2, 7, 8, 13);
        quarter_round(&mut state, 3, 4, 9, 14);
    }
    let mut out = [0u8; 64];
    for i in 0..16 {
        let val = state[i].wrapping_add(orig[i]);
        out[i * 4..i * 4 + 4].copy_from_slice(&val.to_le_bytes());
    }
    out
}

/// Encrypt/decrypt data using ChaCha20 (RFC7539).
pub fn chacha20_crypt(key: &[u8; 32], nonce: &[u8; 12], data: &[u8], counter: u32) -> Vec<u8> {
    let mut result = Vec::with_capacity(data.len());
    let blocks = data.len().div_ceil(64);
    for i in 0..blocks {
        let ks = chacha20_block(key, counter + i as u32, nonce);
        let start = i * 64;
        let end = std::cmp::min(start + 64, data.len());
        for j in start..end {
            result.push(data[j] ^ ks[j - start]);
        }
    }
    result
}

/// PC ChaCha20 key (base64: 6VsxesT4KFadI6hr8nHctT6Eb6dckk1nHbqOOPTKUuE=)
pub const CHACHA_KEY: [u8; 32] = [
    0xe9, 0x5b, 0x31, 0x7a, 0xc4, 0xf8, 0x28, 0x56, 0x9d, 0x23, 0xa8, 0x6b, 0xf2, 0x71, 0xdc, 0xb5,
    0x3e, 0x84, 0x6f, 0xa7, 0x5c, 0x92, 0x4d, 0x67, 0x1d, 0xba, 0x8e, 0x38, 0xf4, 0xca, 0x52, 0xe1,
];
