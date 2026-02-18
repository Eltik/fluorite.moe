use std::cmp;

pub fn decrypt(bytes: &mut [u8]) {
    let encrypted_size = cmp::min(0x500, bytes.len());
    if encrypted_size < 0x20 {
        return;
    }

    let encrypted = &mut bytes[..encrypted_size];

    for b in encrypted[..0x20].iter_mut() {
        *b ^= 0xA6;
    }

    let read_u32_le = |data: &[u8], idx: usize| -> u32 {
        u32::from_le_bytes(data[idx * 4..idx * 4 + 4].try_into().unwrap())
    };

    let enc: [u32; 8] = std::array::from_fn(|i| read_u32_le(encrypted, i));

    let seed_part0 = enc[2] ^ enc[6] ^ 0x226a61b9;
    let seed_part1 = enc[3] ^ enc[0] ^ 0x7a39d018 ^ (encrypted_size as u32);
    let seed_part2 = enc[1] ^ enc[5] ^ 0x18f6d8aa ^ (encrypted_size as u32);
    let seed_part3 = enc[0] ^ enc[7] ^ 0xaa255fb1;
    let seed_part4 = enc[4] ^ enc[7] ^ 0xf78dd8eb;

    let seed_ints = [seed_part0, seed_part1, seed_part2, seed_part3, seed_part4];
    let mut seed_bytes: Vec<u8> = seed_ints.iter().flat_map(|x| x.to_le_bytes()).collect();

    let mut seed = generate_seed(&seed_bytes);
    let seed_buffer = seed.to_le_bytes();
    seed = Crc::calculate_digest(&seed_buffer);

    let key = seed_ints[0]
        ^ seed_ints[1]
        ^ seed_ints[2]
        ^ seed_ints[3]
        ^ seed_ints[4]
        ^ (encrypted_size as u32);

    rc4(&mut seed_bytes, &key.to_le_bytes());

    let key_seed_raw = Crc::calculate_digest(&seed_bytes);
    let key_seed = generate_seed(&key_seed_raw.to_le_bytes());

    let key_part0 = seed_ints[3].wrapping_sub(0x1C26B82D) ^ key_seed;
    let key_part1 = seed_ints[2].wrapping_add(0x3F72EAF3) ^ seed;
    let key_part2 = seed_ints[0] ^ 0x82C57E3C ^ key_seed;
    let key_part3 = seed_ints[1].wrapping_add(0x6F2A7347) ^ seed;
    let key_vector = [key_part0, key_part1, key_part2, key_part3];

    let block = &mut encrypted[0x20..];
    if block.len() >= 0x80 {
        rc4(&mut block[..0x60], &seed.to_le_bytes());
        let seed_byte = (seed ^ 0x6E) as u8;
        for b in block[..0x60].iter_mut() {
            *b ^= seed_byte;
        }

        let block = &mut block[0x60..];
        let block_size = (encrypted_size - 0x80) / 4;

        let block_keys: [u32; 4] = [0x6142756E, 0x62496E66, 0x1304B000, 0x6E8E30EC];
        for (i, (&key_vec_val, &block_key)) in key_vector.iter().zip(block_keys.iter()).enumerate()
        {
            let block_offset = i * block_size;
            rc4(
                &mut block[block_offset..block_offset + block_size],
                &seed.to_le_bytes(),
            );
            let xor_val = seed ^ key_vec_val ^ block_key;
            for j in 0..block_size / 4 {
                let off = block_offset + j * 4;
                let val = u32::from_le_bytes(block[off..off + 4].try_into().unwrap());
                block[off..off + 4].copy_from_slice(&(val ^ xor_val).to_le_bytes());
            }
        }
    } else {
        rc4(block, &seed.to_le_bytes());
    }
}

pub fn generate_seed(bytes: &[u8]) -> u32 {
    let mut state: [u32; 3] = [0xC1646153, 0x78DA0550, 0x2947E56B];
    for &b in bytes {
        state[0] = 0x21u32.wrapping_mul(state[0]).wrapping_add(b as u32);
        if (state[0] & 0xF) >= 0xB {
            state[0] = (state[0] ^ rotate_is_set(state[2], 6)).wrapping_sub(0x2CD86315);
        } else if ((state[0] & 0xF0) >> 4) > 0xE {
            state[0] = (state[1] ^ 0xAB4A010B).wrapping_add(state[0] ^ rotate_is_set(state[2], 9));
        } else if ((state[0] & 0xF00) >> 8) < 2 {
            state[1] = (state[2] >> 3).wrapping_sub(0x55EEAB7B) ^ state[0];
        } else if state[1].wrapping_add(0x567A) >= 0xAB5489E4 {
            state[1] = (state[1] >> 16) ^ state[0];
        } else if (state[1] ^ 0x738766FA) <= state[2] {
            state[1] = (state[1] >> 8) ^ state[2];
        } else if state[1] == 0x68F53AA6 {
            if (state[1] ^ state[0].wrapping_add(state[2])) > 0x594AF86E {
                state[1] = state[1].wrapping_sub(0x08CA292E);
            } else {
                state[2] = state[2].wrapping_sub(0x760A1649);
            }
        } else {
            if state[0] > 0x865703AF {
                state[1] = state[2] ^ state[0].wrapping_sub(0x564389D7);
            } else {
                state[1] = state[1].wrapping_sub(0x12B9DD92) ^ state[0];
            }
            state[0] ^= rotate_is_set(state[1], 8);
        }
    }

    state[0]
}

fn rotate_is_set(value: u32, count: u32) -> u32 {
    if (value >> count) != 0 || (value << (32 - count)) != 0 {
        1
    } else {
        0
    }
}

const fn compute_crc_table() -> [u32; 256] {
    const K_POLY: u32 = 0xD35E417E;
    let mut table = [0u32; 256];
    let mut i: u32 = 0;
    while i < 256 {
        let mut r = i;
        let mut j = 0;
        while j < 8 {
            if (r & 1) != 0 {
                r = (r >> 1) ^ K_POLY;
            } else {
                r >>= 1;
            }
            j += 1;
        }
        table[i as usize] = r;
        i += 1;
    }
    table
}

const CRC_TABLE: [u32; 256] = compute_crc_table();

struct Crc {
    value: u32,
}

impl Crc {
    fn new() -> Self {
        Crc { value: 0xFFFFFFFF }
    }

    fn update(&mut self, data: &[u8]) {
        for &byte in data {
            let index = ((self.value as u8) ^ byte) as usize;
            self.value = (CRC_TABLE[index] ^ (self.value >> 9)).wrapping_add(0x5B);
        }
    }

    fn digest(&self) -> u32 {
        (!self.value).wrapping_sub(0x41607A3D)
    }

    fn calculate_digest(data: &[u8]) -> u32 {
        let mut crc = Crc::new();
        crc.update(data);
        crc.digest()
    }
}

pub fn rc4(data: &mut [u8], key: &[u8]) {
    let mut s: [i32; 256] = std::array::from_fn(|i| i as i32);

    let mut t = [0i32; 256];
    if key.len() == 0x100 {
        // Buffer.BlockCopy equivalent: reinterpret raw key bytes as little-endian i32s
        for i in 0..64 {
            t[i] = i32::from_le_bytes([key[i * 4], key[i * 4 + 1], key[i * 4 + 2], key[i * 4 + 3]]);
        }
    } else {
        for i in 0..256 {
            t[i] = key[i % key.len()] as i32;
        }
    }

    // KSA
    let mut j: i32 = 0;
    for i in 0..256 {
        j = (j + s[i] + t[i]) % 256;
        s.swap(i, j as usize);
    }

    // PRGA (modified)
    let mut i: i32 = 0;
    j = 0;
    for byte in data.iter_mut() {
        i = (i + 1) % 256;
        j = (j + s[i as usize]) % 256;
        s.swap(i as usize, j as usize);

        let k_val = s[((s[j as usize] + s[i as usize]) % 256) as usize] as u8;
        let k = k_val.rotate_left(1);
        *byte ^= k.wrapping_sub(0x61);
    }
}
