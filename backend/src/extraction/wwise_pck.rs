//! Wwise PCK (AudioKinetic Package) decryption and parsing for Arknights Endfield.
//!
//! Endfield wraps Wwise PCK files with the magic `:)xD` (0x3A297844) instead of
//! the standard `AKPK`. The header and audio data are encrypted using HyperGryph's
//! custom XOR-Mul stream cipher (`HGCipherHelper::DecipherInplace`).
//!
//! File layout:
//!   [0x00] 4 bytes  — magic `:)xD`
//!   [0x04] 4 bytes  — header_size (u32 LE)
//!   [0x08] 4 bytes  — unknown / reserved
//!   [0x0C] header_size-4 bytes — encrypted header body
//!   [8+header_size ..] — encrypted audio data
//!
//! Decrypted header body (no version field in Endfield variant):
//!   u32 lang_map_size
//!   u32 banks_table_size
//!   u32 streams_table_size
//!   u32 externals_table_size
//!   [lang_map_size bytes]      — language map
//!   [banks_table_size bytes]   — bank file entries (20-byte records)
//!   [streams_table_size bytes] — stream file entries (20 or 24-byte records)
//!   [externals_table_size bytes] — external file entries (24-byte records)
//!
//! Source: AkSoundEngine.dll / libAkSoundEngine.so (DWARF symbols on Android)
//! Reference: isHarryh/isharryh.github.io — Endfield CBT3 RE: PCK Files Decryption

/// Magic bytes: `:)xD` in ASCII
pub const SMILEY_MAGIC: [u8; 4] = [0x3A, 0x29, 0x78, 0x44];

/// Standard Wwise magic: `AKPK`
pub const AKPK_MAGIC: [u8; 4] = [0x41, 0x4B, 0x50, 0x4B];

/// HGCipherHelper constants
const CIPHER_XOR: u32 = 0x9C5A0B29;
const CIPHER_MUL: u32 = 0x04E11C23;

/// Check if data starts with the `:)xD` magic.
pub fn is_smiley_format(data: &[u8]) -> bool {
    data.len() >= 8 && data[0..4] == SMILEY_MAGIC
}

fn read_u32(data: &[u8], pos: usize) -> u32 {
    u32::from_le_bytes([data[pos], data[pos + 1], data[pos + 2], data[pos + 3]])
}

fn read_u64(data: &[u8], pos: usize) -> u64 {
    u64::from_le_bytes([
        data[pos],
        data[pos + 1],
        data[pos + 2],
        data[pos + 3],
        data[pos + 4],
        data[pos + 5],
        data[pos + 6],
        data[pos + 7],
    ])
}

/// Generate the keystream word for a given word index.
///
/// The algorithm folds each byte of the index through XOR-multiply:
/// ```text
/// T0 = ((index & 0xFF) ^ 0x9C5A0B29) * 0x04E11C23
/// T1 = (T0 ^ ((index >> 8) & 0xFF)) * 0x04E11C23
/// T2 = (T1 ^ ((index >> 16) & 0xFF)) * 0x04E11C23
/// T3 = (T2 ^ ((index >> 24) & 0xFF)) * 0x04E11C23
/// ```
fn cipher_key_word(index: u32) -> u32 {
    let t0 = ((index & 0xFF) ^ CIPHER_XOR).wrapping_mul(CIPHER_MUL);
    let t1 = (t0 ^ ((index >> 8) & 0xFF)).wrapping_mul(CIPHER_MUL);
    let t2 = (t1 ^ ((index >> 16) & 0xFF)).wrapping_mul(CIPHER_MUL);
    (t2 ^ ((index >> 24) & 0xFF)).wrapping_mul(CIPHER_MUL)
}

/// Decrypt data in-place using HGCipherHelper::DecipherInplace.
///
/// Each 4-byte LE word at position `i` is XOR'd with `cipher_key_word(seed + i)`.
/// Trailing bytes (< 4) are left unmodified.
pub fn decipher_inplace(data: &mut [u8], seed: u32, word_offset_start: u32) {
    let word_count = data.len() / 4;
    for i in 0..word_count {
        let offset = i * 4;
        let word = u32::from_le_bytes([
            data[offset],
            data[offset + 1],
            data[offset + 2],
            data[offset + 3],
        ]);
        let key = cipher_key_word(seed.wrapping_add(word_offset_start + i as u32));
        let decrypted = word ^ key;
        let bytes = decrypted.to_le_bytes();
        data[offset] = bytes[0];
        data[offset + 1] = bytes[1];
        data[offset + 2] = bytes[2];
        data[offset + 3] = bytes[3];
    }
}

/// A parsed Wwise PCK file entry.
#[derive(Debug, Clone)]
pub struct WwiseFileEntry {
    /// Wwise file/event ID
    pub id: u64,
    /// Byte offset within the PCK data region
    pub offset: u32,
    /// Size of the audio data in bytes
    pub size: u32,
}

/// Parsed Wwise PCK header.
#[derive(Debug)]
pub struct WwisePck {
    /// Header size (from outer container offset 4)
    pub header_size: u32,
    /// Language map entries: (lang_id, name)
    pub languages: Vec<(u32, String)>,
    /// Sound bank file entries (20-byte records, u32 IDs)
    pub banks: Vec<WwiseFileEntry>,
    /// Streamed audio file entries (20 or 24-byte records)
    pub streams: Vec<WwiseFileEntry>,
    /// External source file entries (24-byte records, u64 IDs)
    pub externals: Vec<WwiseFileEntry>,
}

impl WwisePck {
    /// Iterator over all file entries (banks + streams + externals).
    pub fn all_files(&self) -> impl Iterator<Item = &WwiseFileEntry> {
        self.banks
            .iter()
            .chain(self.streams.iter())
            .chain(self.externals.iter())
    }
}

/// Decrypt the header of a `:)xD` file.
///
/// Returns (decrypted_header_body, header_size).
pub fn decrypt_header(data: &[u8]) -> Result<(Vec<u8>, u32), String> {
    if data.len() < 12 {
        return Err("Data too short for :)xD header".into());
    }

    if data[0..4] != SMILEY_MAGIC {
        return Err(format!(
            "Invalid magic: {:02x}{:02x}{:02x}{:02x}",
            data[0], data[1], data[2], data[3]
        ));
    }

    let header_size = read_u32(data, 4);

    // Encrypted region: offset 12, length header_size - 4
    let encrypt_start = 12;
    let encrypt_len = header_size.saturating_sub(4) as usize;

    if encrypt_start + encrypt_len > data.len() {
        return Err(format!(
            "Header extends past file: offset {} + len {} > file size {}",
            encrypt_start,
            encrypt_len,
            data.len()
        ));
    }

    let mut header = data[encrypt_start..encrypt_start + encrypt_len].to_vec();
    decipher_inplace(&mut header, header_size, 0);

    Ok((header, header_size))
}

/// Parse a language map section.
fn parse_language_map(section: &[u8]) -> Vec<(u32, String)> {
    if section.len() < 4 {
        return Vec::new();
    }

    let num_langs = read_u32(section, 0) as usize;
    let entries_end = 4 + num_langs * 8;
    if entries_end > section.len() {
        return Vec::new();
    }

    let mut languages = Vec::with_capacity(num_langs);
    for i in 0..num_langs {
        let entry_pos = 4 + i * 8;
        let str_offset = read_u32(section, entry_pos) as usize;
        let lang_id = read_u32(section, entry_pos + 4);

        // String offset is relative to the start of this section
        let name = if str_offset < section.len() {
            // Auto-detect UTF-16LE vs UTF-8:
            // if either of the first 2 bytes is 0x00, it's UTF-16LE
            if str_offset + 1 < section.len()
                && (section[str_offset] == 0 || section[str_offset + 1] == 0)
            {
                // UTF-16LE
                let mut chars = Vec::new();
                let mut sp = str_offset;
                while sp + 1 < section.len() {
                    let ch = u16::from_le_bytes([section[sp], section[sp + 1]]);
                    if ch == 0 {
                        break;
                    }
                    chars.push(ch);
                    sp += 2;
                }
                String::from_utf16_lossy(&chars)
            } else {
                // UTF-8 / ASCII null-terminated
                let end = section[str_offset..]
                    .iter()
                    .position(|&b| b == 0)
                    .map_or(section.len(), |p| str_offset + p);
                String::from_utf8_lossy(&section[str_offset..end]).into_owned()
            }
        } else {
            format!("lang_{}", lang_id)
        };

        languages.push((lang_id, name));
    }

    languages
}

/// Parse a file entry table section. Auto-detects 20-byte vs 24-byte entries.
fn parse_file_table(section: &[u8]) -> Result<Vec<WwiseFileEntry>, String> {
    if section.len() < 4 {
        return Ok(Vec::new());
    }

    let count = read_u32(section, 0) as usize;
    if count == 0 {
        return Ok(Vec::new());
    }

    let data_len = section.len() - 4;
    let entry_size = data_len / count;

    let mut entries = Vec::with_capacity(count);
    let mut pos = 4;

    match entry_size {
        20 => {
            // u32 id, u32 block_size, u32 file_size, u32 starting_block, u32 lang_id
            for _ in 0..count {
                if pos + 20 > section.len() {
                    break;
                }
                let id = read_u32(section, pos) as u64;
                let block_size = read_u32(section, pos + 4);
                let file_size = read_u32(section, pos + 8);
                let starting_block = read_u32(section, pos + 12);
                let offset = starting_block.wrapping_mul(block_size);
                entries.push(WwiseFileEntry {
                    id,
                    offset,
                    size: file_size,
                });
                pos += 20;
            }
        }
        24 => {
            // u64 id, u32 block_size, u32 file_size, u32 starting_block, u32 lang_id
            for _ in 0..count {
                if pos + 24 > section.len() {
                    break;
                }
                let id = read_u64(section, pos);
                let block_size = read_u32(section, pos + 8);
                let file_size = read_u32(section, pos + 12);
                let starting_block = read_u32(section, pos + 16);
                let offset = starting_block.wrapping_mul(block_size);
                entries.push(WwiseFileEntry {
                    id,
                    offset,
                    size: file_size,
                });
                pos += 24;
            }
        }
        other => {
            return Err(format!(
                "Unexpected entry size: {} ({} bytes / {} entries)",
                other, data_len, count
            ));
        }
    }

    Ok(entries)
}

/// Parse the decrypted Wwise PCK header body.
///
/// The Endfield variant has no version field. The preamble is 4 section sizes:
/// `lang_map_size, banks_size, streams_size, externals_size`.
pub fn parse_header(decrypted_header: &[u8], header_size: u32) -> Result<WwisePck, String> {
    if decrypted_header.len() < 16 {
        return Err("Decrypted header too short (need 16 bytes for section sizes)".into());
    }

    let lang_map_size = read_u32(decrypted_header, 0) as usize;
    let banks_size = read_u32(decrypted_header, 4) as usize;
    let streams_size = read_u32(decrypted_header, 8) as usize;
    let externals_size = read_u32(decrypted_header, 12) as usize;

    let preamble = 16;
    let expected = preamble + lang_map_size + banks_size + streams_size + externals_size;
    if expected > decrypted_header.len() {
        return Err(format!(
            "Section sizes exceed header: {} > {} (lang={}, banks={}, streams={}, ext={})",
            expected,
            decrypted_header.len(),
            lang_map_size,
            banks_size,
            streams_size,
            externals_size
        ));
    }

    // Language map
    let lang_start = preamble;
    let lang_end = lang_start + lang_map_size;
    let languages = parse_language_map(&decrypted_header[lang_start..lang_end]);

    // Banks table
    let banks_start = lang_end;
    let banks_end = banks_start + banks_size;
    let banks = parse_file_table(&decrypted_header[banks_start..banks_end])?;

    // Streams table
    let streams_start = banks_end;
    let streams_end = streams_start + streams_size;
    let streams = parse_file_table(&decrypted_header[streams_start..streams_end])?;

    // Externals table
    let ext_start = streams_end;
    let ext_end = ext_start + externals_size;
    let externals = parse_file_table(&decrypted_header[ext_start..ext_end])?;

    Ok(WwisePck {
        header_size,
        languages,
        banks,
        streams,
        externals,
    })
}

/// Extract a single audio file from the PCK, decrypting its data.
///
/// `pck_data` is the full `:)xD` file contents.
/// `entry` is a parsed file entry (offset is absolute PCK position).
pub fn extract_audio(pck_data: &[u8], entry: &WwiseFileEntry) -> Result<Vec<u8>, String> {
    // Entry offset is absolute position in the PCK file
    let file_start = entry.offset as usize;
    let file_end = file_start + entry.size as usize;

    if file_end > pck_data.len() {
        return Err(format!(
            "Audio file extends past PCK: {} + {} > {}",
            file_start,
            entry.size,
            pck_data.len()
        ));
    }

    let mut audio = pck_data[file_start..file_end].to_vec();
    // Audio decryption: seed = file_id, counter starts at 0
    decipher_inplace(&mut audio, entry.id as u32, 0);

    Ok(audio)
}
