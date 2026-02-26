/// Phase R4: Network Crypto Analysis & Cracking — offline analysis of encrypted game traffic.
///
/// Pass 1: Multi-capture loader & payload extraction
/// Pass 2: Header hypothesis testing (h0, h1, h2)
/// Pass 3: Known-plaintext crib testing
/// Pass 4: Authentication tag isolation
/// Pass 5: Byte-position frequency analysis
/// Pass 6: Internal structure detection in bulk streams
/// Pass 7: Cross-session comparison (requires 2+ captures)
/// Pass 8: Decryption attempts & export
///
/// Input: pcapng / pcap / TCAP capture files
///
/// Usage:
///   cargo run --bin crypto-crack --release -- <capture1> [capture2...] [--port N] [--server IP] [--output-dir DIR]
use std::collections::HashMap;
use std::fs;
use std::net::Ipv4Addr;

// ══════════════════════════════════════════════════════════════
// Constants
// ══════════════════════════════════════════════════════════════

const TCAP_MAGIC: &[u8; 4] = b"TCAP";
const ENCRYPTED_HEADER: [u8; 4] = [0x05, 0x0F, 0x09, 0x0C];
const CONTROL_PREFIX: u8 = 0x0B;
const MIN_FRAME_SIZE: usize = 3;
const PCAP_MAGIC_LE: u32 = 0xA1B2_C3D4;
const PCAP_MAGIC_BE: u32 = 0xD4C3_B2A1;
const PCAPNG_SHB_MAGIC: [u8; 4] = [0x0A, 0x0D, 0x0D, 0x0A];
const PCAPNG_BOM: u32 = 0x1A2B_3C4D;
const DEFAULT_GAME_PORT: u16 = 30000;
const KNOWN_SERVER_IPS: &[&str] = &["172.65.253.17"];

/// Known protobuf first-byte cribs for known-plaintext analysis
const PROTOBUF_CRIBS: &[(u8, &str)] = &[
    (0x08, "field1 varint (type byte 0x08)"),
    (0x0C, "server response type byte (0x0C)"),
    (0x0A, "field1 length-delimited"),
    (0x10, "field2 varint"),
    (0x12, "field2 length-delimited"),
    (0x18, "field3 varint"),
    (0x20, "field4 varint"),
    (0x22, "field4 length-delimited"),
];

// ══════════════════════════════════════════════════════════════
// Data Structures (from tcp_dissect.rs)
// ══════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Direction {
    ClientToServer,
    ServerToClient,
}

impl std::fmt::Display for Direction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Direction::ClientToServer => write!(f, "C->S"),
            Direction::ServerToClient => write!(f, "S->C"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum FrameType {
    Handshake,
    Encrypted { header_u32s: [u32; 3] },
    Control { control_type: u8 },
    ServerSelect { command: u8 },
    BulkEncrypted,
    Unknown,
}

impl std::fmt::Display for FrameType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FrameType::Handshake => write!(f, "HANDSHAKE"),
            FrameType::Encrypted { header_u32s } => {
                write!(
                    f,
                    "ENCRYPTED [h0={}, h1={}, h2={}]",
                    header_u32s[0], header_u32s[1], header_u32s[2]
                )
            }
            FrameType::Control { control_type } => {
                write!(f, "CONTROL type=0x{control_type:02X}")
            }
            FrameType::ServerSelect { command } => {
                write!(f, "SERVER_SELECT cmd=0x{command:02X}")
            }
            FrameType::BulkEncrypted => write!(f, "BULK_ENCRYPTED"),
            FrameType::Unknown => write!(f, "UNKNOWN"),
        }
    }
}

#[derive(Debug, Clone)]
struct Frame {
    index: usize,
    direction: Direction,
    #[allow(dead_code)]
    timestamp_us: u64,
    frame_type: FrameType,
    raw: Vec<u8>,
    payload: Vec<u8>,
}

struct Capture {
    #[allow(dead_code)]
    version: u32,
    frames: Vec<Frame>,
    format_name: &'static str,
}

/// Structured encrypted payload for crypto analysis
struct EncryptedPayload {
    frame_index: usize,
    direction: Direction,
    h0: u32,
    h1: u32,
    h2: u32,
    ciphertext: Vec<u8>,
}

/// Structured handshake data
struct HandshakeData {
    direction: Direction,
    raw: Vec<u8>,
    fields: Vec<(u32, u8, u64)>,
    field7_bytes: Option<Vec<u8>>,
}

/// Structured bulk stream
struct BulkStream {
    frame_index: usize,
    direction: Direction,
    data: Vec<u8>,
}

// ══════════════════════════════════════════════════════════════
// Helpers (from tcp_dissect.rs)
// ══════════════════════════════════════════════════════════════

fn format_bytes(bytes: u64) -> String {
    const UNITS: &[&str] = &["B", "KB", "MB", "GB"];
    if bytes == 0 {
        return "0 B".into();
    }
    let mut size = bytes as f64;
    let mut i = 0;
    while size >= 1024.0 && i < UNITS.len() - 1 {
        size /= 1024.0;
        i += 1;
    }
    format!("{size:.2} {}", UNITS[i])
}

fn entropy(data: &[u8]) -> f64 {
    if data.is_empty() {
        return 0.0;
    }
    let mut freq = [0u32; 256];
    for &b in data {
        freq[b as usize] += 1;
    }
    let len_f = data.len() as f64;
    freq.iter()
        .filter(|&&c| c > 0)
        .map(|&c| {
            let p = c as f64 / len_f;
            -p * p.log2()
        })
        .sum()
}

fn hex_preview(data: &[u8], max_bytes: usize) -> String {
    let chunk = &data[..data.len().min(max_bytes)];
    let hex: Vec<String> = chunk.iter().map(|b| format!("{b:02X}")).collect();
    let ascii: String = chunk
        .iter()
        .map(|&b| {
            if (0x20..0x7F).contains(&b) {
                b as char
            } else {
                '.'
            }
        })
        .collect();
    let suffix = if data.len() > max_bytes {
        format!(" ... ({} total)", data.len())
    } else {
        String::new()
    };
    format!("{}{suffix}\n           ASCII: {ascii}", hex.join(" "))
}

fn decode_varint(data: &[u8]) -> Option<(u64, usize)> {
    let mut value: u64 = 0;
    let mut shift = 0u32;
    for (i, &byte) in data.iter().enumerate() {
        if shift >= 70 {
            return None;
        }
        value |= ((byte & 0x7F) as u64) << shift;
        shift += 7;
        if byte & 0x80 == 0 {
            return Some((value, i + 1));
        }
    }
    None
}

fn decode_protobuf_fields(data: &[u8]) -> Vec<(u32, u8, u64)> {
    let mut fields = Vec::new();
    let mut pos = 0;
    while pos < data.len() {
        let Some((tag, tag_len)) = decode_varint(&data[pos..]) else {
            break;
        };
        let field_num = (tag >> 3) as u32;
        let wire_type = (tag & 0x07) as u8;
        pos += tag_len;

        if field_num == 0 || field_num > 1000 {
            break;
        }

        match wire_type {
            0 => {
                let Some((val, val_len)) = decode_varint(&data[pos..]) else {
                    break;
                };
                fields.push((field_num, wire_type, val));
                pos += val_len;
            }
            1 => {
                if pos + 8 > data.len() {
                    break;
                }
                let val = u64::from_le_bytes(data[pos..pos + 8].try_into().unwrap());
                fields.push((field_num, wire_type, val));
                pos += 8;
            }
            2 => {
                let Some((len, len_bytes)) = decode_varint(&data[pos..]) else {
                    break;
                };
                let total_len = len as usize;
                pos += len_bytes;
                if pos + total_len > data.len() {
                    break;
                }
                fields.push((field_num, wire_type, len));
                pos += total_len;
            }
            5 => {
                if pos + 4 > data.len() {
                    break;
                }
                let val = u32::from_le_bytes(data[pos..pos + 4].try_into().unwrap()) as u64;
                fields.push((field_num, wire_type, val));
                pos += 4;
            }
            _ => break,
        }
    }
    fields
}

fn looks_like_protobuf(data: &[u8]) -> bool {
    let fields = decode_protobuf_fields(data);
    fields.len() >= 2
}

fn extract_field7_bytes(data: &[u8]) -> Option<Vec<u8>> {
    let mut pos = 0;
    while pos < data.len() {
        let (tag, tag_len) = decode_varint(&data[pos..])?;
        let field_num = (tag >> 3) as u32;
        let wire_type = (tag & 0x07) as u8;
        pos += tag_len;

        match wire_type {
            0 => {
                let start = pos;
                let (_, val_len) = decode_varint(&data[pos..])?;
                pos += val_len;
                if field_num == 7 {
                    return Some(data[start..pos].to_vec());
                }
            }
            1 => {
                if pos + 8 > data.len() {
                    return None;
                }
                if field_num == 7 {
                    return Some(data[pos..pos + 8].to_vec());
                }
                pos += 8;
            }
            2 => {
                let (len, len_bytes) = decode_varint(&data[pos..])?;
                pos += len_bytes;
                let len = len as usize;
                if pos + len > data.len() {
                    return None;
                }
                if field_num == 7 {
                    return Some(data[pos..pos + len].to_vec());
                }
                pos += len;
            }
            5 => {
                if pos + 4 > data.len() {
                    return None;
                }
                if field_num == 7 {
                    return Some(data[pos..pos + 4].to_vec());
                }
                pos += 4;
            }
            _ => return None,
        }
    }
    None
}

fn simple_md5(data: &[u8]) -> Vec<u8> {
    let mut state: [u32; 4] = [0x67452301, 0xEFCDAB89, 0x98BADCFE, 0x10325476];
    for (i, &byte) in data.iter().enumerate() {
        let idx = i % 4;
        state[idx] = state[idx]
            .wrapping_add(byte as u32)
            .wrapping_mul(0x5BD1E995)
            .rotate_left(13);
        state[(idx + 1) % 4] ^= state[idx];
    }
    for _ in 0..4 {
        for j in 0..4 {
            state[j] = state[j]
                .wrapping_mul(0xCC9E2D51)
                .rotate_left(15)
                .wrapping_mul(0x1B873593);
            state[(j + 1) % 4] ^= state[j];
        }
    }
    let mut out = Vec::with_capacity(16);
    for s in &state {
        out.extend_from_slice(&s.to_le_bytes());
    }
    out
}

/// Compute GCD of two numbers.
fn gcd(a: u64, b: u64) -> u64 {
    if b == 0 { a } else { gcd(b, a % b) }
}

/// Chi-squared statistic for byte frequency vs uniform distribution.
fn chi_squared(data: &[u8]) -> f64 {
    if data.is_empty() {
        return 0.0;
    }
    let mut freq = [0u32; 256];
    for &b in data {
        freq[b as usize] += 1;
    }
    let expected = data.len() as f64 / 256.0;
    freq.iter()
        .map(|&c| {
            let diff = c as f64 - expected;
            diff * diff / expected
        })
        .sum()
}

// ══════════════════════════════════════════════════════════════
// Parse: TCAP format
// ══════════════════════════════════════════════════════════════

fn parse_tcap(data: &[u8]) -> Option<Capture> {
    if data.len() < 8 {
        return None;
    }
    if &data[0..4] != TCAP_MAGIC {
        return None;
    }
    let version = u32::from_le_bytes(data[4..8].try_into().unwrap());
    let mut pos = 8;
    let mut frames = Vec::new();
    let mut index = 0;

    while pos + 13 <= data.len() {
        let timestamp_us = u64::from_le_bytes(data[pos..pos + 8].try_into().unwrap());
        let dir_byte = data[pos + 8];
        let length = u32::from_le_bytes(data[pos + 9..pos + 13].try_into().unwrap()) as usize;
        pos += 13;

        if pos + length > data.len() {
            break;
        }

        let raw = data[pos..pos + length].to_vec();
        pos += length;

        let direction = if dir_byte == 0x01 {
            Direction::ClientToServer
        } else {
            Direction::ServerToClient
        };

        let (frame_type, payload) = classify_frame(&raw);

        frames.push(Frame {
            index,
            direction,
            timestamp_us,
            frame_type,
            raw,
            payload,
        });
        index += 1;
    }

    Some(Capture {
        version,
        frames,
        format_name: "TCAP",
    })
}

fn parse_raw_dump(data: &[u8]) -> Capture {
    let mut frames = Vec::new();
    let mut pos = 0;
    let mut index = 0;

    while pos + 13 <= data.len() {
        let dir_byte = data[pos];
        if dir_byte != 0x01 && dir_byte != 0x02 {
            pos += 1;
            continue;
        }

        let ts_f64 = f64::from_le_bytes(data[pos + 1..pos + 9].try_into().unwrap());
        let length = u32::from_le_bytes(data[pos + 9..pos + 13].try_into().unwrap()) as usize;

        if length > 10 * 1024 * 1024 || pos + 13 + length > data.len() {
            pos += 1;
            continue;
        }

        pos += 13;
        let raw = data[pos..pos + length].to_vec();
        pos += length;

        let direction = if dir_byte == 0x01 {
            Direction::ClientToServer
        } else {
            Direction::ServerToClient
        };

        let timestamp_us = (ts_f64 * 1_000_000.0) as u64;
        let (frame_type, payload) = classify_frame(&raw);

        frames.push(Frame {
            index,
            direction,
            timestamp_us,
            frame_type,
            raw,
            payload,
        });
        index += 1;
    }

    Capture {
        version: 0,
        frames,
        format_name: "raw mitmproxy dump",
    }
}

// ══════════════════════════════════════════════════════════════
// Parse: pcap / pcapng files
// ══════════════════════════════════════════════════════════════

struct RawPacket {
    ts_us: u64,
    data: Vec<u8>,
}

struct TcpSegment {
    ts_us: u64,
    src_ip: Ipv4Addr,
    dst_ip: Ipv4Addr,
    src_port: u16,
    dst_port: u16,
    seq: u32,
    payload: Vec<u8>,
}

enum CaptureFormat {
    Tcap,
    PcapLe,
    PcapBe,
    PcapNg,
    Unknown,
}

fn detect_format(data: &[u8]) -> CaptureFormat {
    if data.len() < 4 {
        return CaptureFormat::Unknown;
    }
    if &data[0..4] == TCAP_MAGIC {
        return CaptureFormat::Tcap;
    }
    let magic32 = u32::from_le_bytes(data[0..4].try_into().unwrap());
    if magic32 == PCAP_MAGIC_LE {
        return CaptureFormat::PcapLe;
    }
    if magic32 == PCAP_MAGIC_BE {
        return CaptureFormat::PcapBe;
    }
    if data[0..4] == PCAPNG_SHB_MAGIC {
        return CaptureFormat::PcapNg;
    }
    CaptureFormat::Unknown
}

fn parse_pcap_legacy(data: &[u8], big_endian: bool) -> Vec<RawPacket> {
    let mut packets = Vec::new();
    if data.len() < 24 {
        return packets;
    }

    let read_u32 = if big_endian {
        |d: &[u8]| u32::from_be_bytes(d.try_into().unwrap())
    } else {
        |d: &[u8]| u32::from_le_bytes(d.try_into().unwrap())
    };

    let network = read_u32(&data[20..24]);
    if network != 1 && network != 101 && network != 113 {
        eprintln!("  [!] Unsupported pcap link type: {network}");
        return packets;
    }

    let mut pos = 24;
    while pos + 16 <= data.len() {
        let ts_sec = read_u32(&data[pos..pos + 4]);
        let ts_usec = read_u32(&data[pos + 4..pos + 8]);
        let incl_len = read_u32(&data[pos + 8..pos + 12]) as usize;
        pos += 16;

        if pos + incl_len > data.len() {
            break;
        }

        let ts_us = ts_sec as u64 * 1_000_000 + ts_usec as u64;
        let pkt_data = match network {
            1 => data[pos..pos + incl_len].to_vec(),
            101 => {
                let mut eth = vec![0u8; 14];
                eth[12] = 0x08;
                eth[13] = 0x00;
                eth.extend_from_slice(&data[pos..pos + incl_len]);
                eth
            }
            113 => {
                if incl_len >= 16 {
                    let proto = u16::from_be_bytes(data[pos + 14..pos + 16].try_into().unwrap());
                    if proto == 0x0800 {
                        let mut eth = vec![0u8; 14];
                        eth[12] = 0x08;
                        eth[13] = 0x00;
                        eth.extend_from_slice(&data[pos + 16..pos + incl_len]);
                        eth
                    } else {
                        pos += incl_len;
                        continue;
                    }
                } else {
                    pos += incl_len;
                    continue;
                }
            }
            _ => {
                pos += incl_len;
                continue;
            }
        };

        packets.push(RawPacket {
            ts_us,
            data: pkt_data,
        });
        pos += incl_len;
    }

    packets
}

fn parse_pcapng(data: &[u8]) -> Vec<RawPacket> {
    let mut packets = Vec::new();
    if data.len() < 28 {
        return packets;
    }

    let mut pos = 0;
    let mut is_le = true;
    let mut link_type: u16 = 1;
    let mut ts_resolution: u64 = 1_000_000;

    while pos + 12 <= data.len() {
        let block_type_bytes: [u8; 4] = data[pos..pos + 4].try_into().unwrap();

        if block_type_bytes == PCAPNG_SHB_MAGIC {
            if pos + 16 > data.len() {
                break;
            }
            let bom = u32::from_le_bytes(data[pos + 8..pos + 12].try_into().unwrap());
            is_le = bom == PCAPNG_BOM;

            let block_len = if is_le {
                u32::from_le_bytes(data[pos + 4..pos + 8].try_into().unwrap())
            } else {
                u32::from_be_bytes(data[pos + 4..pos + 8].try_into().unwrap())
            } as usize;

            if block_len < 12 || pos + block_len > data.len() {
                break;
            }
            pos += block_len;
            continue;
        }

        let read_u16 = |d: &[u8]| -> u16 {
            if is_le {
                u16::from_le_bytes(d.try_into().unwrap())
            } else {
                u16::from_be_bytes(d.try_into().unwrap())
            }
        };
        let read_u32 = |d: &[u8]| -> u32 {
            if is_le {
                u32::from_le_bytes(d.try_into().unwrap())
            } else {
                u32::from_be_bytes(d.try_into().unwrap())
            }
        };

        let block_type = read_u32(&data[pos..pos + 4]);
        let block_len = read_u32(&data[pos + 4..pos + 8]) as usize;

        if block_len < 12 || pos + block_len > data.len() {
            break;
        }

        let body_start = pos + 8;
        let body_end = pos + block_len - 4;
        let body_len = body_end.saturating_sub(body_start);

        match block_type {
            1 => {
                // Interface Description Block
                if body_len >= 4 {
                    link_type = read_u16(&data[body_start..body_start + 2]);
                    let mut opos = body_start + 8;
                    while opos + 4 <= body_end {
                        let opt_code = read_u16(&data[opos..opos + 2]);
                        let opt_len = read_u16(&data[opos + 2..opos + 4]) as usize;
                        if opt_code == 0 {
                            break;
                        }
                        if opt_code == 9 && opt_len == 1 && opos + 4 < body_end {
                            let tsresol = data[opos + 4];
                            if tsresol & 0x80 == 0 {
                                ts_resolution = 10u64.pow(tsresol as u32);
                            } else {
                                ts_resolution = 1u64 << (tsresol & 0x7F);
                            }
                        }
                        opos += 4 + ((opt_len + 3) & !3);
                    }
                }
            }
            6 => {
                // Enhanced Packet Block
                if body_len >= 20 {
                    let ts_high = read_u32(&data[body_start + 4..body_start + 8]);
                    let ts_low = read_u32(&data[body_start + 8..body_start + 12]);
                    let cap_len = read_u32(&data[body_start + 12..body_start + 16]) as usize;

                    let ts_raw = ((ts_high as u64) << 32) | (ts_low as u64);
                    let ts_us = if ts_resolution >= 1_000_000 {
                        ts_raw / (ts_resolution / 1_000_000)
                    } else if ts_resolution > 0 {
                        ts_raw * (1_000_000 / ts_resolution)
                    } else {
                        ts_raw
                    };

                    let pkt_start = body_start + 20;
                    if pkt_start + cap_len <= data.len() {
                        let pkt_data = match link_type {
                            1 => data[pkt_start..pkt_start + cap_len].to_vec(),
                            101 => {
                                let mut eth = vec![0u8; 14];
                                eth[12] = 0x08;
                                eth[13] = 0x00;
                                eth.extend_from_slice(&data[pkt_start..pkt_start + cap_len]);
                                eth
                            }
                            _ => {
                                pos += block_len;
                                continue;
                            }
                        };
                        packets.push(RawPacket {
                            ts_us,
                            data: pkt_data,
                        });
                    }
                }
            }
            _ => {}
        }

        pos += block_len;
    }

    packets
}

fn extract_tcp(ethernet_frame: &[u8]) -> Option<TcpSegment> {
    if ethernet_frame.len() < 14 {
        return None;
    }

    let mut ethertype = u16::from_be_bytes(ethernet_frame[12..14].try_into().unwrap());
    let mut ip_start = 14;

    if ethertype == 0x8100 {
        if ethernet_frame.len() < 18 {
            return None;
        }
        ethertype = u16::from_be_bytes(ethernet_frame[16..18].try_into().unwrap());
        ip_start = 18;
    }

    if ethertype != 0x0800 {
        return None;
    }

    if ethernet_frame.len() < ip_start + 20 {
        return None;
    }

    let version_ihl = ethernet_frame[ip_start];
    if version_ihl >> 4 != 4 {
        return None;
    }
    let ihl = (version_ihl & 0x0F) as usize * 4;
    if ihl < 20 {
        return None;
    }

    if ethernet_frame[ip_start + 9] != 6 {
        return None;
    }

    let src_ip = Ipv4Addr::new(
        ethernet_frame[ip_start + 12],
        ethernet_frame[ip_start + 13],
        ethernet_frame[ip_start + 14],
        ethernet_frame[ip_start + 15],
    );
    let dst_ip = Ipv4Addr::new(
        ethernet_frame[ip_start + 16],
        ethernet_frame[ip_start + 17],
        ethernet_frame[ip_start + 18],
        ethernet_frame[ip_start + 19],
    );

    let tcp_start = ip_start + ihl;
    if ethernet_frame.len() < tcp_start + 20 {
        return None;
    }

    let src_port = u16::from_be_bytes(ethernet_frame[tcp_start..tcp_start + 2].try_into().unwrap());
    let dst_port = u16::from_be_bytes(
        ethernet_frame[tcp_start + 2..tcp_start + 4]
            .try_into()
            .unwrap(),
    );
    let seq = u32::from_be_bytes(
        ethernet_frame[tcp_start + 4..tcp_start + 8]
            .try_into()
            .unwrap(),
    );
    let data_offset = ((ethernet_frame[tcp_start + 12] >> 4) as usize) * 4;
    if data_offset < 20 {
        return None;
    }

    let payload_start = tcp_start + data_offset;
    let payload = if payload_start < ethernet_frame.len() {
        ethernet_frame[payload_start..].to_vec()
    } else {
        vec![]
    };

    Some(TcpSegment {
        ts_us: 0,
        src_ip,
        dst_ip,
        src_port,
        dst_port,
        seq,
        payload,
    })
}

// ══════════════════════════════════════════════════════════════
// TCP Stream Reassembly
// ══════════════════════════════════════════════════════════════

struct StreamMeta {
    stream_offset: usize,
    timestamp_us: u64,
}

fn timestamp_at_offset(meta: &[StreamMeta], offset: usize) -> u64 {
    for m in meta.iter().rev() {
        if offset >= m.stream_offset {
            return m.timestamp_us;
        }
    }
    meta.first().map_or(0, |m| m.timestamp_us)
}

fn reassemble_streams(
    segments: &[TcpSegment],
    server_ip: Ipv4Addr,
) -> (Vec<u8>, Vec<StreamMeta>, Vec<u8>, Vec<StreamMeta>) {
    let mut cs_stream: Vec<u8> = Vec::new();
    let mut cs_meta: Vec<StreamMeta> = Vec::new();
    let mut sc_stream: Vec<u8> = Vec::new();
    let mut sc_meta: Vec<StreamMeta> = Vec::new();

    for seg in segments {
        if seg.payload.is_empty() {
            continue;
        }
        let is_cs = seg.dst_ip == server_ip;
        let (stream, meta) = if is_cs {
            (&mut cs_stream, &mut cs_meta)
        } else {
            (&mut sc_stream, &mut sc_meta)
        };

        meta.push(StreamMeta {
            stream_offset: stream.len(),
            timestamp_us: seg.ts_us,
        });
        stream.extend_from_slice(&seg.payload);
    }

    (cs_stream, cs_meta, sc_stream, sc_meta)
}

fn extract_frames_from_stream(
    stream: &[u8],
    meta: &[StreamMeta],
    direction: Direction,
    start_index: &mut usize,
) -> Vec<Frame> {
    let mut frames: Vec<Frame> = Vec::new();

    let mut encrypted_ranges: Vec<(usize, usize)> = Vec::new();
    {
        let mut pos = 0;
        while pos + 16 <= stream.len() {
            if stream[pos..pos + 4] == ENCRYPTED_HEADER {
                let h0 = u32::from_le_bytes(stream[pos + 4..pos + 8].try_into().unwrap()) as usize;
                if h0 >= 16 && pos + h0 <= stream.len() {
                    encrypted_ranges.push((pos, pos + h0));
                    pos += h0;
                    continue;
                }
            }
            pos += 1;
        }
    }

    let mut cursor = 0;
    for &(enc_start, enc_end) in &encrypted_ranges {
        if cursor < enc_start {
            emit_non_encrypted(
                &stream[cursor..enc_start],
                meta,
                cursor,
                direction,
                start_index,
                &mut frames,
            );
        }

        let raw = stream[enc_start..enc_end].to_vec();
        let (ft, payload) = classify_frame(&raw);
        frames.push(Frame {
            index: *start_index,
            direction,
            timestamp_us: timestamp_at_offset(meta, enc_start),
            frame_type: ft,
            raw,
            payload,
        });
        *start_index += 1;
        cursor = enc_end;
    }

    if cursor < stream.len() {
        emit_non_encrypted(
            &stream[cursor..],
            meta,
            cursor,
            direction,
            start_index,
            &mut frames,
        );
    }

    frames
}

fn emit_non_encrypted(
    data: &[u8],
    meta: &[StreamMeta],
    stream_offset: usize,
    direction: Direction,
    index: &mut usize,
    frames: &mut Vec<Frame>,
) {
    if data.is_empty() {
        return;
    }

    let (ft, payload) = classify_frame(data);
    frames.push(Frame {
        index: *index,
        direction,
        timestamp_us: timestamp_at_offset(meta, stream_offset),
        frame_type: ft,
        raw: data.to_vec(),
        payload,
    });
    *index += 1;
}

fn detect_server_ip(segments: &[TcpSegment], port: u16) -> Option<Ipv4Addr> {
    let mut ip_counts: HashMap<Ipv4Addr, usize> = HashMap::new();
    for seg in segments {
        if seg.src_port == port {
            *ip_counts.entry(seg.src_ip).or_default() += 1;
        }
        if seg.dst_port == port {
            *ip_counts.entry(seg.dst_ip).or_default() += 1;
        }
    }

    let mut candidates: Vec<_> = ip_counts.into_iter().collect();
    candidates.sort_by(|a, b| b.1.cmp(&a.1));

    for (ip, _count) in &candidates {
        if !ip.is_private() && !ip.is_loopback() {
            return Some(*ip);
        }
    }

    candidates.first().map(|(ip, _)| *ip)
}

fn parse_pcap_capture(
    data: &[u8],
    format: &CaptureFormat,
    filter_port: u16,
    server_ip_override: Option<Ipv4Addr>,
) -> Capture {
    let raw_packets = match format {
        CaptureFormat::PcapLe => parse_pcap_legacy(data, false),
        CaptureFormat::PcapBe => parse_pcap_legacy(data, true),
        CaptureFormat::PcapNg => parse_pcapng(data),
        _ => vec![],
    };

    let format_name = match format {
        CaptureFormat::PcapLe | CaptureFormat::PcapBe => "pcap",
        CaptureFormat::PcapNg => "pcapng",
        _ => "unknown",
    };

    println!(
        "  Parsed {} raw packets from {format_name} file",
        raw_packets.len()
    );

    let mut segments: Vec<TcpSegment> = Vec::new();
    for pkt in &raw_packets {
        if let Some(mut seg) = extract_tcp(&pkt.data) {
            seg.ts_us = pkt.ts_us;
            segments.push(seg);
        }
    }

    println!("  Extracted {} TCP segments", segments.len());

    let segments: Vec<TcpSegment> = segments
        .into_iter()
        .filter(|s| s.src_port == filter_port || s.dst_port == filter_port)
        .collect();

    println!(
        "  After port {filter_port} filter: {} segments",
        segments.len()
    );

    if segments.is_empty() {
        let mut port_counts: HashMap<u16, usize> = HashMap::new();
        for pkt in &raw_packets {
            if let Some(seg) = extract_tcp(&pkt.data) {
                *port_counts.entry(seg.src_port).or_default() += 1;
                *port_counts.entry(seg.dst_port).or_default() += 1;
            }
        }
        let mut sorted_ports: Vec<_> = port_counts.into_iter().collect();
        sorted_ports.sort_by(|a, b| b.1.cmp(&a.1));
        println!("  Top ports in capture:");
        for (port, count) in sorted_ports.iter().take(10) {
            println!("    port {port}: {count} segments");
        }

        return Capture {
            version: 0,
            frames: vec![],
            format_name,
        };
    }

    // Dedup retransmissions
    let mut seen_seqs: HashMap<(Ipv4Addr, u16, Ipv4Addr, u16, u32), bool> = HashMap::new();
    let mut filtered: Vec<TcpSegment> = Vec::new();

    for seg in segments {
        if seg.payload.is_empty() {
            continue;
        }
        let key = (seg.src_ip, seg.src_port, seg.dst_ip, seg.dst_port, seg.seq);
        if seen_seqs.contains_key(&key) {
            continue;
        }
        seen_seqs.insert(key, true);
        filtered.push(seg);
    }

    println!("  {} segments with payload", filtered.len());

    let server_ip = server_ip_override.or_else(|| {
        for known in KNOWN_SERVER_IPS {
            let ip: Ipv4Addr = known.parse().unwrap();
            if filtered.iter().any(|s| s.src_ip == ip || s.dst_ip == ip) {
                println!("  Server IP: {ip} (known)");
                return Some(ip);
            }
        }
        let detected = detect_server_ip(&filtered, filter_port);
        if let Some(ip) = detected {
            println!("  Server IP: {ip} (auto-detected)");
        }
        detected
    });

    if server_ip.is_none() {
        println!("  [!] Could not determine server IP");
    }
    if server_ip_override.is_some() {
        println!("  Server IP: {} (from --server flag)", server_ip.unwrap());
    }

    let frames = if let Some(srv) = server_ip {
        let (cs_stream, cs_meta, sc_stream, sc_meta) = reassemble_streams(&filtered, srv);

        println!(
            "  Stream reassembly: C->S {} bytes, S->C {} bytes",
            cs_stream.len(),
            sc_stream.len()
        );

        let mut idx = 0;
        let mut frames =
            extract_frames_from_stream(&cs_stream, &cs_meta, Direction::ClientToServer, &mut idx);
        frames.extend(extract_frames_from_stream(
            &sc_stream,
            &sc_meta,
            Direction::ServerToClient,
            &mut idx,
        ));

        frames.sort_by_key(|f| f.timestamp_us);
        for (i, frame) in frames.iter_mut().enumerate() {
            frame.index = i;
        }

        frames
    } else {
        filtered
            .iter()
            .enumerate()
            .map(|(idx, seg)| {
                let (ft, payload) = classify_frame(&seg.payload);
                Frame {
                    index: idx,
                    direction: Direction::ClientToServer,
                    timestamp_us: seg.ts_us,
                    frame_type: ft,
                    raw: seg.payload.clone(),
                    payload,
                }
            })
            .collect()
    };

    Capture {
        version: 0,
        frames,
        format_name,
    }
}

// ══════════════════════════════════════════════════════════════
// Frame Classification
// ══════════════════════════════════════════════════════════════

fn classify_frame(raw: &[u8]) -> (FrameType, Vec<u8>) {
    if raw.len() < MIN_FRAME_SIZE {
        return (FrameType::Unknown, raw.to_vec());
    }

    if raw.len() == 3 && raw[0] == CONTROL_PREFIX && raw[2] == 0x00 {
        return (
            FrameType::Control {
                control_type: raw[1],
            },
            vec![],
        );
    }

    if raw.len() >= 16 && raw[0..4] == ENCRYPTED_HEADER {
        let h0 = u32::from_le_bytes(raw[4..8].try_into().unwrap());
        let h1 = u32::from_le_bytes(raw[8..12].try_into().unwrap());
        let h2 = u32::from_le_bytes(raw[12..16].try_into().unwrap());
        let payload = if raw.len() > 16 {
            raw[16..].to_vec()
        } else {
            vec![]
        };
        return (
            FrameType::Encrypted {
                header_u32s: [h0, h1, h2],
            },
            payload,
        );
    }

    if raw.len() >= 6 && raw[0] == 0x01 && raw[1] == 0x00 && raw[2] == 0x00 {
        let command = if raw.len() > 5 { raw[5] } else { 0 };
        return (FrameType::ServerSelect { command }, raw[3..].to_vec());
    }

    if raw.len() <= 256 && looks_like_protobuf(raw) {
        return (FrameType::Handshake, raw.to_vec());
    }

    if raw.len() > 256 {
        let e = entropy(raw);
        if e > 7.0 {
            return (FrameType::BulkEncrypted, raw.to_vec());
        }
    }

    (FrameType::Unknown, raw.to_vec())
}

// ══════════════════════════════════════════════════════════════
// Extraction: build typed structures from Capture
// ══════════════════════════════════════════════════════════════

fn extract_payloads(
    capture: &Capture,
) -> (Vec<EncryptedPayload>, Vec<HandshakeData>, Vec<BulkStream>) {
    let mut encrypted = Vec::new();
    let mut handshakes = Vec::new();
    let mut bulk = Vec::new();

    for frame in &capture.frames {
        match &frame.frame_type {
            FrameType::Encrypted { header_u32s } => {
                encrypted.push(EncryptedPayload {
                    frame_index: frame.index,
                    direction: frame.direction,
                    h0: header_u32s[0],
                    h1: header_u32s[1],
                    h2: header_u32s[2],
                    ciphertext: frame.payload.clone(),
                });
            }
            FrameType::Handshake => {
                let fields = decode_protobuf_fields(&frame.raw);
                let field7_bytes = extract_field7_bytes(&frame.raw);
                handshakes.push(HandshakeData {
                    direction: frame.direction,
                    raw: frame.raw.clone(),
                    fields,
                    field7_bytes,
                });
            }
            FrameType::BulkEncrypted => {
                bulk.push(BulkStream {
                    frame_index: frame.index,
                    direction: frame.direction,
                    data: frame.raw.clone(),
                });
            }
            _ => {}
        }
    }

    (encrypted, handshakes, bulk)
}

// ══════════════════════════════════════════════════════════════
// Pass 1: Multi-Capture Loader & Payload Extraction
// ══════════════════════════════════════════════════════════════

fn pass1_loader(
    capture: &Capture,
    encrypted: &[EncryptedPayload],
    handshakes: &[HandshakeData],
    bulk: &[BulkStream],
) {
    println!("\n{}", "=".repeat(70));
    println!("  PASS 1: Multi-Capture Loader & Payload Extraction");
    println!("{}", "=".repeat(70));
    println!(
        "  Format: {}  |  Total frames: {}",
        capture.format_name,
        capture.frames.len()
    );
    println!();

    // Type census
    let mut type_counts: HashMap<&str, usize> = HashMap::new();
    let mut cs_bytes = 0u64;
    let mut sc_bytes = 0u64;

    for frame in &capture.frames {
        let key = match &frame.frame_type {
            FrameType::Handshake => "HANDSHAKE",
            FrameType::Encrypted { .. } => "ENCRYPTED",
            FrameType::Control { .. } => "CONTROL",
            FrameType::ServerSelect { .. } => "SERVER_SELECT",
            FrameType::BulkEncrypted => "BULK_ENCRYPTED",
            FrameType::Unknown => "UNKNOWN",
        };
        *type_counts.entry(key).or_default() += 1;

        match frame.direction {
            Direction::ClientToServer => cs_bytes += frame.raw.len() as u64,
            Direction::ServerToClient => sc_bytes += frame.raw.len() as u64,
        }
    }

    println!(
        "  Traffic: C->S {}, S->C {}",
        format_bytes(cs_bytes),
        format_bytes(sc_bytes)
    );
    println!();

    println!("  Frame type census:");
    let mut sorted_types: Vec<_> = type_counts.iter().collect();
    sorted_types.sort_by(|a, b| b.1.cmp(a.1));
    for (type_name, count) in &sorted_types {
        println!("    {type_name:20} : {count}");
    }

    println!();
    println!("  Extracted payloads:");
    println!(
        "    Encrypted frames:  {} (total {} ciphertext)",
        encrypted.len(),
        format_bytes(encrypted.iter().map(|e| e.ciphertext.len() as u64).sum())
    );
    println!("    Handshakes:        {}", handshakes.len());
    println!(
        "    Bulk streams:      {} (total {})",
        bulk.len(),
        format_bytes(bulk.iter().map(|b| b.data.len() as u64).sum())
    );

    // Print encrypted frame details
    if !encrypted.is_empty() {
        println!();
        println!("  Encrypted frame details:");
        println!(
            "    {:<6} {:<6} {:<10} {:<10} {:<10} {:<10} FIRST_BYTES",
            "IDX", "DIR", "h0", "h1", "h2", "CT_LEN"
        );
        println!("    {}", "-".repeat(80));
        for ep in encrypted.iter().take(20) {
            let hex: String = ep
                .ciphertext
                .iter()
                .take(16)
                .map(|b| format!("{b:02X}"))
                .collect::<Vec<_>>()
                .join(" ");
            println!(
                "    {:<6} {:<6} {:<10} {:<10} {:<10} {:<10} {hex}",
                ep.frame_index,
                ep.direction,
                ep.h0,
                ep.h1,
                ep.h2,
                ep.ciphertext.len()
            );
        }
    }

    // Print handshake details
    for hs in handshakes {
        println!();
        println!("  Handshake ({}):", hs.direction);
        for &(field_num, wire_type, value) in &hs.fields {
            println!("    field {field_num} (wt={wire_type}): {value} (0x{value:X})");
        }
        if let Some(ref f7) = hs.field7_bytes {
            println!(
                "    field7 raw ({} bytes): {}",
                f7.len(),
                hex_preview(f7, 32)
            );
        }
    }
}

// ══════════════════════════════════════════════════════════════
// Pass 2: Header Hypothesis Testing
// ══════════════════════════════════════════════════════════════

fn pass2_header_hypothesis(encrypted: &[EncryptedPayload]) {
    println!("\n{}", "=".repeat(70));
    println!("  PASS 2: Header Hypothesis Testing (h0, h1, h2)");
    println!("{}", "=".repeat(70));

    if encrypted.is_empty() {
        println!("  No encrypted frames to analyze.");
        return;
    }

    // Hypothesis 1: h0 = total frame size (header + payload)
    println!("\n  2a: h0 interpretation:");
    for ep in encrypted.iter().take(10) {
        let total_raw = ep.ciphertext.len() + 16; // 4 magic + 3*4 header
        let diff = ep.h0 as i64 - total_raw as i64;
        println!(
            "    frame {:>3}: h0={:<8} raw_total={:<8} diff={:<6} payload_size={:<8} h0-16={}",
            ep.frame_index,
            ep.h0,
            total_raw,
            diff,
            ep.ciphertext.len(),
            ep.h0 as i64 - 16
        );
    }

    // Check: is h0 always == raw_total?
    let h0_is_total = encrypted
        .iter()
        .all(|ep| ep.h0 as usize == ep.ciphertext.len() + 16);
    if h0_is_total {
        println!("    -> CONFIRMED: h0 = total frame size (magic + 3*u32 + payload)");
    } else {
        println!("    -> h0 does NOT always equal total frame size");
    }

    // Hypothesis 2: h1 interpretation
    println!("\n  2b: h1 interpretation:");
    println!("    Testing: h1 = plaintext/compressed size, overhead = ciphertext_len - h1");
    println!();

    let mut overheads: Vec<i64> = Vec::new();
    let mut h1_always_less = true;

    for ep in encrypted.iter().take(20) {
        let ct_len = ep.ciphertext.len() as i64;
        let overhead = ct_len - ep.h1 as i64;
        overheads.push(overhead);
        if ep.h1 as i64 > ct_len {
            h1_always_less = false;
        }

        let mod16 = overhead % 16;
        let mod8 = overhead % 8;
        println!(
            "    frame {:>3}: h1={:<8} ct_len={:<8} overhead={:<6} mod16={} mod8={}",
            ep.frame_index, ep.h1, ct_len, overhead, mod16, mod8
        );
    }

    if !overheads.is_empty() {
        let overhead_gcd = overheads
            .iter()
            .fold(0u64, |acc, &v| gcd(acc, v.unsigned_abs()));

        println!();
        println!("    h1 always < ciphertext_len: {h1_always_less}");
        println!("    Overhead GCD: {overhead_gcd}");

        if overhead_gcd % 16 == 0 {
            println!("    -> Overhead is 16-byte aligned: consistent with PKCS7 padding + MAC");
        } else if overhead_gcd % 8 == 0 {
            println!("    -> Overhead is 8-byte aligned: possible 8-byte block cipher");
        } else {
            println!("    -> Overhead NOT block-aligned: variable auth tag or compression wrapper");
        }

        // Split ciphertext at h1 offset: compare entropy of body vs tail
        println!();
        println!("    Body vs tail entropy (split at h1):");
        for ep in encrypted.iter().take(10) {
            let h1 = ep.h1 as usize;
            if h1 > 0 && h1 < ep.ciphertext.len() {
                let body = &ep.ciphertext[..h1];
                let tail = &ep.ciphertext[h1..];
                let body_ent = entropy(body);
                let tail_ent = entropy(tail);
                println!(
                    "      frame {:>3}: body[..{}] ent={:.4}  tail[{}..] ({} B) ent={:.4}",
                    ep.frame_index,
                    h1,
                    body_ent,
                    h1,
                    tail.len(),
                    tail_ent
                );
            }
        }
    }

    // Hypothesis 3: h2 interpretation
    println!("\n  2c: h2 interpretation:");
    let h2_vals: Vec<u32> = encrypted.iter().map(|ep| ep.h2).collect();

    let is_strictly_inc = h2_vals.windows(2).all(|w| w[1] > w[0]);
    let is_monotonic = h2_vals.windows(2).all(|w| w[1] >= w[0]);

    println!("    h2 values: {:?}", &h2_vals[..h2_vals.len().min(20)]);
    println!("    Strictly increasing: {is_strictly_inc}");
    println!("    Monotonically increasing: {is_monotonic}");

    if h2_vals.len() >= 2 {
        let gaps: Vec<i64> = h2_vals
            .windows(2)
            .map(|w| w[1] as i64 - w[0] as i64)
            .collect();
        let gap_gcd = gaps.iter().fold(0u64, |acc, &v| gcd(acc, v.unsigned_abs()));
        println!("    h2 gaps: {:?}", &gaps[..gaps.len().min(20)]);
        println!("    Gap GCD: {gap_gcd}");

        if is_strictly_inc && gap_gcd == 1 {
            println!("    -> h2 is a dense sequence counter (increment by 1)");
        } else if is_strictly_inc {
            println!(
                "    -> h2 is a sequence counter with gap={gap_gcd} (implies {gap_gcd} msgs between captures)"
            );
        } else {
            println!("    -> h2 does not follow a simple counter pattern");
        }
    }

    // Summary
    println!("\n  2d: Header hypothesis summary:");
    println!("    h0 = total frame size (4-byte magic + 12-byte header + payload)");
    if h1_always_less {
        println!(
            "    h1 = plaintext/compressed body size (overhead = MAC/padding/compression wrapper)"
        );
    } else {
        println!("    h1 = unclear interpretation (not always < ciphertext)");
    }
    if is_strictly_inc {
        println!("    h2 = sequence counter");
    } else {
        println!("    h2 = unclear interpretation");
    }
}

// ══════════════════════════════════════════════════════════════
// Pass 3: Known-Plaintext Crib Testing
// ══════════════════════════════════════════════════════════════

fn pass3_crib_testing(encrypted: &[EncryptedPayload], handshakes: &[HandshakeData]) {
    println!("\n{}", "=".repeat(70));
    println!("  PASS 3: Known-Plaintext Crib Testing");
    println!("{}", "=".repeat(70));

    if encrypted.is_empty() {
        println!("  No encrypted frames to test.");
        return;
    }

    // Determine handshake type bytes
    println!("\n  3a: Handshake type byte analysis:");
    for hs in handshakes {
        if !hs.raw.is_empty() {
            println!(
                "    {} handshake first byte: 0x{:02X}",
                hs.direction, hs.raw[0]
            );
        }
    }

    // Test XOR of first byte against cribs
    println!("\n  3b: XOR crib test (byte 0 of each encrypted frame):");
    println!("    If stream cipher: XOR(ct[0], known_pt[0]) = keystream[0]");
    println!();

    let cs_frames: Vec<_> = encrypted
        .iter()
        .filter(|ep| ep.direction == Direction::ClientToServer)
        .collect();
    let sc_frames: Vec<_> = encrypted
        .iter()
        .filter(|ep| ep.direction == Direction::ServerToClient)
        .collect();

    for (label, frames, expected_type) in
        [("C->S", &cs_frames, 0x08u8), ("S->C", &sc_frames, 0x0Cu8)]
    {
        if frames.is_empty() {
            continue;
        }

        println!("    {label} frames ({} total):", frames.len());
        println!("      Expected type byte: 0x{expected_type:02X}");

        // XOR first byte with expected type byte to get keystream byte 0
        let keystream_bytes: Vec<u8> = frames
            .iter()
            .filter(|ep| !ep.ciphertext.is_empty())
            .map(|ep| ep.ciphertext[0] ^ expected_type)
            .collect();

        let unique_ks: std::collections::HashSet<_> = keystream_bytes.iter().collect();
        println!(
            "      Keystream byte 0 candidates ({} frames): {} unique values",
            keystream_bytes.len(),
            unique_ks.len()
        );

        if unique_ks.len() == 1 {
            let ks = keystream_bytes[0];
            println!("      *** ALL SAME: keystream[0] = 0x{ks:02X} ***");
            println!("      -> KEYSTREAM REUSE DETECTED! Stream cipher with same key+nonce");
        } else if unique_ks.len() <= 3 {
            println!("      Few unique values — possible mode with limited nonce space");
            for &ks in &keystream_bytes {
                print!("0x{ks:02X} ");
            }
            println!();
        } else {
            println!(
                "      All different — unique nonces per frame (no keystream reuse at byte 0)"
            );
        }

        // Test all cribs against all frames
        println!();
        println!("      Full crib test (all known protobuf prefixes):");
        for &(crib, desc) in PROTOBUF_CRIBS {
            let ks_vals: Vec<u8> = frames
                .iter()
                .filter(|ep| !ep.ciphertext.is_empty())
                .map(|ep| ep.ciphertext[0] ^ crib)
                .collect();
            let unique: std::collections::HashSet<_> = ks_vals.iter().collect();
            let consistency = if unique.len() == 1 {
                "*** CONSISTENT ***"
            } else if unique.len() <= frames.len() / 2 {
                "partially consistent"
            } else {
                "inconsistent"
            };
            println!(
                "        crib=0x{crib:02X} ({desc}): {}/{} unique ks values -> {consistency}",
                unique.len(),
                ks_vals.len()
            );
        }
    }

    // Multi-byte crib test (first 4 bytes)
    println!("\n  3c: Multi-byte crib test (first 4 bytes):");
    for ep in encrypted.iter().take(10) {
        if ep.ciphertext.len() < 4 {
            continue;
        }
        let ct = &ep.ciphertext[..4];

        // For C->S: plaintext likely starts with protobuf tag 0x08 or 0x0A
        // For S->C: plaintext likely starts with 0x0C or 0x0A
        let type_byte = match ep.direction {
            Direction::ClientToServer => 0x08u8,
            Direction::ServerToClient => 0x0Cu8,
        };

        // XOR with assumed type byte as first plaintext byte
        let ks0 = ct[0] ^ type_byte;
        println!(
            "    frame {:>3} ({}): ct[0..4]=[{:02X} {:02X} {:02X} {:02X}]  type=0x{type_byte:02X}  ks[0]=0x{ks0:02X}",
            ep.frame_index, ep.direction, ct[0], ct[1], ct[2], ct[3]
        );
    }

    // Cross-frame XOR (pairs of same direction)
    println!("\n  3d: Cross-frame XOR (same direction, first 32 bytes):");
    for dir_label in ["C->S", "S->C"] {
        let dir = if dir_label == "C->S" {
            Direction::ClientToServer
        } else {
            Direction::ServerToClient
        };
        let same_dir: Vec<_> = encrypted
            .iter()
            .filter(|ep| ep.direction == dir && ep.ciphertext.len() >= 32)
            .collect();

        if same_dir.len() >= 2 {
            println!("    {dir_label}: {} frames with 32+ bytes", same_dir.len());
            for i in 0..same_dir.len().min(5) {
                for j in (i + 1)..same_dir.len().min(5) {
                    let xored: Vec<u8> = same_dir[i]
                        .ciphertext
                        .iter()
                        .zip(same_dir[j].ciphertext.iter())
                        .take(32)
                        .map(|(a, b)| a ^ b)
                        .collect();
                    let xor_ent = entropy(&xored);
                    let hex: String = xored
                        .iter()
                        .take(16)
                        .map(|b| format!("{b:02X}"))
                        .collect::<Vec<_>>()
                        .join(" ");
                    println!(
                        "      f{} ^ f{}: ent={:.4}  xor={hex}",
                        same_dir[i].frame_index, same_dir[j].frame_index, xor_ent
                    );
                }
            }
        }
    }
}

// ══════════════════════════════════════════════════════════════
// Pass 4: Authentication Tag Isolation
// ══════════════════════════════════════════════════════════════

fn pass4_auth_tag(encrypted: &[EncryptedPayload]) {
    println!("\n{}", "=".repeat(70));
    println!("  PASS 4: Authentication Tag Isolation");
    println!("{}", "=".repeat(70));

    if encrypted.is_empty() {
        println!("  No encrypted frames to analyze.");
        return;
    }

    // Split at h1 offset
    println!("\n  4a: Split at h1 offset — body vs tail:");
    let common_mac_sizes = [4, 8, 12, 16, 20, 32];

    for ep in encrypted.iter().take(10) {
        let h1 = ep.h1 as usize;
        if h1 == 0 || h1 >= ep.ciphertext.len() {
            println!(
                "    frame {:>3}: h1={} >= ct_len={} — cannot split",
                ep.frame_index,
                h1,
                ep.ciphertext.len()
            );
            continue;
        }

        let tail = &ep.ciphertext[h1..];
        let tail_len = tail.len();
        let tail_ent = entropy(tail);
        let tail_hex: String = tail
            .iter()
            .take(32)
            .map(|b| format!("{b:02X}"))
            .collect::<Vec<_>>()
            .join(" ");

        println!(
            "    frame {:>3}: tail[h1={}..] = {} bytes, ent={:.4}",
            ep.frame_index, h1, tail_len, tail_ent
        );
        println!("      tail hex: {tail_hex}");

        // Check for PKCS7 padding in last bytes of tail
        if let Some(&last_byte) = tail.last() {
            let pad_len = last_byte as usize;
            if pad_len > 0 && pad_len <= 16 && pad_len <= tail.len() {
                let is_pkcs7 = tail[tail.len() - pad_len..].iter().all(|&b| b == last_byte);
                if is_pkcs7 {
                    println!("      -> PKCS7 padding detected: pad_len={pad_len}");
                }
            }
        }
    }

    // Check common MAC sizes from end of ciphertext
    println!("\n  4b: Common MAC size test (from end of ciphertext):");
    for mac_size in &common_mac_sizes {
        let mut tails: Vec<Vec<u8>> = Vec::new();
        let mut all_different = true;

        for ep in encrypted {
            if ep.ciphertext.len() > *mac_size {
                let mac = ep.ciphertext[ep.ciphertext.len() - mac_size..].to_vec();
                if tails.iter().any(|t| t == &mac) {
                    all_different = false;
                }
                tails.push(mac);
            }
        }

        let unique_count = {
            let mut unique = tails.clone();
            unique.sort();
            unique.dedup();
            unique.len()
        };

        println!(
            "    MAC size {mac_size}: {unique_count}/{} unique tails, all_different={}",
            tails.len(),
            all_different
        );

        if all_different && !tails.is_empty() {
            println!("      -> All different: consistent with data-dependent MAC/HMAC");
        }
    }

    // Check: does h1 + fixed_overhead == ciphertext_len for any fixed overhead?
    println!("\n  4c: Fixed overhead search (h1 + X = ct_len):");
    if encrypted.len() >= 2 {
        let overheads: Vec<i64> = encrypted
            .iter()
            .map(|ep| ep.ciphertext.len() as i64 - ep.h1 as i64)
            .collect();

        let unique_overheads: std::collections::HashSet<i64> = overheads.iter().copied().collect();

        if unique_overheads.len() == 1 {
            let fixed = overheads[0];
            println!("    *** FIXED OVERHEAD: {fixed} bytes ***");
            println!("    -> h1 + {fixed} = ciphertext_len for ALL frames");
            if fixed == 16 {
                println!("    -> Exactly 16 bytes: likely AES-GCM auth tag");
            } else if fixed == 32 {
                println!("    -> Exactly 32 bytes: likely HMAC-SHA256");
            } else if fixed == 12 {
                println!("    -> Exactly 12 bytes: likely ChaCha20-Poly1305 tag");
            }
        } else {
            println!("    Overhead varies across frames:");
            let mut sorted_overheads: Vec<_> = unique_overheads.iter().collect();
            sorted_overheads.sort();
            for oh in sorted_overheads.iter().take(20) {
                let count = overheads.iter().filter(|&&v| v == **oh).count();
                println!("      overhead {oh}: {count} frames");
            }
            println!("    -> Variable overhead: compression + MAC, or variable-length auth");
        }
    }
}

// ══════════════════════════════════════════════════════════════
// Pass 5: Byte-Position Frequency Analysis
// ══════════════════════════════════════════════════════════════

fn pass5_frequency_analysis(encrypted: &[EncryptedPayload], bulk: &[BulkStream]) {
    println!("\n{}", "=".repeat(70));
    println!("  PASS 5: Byte-Position Frequency Analysis");
    println!("{}", "=".repeat(70));

    // Per-position chi-squared across all encrypted payloads
    println!("\n  5a: Per-position chi-squared (encrypted frames):");

    if encrypted.is_empty() {
        println!("    No encrypted frames.");
    } else {
        let max_len = encrypted
            .iter()
            .map(|ep| ep.ciphertext.len())
            .max()
            .unwrap_or(0);
        let check_len = max_len.min(128); // Check first 128 positions

        let mut biased_positions: Vec<(usize, f64, usize)> = Vec::new(); // (pos, chi2, sample_count)

        for pos in 0..check_len {
            let bytes_at_pos: Vec<u8> = encrypted
                .iter()
                .filter(|ep| ep.ciphertext.len() > pos)
                .map(|ep| ep.ciphertext[pos])
                .collect();

            if bytes_at_pos.len() < 10 {
                continue;
            }

            let chi2 = chi_squared(&bytes_at_pos);
            // Critical value for chi-squared with 255 df at p=0.01 is ~310
            if chi2 > 310.0 {
                biased_positions.push((pos, chi2, bytes_at_pos.len()));
            }
        }

        if biased_positions.is_empty() {
            println!("    No statistically significant bias at any position (p>0.01)");
            println!("    -> Cipher has good diffusion");
        } else {
            println!("    Biased positions (chi-squared > 310, p<0.01):");
            for (pos, chi2, n) in &biased_positions {
                println!("      position {pos}: chi2={chi2:.1}, n={n}");
            }
            println!("    -> Cipher has measurable bias at these positions!");
        }

        // Also compute per-position entropy
        println!();
        println!("    Per-position entropy (first 32 positions):");
        for pos in 0..check_len.min(32) {
            let bytes_at_pos: Vec<u8> = encrypted
                .iter()
                .filter(|ep| ep.ciphertext.len() > pos)
                .map(|ep| ep.ciphertext[pos])
                .collect();
            if bytes_at_pos.len() >= 2 {
                let ent = entropy(&bytes_at_pos);
                let unique = bytes_at_pos
                    .iter()
                    .collect::<std::collections::HashSet<_>>()
                    .len();
                let marker = if ent < 3.0 { " *** LOW ***" } else { "" };
                println!(
                    "      pos {:>3}: ent={:.4}, {}/{} unique values{marker}",
                    pos,
                    ent,
                    unique,
                    bytes_at_pos.len()
                );
            }
        }
    }

    // Sliding-window entropy on bulk streams
    println!("\n  5b: Sliding-window entropy on bulk streams:");
    for bs in bulk {
        if bs.data.len() < 256 {
            println!(
                "    Stream {} ({}): too short ({} bytes)",
                bs.frame_index,
                bs.direction,
                bs.data.len()
            );
            continue;
        }

        let window = 256;
        let step = 64;
        let mut low_entropy_windows: Vec<(usize, f64)> = Vec::new();
        let mut max_ent = 0.0f64;
        let mut min_ent = 8.0f64;
        let mut total_ent = 0.0f64;
        let mut count = 0;

        let mut offset = 0;
        while offset + window <= bs.data.len() {
            let ent = entropy(&bs.data[offset..offset + window]);
            total_ent += ent;
            count += 1;
            if ent > max_ent {
                max_ent = ent;
            }
            if ent < min_ent {
                min_ent = ent;
            }
            if ent < 7.0 {
                low_entropy_windows.push((offset, ent));
            }
            offset += step;
        }

        let avg_ent = if count > 0 {
            total_ent / count as f64
        } else {
            0.0
        };
        println!(
            "    Stream {} ({}): {} bytes, {} windows",
            bs.frame_index,
            bs.direction,
            bs.data.len(),
            count
        );
        println!("      Entropy: min={min_ent:.4} max={max_ent:.4} avg={avg_ent:.4}");

        if low_entropy_windows.is_empty() {
            println!("      No low-entropy windows (all > 7.0) — fully encrypted");
        } else {
            println!(
                "      {} low-entropy windows (< 7.0) — potential plaintext leaks:",
                low_entropy_windows.len()
            );
            for (off, ent) in low_entropy_windows.iter().take(10) {
                let preview: String = bs.data[*off..*off + 16.min(window)]
                    .iter()
                    .map(|b| format!("{b:02X}"))
                    .collect::<Vec<_>>()
                    .join(" ");
                println!("        offset 0x{off:06X}: ent={ent:.4}  {preview}");
            }
        }
    }
}

// ══════════════════════════════════════════════════════════════
// Pass 6: Internal Structure Detection in Bulk Streams
// ══════════════════════════════════════════════════════════════

fn pass6_bulk_structure(bulk: &[BulkStream]) {
    println!("\n{}", "=".repeat(70));
    println!("  PASS 6: Internal Structure Detection in Bulk Streams");
    println!("{}", "=".repeat(70));

    if bulk.is_empty() {
        println!("  No bulk encrypted streams to analyze.");
        return;
    }

    for bs in bulk {
        println!(
            "\n  Stream {} ({}, {} bytes):",
            bs.frame_index,
            bs.direction,
            bs.data.len()
        );

        // 6a: Autocorrelation at specific lags
        println!("    6a: Autocorrelation:");
        let lags = [16, 32, 64, 128, 256, 512, 1024, 4096];
        for lag in &lags {
            if *lag >= bs.data.len() {
                continue;
            }
            let overlap = bs.data.len() - lag;
            let matching: usize = bs.data[..*lag]
                .iter()
                .zip(bs.data[*lag..lag * 2.min(bs.data.len())].iter())
                .filter(|(a, b)| a == b)
                .count();
            let match_rate = matching as f64 / overlap.min(*lag) as f64;
            let expected = 1.0 / 256.0;
            let ratio = match_rate / expected;
            let marker = if ratio > 2.0 { " *** HIGH ***" } else { "" };
            println!(
                "      lag {:>5}: match_rate={:.6} (expected {expected:.6}, ratio {ratio:.2}x){marker}",
                lag, match_rate
            );
        }

        // 6b: Bigram frequency analysis
        println!("    6b: Bigram frequency:");
        if bs.data.len() >= 2 {
            let mut bigram_freq: HashMap<[u8; 2], u32> = HashMap::new();
            for pair in bs.data.windows(2) {
                let key: [u8; 2] = [pair[0], pair[1]];
                *bigram_freq.entry(key).or_default() += 1;
            }

            let total_bigrams = (bs.data.len() - 1) as f64;
            let expected_per_bigram = total_bigrams / 65536.0;

            // Find over-represented bigrams
            let mut anomalous: Vec<_> = bigram_freq
                .iter()
                .filter(|&(_, count)| *count as f64 > expected_per_bigram * 3.0 && *count >= 5)
                .collect();
            anomalous.sort_by(|a, b| b.1.cmp(a.1));

            if anomalous.is_empty() {
                println!("      No over-represented bigrams — uniform distribution");
            } else {
                println!("      Top over-represented bigrams (> 3x expected):");
                for (pair, count) in anomalous.iter().take(10) {
                    let ratio = **count as f64 / expected_per_bigram;
                    println!(
                        "        [{:02X} {:02X}]: {} times ({ratio:.1}x expected)",
                        pair[0], pair[1], count
                    );
                }
            }
        }

        // 6c: N-gram repeat scan
        println!("    6c: Repeated N-gram scan:");
        for n in [4, 8, 16] {
            if bs.data.len() < n * 2 {
                continue;
            }

            let mut ngram_freq: HashMap<&[u8], u32> = HashMap::new();
            for window in bs.data.windows(n) {
                *ngram_freq.entry(window).or_default() += 1;
            }

            let repeats: Vec<_> = ngram_freq
                .iter()
                .filter(|&(_, count)| *count >= 3)
                .collect();

            let total_windows = bs.data.len() - n + 1;
            // For random data, probability of any specific n-gram repeating 3+ times is negligible
            // for n >= 8 in reasonable data sizes
            if repeats.is_empty() {
                println!("      {n}-gram: no repeats (3+) — consistent with strong cipher");
            } else {
                let mut sorted_repeats: Vec<_> = repeats.into_iter().collect();
                sorted_repeats.sort_by(|a, b| b.1.cmp(a.1));
                println!(
                    "      {n}-gram: {} patterns repeat 3+ times out of {total_windows} windows:",
                    sorted_repeats.len()
                );
                for (pattern, count) in sorted_repeats.iter().take(5) {
                    let hex: String = pattern
                        .iter()
                        .map(|b| format!("{b:02X}"))
                        .collect::<Vec<_>>()
                        .join(" ");
                    println!("        {hex}: {count} times");
                }
            }
        }
    }
}

// ══════════════════════════════════════════════════════════════
// Pass 7: Cross-Session Comparison
// ══════════════════════════════════════════════════════════════

fn pass7_cross_session(captures: &[(Capture, Vec<EncryptedPayload>, Vec<HandshakeData>)]) {
    println!("\n{}", "=".repeat(70));
    println!("  PASS 7: Cross-Session Comparison");
    println!("{}", "=".repeat(70));

    if captures.len() < 2 {
        println!(
            "  Only {} capture(s) loaded. Need 2+ for cross-session analysis.",
            captures.len()
        );
        println!();
        println!("  To enable this pass, provide multiple capture files:");
        println!("    cargo run --bin crypto-crack --release -- capture1.pcapng capture2.pcapng");
        println!();
        println!("  What this pass would check:");
        println!("    - h2 starting values (static counter vs session-derived nonce)");
        println!("    - XOR first encrypted frames across captures (same keystream = exploitable)");
        println!("    - Handshake field 7 values (changing = client random / DH)");
        return;
    }

    println!("  Comparing {} captures:", captures.len());

    // Compare h2 starting values
    println!("\n  7a: h2 starting values across sessions:");
    for (i, (_, enc, _)) in captures.iter().enumerate() {
        if let Some(first) = enc.first() {
            println!(
                "    Capture {i}: h2_start = {} (0x{:08X})",
                first.h2, first.h2
            );
        }
    }

    let h2_starts: Vec<u32> = captures
        .iter()
        .filter_map(|(_, enc, _)| enc.first().map(|ep| ep.h2))
        .collect();
    let unique_h2: std::collections::HashSet<_> = h2_starts.iter().collect();
    if unique_h2.len() == 1 {
        println!("    -> Same h2 start: static counter (NOT session-dependent)");
    } else {
        println!("    -> Different h2 starts: session-dependent counter/nonce");
    }

    // XOR first encrypted frames across captures
    println!("\n  7b: Cross-session XOR (first encrypted frame):");
    for i in 0..captures.len() {
        for j in (i + 1)..captures.len() {
            let a = captures[i].1.first();
            let b = captures[j].1.first();
            if let (Some(a), Some(b)) = (a, b) {
                let overlap = a.ciphertext.len().min(b.ciphertext.len());
                if overlap >= 16 {
                    let xored: Vec<u8> = a
                        .ciphertext
                        .iter()
                        .zip(b.ciphertext.iter())
                        .take(overlap)
                        .map(|(x, y)| x ^ y)
                        .collect();
                    let xor_ent = entropy(&xored);
                    let hex: String = xored
                        .iter()
                        .take(16)
                        .map(|b| format!("{b:02X}"))
                        .collect::<Vec<_>>()
                        .join(" ");
                    println!(
                        "    Capture {} ^ {}: xor_ent={:.4}, first16={hex}",
                        i, j, xor_ent
                    );
                    if xor_ent < 6.0 {
                        println!("      *** LOW XOR ENTROPY: SAME KEYSTREAM ACROSS SESSIONS! ***");
                    }
                }
            }
        }
    }

    // Compare handshake field 7
    println!("\n  7c: Handshake field 7 comparison:");
    for (i, (_, _, hs)) in captures.iter().enumerate() {
        for h in hs {
            if let Some(ref f7) = h.field7_bytes {
                let hex: String = f7
                    .iter()
                    .take(16)
                    .map(|b| format!("{b:02X}"))
                    .collect::<Vec<_>>()
                    .join(" ");
                println!(
                    "    Capture {i} ({} handshake): field7 = {hex} ({} bytes)",
                    h.direction,
                    f7.len()
                );
            }
        }
    }
}

// ══════════════════════════════════════════════════════════════
// Pass 8: Decryption Attempts & Export
// ══════════════════════════════════════════════════════════════

fn pass8_decrypt_and_export(
    encrypted: &[EncryptedPayload],
    handshakes: &[HandshakeData],
    bulk: &[BulkStream],
    output_dir: &str,
) {
    println!("\n{}", "=".repeat(70));
    println!("  PASS 8: Decryption Attempts & Export");
    println!("{}", "=".repeat(70));

    // 8a: Export raw payloads as binary files
    println!("\n  8a: Exporting raw payloads to {output_dir}/");

    let payload_dir = format!("{output_dir}/payloads");
    let _ = fs::create_dir_all(&payload_dir);

    let mut exported = 0;
    for ep in encrypted {
        if !ep.ciphertext.is_empty() {
            let filename = format!(
                "{payload_dir}/encrypted_f{:04}_{}.bin",
                ep.frame_index,
                if ep.direction == Direction::ClientToServer {
                    "cs"
                } else {
                    "sc"
                }
            );
            if fs::write(&filename, &ep.ciphertext).is_ok() {
                exported += 1;
            }
        }
    }
    println!("    Exported {exported} encrypted payloads");

    for bs in bulk {
        let filename = format!(
            "{payload_dir}/bulk_f{:04}_{}.bin",
            bs.frame_index,
            if bs.direction == Direction::ClientToServer {
                "cs"
            } else {
                "sc"
            }
        );
        let _ = fs::write(&filename, &bs.data);
    }
    println!("    Exported {} bulk streams", bulk.len());

    // Export handshakes
    for (i, hs) in handshakes.iter().enumerate() {
        let filename = format!(
            "{payload_dir}/handshake_{i}_{}.bin",
            if hs.direction == Direction::ClientToServer {
                "cs"
            } else {
                "sc"
            }
        );
        let _ = fs::write(&filename, &hs.raw);
    }
    println!("    Exported {} handshakes", handshakes.len());

    // 8b: Build key candidates from handshake
    println!("\n  8b: Key candidate generation:");
    let mut key_candidates: Vec<(String, Vec<u8>)> = Vec::new();

    let client_f7 = handshakes
        .iter()
        .find(|h| h.direction == Direction::ClientToServer)
        .and_then(|h| h.field7_bytes.clone());
    let server_f7 = handshakes
        .iter()
        .find(|h| h.direction == Direction::ServerToClient)
        .and_then(|h| h.field7_bytes.clone());

    if let Some(ref cf7) = client_f7 {
        if cf7.len() >= 16 {
            key_candidates.push(("client_f7[..16]".into(), cf7[..16].to_vec()));
        }
        key_candidates.push(("hash(client_f7)".into(), simple_md5(cf7)));
    }
    if let Some(ref sf7) = server_f7 {
        if sf7.len() >= 16 {
            key_candidates.push(("server_f7[..16]".into(), sf7[..16].to_vec()));
        }
        key_candidates.push(("hash(server_f7)".into(), simple_md5(sf7)));
    }
    if let (Some(c), Some(s)) = (&client_f7, &server_f7) {
        let len = c.len().min(s.len()).min(16);
        let xored: Vec<u8> = c
            .iter()
            .zip(s.iter())
            .take(len)
            .map(|(a, b)| a ^ b)
            .collect();
        if xored.len() >= 16 {
            key_candidates.push(("xor(c_f7, s_f7)".into(), xored));
        }
        let mut concat = c.clone();
        concat.extend_from_slice(s);
        key_candidates.push(("hash(c_f7 || s_f7)".into(), simple_md5(&concat)));
    }

    // h2 as counter/nonce component
    if let Some(first) = encrypted.first() {
        let h2_bytes = first.h2.to_le_bytes();
        let mut key = vec![0u8; 16];
        key[..4].copy_from_slice(&h2_bytes);
        key_candidates.push(("h2_as_nonce_le".into(), key));
    }

    if key_candidates.is_empty() {
        println!("    No key candidates (no handshake data)");
    } else {
        println!("    Generated {} key candidates:", key_candidates.len());
        for (name, key) in &key_candidates {
            let hex: String = key
                .iter()
                .map(|b| format!("{b:02X}"))
                .collect::<Vec<_>>()
                .join("");
            println!("      {name}: {hex}");
        }
    }

    // 8c: Decryption attempts
    println!("\n  8c: Decryption attempts:");

    if let Some(first_enc) = encrypted.first() {
        if first_enc.ciphertext.is_empty() {
            println!("    First encrypted frame has empty ciphertext.");
            return;
        }

        println!(
            "    Testing against frame {} ({} bytes):",
            first_enc.frame_index,
            first_enc.ciphertext.len()
        );

        for (name, key) in &key_candidates {
            // XOR decryption
            let decrypted: Vec<u8> = first_enc
                .ciphertext
                .iter()
                .zip(key.iter().cycle())
                .map(|(a, b)| a ^ b)
                .collect();
            let ent = entropy(&decrypted);
            let printable = decrypted
                .iter()
                .take(64)
                .filter(|&&b| (0x20..0x7F).contains(&b))
                .count();
            let printable_pct = printable as f64 / 64.0f64.min(decrypted.len() as f64) * 100.0;
            let is_proto = looks_like_protobuf(&decrypted);

            let status = if ent < 5.0 && printable_pct > 60.0 {
                "*** POSSIBLE HIT ***"
            } else if is_proto {
                "*** PROTOBUF DETECTED ***"
            } else if ent < 6.5 {
                "low entropy"
            } else {
                "no match"
            };

            println!(
                "      XOR({name}): ent={ent:.2}, printable={printable_pct:.0}%, proto={is_proto} -> {status}"
            );

            if ent < 6.5 || is_proto {
                println!("        preview: {}", hex_preview(&decrypted, 32));
            }
        }

        // Try AES-128-ECB with each key candidate (using aes crate)
        println!();
        println!("    AES-128-ECB attempts:");
        use aes::Aes128;
        use aes::cipher::{BlockDecrypt, BlockEncrypt, KeyInit};

        for (name, key) in &key_candidates {
            if key.len() < 16 {
                continue;
            }

            let cipher = Aes128::new(key[..16].into());
            let ct = &first_enc.ciphertext;
            if ct.len() < 16 {
                continue;
            }

            // Decrypt first block
            let mut block = aes::Block::clone_from_slice(&ct[..16]);
            cipher.decrypt_block(&mut block);

            let ent = entropy(&block);
            let printable = block.iter().filter(|&&b| (0x20..0x7F).contains(&b)).count();
            let hex: String = block
                .iter()
                .map(|b| format!("{b:02X}"))
                .collect::<Vec<_>>()
                .join(" ");

            let status = if ent < 3.0 {
                "*** LOW ENTROPY ***"
            } else {
                "no match"
            };
            println!(
                "      AES-ECB({name}): block0 ent={ent:.2}, printable={printable}/16 -> {status}"
            );
            println!("        block0: {hex}");
        }

        // Try AES-128-CTR with h2 as nonce
        println!();
        println!("    AES-128-CTR attempts (h2 as nonce):");
        for (name, key) in &key_candidates {
            if key.len() < 16 {
                continue;
            }

            let cipher = Aes128::new(key[..16].into());
            let ct = &first_enc.ciphertext;

            // CTR mode: encrypt counter blocks and XOR with ciphertext
            let mut nonce = [0u8; 16];
            nonce[..4].copy_from_slice(&first_enc.h2.to_le_bytes());

            let mut keystream_block = aes::Block::clone_from_slice(&nonce);
            cipher.encrypt_block(&mut keystream_block);

            let decrypted: Vec<u8> = ct
                .iter()
                .take(16)
                .zip(keystream_block.iter())
                .map(|(c, k)| c ^ k)
                .collect();

            let ent = entropy(&decrypted);
            let is_proto = looks_like_protobuf(&decrypted);
            let hex: String = decrypted
                .iter()
                .map(|b| format!("{b:02X}"))
                .collect::<Vec<_>>()
                .join(" ");

            let status = if ent < 3.0 || is_proto {
                "*** CHECK ***"
            } else {
                "no match"
            };
            println!("      AES-CTR({name}, nonce=h2): ent={ent:.2}, proto={is_proto} -> {status}");
            println!("        first16: {hex}");
        }
    }

    // 8d: Generate Frida hook script
    println!("\n  8d: Frida hook script:");
    let hooks_path = format!("{output_dir}/frida_hooks_net.js");
    println!("    See: {hooks_path}");
    println!("    (Generate separately or find at backend/output/frida/frida_hooks_net.js)");

    // 8e: Summary and next steps
    println!("\n  8e: Analysis summary & next steps:");
    println!("    Exported files: {payload_dir}/");
    println!("    Total encrypted payloads: {}", encrypted.len());
    println!("    Total bulk streams: {}", bulk.len());
    println!();
    println!("    Most likely: strong encryption requires runtime key extraction");
    println!("    Next steps:");
    println!("      1. Run with 2+ captures for cross-session comparison");
    println!("      2. Use frida_hooks_net.js to intercept send/recv at runtime");
    println!("      3. Correlate intercepted plaintext with exported ciphertext");
    println!("      4. Once key is known, implement full decryptor");
}

// ══════════════════════════════════════════════════════════════
// CLI Argument Parsing
// ══════════════════════════════════════════════════════════════

struct CliArgs {
    file_paths: Vec<String>,
    port: u16,
    server_ip: Option<Ipv4Addr>,
    output_dir: String,
}

fn parse_args() -> CliArgs {
    let args: Vec<String> = std::env::args().collect();

    if args.len() < 2 {
        eprintln!(
            "Usage: crypto-crack <capture1> [capture2...] [--port N] [--server IP] [--output-dir DIR]"
        );
        eprintln!();
        eprintln!("Phase R4: Network Crypto Analysis & Cracking");
        eprintln!();
        eprintln!("Formats (auto-detected): TCAP, pcap, pcapng, raw mitmproxy dump");
        eprintln!();
        eprintln!("Options:");
        eprintln!("  --port N          Filter pcap to TCP port N (default: {DEFAULT_GAME_PORT})");
        eprintln!("  --server IP       Server IP for direction detection");
        eprintln!("  --output-dir DIR  Directory for exported payloads (default: ./output/crypto)");
        eprintln!();
        eprintln!("Examples:");
        eprintln!("  cargo run --bin crypto-crack --release -- capture.pcapng");
        eprintln!("  cargo run --bin crypto-crack --release -- cap1.pcapng cap2.pcapng");
        eprintln!(
            "  cargo run --bin crypto-crack --release -- capture.pcapng --output-dir ./output/crypto"
        );
        std::process::exit(1);
    }

    let mut file_paths = Vec::new();
    let mut port = DEFAULT_GAME_PORT;
    let mut server_ip = None;
    let mut output_dir = String::from("./output/crypto");

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--port" => {
                if i + 1 < args.len() {
                    port = args[i + 1].parse().unwrap_or_else(|_| {
                        eprintln!("  [!] Invalid port: {}", args[i + 1]);
                        std::process::exit(1);
                    });
                    i += 1;
                }
            }
            "--server" => {
                if i + 1 < args.len() {
                    server_ip = Some(args[i + 1].parse().unwrap_or_else(|_| {
                        eprintln!("  [!] Invalid IP: {}", args[i + 1]);
                        std::process::exit(1);
                    }));
                    i += 1;
                }
            }
            "--output-dir" => {
                if i + 1 < args.len() {
                    output_dir = args[i + 1].clone();
                    i += 1;
                }
            }
            arg if !arg.starts_with("--") => {
                file_paths.push(arg.to_string());
            }
            _ => {
                eprintln!("  [!] Unknown flag: {}", args[i]);
            }
        }
        i += 1;
    }

    if file_paths.is_empty() {
        eprintln!("  [!] No capture files specified");
        std::process::exit(1);
    }

    CliArgs {
        file_paths,
        port,
        server_ip,
        output_dir,
    }
}

// ══════════════════════════════════════════════════════════════
// Load a single capture file
// ══════════════════════════════════════════════════════════════

fn load_capture(path: &str, port: u16, server_ip: Option<Ipv4Addr>) -> Capture {
    println!("\n  Loading: {path}");
    let data = match fs::read(path) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("  [!] Failed to read {path}: {e}");
            std::process::exit(1);
        }
    };
    println!("  Size: {}", format_bytes(data.len() as u64));

    let format = detect_format(&data);
    match format {
        CaptureFormat::Tcap => {
            if let Some(tcap) = parse_tcap(&data) {
                println!("  Format: TCAP (version {})", tcap.version);
                tcap
            } else {
                eprintln!("  [!] TCAP magic found but parse failed");
                std::process::exit(1);
            }
        }
        CaptureFormat::PcapLe | CaptureFormat::PcapBe | CaptureFormat::PcapNg => {
            let fmt_name = match format {
                CaptureFormat::PcapLe => "pcap (LE)",
                CaptureFormat::PcapBe => "pcap (BE)",
                CaptureFormat::PcapNg => "pcapng",
                _ => unreachable!(),
            };
            println!("  Format: {fmt_name}, port filter: {port}");
            parse_pcap_capture(&data, &format, port, server_ip)
        }
        CaptureFormat::Unknown => {
            println!("  Format: raw mitmproxy dump (no recognized magic)");
            parse_raw_dump(&data)
        }
    }
}

// ══════════════════════════════════════════════════════════════
// Main
// ══════════════════════════════════════════════════════════════

fn main() {
    let cli = parse_args();

    println!("╔══════════════════════════════════════════════════════════╗");
    println!("║  Phase R4: Network Crypto Analysis & Cracking           ║");
    println!("╚══════════════════════════════════════════════════════════╝");
    println!();
    println!("  Captures: {}", cli.file_paths.len());
    println!("  Output:   {}", cli.output_dir);

    // Ensure output directory exists
    let _ = fs::create_dir_all(&cli.output_dir);

    // Load all captures
    let mut all_captures: Vec<(Capture, Vec<EncryptedPayload>, Vec<HandshakeData>)> = Vec::new();
    let mut primary_bulk: Vec<BulkStream> = Vec::new();

    for path in &cli.file_paths {
        let capture = load_capture(path, cli.port, cli.server_ip);

        if capture.frames.is_empty() {
            println!("\n  [!] No frames in {path}, skipping.");
            continue;
        }

        let (encrypted, handshakes, bulk) = extract_payloads(&capture);
        println!(
            "  Frames: {}, encrypted: {}, handshakes: {}, bulk: {}",
            capture.frames.len(),
            encrypted.len(),
            handshakes.len(),
            bulk.len()
        );

        if all_captures.is_empty() {
            primary_bulk = bulk;
        }

        all_captures.push((capture, encrypted, handshakes));
    }

    if all_captures.is_empty() {
        println!("\n  [!] No valid captures loaded.");
        std::process::exit(1);
    }

    // Run passes on primary capture
    let (ref capture, ref encrypted, ref handshakes) = all_captures[0];

    pass1_loader(capture, encrypted, handshakes, &primary_bulk);
    pass2_header_hypothesis(encrypted);
    pass3_crib_testing(encrypted, handshakes);
    pass4_auth_tag(encrypted);
    pass5_frequency_analysis(encrypted, &primary_bulk);
    pass6_bulk_structure(&primary_bulk);
    pass7_cross_session(&all_captures);
    pass8_decrypt_and_export(encrypted, handshakes, &primary_bulk, &cli.output_dir);

    println!("\n{}", "=".repeat(70));
    println!("  Analysis complete.");
    println!("  Captures processed: {}", all_captures.len());
    println!(
        "  Total encrypted frames: {}",
        all_captures.iter().map(|c| c.1.len()).sum::<usize>()
    );
    println!("  Exported payloads: {}/payloads/", cli.output_dir);
    println!("{}", "=".repeat(70));
}
