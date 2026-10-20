/// Lua script decryption: the `Lua` VFS group stores each script as base64 text wrapping an
/// XXTEA-encrypted payload whose last word is the plaintext length.
use super::crypto_constants::LUA_XXTEA_KEY;
use super::utils::write_file;
use base64::Engine;
use std::fs;
use std::io;
use std::path::Path;

/// BLC group whose `.lua` files are encrypted scripts.
pub const LUA_GROUP: &str = "Lua";

const DELTA: u32 = 0x9E37_79B9;

/// Corrected Block TEA (XXTEA) decryption of `v` in place; `v` must hold at least two words.
pub fn xxtea_decrypt(v: &mut [u32], key: &[u8; 16]) {
    let n = v.len();
    if n < 2 {
        return;
    }
    let k: [u32; 4] =
        std::array::from_fn(|i| u32::from_le_bytes(key[i * 4..i * 4 + 4].try_into().unwrap()));
    let mix = |sum: u32, y: u32, z: u32, p: usize, e: usize| {
        (((z >> 5) ^ (y << 2)).wrapping_add((y >> 3) ^ (z << 4)))
            ^ ((sum ^ y).wrapping_add(k[(p & 3) ^ e] ^ z))
    };
    let rounds = 6 + 52 / n as u32;
    let mut sum = rounds.wrapping_mul(DELTA);
    let mut y = v[0];
    while sum != 0 {
        let e = ((sum >> 2) & 3) as usize;
        for p in (1..n).rev() {
            let z = v[p - 1];
            v[p] = v[p].wrapping_sub(mix(sum, y, z, p, e));
            y = v[p];
        }
        let z = v[n - 1];
        v[0] = v[0].wrapping_sub(mix(sum, y, z, 0, e));
        y = v[0];
        sum = sum.wrapping_sub(DELTA);
    }
}

/// Decrypt one stored Lua script to its source bytes.
///
/// Returns `None` when `content` is not in the encrypted format (e.g. already decrypted):
/// it must be strict base64 of whole words whose decrypted length word fits the payload,
/// and the plaintext must be UTF-8.
pub fn decrypt_lua(content: &[u8]) -> Option<Vec<u8>> {
    let raw = base64::engine::general_purpose::STANDARD
        .decode(content.trim_ascii())
        .ok()?;
    if raw.len() < 8 || !raw.len().is_multiple_of(4) {
        return None;
    }
    let mut words: Vec<u32> = raw
        .as_chunks::<4>()
        .0
        .iter()
        .map(|w| u32::from_le_bytes(*w))
        .collect();
    xxtea_decrypt(&mut words, &LUA_XXTEA_KEY);
    let capacity = (words.len() - 1) * 4;
    let len = *words.last()? as usize;
    // The plaintext is zero-padded to a whole word before the length word is appended.
    if len > capacity || capacity - len > 3 {
        return None;
    }
    let mut plain: Vec<u8> = words[..words.len() - 1]
        .iter()
        .flat_map(|w| w.to_le_bytes())
        .collect();
    plain.truncate(len);
    std::str::from_utf8(&plain).ok()?;
    Some(plain)
}

/// Whether a `blc_metadata.txt` belongs to the Lua group (its first line is `Group: Lua (<hash>)`).
pub fn is_lua_metadata(metadata: &str) -> bool {
    metadata
        .lines()
        .next()
        .and_then(|line| line.strip_prefix("Group: "))
        .and_then(|rest| rest.split_once(" ("))
        .is_some_and(|(group, _)| group == LUA_GROUP)
}

/// Totals from [`decrypt_tree`].
#[derive(Debug, Default, PartialEq, Eq)]
pub struct TreeDecryptResult {
    pub decrypted: u64,
    pub write_errors: u64,
}

/// Decrypt, in place, every still-encrypted `.lua` file under `dir`.
///
/// Files that are already plain source are left alone, so rerunning is a no-op.
pub fn decrypt_tree(dir: &Path) -> io::Result<TreeDecryptResult> {
    let mut result = TreeDecryptResult::default();
    let mut pending = vec![dir.to_path_buf()];
    while let Some(dir) = pending.pop() {
        for entry in fs::read_dir(&dir)? {
            let entry = entry?;
            let path = entry.path();
            if entry.file_type()?.is_dir() {
                pending.push(path);
                continue;
            }
            if path.extension().is_none_or(|ext| ext != "lua") {
                continue;
            }
            let Some(plain) = decrypt_lua(&fs::read(&path)?) else {
                continue;
            };
            if write_file(&path, &plain) {
                result.decrypted += 1;
            } else {
                result.write_errors += 1;
            }
        }
    }
    Ok(result)
}
