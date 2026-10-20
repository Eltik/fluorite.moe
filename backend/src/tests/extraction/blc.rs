use crate::extraction::blc::{BlcFileEntry, decrypt_file, parse_blc, relative_file_path};
use crate::extraction::chacha20::{CHACHA_KEY, chacha20_crypt};
use std::path::Path;

fn string(out: &mut Vec<u8>, s: &str) {
    out.extend((s.len() as u16).to_le_bytes());
    out.extend(s.as_bytes());
}

/// Build an encrypted v5 catalog with one chunk holding a plain and an encrypted file.
fn v5_catalog() -> Vec<u8> {
    let mut plain = Vec::new();
    plain.extend(3i32.to_le_bytes()); // version (first nonce word)
    plain.extend([0xAA; 8]); // rest of the nonce
    plain.extend(5i32.to_le_bytes()); // format revision
    plain.extend([0x99; 4]);
    string(&mut plain, "Lua");
    plain.extend([0x19, 0xE3, 0xAE, 0x45]); // group hash, big-endian
    plain.extend([0xFF; 4]);
    plain.extend(2i32.to_le_bytes()); // file count
    plain.extend(100i64.to_le_bytes()); // chunks length
    plain.push(1);
    plain.extend(1i32.to_le_bytes()); // chunk count

    plain.extend([0x11; 16]); // chunk md5
    plain.extend([0x22; 16]); // content md5
    plain.extend(100i64.to_le_bytes());
    plain.push(1);
    plain.extend(0i64.to_le_bytes()); // v5 extra field
    plain.extend(2i32.to_le_bytes());

    string(&mut plain, "Data/LuaScripts/a.lua");
    plain.extend(7i64.to_le_bytes());
    plain.extend([0x11; 16]);
    plain.extend([0x33; 16]);
    plain.extend(0i64.to_le_bytes()); // offset
    plain.extend(4i64.to_le_bytes()); // len
    plain.push(1);
    plain.push(0); // not encrypted
    plain.extend(0i64.to_le_bytes());

    string(&mut plain, "Data/LuaScripts/b.lua");
    plain.extend(8i64.to_le_bytes());
    plain.extend([0x11; 16]);
    plain.extend([0x44; 16]);
    plain.extend(4i64.to_le_bytes());
    plain.extend(3i64.to_le_bytes());
    plain.push(1);
    plain.push(1); // encrypted
    plain.extend(42i64.to_le_bytes()); // iv seed
    plain.extend(0i64.to_le_bytes());

    let nonce: [u8; 12] = plain[..12].try_into().unwrap();
    let mut raw = plain[..12].to_vec();
    raw.extend(chacha20_crypt(&CHACHA_KEY, &nonce, &plain[12..], 1));
    raw
}

#[test]
fn parses_v5_catalog() {
    let blc = parse_blc(&v5_catalog()).unwrap();
    assert_eq!(blc.version, 3);
    assert_eq!(blc.group_cfg_name, "Lua");
    assert_eq!(blc.group_cfg_hash, "19E3AE45");
    assert_eq!(blc.files.len(), 2);
    assert_eq!(blc.files[0].file_name, "Data/LuaScripts/a.lua");
    assert_eq!(blc.files[0].chunk_md5, "11".repeat(16));
    assert!(!blc.files[0].use_encrypt);
    assert_eq!((blc.files[1].offset, blc.files[1].len), (4, 3));
    assert!(blc.files[1].use_encrypt);
    assert_eq!(blc.files[1].iv_seed, 42);
}

#[test]
fn decrypts_files_with_catalog_version_nonce() {
    let blc = parse_blc(&v5_catalog()).unwrap();
    let mut nonce = [0u8; 12];
    nonce[..4].copy_from_slice(&blc.version.to_le_bytes());
    nonce[4..].copy_from_slice(&42i64.to_le_bytes());
    let mut chunk = b"lua!".to_vec();
    chunk.extend(chacha20_crypt(&CHACHA_KEY, &nonce, b"end", 1));
    assert_eq!(
        decrypt_file(&chunk, &blc.files[0], blc.version).unwrap(),
        b"lua!"
    );
    assert_eq!(
        decrypt_file(&chunk, &blc.files[1], blc.version).unwrap(),
        b"end"
    );
    assert!(decrypt_file(&chunk[..5], &blc.files[1], blc.version).is_none());
}

#[test]
fn decrypt_file_rejects_negative_or_overflowing_ranges() {
    let chunk = [0u8; 16];
    for (offset, len) in [(-1, 4), (0, -1), (i64::MAX, i64::MAX), (8, i64::MAX)] {
        let entry = BlcFileEntry {
            file_name: "Data/x.bytes".into(),
            chunk_md5: String::new(),
            offset,
            len,
            use_encrypt: false,
            iv_seed: 0,
        };
        assert!(
            decrypt_file(&chunk, &entry, 3).is_none(),
            "offset {offset} len {len}"
        );
    }
}

#[test]
fn truncated_catalog_errors_instead_of_panicking() {
    let raw = v5_catalog();
    for len in [16, 30, 60, raw.len() - 1] {
        assert!(parse_blc(&raw[..len]).is_err(), "len {len}");
    }
}

#[test]
fn relative_file_path_strips_data_prefix_and_traversal() {
    assert_eq!(
        relative_file_path("Data/Audio/PCK/Windows/init.pck"),
        Path::new("Audio/PCK/Windows/init.pck")
    );
    assert_eq!(relative_file_path("../../etc/x:y"), Path::new("etc/x_y"));
    assert_eq!(relative_file_path("Data/"), Path::new("_unnamed"));
}
