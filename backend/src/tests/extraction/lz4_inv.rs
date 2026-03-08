use crate::extraction::lz4_inv::{decompress, parse_token, read_back_offset};

// ── Migrated from src/extraction/lz4_inv.rs ──

#[test]
fn parse_token_boundary_values() {
    assert_eq!(parse_token(0x00), (0, 0));
    assert_eq!(parse_token(0xFF), (15, 15));
}

#[test]
fn parse_token_roundtrip_all_values() {
    for lit in 0..16u8 {
        for enc in 0..16u8 {
            let lit_interleaved = (lit & 0b11) | ((lit & 0b1100) << 2);
            let enc_interleaved = ((enc & 0b11) | ((enc & 0b1100) << 2)) << 2;
            let token = lit_interleaved | enc_interleaved;
            let (parsed_lit, parsed_enc) = parse_token(token);
            assert_eq!(
                (parsed_lit, parsed_enc),
                (lit, enc),
                "Failed for lit={}, enc={}, token=0x{:02X}",
                lit,
                enc,
                token
            );
        }
    }
}

#[test]
fn read_back_offset_big_endian() {
    assert_eq!(read_back_offset(&[0x00, 0x01], 0), 1);
    assert_eq!(read_back_offset(&[0x01, 0x00], 0), 256);
    assert_eq!(read_back_offset(&[0xFF, 0xFF], 0), 65535);
}

// ── New tests ──

#[test]
fn read_back_offset_with_nonzero_pos() {
    assert_eq!(read_back_offset(&[0x00, 0x00, 0x01, 0x02], 2), 0x0102);
}

#[test]
fn decompress_literal_only() {
    // Token: lit=5, enc=0. Interleaved: lit bits 0,1,4,5 = 0b00_01_0000 | 0b01 = 0x11
    // lit=5=0b0101: bit0=1, bit1=0, bit4=1, bit5=0 -> 0b00_01_00_01 = 0x11
    // enc=0: all zero -> OR'd in positions 2,3,6,7 = 0x00
    // token = 0x11 | 0x00 = 0x11
    let lit_interleaved = (5u8 & 0b11) | ((5u8 & 0b1100) << 2);
    let token = lit_interleaved;
    let compressed = [token, b'H', b'e', b'l', b'l', b'o'];
    let result = decompress(&compressed, 5).unwrap();
    assert_eq!(result, b"Hello");
}

#[test]
fn decompress_empty_input_zero_size() {
    let result = decompress(&[], 0).unwrap();
    assert!(result.is_empty());
}

#[test]
fn decompress_size_mismatch_returns_err() {
    // Token with lit=1, enc=0
    let lit_interleaved = 1u8 & 0b11;
    let token = lit_interleaved;
    let compressed = [token, b'A'];
    // Request 5 bytes but only 1 literal available
    let result = decompress(&compressed, 5);
    assert!(result.is_err());
}

#[test]
fn decompress_literal_overrun_returns_err() {
    // Token says lit=3 but only 1 byte of data follows
    let token = 3u8 & 0b11;
    let compressed = [token, b'A'];
    let result = decompress(&compressed, 3);
    assert!(result.is_err());
}
