//! Wwise audio packages (`:)xD` PCKs) read entry by entry, payload identification,
//! minimal soundbank parsing and a searchable index across packages.
//!
//! The PCK container and its cipher are described in [`super::wwise_pck`]; this module
//! reads only the header up front and every entry on demand, so multi-GB stream packages
//! never have to be loaded whole.
//!
//! Each entry is enciphered on its own with the HGCipher keystream seeded by the low 32
//! bits of its id, counter starting at 0 (the same as [`super::wwise_pck::extract_audio`]).
//! Offsets are `starting_block * block_size` (block size is 1 in every Endfield package).
//!
//! Ids:
//! - Banks and streamed media use 32-bit Wwise ShortIDs. Bank ids are the FNV-1 32 hash
//!   of the lowercased bank name; media ids are random and named only through events.
//! - Voice-over lives in the language packages as *external sources* with 64-bit ids:
//!   `FNV-1 64(lowercase("voice/<language>/" + AudioDialog.path))`, e.g.
//!   `voice/english/v1d0/characters/chr_0005_chen/chr_0005_chen_chrbark_squad_01.wem`.
//! - `AudioDialog` keys are the signed FNV-1 32 hash of the lowercased voice name.

use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::wwise_pck::{SMILEY_MAGIC, decipher_inplace};

// ── Hashes ───────────────────────────────────────────────────────────────

/// Wwise ShortID: FNV-1 32 of the lowercased ASCII name.
pub fn short_id(name: &str) -> u32 {
    let mut h: u32 = 0x811c_9dc5;
    for b in name.bytes() {
        h = h.wrapping_mul(0x0100_0193);
        h ^= u32::from(b.to_ascii_lowercase());
    }
    h
}

/// FNV-1 64 of the lowercased string (the Wwise package hash for external sources).
pub fn fnv1_64_lower(s: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.bytes() {
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
        h ^= u64::from(b.to_ascii_lowercase());
    }
    h
}

/// Package id of the external source `path` (an `AudioDialog.path`) in `language`
/// (the package language name, e.g. `english`).
pub fn external_id(language: &str, path: &str) -> u64 {
    fnv1_64_lower(&format!("voice/{language}/{path}"))
}

/// `AudioDialog` key of a voice name (`profileVoice.voId`, a path's file stem, ...).
pub fn dialog_key(name: &str) -> i32 {
    short_id(name) as i32
}

// ── Package table ────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EntryKind {
    Bank,
    Stream,
    External,
}

impl EntryKind {
    pub fn as_str(self) -> &'static str {
        match self {
            EntryKind::Bank => "bank",
            EntryKind::Stream => "stream",
            EntryKind::External => "external",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "bank" | "banks" => Some(EntryKind::Bank),
            "stream" | "streams" => Some(EntryKind::Stream),
            "external" | "externals" => Some(EntryKind::External),
            _ => None,
        }
    }
}

/// One file in a package.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PckEntry {
    pub id: u64,
    pub kind: EntryKind,
    /// Key into [`PckTable::languages`].
    pub lang_id: u32,
    /// Absolute offset in the package file.
    pub offset: u64,
    pub size: u32,
}

impl PckEntry {
    /// Cipher seed: the low 32 bits of the id.
    pub fn seed(&self) -> u32 {
        self.id as u32
    }

    pub fn end(&self) -> u64 {
        self.offset + u64::from(self.size)
    }
}

/// Parsed package header.
#[derive(Debug, Clone, Default)]
pub struct PckTable {
    pub header_size: u32,
    /// `(lang_id, name)`, e.g. `(0, "sfx")`, `(1, "english")`.
    pub languages: Vec<(u32, String)>,
    pub entries: Vec<PckEntry>,
}

impl PckTable {
    pub fn language(&self, lang_id: u32) -> Option<&str> {
        self.languages
            .iter()
            .find(|(id, _)| *id == lang_id)
            .map(|(_, n)| n.as_str())
    }

    /// The package's voice language: its only language other than `sfx`.
    pub fn voice_language(&self) -> Option<&str> {
        let mut it = self
            .languages
            .iter()
            .map(|(_, n)| n.as_str())
            .filter(|n| !n.eq_ignore_ascii_case("sfx"));
        let first = it.next()?;
        it.next().is_none().then_some(first)
    }

    pub fn count(&self, kind: EntryKind) -> usize {
        self.entries.iter().filter(|e| e.kind == kind).count()
    }

    /// Bytes the header occupies at the start of the file (`8 + header_size`).
    pub fn header_len(&self) -> u64 {
        8 + u64::from(self.header_size)
    }
}

fn u32_at(data: &[u8], pos: usize) -> Option<u32> {
    Some(u32::from_le_bytes(data.get(pos..pos + 4)?.try_into().ok()?))
}

fn u64_at(data: &[u8], pos: usize) -> Option<u64> {
    Some(u64::from_le_bytes(data.get(pos..pos + 8)?.try_into().ok()?))
}

fn u16_at(data: &[u8], pos: usize) -> Option<u16> {
    Some(u16::from_le_bytes(data.get(pos..pos + 2)?.try_into().ok()?))
}

/// Total header bytes (`8 + header_size`) named by the first 8 bytes of a package.
pub fn header_len(first: &[u8]) -> Result<u64, String> {
    if first.len() < 8 {
        return Err("package too short".into());
    }
    if first[..4] != SMILEY_MAGIC {
        return Err(format!("not a :)xD package (magic {:02x?})", &first[..4]));
    }
    let size = u32_at(first, 4).unwrap_or(0);
    if size < 20 {
        return Err(format!("implausible header size {size}"));
    }
    Ok(8 + u64::from(size))
}

fn parse_languages(section: &[u8]) -> Vec<(u32, String)> {
    let Some(count) = u32_at(section, 0) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for i in 0..count as usize {
        let (Some(str_off), Some(id)) = (u32_at(section, 4 + i * 8), u32_at(section, 8 + i * 8))
        else {
            break;
        };
        let at = str_off as usize;
        let tail = section.get(at..).unwrap_or(&[]);
        // UTF-16LE when the second byte is NUL, else NUL-terminated UTF-8.
        let name = if tail.len() >= 2 && tail[1] == 0 {
            let units: Vec<u16> = tail
                .as_chunks::<2>()
                .0
                .iter()
                .map(|c| u16::from_le_bytes(*c))
                .take_while(|&u| u != 0)
                .collect();
            String::from_utf16_lossy(&units)
        } else {
            let end = tail.iter().position(|&b| b == 0).unwrap_or(tail.len());
            String::from_utf8_lossy(&tail[..end]).into_owned()
        };
        out.push((id, name));
    }
    out
}

fn parse_entries(section: &[u8], kind: EntryKind, out: &mut Vec<PckEntry>) -> Result<(), String> {
    let Some(count) = u32_at(section, 0) else {
        return Ok(());
    };
    let count = count as usize;
    if count == 0 {
        return Ok(());
    }
    let record = (section.len() - 4) / count;
    let wide = match record {
        20 => false,
        24 => true,
        other => {
            return Err(format!(
                "{} table: unexpected record size {other} ({count} entries)",
                kind.as_str()
            ));
        }
    };
    for i in 0..count {
        let p = 4 + i * record;
        let (id, rest) = if wide {
            (u64_at(section, p), p + 8)
        } else {
            (u32_at(section, p).map(u64::from), p + 4)
        };
        let (Some(id), Some(block), Some(size), Some(start), Some(lang_id)) = (
            id,
            u32_at(section, rest),
            u32_at(section, rest + 4),
            u32_at(section, rest + 8),
            u32_at(section, rest + 12),
        ) else {
            return Err(format!("{} table truncated", kind.as_str()));
        };
        out.push(PckEntry {
            id,
            kind,
            lang_id,
            offset: u64::from(start) * u64::from(block.max(1)),
            size,
        });
    }
    Ok(())
}

/// Parse a package header from its first [`header_len`] bytes (more is fine).
pub fn parse_table(head: &[u8]) -> Result<PckTable, String> {
    let total = header_len(head)? as usize;
    if head.len() < total {
        return Err(format!("header needs {total} bytes, got {}", head.len()));
    }
    let header_size = u32_at(head, 4).unwrap_or(0);
    // Bytes 8..12 are not enciphered (unknown); the body starts at 12.
    let mut body = head[12..total].to_vec();
    decipher_inplace(&mut body, header_size, 0);
    let sizes: Vec<usize> = (0..4)
        .map(|i| u32_at(&body, i * 4).unwrap_or(0) as usize)
        .collect();
    let needed = 16 + sizes.iter().sum::<usize>();
    if needed > body.len() {
        return Err(format!(
            "section sizes {sizes:?} exceed header ({} bytes)",
            body.len()
        ));
    }
    let mut at = 16;
    let mut section = |len: usize| {
        let s = &body[at..at + len];
        at += len;
        s
    };
    let languages = parse_languages(section(sizes[0]));
    let mut entries = Vec::new();
    parse_entries(section(sizes[1]), EntryKind::Bank, &mut entries)?;
    parse_entries(section(sizes[2]), EntryKind::Stream, &mut entries)?;
    parse_entries(section(sizes[3]), EntryKind::External, &mut entries)?;
    Ok(PckTable {
        header_size,
        languages,
        entries,
    })
}

/// Decipher an entry's bytes in place. `data` must start at the entry's first byte.
pub fn decipher_entry(data: &mut [u8], entry: &PckEntry) {
    decipher_inplace(data, entry.seed(), 0);
}

/// A file to put in a package built by [`build_package`].
pub struct PackageFile<'a> {
    pub kind: EntryKind,
    pub id: u64,
    pub lang_id: u32,
    pub data: &'a [u8],
}

/// Build an enciphered `:)xD` package (UTF-16 language names, 20-byte bank/stream
/// records, 24-byte external records). The inverse of [`parse_table`] + entry reads; used
/// by tests and tools.
pub fn build_package(languages: &[(u32, &str)], files: &[PackageFile<'_>]) -> Vec<u8> {
    let pad4 = |v: &mut Vec<u8>| {
        while !v.len().is_multiple_of(4) {
            v.push(0);
        }
    };
    let mut langs = Vec::new();
    langs.extend_from_slice(&(languages.len() as u32).to_le_bytes());
    let mut strings = Vec::new();
    let base = 4 + languages.len() * 8;
    for (id, name) in languages {
        langs.extend_from_slice(&((base + strings.len()) as u32).to_le_bytes());
        langs.extend_from_slice(&id.to_le_bytes());
        for u in name.encode_utf16().chain([0]) {
            strings.extend_from_slice(&u.to_le_bytes());
        }
    }
    langs.extend_from_slice(&strings);
    pad4(&mut langs);

    let tables_len = |kind: EntryKind| {
        let n = files.iter().filter(|f| f.kind == kind).count();
        4 + n * if kind == EntryKind::External { 24 } else { 20 }
    };
    let header_body = 16
        + langs.len()
        + [EntryKind::Bank, EntryKind::Stream, EntryKind::External]
            .iter()
            .map(|&k| tables_len(k))
            .sum::<usize>();
    let header_size = (4 + header_body) as u32;
    let mut offset = 8 + u64::from(header_size);
    let mut placed = Vec::new();
    for f in files {
        placed.push(offset);
        offset += f.data.len().next_multiple_of(16) as u64;
    }

    let mut body = Vec::with_capacity(header_body);
    for len in [
        langs.len(),
        tables_len(EntryKind::Bank),
        tables_len(EntryKind::Stream),
        tables_len(EntryKind::External),
    ] {
        body.extend_from_slice(&(len as u32).to_le_bytes());
    }
    body.extend_from_slice(&langs);
    for kind in [EntryKind::Bank, EntryKind::Stream, EntryKind::External] {
        let rows: Vec<(usize, &PackageFile)> = files
            .iter()
            .enumerate()
            .filter(|(_, f)| f.kind == kind)
            .collect();
        body.extend_from_slice(&(rows.len() as u32).to_le_bytes());
        for (i, f) in rows {
            if kind == EntryKind::External {
                body.extend_from_slice(&f.id.to_le_bytes());
            } else {
                body.extend_from_slice(&(f.id as u32).to_le_bytes());
            }
            body.extend_from_slice(&1u32.to_le_bytes());
            body.extend_from_slice(&(f.data.len() as u32).to_le_bytes());
            body.extend_from_slice(&(placed[i] as u32).to_le_bytes());
            body.extend_from_slice(&f.lang_id.to_le_bytes());
        }
    }
    decipher_inplace(&mut body, header_size, 0);

    let mut out = Vec::new();
    out.extend_from_slice(&SMILEY_MAGIC);
    out.extend_from_slice(&header_size.to_le_bytes());
    out.extend_from_slice(&[0; 4]);
    out.extend_from_slice(&body);
    for (f, at) in files.iter().zip(&placed) {
        out.resize(*at as usize, 0);
        let mut data = f.data.to_vec();
        decipher_inplace(&mut data, f.id as u32, 0);
        out.extend_from_slice(&data);
    }
    out
}

// ── Local package file ───────────────────────────────────────────────────

/// A package on disk: header parsed once, entries read with seek + read.
#[derive(Debug, Clone)]
pub struct PckFile {
    pub path: PathBuf,
    pub len: u64,
    pub table: PckTable,
}

impl PckFile {
    pub fn open(path: impl AsRef<Path>) -> std::io::Result<Self> {
        let path = path.as_ref().to_path_buf();
        let mut f = File::open(&path)?;
        let len = f.metadata()?.len();
        let mut first = [0u8; 8];
        f.read_exact(&mut first)?;
        let total = header_len(&first).map_err(std::io::Error::other)?;
        if total > len {
            return Err(std::io::Error::other("header extends past end of file"));
        }
        let mut head = vec![0u8; total as usize];
        f.seek(SeekFrom::Start(0))?;
        f.read_exact(&mut head)?;
        let table = parse_table(&head).map_err(std::io::Error::other)?;
        Ok(Self { path, len, table })
    }

    pub fn find(&self, id: u64) -> Option<&PckEntry> {
        self.table.entries.iter().find(|e| e.id == id)
    }

    /// Read and decipher the first `limit` bytes of an entry (all of it when `None`).
    pub fn read_entry(&self, entry: &PckEntry, limit: Option<usize>) -> std::io::Result<Vec<u8>> {
        if entry.end() > self.len {
            return Err(std::io::Error::other(format!(
                "entry {} extends past end of package",
                entry.id
            )));
        }
        let size = entry.size as usize;
        let len = limit.map_or(size, |l| l.min(size));
        // The cipher works on whole words: read through the word boundary, then cut.
        let read = len.next_multiple_of(4).min(size);
        let mut f = File::open(&self.path)?;
        f.seek(SeekFrom::Start(entry.offset))?;
        let mut buf = vec![0u8; read];
        f.read_exact(&mut buf)?;
        decipher_entry(&mut buf, entry);
        buf.truncate(len);
        Ok(buf)
    }
}

// ── Payload identification ───────────────────────────────────────────────

/// Codec family name for a RIFF/WEM `fmt` format tag.
pub fn codec_name(tag: u16) -> &'static str {
    match tag {
        0xFFFF => "vorbis",
        0x3040 | 0x3041 => "opus",
        0x0001 | 0xFFFE => "pcm",
        0x0002 => "adpcm",
        0x0069 => "ima",
        0x0165 | 0x0166 => "xma",
        0xAAC0 => "aac",
        0x3039 => "opusnx",
        0x8311 => "ptadpcm",
        _ => "unknown",
    }
}

/// Header facts of a RIFF/WEM payload.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct WemInfo {
    pub format_tag: u16,
    pub channels: u16,
    pub sample_rate: u32,
    /// Total samples per channel when the header states it.
    pub samples: Option<u32>,
}

impl WemInfo {
    pub fn codec(&self) -> &'static str {
        codec_name(self.format_tag)
    }

    pub fn duration(&self) -> Option<f64> {
        let s = self.samples?;
        (self.sample_rate > 0).then(|| f64::from(s) / f64::from(self.sample_rate))
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Payload {
    /// A soundbank (`BKHD`) with its id.
    Bank {
        id: u32,
        version: u32,
    },
    Wem(WemInfo),
    Unknown,
}

/// Identify a deciphered payload from its first bytes (256 is plenty).
pub fn identify(head: &[u8]) -> Payload {
    if head.starts_with(b"BKHD") {
        return Payload::Bank {
            version: u32_at(head, 8).unwrap_or(0),
            id: u32_at(head, 12).unwrap_or(0),
        };
    }
    if !(head.starts_with(b"RIFF") || head.starts_with(b"RIFX")) || head.get(8..12) != Some(b"WAVE")
    {
        return Payload::Unknown;
    }
    let mut p = 12;
    let mut fmt: Option<(usize, usize)> = None;
    let mut data_len: Option<u32> = None;
    while p + 8 <= head.len() {
        let tag = &head[p..p + 4];
        let Some(len) = u32_at(head, p + 4) else {
            break;
        };
        if tag == b"fmt " {
            fmt = Some((p + 8, len as usize));
        } else if tag == b"data" {
            data_len = Some(len);
            break;
        }
        p += 8 + len as usize + (len as usize & 1);
    }
    let Some((f, flen)) = fmt else {
        return Payload::Unknown;
    };
    let (Some(tag), Some(channels), Some(rate)) =
        (u16_at(head, f), u16_at(head, f + 2), u32_at(head, f + 4))
    else {
        return Payload::Unknown;
    };
    let samples = match tag {
        // Wwise Vorbis/Opus extensible headers carry the sample count at +0x18.
        0xFFFF | 0x3040 | 0x3041 if flen >= 0x1C => u32_at(head, f + 0x18),
        0x0001 | 0xFFFE => {
            let bits = u16_at(head, f + 14).unwrap_or(16).max(8);
            let frame = u32::from(channels.max(1)) * u32::from(bits / 8);
            data_len.map(|d| d / frame.max(1))
        }
        _ => None,
    }
    .filter(|&s| s > 0);
    Payload::Wem(WemInfo {
        format_tag: tag,
        channels,
        sample_rate: rate,
        samples,
    })
}

// ── Soundbanks ───────────────────────────────────────────────────────────

/// HIRC object types this module reads.
pub mod hirc {
    pub const SOUND: u8 = 2;
    pub const ACTION: u8 = 3;
    pub const EVENT: u8 = 4;
    pub const RANDOM_SEQUENCE: u8 = 5;
    pub const SWITCH: u8 = 6;
    pub const ACTOR_MIXER: u8 = 7;
    pub const LAYER: u8 = 9;
    pub const MUSIC_SEGMENT: u8 = 10;
    pub const MUSIC_TRACK: u8 = 11;
    pub const MUSIC_SWITCH: u8 = 12;
    pub const MUSIC_RANDOM_SEQUENCE: u8 = 13;
    /// `Play` action type.
    pub const ACTION_PLAY: u16 = 0x0403;
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SoundObj {
    pub id: u32,
    pub media_id: u32,
    pub plugin: u32,
    /// 0 = embedded in a bank (DIDX/DATA), 1/2 = streamed from a package.
    pub stream_type: u8,
    pub parent: Option<u32>,
}

/// What a soundbank says about media, sounds and events (bank version 150 layout).
#[derive(Debug, Clone, Default)]
pub struct BankInfo {
    pub id: u32,
    pub version: u32,
    pub language_id: u32,
    /// Media embedded in the bank (`DIDX`).
    pub embedded: Vec<u32>,
    pub sounds: Vec<SoundObj>,
    /// Music tracks and the media they play.
    pub tracks: Vec<(u32, Vec<u32>)>,
    /// Hierarchy nodes other than sounds: `(id, parent)`.
    pub nodes: Vec<(u32, u32)>,
    /// `(event id, action ids)`.
    pub events: Vec<(u32, Vec<u32>)>,
    /// `(action id, action type, target id)`.
    pub actions: Vec<(u32, u16, u32)>,
}

/// Size of an `AkBankSourceData` record (plugin, stream type, source id, size, bits),
/// plus plugin parameters for source plugins.
fn source_len(payload: &[u8], at: usize) -> Option<usize> {
    let plugin = u32_at(payload, at)?;
    let mut len = 14;
    if plugin & 0x0F == 2 {
        // Source plugin: u32 parameter size, then the parameters.
        len += 4 + u32_at(payload, at + 14)? as usize;
    }
    Some(len)
}

/// `DirectParentID` of an `AkNodeBaseParams` starting at `at`.
fn node_parent(payload: &[u8], at: usize) -> Option<u32> {
    let mut p = at;
    // Initial FX: override flag, count, [bypass bits, count * (index u8, fx id u32,
    // is-shareset u8, is-rendered u8)].
    let fx = *payload.get(p + 1)? as usize;
    p += 2;
    if fx > 0 {
        p += 1 + fx * 7;
    }
    // Metadata FX: override flag, count, count * (index u8, fx id u32, is-shareset u8).
    let meta = *payload.get(p + 1)? as usize;
    p += 2 + meta * 6;
    // Override bus id, then the direct parent.
    u32_at(payload, p + 4)
}

fn parse_hirc(chunk: &[u8], bank: &mut BankInfo) {
    let Some(count) = u32_at(chunk, 0) else {
        return;
    };
    let mut p = 4;
    for _ in 0..count {
        let (Some(&ty), Some(len)) = (chunk.get(p), u32_at(chunk, p + 1)) else {
            break;
        };
        let len = len as usize;
        let Some(obj) = chunk.get(p + 5..p + 5 + len) else {
            break;
        };
        p += 5 + len;
        let Some(id) = u32_at(obj, 0) else {
            continue;
        };
        let pl = &obj[4..];
        match ty {
            hirc::SOUND => {
                if let (Some(plugin), Some(&stream_type), Some(media_id)) =
                    (u32_at(pl, 0), pl.get(4), u32_at(pl, 5))
                {
                    let parent = source_len(pl, 0).and_then(|l| node_parent(pl, l));
                    bank.sounds.push(SoundObj {
                        id,
                        media_id,
                        plugin,
                        stream_type,
                        parent,
                    });
                }
            }
            hirc::ACTION => {
                if let (Some(kind), Some(target)) = (u16_at(pl, 0), u32_at(pl, 2)) {
                    bank.actions.push((id, kind, target));
                }
            }
            hirc::EVENT => {
                // u8 action count (bank v150), then action ids.
                let n = pl.first().copied().unwrap_or(0) as usize;
                let acts: Vec<u32> = (0..n).filter_map(|i| u32_at(pl, 1 + i * 4)).collect();
                bank.events.push((id, acts));
            }
            hirc::RANDOM_SEQUENCE | hirc::SWITCH | hirc::ACTOR_MIXER | hirc::LAYER => {
                if let Some(parent) = node_parent(pl, 0) {
                    bank.nodes.push((id, parent));
                }
            }
            hirc::MUSIC_SEGMENT | hirc::MUSIC_SWITCH | hirc::MUSIC_RANDOM_SEQUENCE => {
                // Music flags byte, then the node base params.
                if let Some(parent) = node_parent(pl, 1) {
                    bank.nodes.push((id, parent));
                }
            }
            hirc::MUSIC_TRACK => {
                // Flags u8, source count u32, sources.
                let n = u32_at(pl, 1).unwrap_or(0) as usize;
                let mut at = 5;
                let mut media = Vec::new();
                for _ in 0..n.min(4096) {
                    let Some(l) = source_len(pl, at) else { break };
                    if let Some(m) = u32_at(pl, at + 5) {
                        media.push(m);
                    }
                    at += l;
                }
                bank.tracks.push((id, media));
            }
            _ => {}
        }
    }
}

/// Parse a deciphered soundbank.
pub fn parse_bank(data: &[u8]) -> Result<BankInfo, String> {
    if !data.starts_with(b"BKHD") {
        return Err("not a soundbank (no BKHD)".into());
    }
    let mut bank = BankInfo::default();
    let mut p = 0;
    while p + 8 <= data.len() {
        let tag = &data[p..p + 4];
        let len = u32_at(data, p + 4).unwrap_or(0) as usize;
        let Some(chunk) = data.get(p + 8..p + 8 + len) else {
            break;
        };
        match tag {
            b"BKHD" => {
                bank.version = u32_at(chunk, 0).unwrap_or(0);
                bank.id = u32_at(chunk, 4).unwrap_or(0);
                bank.language_id = u32_at(chunk, 8).unwrap_or(0);
            }
            b"DIDX" => {
                bank.embedded = chunk
                    .as_chunks::<12>()
                    .0
                    .iter()
                    .filter_map(|c| u32_at(c, 0))
                    .collect();
            }
            b"HIRC" => parse_hirc(chunk, &mut bank),
            _ => {}
        }
        p += 8 + len;
    }
    Ok(bank)
}

/// Media id → ids of the events that can play it, from a set of parsed banks.
///
/// An event reaches media when one of its `Play` actions targets the media's sound (or
/// music track) or any ancestor of it in the actor-mixer / container hierarchy.
#[derive(Debug, Default)]
pub struct EventGraph {
    parent: HashMap<u32, u32>,
    /// Hierarchy object id → playing events.
    by_target: HashMap<u32, Vec<u32>>,
    /// Media id → sound / track objects using it.
    media: HashMap<u32, Vec<u32>>,
    pub events: HashSet<u32>,
}

impl EventGraph {
    pub fn add(&mut self, bank: &BankInfo) {
        let actions: HashMap<u32, (u16, u32)> = bank
            .actions
            .iter()
            .map(|&(id, kind, target)| (id, (kind, target)))
            .collect();
        for (event, acts) in &bank.events {
            self.events.insert(*event);
            for a in acts {
                if let Some(&(kind, target)) = actions.get(a)
                    && kind == hirc::ACTION_PLAY
                    && target != 0
                {
                    let v = self.by_target.entry(target).or_default();
                    if !v.contains(event) {
                        v.push(*event);
                    }
                }
            }
        }
        for s in &bank.sounds {
            if let Some(p) = s.parent.filter(|&p| p != 0) {
                self.parent.insert(s.id, p);
            }
            self.media.entry(s.media_id).or_default().push(s.id);
        }
        for (id, media) in &bank.tracks {
            for m in media {
                self.media.entry(*m).or_default().push(*id);
            }
        }
        for &(id, parent) in &bank.nodes {
            if parent != 0 {
                self.parent.insert(id, parent);
            }
        }
    }

    /// Whether some sound or track in the banks plays `media_id`.
    pub fn has_media(&self, media_id: u32) -> bool {
        self.media.contains_key(&media_id)
    }

    /// Events that can play `media_id` (sorted, deduplicated).
    pub fn events_for_media(&self, media_id: u32) -> Vec<u32> {
        let mut out = Vec::new();
        for &obj in self.media.get(&media_id).map(Vec::as_slice).unwrap_or(&[]) {
            let mut cur = Some(obj);
            let mut depth = 0;
            while let Some(id) = cur {
                if let Some(evs) = self.by_target.get(&id) {
                    out.extend_from_slice(evs);
                }
                depth += 1;
                if depth > 64 {
                    break;
                }
                cur = self.parent.get(&id).copied().filter(|&p| p != id);
            }
        }
        out.sort_unstable();
        out.dedup();
        out
    }
}

/// Find names among the identifier tokens of `bytes` (`[A-Za-z0-9_]` runs of 3..=100
/// bytes starting with a letter, lowercased) whose ShortID is in `wanted`. Works on any
/// text (JSON tables, Lua): event and bank names appear verbatim or as path stems.
pub fn scan_names(bytes: &[u8], wanted: &HashSet<u32>, out: &mut HashMap<u32, String>) {
    let word = |b: u8| b.is_ascii_alphanumeric() || b == b'_';
    let mut i = 0;
    while i < bytes.len() {
        if !word(bytes[i]) {
            i += 1;
            continue;
        }
        let start = i;
        while i < bytes.len() && word(bytes[i]) {
            i += 1;
        }
        let tok = &bytes[start..i];
        if (3..=100).contains(&tok.len()) && tok[0].is_ascii_alphabetic() {
            let mut h: u32 = 0x811c_9dc5;
            for &b in tok {
                h = h.wrapping_mul(0x0100_0193);
                h ^= u32::from(b.to_ascii_lowercase());
            }
            if wanted.contains(&h) && !out.contains_key(&h) {
                out.insert(h, String::from_utf8_lossy(tok).to_ascii_lowercase());
            }
        }
    }
}

// ── Index across packages ────────────────────────────────────────────────

/// One package in an [`AudioIndex`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackInfo {
    /// File stem, unique across groups (`default_stream_0`, `default_english_banks`).
    pub name: String,
    /// VFS group (`Audio`, `InitAudio`, `AudioEnglish`, ...).
    pub group: String,
    pub size: u64,
    pub languages: Vec<String>,
    pub banks: usize,
    pub streams: usize,
    pub externals: usize,
    /// Read over HTTP ranges instead of from disk.
    pub remote: bool,
}

/// A voice line attached to an external-source entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DialogLine {
    /// `AudioDialog` key (signed ShortID of the name).
    pub key: i32,
    /// File stem of the voice path, e.g. `chr_0005_chen_chrbark_squad_01`.
    pub name: String,
    pub path: String,
    pub vo_type: String,
    pub speaker: String,
    /// Per-language durations keyed by package language name.
    pub durations: HashMap<String, f32>,
}

/// One indexed entry. Field names are short on disk: the cached index holds ~230k.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexEntry {
    /// Index into [`AudioIndex::packs`].
    #[serde(rename = "p")]
    pub pack: u32,
    #[serde(rename = "i")]
    pub id: u64,
    #[serde(rename = "k")]
    pub kind: EntryKind,
    /// Package language name (`sfx`, `english`, ...).
    #[serde(rename = "l")]
    pub lang: String,
    #[serde(rename = "o")]
    pub offset: u64,
    #[serde(rename = "s")]
    pub size: u32,
    /// Codec family when known (`vorbis`, `opus`, ... or `bank`).
    #[serde(rename = "c", default, skip_serializing_if = "Option::is_none")]
    pub codec: Option<String>,
    #[serde(rename = "ch", default, skip_serializing_if = "Option::is_none")]
    pub channels: Option<u16>,
    #[serde(rename = "d", default, skip_serializing_if = "Option::is_none")]
    pub duration: Option<f32>,
    /// Display name: bank name, or voice name for voice-over.
    #[serde(rename = "n", default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Index into [`AudioIndex::dialog`].
    #[serde(rename = "dl", default, skip_serializing_if = "Option::is_none")]
    pub dialog: Option<u32>,
    /// Events that can play this media.
    #[serde(rename = "e", default, skip_serializing_if = "Vec::is_empty")]
    pub events: Vec<u32>,
}

/// Mapping coverage, reported by the API.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Coverage {
    pub streams: usize,
    /// Streams referenced by some Sound / music track object in a bank.
    pub streams_with_sound: usize,
    /// Streams reachable from at least one event.
    pub streams_with_event: usize,
    pub events: usize,
    pub events_named: usize,
    pub banks: usize,
    pub banks_named: usize,
    pub externals: usize,
    /// External sources matched to an `AudioDialog` line.
    pub externals_with_dialog: usize,
    pub dialog_lines: usize,
    /// `AudioDialog` lines with audio in at least one language.
    pub dialog_with_audio: usize,
}

/// Everything the audio endpoints serve, built once from the packages and tables.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AudioIndex {
    pub packs: Vec<PackInfo>,
    pub entries: Vec<IndexEntry>,
    pub dialog: Vec<DialogLine>,
    pub event_names: HashMap<u32, String>,
    pub coverage: Coverage,
}

impl AudioIndex {
    pub fn pack(&self, name: &str) -> Option<(u32, &PackInfo)> {
        self.packs
            .iter()
            .enumerate()
            .find(|(_, p)| p.name == name)
            .map(|(i, p)| (i as u32, p))
    }

    /// Lookup tables over the entries: `(pack, id)` → entry and dialog key → entries.
    pub fn lookups(&self) -> IndexLookups {
        let mut by_id = HashMap::with_capacity(self.entries.len());
        let mut by_dialog: HashMap<i32, Vec<u32>> = HashMap::new();
        for (i, e) in self.entries.iter().enumerate() {
            by_id.insert((e.pack, e.id), i as u32);
            if let Some(d) = e.dialog {
                by_dialog
                    .entry(self.dialog[d as usize].key)
                    .or_default()
                    .push(i as u32);
            }
        }
        IndexLookups { by_id, by_dialog }
    }
}

/// Hash lookups derived from an [`AudioIndex`] (not serialized).
#[derive(Debug, Default)]
pub struct IndexLookups {
    pub by_id: HashMap<(u32, u64), u32>,
    pub by_dialog: HashMap<i32, Vec<u32>>,
}

/// A package handed to [`IndexBuilder`].
pub struct PackInput<'a> {
    pub name: String,
    pub group: String,
    pub size: u64,
    pub table: &'a PckTable,
    /// Local file to probe entry headers and parse banks from; `None` for remote streams.
    pub local: Option<&'a PckFile>,
    pub remote: bool,
}

/// `AudioDialog` row as the index needs it.
pub struct DialogInput {
    pub key: i32,
    pub path: String,
    pub vo_type: String,
    pub speaker: String,
    pub codec: i64,
    /// `(package language, duration)`; the base duration is the Chinese one.
    pub durations: Vec<(String, f32)>,
}

/// Builds an [`AudioIndex`].
#[derive(Default)]
pub struct IndexBuilder {
    index: AudioIndex,
    graph: EventGraph,
}

impl IndexBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a package: every entry, headers probed and banks parsed when it is local.
    pub fn add_pack(&mut self, input: PackInput<'_>) {
        let table = input.table;
        let pack = self.index.packs.len() as u32;
        self.index.packs.push(PackInfo {
            name: input.name.clone(),
            group: input.group,
            size: input.size,
            languages: table.languages.iter().map(|(_, n)| n.clone()).collect(),
            banks: table.count(EntryKind::Bank),
            streams: table.count(EntryKind::Stream),
            externals: table.count(EntryKind::External),
            remote: input.remote,
        });
        let voice = table.voice_language().map(str::to_string);
        for e in &table.entries {
            let mut lang = table
                .language(e.lang_id)
                .unwrap_or("sfx")
                .to_ascii_lowercase();
            // Externals are tagged `sfx` even in the voice packages.
            if e.kind == EntryKind::External
                && let Some(v) = &voice
            {
                lang = v.clone();
            }
            let mut entry = IndexEntry {
                pack,
                id: e.id,
                kind: e.kind,
                lang,
                offset: e.offset,
                size: e.size,
                codec: None,
                channels: None,
                duration: None,
                name: None,
                dialog: None,
                events: Vec::new(),
            };
            if e.kind == EntryKind::Bank {
                entry.codec = Some("bank".into());
                if let Some(f) = input.local {
                    match f.read_entry(e, None).map(|d| parse_bank(&d)) {
                        Ok(Ok(bank)) => self.graph.add(&bank),
                        Ok(Err(err)) => tracing::debug!("{}: bank {}: {err}", input.name, e.id),
                        Err(err) => tracing::debug!("{}: bank {}: {err}", input.name, e.id),
                    }
                }
            } else if let Some(f) = input.local
                && let Ok(head) = f.read_entry(e, Some(256))
                && let Payload::Wem(info) = identify(&head)
            {
                entry.codec = Some(info.codec().to_string());
                entry.channels = Some(info.channels);
                entry.duration = info.duration().map(|d| d as f32);
            }
            self.index.entries.push(entry);
        }
    }

    /// Attach `AudioDialog` lines to external sources (by [`external_id`]).
    pub fn add_dialog(&mut self, rows: Vec<DialogInput>) {
        let mut externals: HashMap<(String, u64), Vec<usize>> = HashMap::new();
        for (i, e) in self.index.entries.iter().enumerate() {
            if e.kind == EntryKind::External {
                externals.entry((e.lang.clone(), e.id)).or_default().push(i);
            }
        }
        let languages: Vec<String> = {
            let mut l: Vec<String> = externals.keys().map(|(l, _)| l.clone()).collect();
            l.sort();
            l.dedup();
            l
        };
        let mut with_audio = 0;
        let total = rows.len();
        for row in rows {
            let name = row
                .path
                .rsplit('/')
                .next()
                .unwrap_or(&row.path)
                .trim_end_matches(".wem")
                .to_string();
            let line = self.index.dialog.len() as u32;
            let mut matched = false;
            for lang in &languages {
                let Some(hits) = externals.get(&(lang.clone(), external_id(lang, &row.path)))
                else {
                    continue;
                };
                matched = true;
                let dur = row
                    .durations
                    .iter()
                    .find(|(l, _)| l == lang)
                    .map(|(_, d)| *d)
                    .filter(|d| *d > 0.0);
                for &i in hits {
                    let e = &mut self.index.entries[i];
                    e.dialog = Some(line);
                    e.name = Some(name.clone());
                    if e.duration.is_none() {
                        e.duration = dur;
                    }
                    if e.codec.is_none() {
                        e.codec = match row.codec {
                            4 => Some("vorbis".into()),
                            20 => Some("opus".into()),
                            _ => None,
                        };
                    }
                }
            }
            with_audio += usize::from(matched);
            self.index.dialog.push(DialogLine {
                key: row.key,
                name,
                path: row.path,
                vo_type: row.vo_type,
                speaker: row.speaker,
                durations: row.durations.into_iter().collect(),
            });
        }
        self.index.coverage.dialog_lines = total;
        self.index.coverage.dialog_with_audio = with_audio;
    }

    /// ShortIDs worth naming: every event and bank (feed to [`scan_names`]).
    pub fn wanted_names(&self) -> HashSet<u32> {
        let mut ids: HashSet<u32> = self.graph.events.clone();
        ids.extend(
            self.index
                .entries
                .iter()
                .filter(|e| e.kind == EntryKind::Bank)
                .map(|e| e.id as u32),
        );
        ids
    }

    /// Link streams to events, name events and banks from `names` (ShortID → name),
    /// compute coverage.
    pub fn finish(mut self, names: &HashMap<u32, String>) -> AudioIndex {
        let by_hash = names;
        let mut cov = std::mem::take(&mut self.index.coverage);
        let mut used_events = HashSet::new();
        for e in &mut self.index.entries {
            match e.kind {
                EntryKind::Stream => {
                    cov.streams += 1;
                    let media = e.id as u32;
                    if self.graph.has_media(media) {
                        cov.streams_with_sound += 1;
                    }
                    e.events = self.graph.events_for_media(media);
                    if !e.events.is_empty() {
                        cov.streams_with_event += 1;
                        used_events.extend(e.events.iter().copied());
                    }
                }
                EntryKind::Bank => {
                    cov.banks += 1;
                    if let Some(n) = by_hash.get(&(e.id as u32)) {
                        e.name = Some(n.clone());
                        cov.banks_named += 1;
                    }
                }
                EntryKind::External => {
                    cov.externals += 1;
                    if e.dialog.is_some() {
                        cov.externals_with_dialog += 1;
                    }
                }
            }
        }
        for ev in self.graph.events.iter().chain(used_events.iter()) {
            if let Some(n) = by_hash.get(ev) {
                self.index.event_names.insert(*ev, n.clone());
            }
        }
        cov.events = self.graph.events.len();
        cov.events_named = self.index.event_names.len();
        self.index.coverage = cov;
        self.index
    }
}
