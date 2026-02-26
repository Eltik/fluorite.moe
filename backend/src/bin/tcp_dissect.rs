/// Phase R3: TCP Protocol Dissector — offline analysis of captured game traffic.
///
/// Pass 1: Frame parser (handshake/encrypted/control/unknown)
/// Pass 2: Statistical analysis (size distribution, entropy, repeats)
/// Pass 3: Crypto analysis (keystream reuse, cipher mode detection)
/// Pass 4: Session key hunting (derive keys from handshake values)
/// Pass 5: Protocol structure inference (message type mapping)
///
/// Input: TCAP binary (from tcp_capture.py), raw mitmproxy dump, pcap, or pcapng
///
/// Usage:
///   cargo run --bin tcp-dissect --release -- <capture_file> [--raw] [--port N] [--server IP]
use std::collections::HashMap;
use std::fs;
use std::net::Ipv4Addr;

// ══════════════════════════════════════════════════════════════
// Constants
// ══════════════════════════════════════════════════════════════

/// TCAP file magic bytes
const TCAP_MAGIC: &[u8; 4] = b"TCAP";

/// Known encrypted frame header
const ENCRYPTED_HEADER: [u8; 4] = [0x05, 0x0F, 0x09, 0x0C];

/// Control/ACK frame prefix
const CONTROL_PREFIX: u8 = 0x0B;

/// Minimum frame size for meaningful analysis
const MIN_FRAME_SIZE: usize = 3;

/// PCAP magic (little-endian native byte order)
const PCAP_MAGIC_LE: u32 = 0xA1B2_C3D4;

/// PCAP magic (big-endian byte order)
const PCAP_MAGIC_BE: u32 = 0xD4C3_B2A1;

/// PCAPNG Section Header Block magic
const PCAPNG_SHB_MAGIC: [u8; 4] = [0x0A, 0x0D, 0x0D, 0x0A];

/// PCAPNG Byte-Order Magic
const PCAPNG_BOM: u32 = 0x1A2B_3C4D;

/// Default game server port
const DEFAULT_GAME_PORT: u16 = 30000;

/// Known game server IPs
const KNOWN_SERVER_IPS: &[&str] = &["172.65.253.17"];

// ══════════════════════════════════════════════════════════════
// Data Structures
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
    /// Protobuf handshake message (small, ≤256 bytes)
    Handshake,
    /// Encrypted data frame: header + 3 x LE uint32 + ciphertext
    Encrypted { header_u32s: [u32; 3] },
    /// Control/ACK: 0x0B + type + 0x00
    Control { control_type: u8 },
    /// Server selection frame: 01 00 00 + BE uint16 len + cmd + session
    ServerSelect { command: u8 },
    /// Bulk encrypted stream (continuous ciphertext, no per-message framing)
    BulkEncrypted,
    /// Unknown frame type
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
    timestamp_us: u64,
    frame_type: FrameType,
    raw: Vec<u8>,
    /// Payload after any header (encrypted data, protobuf body, etc.)
    payload: Vec<u8>,
}

/// Result of a capture file parse
struct Capture {
    version: u32,
    frames: Vec<Frame>,
    format_name: &'static str,
}

// ══════════════════════════════════════════════════════════════
// Helpers
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

/// Decode a protobuf varint from a byte slice. Returns (value, bytes_consumed).
fn decode_varint(data: &[u8]) -> Option<(u64, usize)> {
    let mut value: u64 = 0;
    let mut shift = 0u32;
    for (i, &byte) in data.iter().enumerate() {
        if shift >= 70 {
            return None; // too many bytes
        }
        value |= ((byte & 0x7F) as u64) << shift;
        shift += 7;
        if byte & 0x80 == 0 {
            return Some((value, i + 1));
        }
    }
    None
}

/// Try to decode protobuf fields from raw bytes. Returns list of (field_number, wire_type, value/length).
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

        // Sanity: field number should be reasonable
        if field_num == 0 || field_num > 1000 {
            break;
        }

        match wire_type {
            0 => {
                // Varint
                let Some((val, val_len)) = decode_varint(&data[pos..]) else {
                    break;
                };
                fields.push((field_num, wire_type, val));
                pos += val_len;
            }
            1 => {
                // 64-bit fixed
                if pos + 8 > data.len() {
                    break;
                }
                let val = u64::from_le_bytes(data[pos..pos + 8].try_into().unwrap());
                fields.push((field_num, wire_type, val));
                pos += 8;
            }
            2 => {
                // Length-delimited
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
                // 32-bit fixed
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

/// Check if data looks like a protobuf message (has multiple valid fields).
fn looks_like_protobuf(data: &[u8]) -> bool {
    let fields = decode_protobuf_fields(data);
    fields.len() >= 2
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
        // Per frame: [timestamp_us: u64][direction: u8][length: u32][data: bytes]
        let timestamp_us = u64::from_le_bytes(data[pos..pos + 8].try_into().unwrap());
        let dir_byte = data[pos + 8];
        let length = u32::from_le_bytes(data[pos + 9..pos + 13].try_into().unwrap()) as usize;
        pos += 13;

        if pos + length > data.len() {
            eprintln!(
                "  [!] Truncated frame at index {index}: need {length} bytes, have {}",
                data.len() - pos
            );
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

/// Parse raw mitmproxy dump format: [direction:1][timestamp:8 (f64)][length:4][data:N]
fn parse_raw_dump(data: &[u8]) -> Capture {
    let mut frames = Vec::new();
    let mut pos = 0;
    let mut index = 0;

    while pos + 13 <= data.len() {
        let dir_byte = data[pos];
        if dir_byte != 0x01 && dir_byte != 0x02 {
            // Try to resync — skip one byte
            pos += 1;
            continue;
        }

        let ts_f64 = f64::from_le_bytes(data[pos + 1..pos + 9].try_into().unwrap());
        let length = u32::from_le_bytes(data[pos + 9..pos + 13].try_into().unwrap()) as usize;

        // Sanity check: length should be reasonable
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

        // Convert f64 timestamp (seconds) to u64 microseconds
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
// Parse: pcap / pcapng files (manual, zero dependencies)
// ══════════════════════════════════════════════════════════════

/// A raw packet extracted from a pcap/pcapng file.
struct RawPacket {
    ts_us: u64,
    data: Vec<u8>,
}

/// TCP segment extracted from a network packet.
struct TcpSegment {
    ts_us: u64,
    src_ip: Ipv4Addr,
    dst_ip: Ipv4Addr,
    src_port: u16,
    dst_port: u16,
    seq: u32,
    payload: Vec<u8>,
}

/// Detect capture format from magic bytes.
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

/// Parse a pcap (libpcap) file. Handles both LE and BE byte orders.
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

    let _version_major = if big_endian {
        u16::from_be_bytes(data[4..6].try_into().unwrap())
    } else {
        u16::from_le_bytes(data[4..6].try_into().unwrap())
    };
    let network = read_u32(&data[20..24]);

    // Validate link type (1 = Ethernet, 101 = Raw IP, 113 = Linux SLL)
    if network != 1 && network != 101 && network != 113 {
        eprintln!(
            "  [!] Unsupported pcap link type: {network} (need Ethernet=1, RawIP=101, or SLL=113)"
        );
        return packets;
    }

    let mut pos = 24; // skip global header
    while pos + 16 <= data.len() {
        let ts_sec = read_u32(&data[pos..pos + 4]);
        let ts_usec = read_u32(&data[pos + 4..pos + 8]);
        let incl_len = read_u32(&data[pos + 8..pos + 12]) as usize;
        // let _orig_len = read_u32(&data[pos + 12..pos + 16]);
        pos += 16;

        if pos + incl_len > data.len() {
            break;
        }

        let ts_us = ts_sec as u64 * 1_000_000 + ts_usec as u64;
        let pkt_data = match network {
            1 => data[pos..pos + incl_len].to_vec(), // Ethernet frame
            101 => {
                // Raw IP — prepend a fake Ethernet header so downstream parsing works
                let mut eth = vec![0u8; 14];
                eth[12] = 0x08; // EtherType = IPv4 (0x0800)
                eth[13] = 0x00;
                eth.extend_from_slice(&data[pos..pos + incl_len]);
                eth
            }
            113 => {
                // Linux SLL — 16-byte header, protocol at [14..16]
                if incl_len >= 16 {
                    let proto = u16::from_be_bytes(data[pos + 14..pos + 16].try_into().unwrap());
                    if proto == 0x0800 {
                        // Wrap raw IP in fake Ethernet header
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

/// Parse a pcapng file. Handles byte order from SHB.
fn parse_pcapng(data: &[u8]) -> Vec<RawPacket> {
    let mut packets = Vec::new();
    if data.len() < 28 {
        return packets;
    }

    let mut pos = 0;
    let mut is_le = true;
    let mut link_type: u16 = 1; // default Ethernet
    let mut ts_resolution: u64 = 1_000_000; // default: microseconds

    while pos + 12 <= data.len() {
        // Block header: type(4) + total_length(4)
        // For SHB, we read type as raw bytes first
        let block_type_bytes: [u8; 4] = data[pos..pos + 4].try_into().unwrap();

        // Special handling for SHB: need to determine byte order first
        if block_type_bytes == PCAPNG_SHB_MAGIC {
            // SHB: type(4) + length(4) + BOM(4) + version(4) + section_length(8) + options...
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

        // Normal block reading with known byte order
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
        let body_end = pos + block_len - 4; // exclude trailing length
        let body_len = body_end.saturating_sub(body_start);

        match block_type {
            1 => {
                // Interface Description Block
                if body_len >= 4 {
                    link_type = read_u16(&data[body_start..body_start + 2]);
                    // Parse options for if_tsresol (code 9)
                    let mut opos = body_start + 8; // skip linktype(2)+reserved(2)+snaplen(4)
                    while opos + 4 <= body_end {
                        let opt_code = read_u16(&data[opos..opos + 2]);
                        let opt_len = read_u16(&data[opos + 2..opos + 4]) as usize;
                        if opt_code == 0 {
                            break; // end of options
                        }
                        if opt_code == 9 && opt_len == 1 && opos + 4 < body_end {
                            let tsresol = data[opos + 4];
                            if tsresol & 0x80 == 0 {
                                // Power of 10
                                ts_resolution = 10u64.pow(tsresol as u32);
                            } else {
                                // Power of 2
                                ts_resolution = 1u64 << (tsresol & 0x7F);
                            }
                        }
                        // Options are padded to 4-byte boundary
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
                    // Convert to microseconds
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
                                // Raw IP → fake Ethernet header
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
            _ => {} // Skip other block types
        }

        pos += block_len;
    }

    packets
}

/// Extract TCP segment info from an Ethernet frame (manual header parsing).
fn extract_tcp(ethernet_frame: &[u8]) -> Option<TcpSegment> {
    // Ethernet header: 14 bytes
    if ethernet_frame.len() < 14 {
        return None;
    }

    let mut ethertype = u16::from_be_bytes(ethernet_frame[12..14].try_into().unwrap());
    let mut ip_start = 14;

    // Handle 802.1Q VLAN tags
    if ethertype == 0x8100 {
        if ethernet_frame.len() < 18 {
            return None;
        }
        ethertype = u16::from_be_bytes(ethernet_frame[16..18].try_into().unwrap());
        ip_start = 18;
    }

    // Must be IPv4
    if ethertype != 0x0800 {
        return None;
    }

    // IPv4 header: minimum 20 bytes
    if ethernet_frame.len() < ip_start + 20 {
        return None;
    }

    let version_ihl = ethernet_frame[ip_start];
    let version = version_ihl >> 4;
    if version != 4 {
        return None;
    }
    let ihl = (version_ihl & 0x0F) as usize * 4;
    if ihl < 20 {
        return None;
    }

    let protocol = ethernet_frame[ip_start + 9];
    if protocol != 6 {
        return None; // Not TCP
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

    // TCP header: minimum 20 bytes
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
        ts_us: 0, // filled by caller
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

/// Metadata for tracking which TCP segment a byte offset belongs to.
struct StreamMeta {
    stream_offset: usize,
    timestamp_us: u64,
}

/// Find the timestamp for a given byte offset in the assembled stream.
fn timestamp_at_offset(meta: &[StreamMeta], offset: usize) -> u64 {
    for m in meta.iter().rev() {
        if offset >= m.stream_offset {
            return m.timestamp_us;
        }
    }
    meta.first().map_or(0, |m| m.timestamp_us)
}

/// Reassemble TCP segments into per-direction byte streams.
/// Returns (cs_stream, cs_meta, sc_stream, sc_meta).
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

/// Scan an assembled byte stream for game protocol frames.
///
/// Strategy: scan for `05 0F 09 0C` encrypted frame markers, use h0 for frame length.
/// Everything between encrypted frames is treated as non-encrypted protocol data
/// (handshake, control, keepalive, etc.).
fn extract_frames_from_stream(
    stream: &[u8],
    meta: &[StreamMeta],
    direction: Direction,
    start_index: &mut usize,
) -> Vec<Frame> {
    let mut frames: Vec<Frame> = Vec::new();

    // Phase 1: find all encrypted frame positions
    let mut encrypted_ranges: Vec<(usize, usize)> = Vec::new(); // (start, end)
    {
        let mut pos = 0;
        while pos + 16 <= stream.len() {
            if stream[pos..pos + 4] == ENCRYPTED_HEADER {
                let h0 = u32::from_le_bytes(stream[pos + 4..pos + 8].try_into().unwrap()) as usize;
                // Validate: h0 must include the 16-byte header and not exceed stream
                if h0 >= 16 && pos + h0 <= stream.len() {
                    encrypted_ranges.push((pos, pos + h0));
                    pos += h0;
                    continue;
                }
            }
            pos += 1;
        }
    }

    // Phase 2: build frame list — encrypted frames + non-encrypted gaps
    let mut cursor = 0;
    for &(enc_start, enc_end) in &encrypted_ranges {
        // Emit non-encrypted gap before this encrypted frame
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

        // Emit encrypted frame
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

    // Emit trailing non-encrypted data
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

/// Emit a non-encrypted region of the stream as a frame.
///
/// We do NOT scan for `0B XX 00` within large blocks — those are almost certainly
/// coincidental byte patterns within encrypted bulk data (probability ~1/65536 per
/// byte position means ~7.6 false matches per 500KB of random data).
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

/// Auto-detect the server IP from TCP segments: the most common non-private IP on the target port.
fn detect_server_ip(segments: &[TcpSegment], port: u16) -> Option<Ipv4Addr> {
    let mut ip_counts: HashMap<Ipv4Addr, usize> = HashMap::new();

    for seg in segments {
        // Count IPs that appear on the filtered port
        if seg.src_port == port {
            *ip_counts.entry(seg.src_ip).or_default() += 1;
        }
        if seg.dst_port == port {
            *ip_counts.entry(seg.dst_ip).or_default() += 1;
        }
    }

    // Prefer non-private IPs
    let mut candidates: Vec<_> = ip_counts.into_iter().collect();
    candidates.sort_by(|a, b| b.1.cmp(&a.1));

    for (ip, _count) in &candidates {
        if !ip.is_private() && !ip.is_loopback() {
            return Some(*ip);
        }
    }

    // Fall back to most common IP overall
    candidates.first().map(|(ip, _)| *ip)
}

/// Parse a pcap or pcapng file into a Capture.
fn parse_pcap_capture(
    data: &[u8],
    format: &CaptureFormat,
    filter_port: u16,
    server_ip_override: Option<Ipv4Addr>,
) -> Capture {
    // Step 1: Parse raw packets from file
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

    // Step 2: Extract TCP segments from Ethernet frames
    let mut segments: Vec<TcpSegment> = Vec::new();
    for pkt in &raw_packets {
        if let Some(mut seg) = extract_tcp(&pkt.data) {
            seg.ts_us = pkt.ts_us;
            segments.push(seg);
        }
    }

    println!("  Extracted {} TCP segments", segments.len());

    // Step 3: Filter by port
    let segments: Vec<TcpSegment> = segments
        .into_iter()
        .filter(|s| s.src_port == filter_port || s.dst_port == filter_port)
        .collect();

    println!(
        "  After port {filter_port} filter: {} segments",
        segments.len()
    );

    if segments.is_empty() {
        // Show what ports we found for debugging
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

    // Step 4: Filter empty payloads (pure ACKs) and retransmissions
    let mut seen_seqs: HashMap<(Ipv4Addr, u16, Ipv4Addr, u16, u32), bool> = HashMap::new();
    let mut filtered: Vec<TcpSegment> = Vec::new();

    for seg in segments {
        if seg.payload.is_empty() {
            continue; // Skip pure ACKs
        }
        let key = (seg.src_ip, seg.src_port, seg.dst_ip, seg.dst_port, seg.seq);
        if seen_seqs.contains_key(&key) {
            continue; // Skip retransmission
        }
        seen_seqs.insert(key, true);
        filtered.push(seg);
    }

    let retransmit_count = seen_seqs.len().saturating_sub(filtered.len());
    if retransmit_count > 0 {
        println!(
            "  Filtered {retransmit_count} retransmissions, {} segments with payload",
            filtered.len()
        );
    } else {
        println!("  {} segments with payload", filtered.len());
    }

    // Step 5: Detect server IP
    let server_ip = server_ip_override.or_else(|| {
        // Try known IPs first
        for known in KNOWN_SERVER_IPS {
            let ip: Ipv4Addr = known.parse().unwrap();
            if filtered.iter().any(|s| s.src_ip == ip || s.dst_ip == ip) {
                println!("  Server IP: {ip} (known)");
                return Some(ip);
            }
        }
        // Auto-detect
        let detected = detect_server_ip(&filtered, filter_port);
        if let Some(ip) = detected {
            println!("  Server IP: {ip} (auto-detected)");
        }
        detected
    });

    if server_ip.is_none() {
        println!("  [!] Could not determine server IP — all frames will be UNKNOWN direction");
    }
    if server_ip_override.is_some() {
        println!("  Server IP: {} (from --server flag)", server_ip.unwrap());
    }

    // Step 6: TCP stream reassembly + frame extraction
    let frames = if let Some(srv) = server_ip {
        let (cs_stream, cs_meta, sc_stream, sc_meta) = reassemble_streams(&filtered, srv);

        println!(
            "  Stream reassembly: C->S {} bytes, S->C {} bytes",
            cs_stream.len(),
            sc_stream.len()
        );

        // Scan for encrypted frame markers in each stream
        let cs_enc_count = cs_stream
            .windows(4)
            .filter(|w| *w == ENCRYPTED_HEADER)
            .count();
        let sc_enc_count = sc_stream
            .windows(4)
            .filter(|w| *w == ENCRYPTED_HEADER)
            .count();
        println!("  Encrypted frame markers: C->S {cs_enc_count}, S->C {sc_enc_count}");

        let mut idx = 0;
        let mut frames =
            extract_frames_from_stream(&cs_stream, &cs_meta, Direction::ClientToServer, &mut idx);
        frames.extend(extract_frames_from_stream(
            &sc_stream,
            &sc_meta,
            Direction::ServerToClient,
            &mut idx,
        ));

        // Sort by timestamp for chronological output
        frames.sort_by_key(|f| f.timestamp_us);

        // Re-index after sorting
        for (i, frame) in frames.iter_mut().enumerate() {
            frame.index = i;
        }

        frames
    } else {
        // Fallback: per-segment classification (no server IP for direction)
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

    // Control/ACK: 0x0B XX 0x00 (exactly 3 bytes)
    if raw.len() == 3 && raw[0] == CONTROL_PREFIX && raw[2] == 0x00 {
        return (
            FrameType::Control {
                control_type: raw[1],
            },
            vec![],
        );
    }

    // Encrypted data frame: starts with 05 0F 09 0C + 3 LE uint32 + payload
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

    // Server selection: 01 00 00 + BE uint16 total_length + command + session
    if raw.len() >= 6 && raw[0] == 0x01 && raw[1] == 0x00 && raw[2] == 0x00 {
        let command = if raw.len() > 5 { raw[5] } else { 0 };
        return (FrameType::ServerSelect { command }, raw[3..].to_vec());
    }

    // Protobuf handshake: only for small messages (≤256 bytes)
    // Large blobs are bulk encrypted streams that coincidentally match protobuf heuristics
    if raw.len() <= 256 && looks_like_protobuf(raw) {
        return (FrameType::Handshake, raw.to_vec());
    }

    // Bulk encrypted stream: large blobs with high entropy and no framing markers
    if raw.len() > 256 {
        let e = entropy(raw);
        if e > 7.0 {
            return (FrameType::BulkEncrypted, raw.to_vec());
        }
    }

    (FrameType::Unknown, raw.to_vec())
}

// ══════════════════════════════════════════════════════════════
// Pass 1: Frame Parser
// ══════════════════════════════════════════════════════════════

fn pass1_frame_parser(capture: &Capture) {
    println!("\n{}", "=".repeat(70));
    println!("  PASS 1: Frame Parser");
    println!("{}", "=".repeat(70));
    println!(
        "  Format: {}  |  Version: {}  |  Total frames: {}",
        capture.format_name,
        capture.version,
        capture.frames.len()
    );
    println!();

    // Type census
    let mut type_counts: HashMap<String, usize> = HashMap::new();
    let mut cs_count = 0usize;
    let mut sc_count = 0usize;
    let mut cs_bytes = 0u64;
    let mut sc_bytes = 0u64;

    for frame in &capture.frames {
        let type_key = match &frame.frame_type {
            FrameType::Handshake => "HANDSHAKE".to_string(),
            FrameType::Encrypted { .. } => "ENCRYPTED".to_string(),
            FrameType::Control { .. } => "CONTROL".to_string(),
            FrameType::ServerSelect { .. } => "SERVER_SELECT".to_string(),
            FrameType::BulkEncrypted => "BULK_ENCRYPTED".to_string(),
            FrameType::Unknown => "UNKNOWN".to_string(),
        };
        *type_counts.entry(type_key).or_default() += 1;

        match frame.direction {
            Direction::ClientToServer => {
                cs_count += 1;
                cs_bytes += frame.raw.len() as u64;
            }
            Direction::ServerToClient => {
                sc_count += 1;
                sc_bytes += frame.raw.len() as u64;
            }
        }
    }

    println!("  Direction breakdown:");
    println!("    C->S: {} frames, {}", cs_count, format_bytes(cs_bytes));
    println!("    S->C: {} frames, {}", sc_count, format_bytes(sc_bytes));
    println!();

    println!("  Frame type census:");
    let mut sorted_types: Vec<_> = type_counts.iter().collect();
    sorted_types.sort_by(|a, b| b.1.cmp(a.1));
    for (type_name, count) in &sorted_types {
        println!("    {type_name:20} : {count}");
    }
    println!();

    // Print first 30 frames in detail
    let show_count = capture.frames.len().min(30);
    println!("  First {show_count} frames:");
    println!(
        "  {:<6} {:<6} {:<42} {:<8} HEX (first 32 bytes)",
        "INDEX", "DIR", "TYPE", "SIZE"
    );
    println!("  {}", "-".repeat(110));

    for frame in capture.frames.iter().take(show_count) {
        let hex: String = frame
            .raw
            .iter()
            .take(32)
            .map(|b| format!("{b:02X}"))
            .collect::<Vec<_>>()
            .join(" ");
        let suffix = if frame.raw.len() > 32 { " ..." } else { "" };
        println!(
            "  {:<6} {:<6} {:<42} {:<8} {hex}{suffix}",
            frame.index,
            frame.direction,
            frame.frame_type,
            frame.raw.len(),
        );
    }

    // Handshake details
    let handshakes: Vec<_> = capture
        .frames
        .iter()
        .filter(|f| f.frame_type == FrameType::Handshake)
        .collect();

    if !handshakes.is_empty() {
        println!("\n  Handshake details:");
        for frame in &handshakes {
            println!(
                "    Frame {} ({}), {} bytes:",
                frame.index,
                frame.direction,
                frame.raw.len()
            );
            let fields = decode_protobuf_fields(&frame.raw);
            for (field_num, wire_type, value) in &fields {
                let wt_name = match wire_type {
                    0 => "varint",
                    1 => "fixed64",
                    2 => "bytes",
                    5 => "fixed32",
                    _ => "unknown",
                };
                println!("      field {field_num} ({wt_name}): {value} (0x{value:X})");
            }
            println!("    Raw: {}", hex_preview(&frame.raw, 64));
        }
    }

    // Server-select details
    let server_selects: Vec<_> = capture
        .frames
        .iter()
        .filter(|f| matches!(f.frame_type, FrameType::ServerSelect { .. }))
        .collect();

    if !server_selects.is_empty() {
        println!("\n  Server-select frame details:");
        for frame in &server_selects {
            let raw = &frame.raw;
            if raw.len() >= 5 {
                let total_len = u16::from_be_bytes([raw[3], raw[4]]);
                let cmd = if raw.len() > 5 { raw[5] } else { 0 };
                print!(
                    "    Frame {} ({}): len={total_len}, cmd=0x{cmd:02X}",
                    frame.index, frame.direction
                );

                // Extract session ID if present (bytes 6-9 for 4-byte session)
                if raw.len() >= 10 {
                    let session_id = u32::from_be_bytes(raw[6..10].try_into().unwrap());
                    print!(", session=0x{session_id:08X}");
                }

                // Check for token (bytes 17-32 based on mitmproxy observations)
                if raw.len() >= 32 {
                    let token_hex: String = raw[17..33.min(raw.len())]
                        .iter()
                        .map(|b| format!("{b:02X}"))
                        .collect::<Vec<_>>()
                        .join("");
                    print!(", token={token_hex}");
                }

                println!();
            }
            println!("      Raw: {}", hex_preview(raw, 48));
        }
    }
}

// ══════════════════════════════════════════════════════════════
// Pass 2: Statistical Analysis
// ══════════════════════════════════════════════════════════════

fn pass2_statistics(capture: &Capture) {
    println!("\n{}", "=".repeat(70));
    println!("  PASS 2: Statistical Analysis");
    println!("{}", "=".repeat(70));

    let encrypted: Vec<_> = capture
        .frames
        .iter()
        .filter(|f| matches!(f.frame_type, FrameType::Encrypted { .. }))
        .collect();

    if encrypted.is_empty() {
        println!("  No encrypted frames found.");
        return;
    }

    println!("  Encrypted frames: {}", encrypted.len());

    // Size distribution per direction
    let mut cs_sizes: Vec<usize> = Vec::new();
    let mut sc_sizes: Vec<usize> = Vec::new();

    for frame in &encrypted {
        let size = frame.payload.len();
        match frame.direction {
            Direction::ClientToServer => cs_sizes.push(size),
            Direction::ServerToClient => sc_sizes.push(size),
        }
    }

    fn print_size_stats(label: &str, sizes: &mut [usize]) {
        if sizes.is_empty() {
            println!("  {label}: no frames");
            return;
        }
        sizes.sort();
        let total: usize = sizes.iter().sum();
        let min = sizes[0];
        let max = sizes[sizes.len() - 1];
        let median = sizes[sizes.len() / 2];
        let avg = total / sizes.len();
        println!("  {label} payload sizes ({} frames):", sizes.len());
        println!(
            "    min={min}  max={max}  median={median}  avg={avg}  total={}",
            format_bytes(total as u64)
        );

        // Histogram: bucket by powers of 2
        let mut buckets: HashMap<u32, usize> = HashMap::new();
        for &s in sizes.iter() {
            let bucket = if s == 0 {
                0
            } else {
                (s as f64).log2().floor() as u32
            };
            *buckets.entry(bucket).or_default() += 1;
        }
        let mut sorted_buckets: Vec<_> = buckets.into_iter().collect();
        sorted_buckets.sort();
        println!("    Size histogram:");
        for (bucket, count) in &sorted_buckets {
            let lo = if *bucket == 0 { 0 } else { 1usize << bucket };
            let hi = (1usize << (bucket + 1)) - 1;
            let bar = "#".repeat((*count).min(50));
            println!("      {lo:>8}-{hi:<8}: {count:>4} {bar}");
        }
    }

    print_size_stats("C->S", &mut cs_sizes);
    println!();
    print_size_stats("S->C", &mut sc_sizes);

    // Entropy analysis
    println!("\n  Entropy analysis (encrypted payloads):");
    let mut entropies: Vec<(usize, f64, usize)> = Vec::new(); // (index, entropy, size)
    for frame in &encrypted {
        if !frame.payload.is_empty() {
            let ent = entropy(&frame.payload);
            entropies.push((frame.index, ent, frame.payload.len()));
        }
    }

    if !entropies.is_empty() {
        let avg_ent: f64 = entropies.iter().map(|e| e.1).sum::<f64>() / entropies.len() as f64;
        let min_ent = entropies
            .iter()
            .min_by(|a, b| a.1.partial_cmp(&b.1).unwrap())
            .unwrap();
        let max_ent = entropies
            .iter()
            .max_by(|a, b| a.1.partial_cmp(&b.1).unwrap())
            .unwrap();

        println!("    avg entropy: {avg_ent:.4}");
        println!(
            "    min entropy: {:.4} (frame {}, {} bytes)",
            min_ent.1, min_ent.0, min_ent.2
        );
        println!(
            "    max entropy: {:.4} (frame {}, {} bytes)",
            max_ent.1, max_ent.0, max_ent.2
        );
        println!(
            "    {} — (max theoretical for random = 8.0)",
            if avg_ent > 7.9 {
                "High entropy: encrypted or compressed"
            } else if avg_ent > 6.0 {
                "Moderate entropy: possible weak cipher or partial plaintext"
            } else {
                "Low entropy: likely plaintext or weak obfuscation"
            }
        );

        // Low-entropy outliers (potential plaintext leaks)
        let outliers: Vec<_> = entropies.iter().filter(|e| e.1 < 6.0).collect();
        if !outliers.is_empty() {
            println!("\n    Low-entropy outliers (potential plaintext leaks):");
            for (idx, ent, size) in &outliers {
                let frame =
                    &capture.frames[encrypted.iter().position(|f| f.index == *idx).unwrap_or(0)];
                println!(
                    "      frame {idx}: entropy={ent:.4}, size={size}, dir={}",
                    frame.direction
                );
                println!("        {}", hex_preview(&frame.payload, 48));
            }
        }
    }

    // Repeated payload detection (ECB / deterministic cipher indicator)
    println!("\n  Repeated payload detection:");
    let mut payload_hashes: HashMap<Vec<u8>, Vec<usize>> = HashMap::new();
    for frame in &encrypted {
        if !frame.payload.is_empty() {
            payload_hashes
                .entry(frame.payload.clone())
                .or_default()
                .push(frame.index);
        }
    }
    let duplicates: Vec<_> = payload_hashes
        .iter()
        .filter(|(_, indices)| indices.len() > 1)
        .collect();

    if duplicates.is_empty() {
        println!(
            "    No duplicate encrypted payloads — good (non-deterministic cipher or unique plaintexts)"
        );
    } else {
        println!(
            "    {} unique payloads appear multiple times (ECB / deterministic cipher?):",
            duplicates.len()
        );
        for (payload, indices) in duplicates.iter().take(10) {
            println!(
                "      {} bytes, appears {} times at frames: {:?}",
                payload.len(),
                indices.len(),
                indices
            );
        }
    }

    // Inter-frame timing
    if capture.frames.len() >= 2 {
        println!("\n  Inter-frame timing:");
        let mut deltas: Vec<f64> = Vec::new();
        for pair in capture.frames.windows(2) {
            if pair[1].timestamp_us >= pair[0].timestamp_us {
                let delta_ms = (pair[1].timestamp_us - pair[0].timestamp_us) as f64 / 1000.0;
                deltas.push(delta_ms);
            }
        }
        if !deltas.is_empty() {
            deltas.sort_by(|a, b| a.partial_cmp(b).unwrap());
            let avg = deltas.iter().sum::<f64>() / deltas.len() as f64;
            println!(
                "    min={:.2}ms  max={:.2}ms  avg={:.2}ms  median={:.2}ms",
                deltas[0],
                deltas[deltas.len() - 1],
                avg,
                deltas[deltas.len() / 2]
            );
        }
    }

    // Header uint32 analysis for encrypted frames
    println!("\n  Encrypted frame header analysis (3 x LE uint32):");
    let mut h0_vals: Vec<u32> = Vec::new();
    let mut h1_vals: Vec<u32> = Vec::new();
    let mut h2_vals: Vec<u32> = Vec::new();

    for frame in &encrypted {
        if let FrameType::Encrypted { header_u32s } = &frame.frame_type {
            h0_vals.push(header_u32s[0]);
            h1_vals.push(header_u32s[1]);
            h2_vals.push(header_u32s[2]);
        }
    }

    fn analyze_header_field(name: &str, vals: &[u32]) {
        if vals.is_empty() {
            return;
        }
        let unique: std::collections::HashSet<_> = vals.iter().collect();
        let min = vals.iter().min().unwrap();
        let max = vals.iter().max().unwrap();
        println!(
            "    {name}: {unique} unique values, range [{min}, {max}]",
            unique = unique.len()
        );

        // Check if monotonically increasing (sequence number?)
        let is_monotonic = vals.windows(2).all(|w| w[1] >= w[0]);
        let is_strictly_inc = vals.windows(2).all(|w| w[1] > w[0]);
        if is_strictly_inc {
            println!("      -> Strictly increasing (likely sequence number)");
        } else if is_monotonic {
            println!("      -> Monotonically increasing (possible counter)");
        }

        // Show first few values
        let preview: Vec<_> = vals.iter().take(10).map(|v| format!("{v}")).collect();
        println!("      First values: [{}]", preview.join(", "));
    }

    analyze_header_field("h0 (field 1)", &h0_vals);
    analyze_header_field("h1 (field 2)", &h1_vals);
    analyze_header_field("h2 (field 3)", &h2_vals);

    // Correlate h0 with payload size
    if !encrypted.is_empty() {
        println!("\n    Header vs payload size correlation:");
        for frame in encrypted.iter().take(10) {
            if let FrameType::Encrypted { header_u32s } = &frame.frame_type {
                let plen = frame.payload.len();
                let rlen = frame.raw.len();
                println!(
                    "      frame {:>4}: h=[{:>6}, {:>6}, {:>6}]  payload={:>6}  raw={:>6}  diff(h0-payload)={:>6}",
                    frame.index,
                    header_u32s[0],
                    header_u32s[1],
                    header_u32s[2],
                    plen,
                    rlen,
                    header_u32s[0] as i64 - plen as i64,
                );
            }
        }
    }

    // Bulk encrypted stream analysis
    let bulk: Vec<_> = capture
        .frames
        .iter()
        .filter(|f| f.frame_type == FrameType::BulkEncrypted)
        .collect();
    if !bulk.is_empty() {
        println!("\n  Bulk encrypted stream analysis:");
        for frame in &bulk {
            let ent = entropy(&frame.raw);
            println!(
                "    Frame {} ({}): {} bytes, entropy={:.4}",
                frame.index,
                frame.direction,
                frame.raw.len(),
                ent
            );

            // Look for potential internal framing within the bulk stream
            // Scan for repeated byte patterns that might indicate message boundaries
            let data = &frame.raw;
            let mut marker_hits: HashMap<[u8; 4], usize> = HashMap::new();
            for window in data.windows(4) {
                let key: [u8; 4] = window.try_into().unwrap();
                *marker_hits.entry(key).or_default() += 1;
            }
            // Any 4-byte pattern appearing significantly more than expected (~len/2^32)?
            let expected_random = (data.len() as f64) / (1u64 << 32) as f64;
            let mut anomalous: Vec<_> = marker_hits
                .iter()
                .filter(|&(_, count)| *count as f64 > expected_random * 100.0 && *count >= 3)
                .collect();
            anomalous.sort_by(|a, b| b.1.cmp(a.1));

            if !anomalous.is_empty() {
                println!("      Repeated 4-byte patterns (potential framing):");
                for (pattern, count) in anomalous.iter().take(10) {
                    println!(
                        "        {:02X} {:02X} {:02X} {:02X} — {} times",
                        pattern[0], pattern[1], pattern[2], pattern[3], count
                    );
                }
            }

            // Check if the first few bytes suggest a known protocol layer
            if data.len() >= 16 {
                println!(
                    "      First 16 bytes: {}",
                    data[..16]
                        .iter()
                        .map(|b| format!("{b:02X}"))
                        .collect::<Vec<_>>()
                        .join(" ")
                );
                println!(
                    "      Last 16 bytes:  {}",
                    data[data.len() - 16..]
                        .iter()
                        .map(|b| format!("{b:02X}"))
                        .collect::<Vec<_>>()
                        .join(" ")
                );
            }

            // Byte frequency analysis — check for uniformity
            let mut freq = [0u32; 256];
            for &b in data {
                freq[b as usize] += 1;
            }
            let unique_bytes = freq.iter().filter(|&&c| c > 0).count();
            let min_freq = *freq.iter().filter(|&&c| c > 0).min().unwrap_or(&0);
            let max_freq = *freq.iter().max().unwrap_or(&0);
            println!(
                "      Byte distribution: {}/256 unique, freq range [{}, {}], ratio {:.2}",
                unique_bytes,
                min_freq,
                max_freq,
                max_freq as f64 / min_freq.max(1) as f64
            );
        }
    }
}

// ══════════════════════════════════════════════════════════════
// Pass 3: Crypto Analysis
// ══════════════════════════════════════════════════════════════

fn pass3_crypto(capture: &Capture) {
    println!("\n{}", "=".repeat(70));
    println!("  PASS 3: Crypto Analysis");
    println!("{}", "=".repeat(70));

    let encrypted: Vec<_> = capture
        .frames
        .iter()
        .filter(|f| matches!(f.frame_type, FrameType::Encrypted { .. }))
        .collect();

    if encrypted.len() < 2 {
        println!("  Need at least 2 encrypted frames for analysis.");
        return;
    }

    // 3a: XOR adjacent frames for keystream reuse detection
    println!("\n  3a: XOR analysis (keystream reuse detection):");
    let mut xor_results: Vec<(usize, usize, f64, usize)> = Vec::new(); // (i, j, entropy, overlap_len)

    for i in 0..encrypted.len().min(20) {
        for j in (i + 1)..encrypted.len().min(20) {
            let a = &encrypted[i].payload;
            let b = &encrypted[j].payload;
            let overlap = a.len().min(b.len());
            if overlap < 16 {
                continue;
            }

            let xored: Vec<u8> = a.iter().zip(b.iter()).map(|(x, y)| x ^ y).collect();
            let ent = entropy(&xored);
            xor_results.push((encrypted[i].index, encrypted[j].index, ent, overlap));
        }
    }

    if !xor_results.is_empty() {
        let avg_xor_ent = xor_results.iter().map(|r| r.2).sum::<f64>() / xor_results.len() as f64;
        println!("    Average XOR entropy: {avg_xor_ent:.4}");

        // Low-entropy XOR = keystream reuse
        let low_xor: Vec<_> = xor_results.iter().filter(|r| r.2 < 6.0).collect();
        if low_xor.is_empty() {
            println!("    No keystream reuse detected (all XOR entropy > 6.0)");
            println!("    -> Different keys/nonces per frame, OR cipher with good diffusion");
        } else {
            println!(
                "    {} frame pairs with low XOR entropy (keystream reuse!):",
                low_xor.len()
            );
            for &(fi, fj, ent, overlap) in low_xor.iter().take(10) {
                println!("      frames {fi} ^ {fj}: entropy={ent:.4}, overlap={overlap}");
            }
        }
    }

    // 3b: Block structure detection
    println!("\n  3b: Block structure detection:");

    // Check if all encrypted payloads are multiples of common block sizes
    let block_sizes = [8, 16, 32];
    for bs in &block_sizes {
        let aligned = encrypted
            .iter()
            .filter(|f| !f.payload.is_empty() && f.payload.len() % bs == 0)
            .count();
        let pct = if encrypted.is_empty() {
            0.0
        } else {
            aligned as f64 / encrypted.len() as f64 * 100.0
        };
        println!(
            "    {bs}-byte aligned: {aligned}/{} ({pct:.1}%)",
            encrypted.len()
        );
    }

    // 3c: First-byte analysis (repeating structure before encryption?)
    println!("\n  3c: First-byte distribution of encrypted payloads:");
    let mut first_bytes: HashMap<u8, usize> = HashMap::new();
    for frame in &encrypted {
        if let Some(&b) = frame.payload.first() {
            *first_bytes.entry(b).or_default() += 1;
        }
    }

    let mut sorted_fb: Vec<_> = first_bytes.iter().collect();
    sorted_fb.sort_by(|a, b| b.1.cmp(a.1));
    let total_enc = encrypted.len();
    for (byte, count) in sorted_fb.iter().take(10) {
        let pct = **count as f64 / total_enc as f64 * 100.0;
        println!("    0x{:02X}: {count:>4} ({pct:.1}%)", byte);
    }
    let unique_first = first_bytes.len();
    println!("    {unique_first} unique first bytes out of {total_enc} frames");
    if unique_first < total_enc / 4 {
        println!("    -> Low diversity: possible ECB mode or fixed message type prefix");
    } else {
        println!("    -> High diversity: stream cipher, CTR mode, or CBC with random IV");
    }

    // 3d: Bit-flip propagation analysis
    println!("\n  3d: Byte position correlation (adjacent frames):");
    // For consecutive frames of same direction + similar size, check which bytes differ
    let mut same_dir_pairs = 0;
    let mut diff_positions: HashMap<usize, usize> = HashMap::new();
    for pair in encrypted.windows(2) {
        if pair[0].direction != pair[1].direction {
            continue;
        }
        let a = &pair[0].payload;
        let b = &pair[1].payload;
        let overlap = a.len().min(b.len());
        if overlap < 16 {
            continue;
        }

        same_dir_pairs += 1;
        for pos in 0..overlap {
            if a[pos] != b[pos] {
                *diff_positions.entry(pos).or_default() += 1;
            }
        }
    }

    if same_dir_pairs > 0 {
        println!("    Analyzed {same_dir_pairs} same-direction frame pairs");

        // Find positions that ALWAYS differ vs rarely differ
        let always_diff: Vec<_> = diff_positions
            .iter()
            .filter(|(_, count)| **count == same_dir_pairs)
            .map(|(pos, _)| *pos)
            .collect();
        let never_diff: Vec<usize> = (0..16)
            .filter(|pos| !diff_positions.contains_key(pos))
            .collect();

        if !always_diff.is_empty() && always_diff.len() <= 20 {
            let positions: Vec<_> = always_diff
                .iter()
                .take(20)
                .map(|p| format!("{p}"))
                .collect();
            println!(
                "    Positions that ALWAYS differ: [{}]",
                positions.join(", ")
            );
            println!("    -> These could be counter/nonce/sequence bytes");
        }
        if !never_diff.is_empty() {
            let positions: Vec<_> = never_diff.iter().map(|p| format!("{p}")).collect();
            println!(
                "    Positions that NEVER differ (first 16): [{}]",
                positions.join(", ")
            );
            println!("    -> These could be a fixed header or message type field");
        }
    }

    // 3e: Cipher mode inference
    println!("\n  3e: Cipher mode inference:");
    let avg_ent = encrypted
        .iter()
        .filter(|f| !f.payload.is_empty())
        .map(|f| entropy(&f.payload))
        .sum::<f64>()
        / encrypted
            .iter()
            .filter(|f| !f.payload.is_empty())
            .count()
            .max(1) as f64;

    let all_16_aligned = encrypted
        .iter()
        .filter(|f| !f.payload.is_empty())
        .all(|f| f.payload.len() % 16 == 0);

    let has_keystream_reuse = xor_results.iter().any(|r| r.2 < 6.0);
    let has_dup_payloads = !encrypted
        .iter()
        .map(|f| &f.payload)
        .collect::<std::collections::HashSet<_>>()
        .len()
        == encrypted.len();

    println!("    avg payload entropy: {avg_ent:.4}");
    println!("    all 16-byte aligned: {all_16_aligned}");
    println!("    keystream reuse:     {has_keystream_reuse}");
    println!("    duplicate payloads:  {has_dup_payloads}");
    println!();

    if has_keystream_reuse {
        println!("    VERDICT: Stream cipher or CTR mode WITH nonce reuse");
        println!("    -> Can exploit with known-plaintext XOR attack!");
    } else if has_dup_payloads {
        println!("    VERDICT: ECB mode (deterministic, no IV/nonce)");
        println!("    -> Can exploit with known-plaintext crib dragging");
    } else if all_16_aligned && avg_ent > 7.5 {
        println!("    VERDICT: Likely AES-CBC or AES-GCM (16-byte blocks, high entropy, unique)");
    } else if avg_ent > 7.5 {
        println!("    VERDICT: Stream cipher or CTR mode with unique nonces");
        println!("    -> Strong encryption, need key to decrypt");
    } else {
        println!("    VERDICT: Possibly weak or custom cipher (moderate entropy)");
    }
}

// ══════════════════════════════════════════════════════════════
// Pass 4: Session Key Hunting
// ══════════════════════════════════════════════════════════════

fn pass4_key_hunt(capture: &Capture) {
    println!("\n{}", "=".repeat(70));
    println!("  PASS 4: Session Key Hunting");
    println!("{}", "=".repeat(70));

    // Find handshake frames
    let handshakes: Vec<_> = capture
        .frames
        .iter()
        .filter(|f| f.frame_type == FrameType::Handshake)
        .collect();

    let encrypted: Vec<_> = capture
        .frames
        .iter()
        .filter(|f| matches!(f.frame_type, FrameType::Encrypted { .. }))
        .collect();

    if handshakes.is_empty() {
        println!("  No handshake frames found — cannot extract key exchange values.");
        println!("  Handshake may have occurred before capture started.");
        return;
    }

    println!("  Found {} handshake frames:", handshakes.len());

    let mut client_field7: Option<Vec<u8>> = None;
    let mut server_field7: Option<Vec<u8>> = None;

    for frame in &handshakes {
        let fields = decode_protobuf_fields(&frame.raw);
        println!("\n    Frame {} ({}):", frame.index, frame.direction);

        for &(field_num, wire_type, value) in &fields {
            println!("      field {field_num} (wt={wire_type}): {value} (0x{value:X})");

            if field_num == 7 {
                // Extract raw bytes for field 7
                let raw_bytes = extract_field7_bytes(&frame.raw);
                if let Some(ref bytes) = raw_bytes {
                    println!(
                        "      field 7 raw bytes ({} bytes): {}",
                        bytes.len(),
                        hex_preview(bytes, 32)
                    );
                }

                match frame.direction {
                    Direction::ClientToServer => client_field7 = raw_bytes,
                    Direction::ServerToClient => server_field7 = raw_bytes,
                }
            }
        }
    }

    // Try key derivation strategies
    println!("\n  Key derivation attempts:");

    let mut key_candidates: Vec<(String, Vec<u8>)> = Vec::new();

    // Strategy 1: Raw field 7 bytes as key material
    if let Some(ref client_bytes) = client_field7 {
        if client_bytes.len() >= 16 {
            let key = client_bytes[..16].to_vec();
            key_candidates.push(("client_field7_first16".into(), key));
        }
        // MD5 of client field 7
        let md5 = simple_md5(client_bytes);
        key_candidates.push(("md5(client_field7)".into(), md5));
    }

    if let Some(ref server_bytes) = server_field7 {
        if server_bytes.len() >= 16 {
            let key = server_bytes[..16].to_vec();
            key_candidates.push(("server_field7_first16".into(), key));
        }
        let md5 = simple_md5(server_bytes);
        key_candidates.push(("md5(server_field7)".into(), md5));
    }

    // Strategy 2: XOR of client and server field 7
    if let (Some(c), Some(s)) = (&client_field7, &server_field7) {
        let len = c.len().min(s.len()).min(16);
        let xored: Vec<u8> = c
            .iter()
            .zip(s.iter())
            .take(len)
            .map(|(a, b)| a ^ b)
            .collect();
        if xored.len() >= 16 {
            key_candidates.push(("xor(client_f7, server_f7)".into(), xored));
        }
    }

    // Strategy 3: Concatenation variants
    if let (Some(c), Some(s)) = (&client_field7, &server_field7) {
        let mut concat = c.clone();
        concat.extend_from_slice(s);
        let md5 = simple_md5(&concat);
        key_candidates.push(("md5(client_f7 || server_f7)".into(), md5));

        let mut concat2 = s.clone();
        concat2.extend_from_slice(c);
        let md5_2 = simple_md5(&concat2);
        key_candidates.push(("md5(server_f7 || client_f7)".into(), md5_2));
    }

    if key_candidates.is_empty() {
        println!("  No key candidates could be derived from handshake.");
        return;
    }

    println!("  Generated {} key candidates:", key_candidates.len());
    for (name, key) in &key_candidates {
        let hex: String = key
            .iter()
            .map(|b| format!("{b:02X}"))
            .collect::<Vec<_>>()
            .join("");
        println!("    {name}: {hex}");
    }

    // Try each key against first encrypted frame
    if let Some(first_enc) = encrypted.first() {
        println!(
            "\n  Testing keys against frame {} ({} byte payload):",
            first_enc.index,
            first_enc.payload.len()
        );

        for (name, key) in &key_candidates {
            // Try XOR decryption
            let decrypted: Vec<u8> = first_enc
                .payload
                .iter()
                .zip(key.iter().cycle())
                .map(|(a, b)| a ^ b)
                .collect();
            let ent = entropy(&decrypted);
            let has_printable = decrypted
                .iter()
                .take(64)
                .filter(|&&b| (0x20..0x7F).contains(&b))
                .count();
            let printable_pct = has_printable as f64 / 64.0f64.min(decrypted.len() as f64) * 100.0;

            let status = if ent < 5.0 && printable_pct > 60.0 {
                "*** POSSIBLE HIT ***"
            } else if ent < 6.5 {
                "low entropy (investigate)"
            } else {
                "no match"
            };

            println!("    {name}: entropy={ent:.2}, printable={printable_pct:.0}% -> {status}");

            if ent < 6.0 {
                println!("      Decrypted preview: {}", hex_preview(&decrypted, 32));
                // Try to interpret as protobuf
                if looks_like_protobuf(&decrypted) {
                    println!("      -> Looks like PROTOBUF!");
                    let fields = decode_protobuf_fields(&decrypted);
                    for (f, w, v) in fields.iter().take(5) {
                        println!("         field {f} (wt={w}): {v}");
                    }
                }
            }
        }
    }
}

/// Extract the raw bytes of protobuf field 7 from a message.
fn extract_field7_bytes(data: &[u8]) -> Option<Vec<u8>> {
    let mut pos = 0;
    while pos < data.len() {
        let (tag, tag_len) = decode_varint(&data[pos..])?;
        let field_num = (tag >> 3) as u32;
        let wire_type = (tag & 0x07) as u8;
        pos += tag_len;

        match wire_type {
            0 => {
                // Varint: read the raw varint bytes
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

/// Simple MD5-like hash (not cryptographic — just for key derivation testing).
/// Uses a basic Merkle-Damgard-style construction for deterministic output.
fn simple_md5(data: &[u8]) -> Vec<u8> {
    // We don't have an MD5 crate, so implement a simple deterministic hash
    // that produces 16 bytes. This is NOT MD5, just a placeholder for testing.
    let mut state: [u32; 4] = [0x67452301, 0xEFCDAB89, 0x98BADCFE, 0x10325476];

    for (i, &byte) in data.iter().enumerate() {
        let idx = i % 4;
        state[idx] = state[idx]
            .wrapping_add(byte as u32)
            .wrapping_mul(0x5BD1E995)
            .rotate_left(13);
        state[(idx + 1) % 4] ^= state[idx];
    }

    // Mix
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

// ══════════════════════════════════════════════════════════════
// Pass 5: Protocol Structure Inference
// ══════════════════════════════════════════════════════════════

fn pass5_protocol(capture: &Capture) {
    println!("\n{}", "=".repeat(70));
    println!("  PASS 5: Protocol Structure Inference");
    println!("{}", "=".repeat(70));

    let encrypted: Vec<_> = capture
        .frames
        .iter()
        .filter(|f| matches!(f.frame_type, FrameType::Encrypted { .. }))
        .collect();

    if encrypted.is_empty() {
        println!("  No encrypted frames to analyze.");
        return;
    }

    // 5a: Message size clustering (similar sizes = same message type)
    println!("\n  5a: Message size clustering:");
    let mut size_clusters: HashMap<usize, Vec<usize>> = HashMap::new();
    for frame in &encrypted {
        // Bucket to nearest 16 bytes
        let bucket = (frame.payload.len() / 16) * 16;
        size_clusters.entry(bucket).or_default().push(frame.index);
    }

    let mut sorted_clusters: Vec<_> = size_clusters.iter().collect();
    sorted_clusters.sort_by(|a, b| b.1.len().cmp(&a.1.len()));

    println!(
        "  {:<12} {:<8} {:<10} POSSIBLE_TYPE",
        "SIZE_BUCKET", "COUNT", "PCT"
    );
    println!("  {}", "-".repeat(70));

    for (bucket, indices) in sorted_clusters.iter().take(15) {
        let pct = indices.len() as f64 / encrypted.len() as f64 * 100.0;
        let guess = match *bucket {
            0..=32 => "heartbeat/ack",
            33..=128 => "status/stat update",
            129..=512 => "action/command",
            513..=2048 => "entity data",
            2049..=8192 => "scene/inventory",
            _ => "bulk transfer",
        };
        println!(
            "  {:<12} {:<8} {:<10} {guess}",
            format!("{}-{}", bucket, *bucket + 15),
            indices.len(),
            format!("{pct:.1}%"),
        );
    }

    // 5b: Request-response correlation
    println!("\n  5b: Request-response correlation:");
    // Group frames into pairs: C->S followed by S->C
    let mut pairs: Vec<(&Frame, &Frame)> = Vec::new();
    let mut i = 0;
    while i + 1 < encrypted.len() {
        if encrypted[i].direction == Direction::ClientToServer
            && encrypted[i + 1].direction == Direction::ServerToClient
        {
            pairs.push((encrypted[i], encrypted[i + 1]));
            i += 2;
        } else {
            i += 1;
        }
    }

    if !pairs.is_empty() {
        println!("    Found {} request-response pairs:", pairs.len());
        println!(
            "    {:<8} {:<12} {:<12} {:<10}",
            "PAIR", "REQ_SIZE", "RESP_SIZE", "RATIO"
        );

        for (idx, (req, resp)) in pairs.iter().take(15).enumerate() {
            let ratio = if req.payload.is_empty() {
                0.0
            } else {
                resp.payload.len() as f64 / req.payload.len() as f64
            };
            println!(
                "    {:<8} {:<12} {:<12} {:<10.2}",
                idx,
                req.payload.len(),
                resp.payload.len(),
                ratio,
            );
        }

        // Stat-fetch heuristic: small request, medium response
        let stat_candidates: Vec<_> = pairs
            .iter()
            .filter(|(req, resp)| {
                req.payload.len() < 100 && resp.payload.len() > 200 && resp.payload.len() < 4096
            })
            .collect();

        if !stat_candidates.is_empty() {
            println!(
                "\n    {} pairs match stat-fetch pattern (small req, medium resp):",
                stat_candidates.len()
            );
            for (req, resp) in stat_candidates.iter().take(5) {
                println!(
                    "      req frame {} ({} B) -> resp frame {} ({} B)",
                    req.index,
                    req.payload.len(),
                    resp.index,
                    resp.payload.len()
                );
            }
        }
    }

    // 5c: Temporal patterns (bursts of activity)
    println!("\n  5c: Temporal patterns:");
    if capture.frames.len() >= 10 {
        // Detect bursts: groups of frames with < 100ms gaps
        let mut bursts: Vec<Vec<usize>> = Vec::new();
        let mut current_burst: Vec<usize> = vec![capture.frames[0].index];

        for pair in capture.frames.windows(2) {
            let gap_ms = if pair[1].timestamp_us >= pair[0].timestamp_us {
                (pair[1].timestamp_us - pair[0].timestamp_us) / 1000
            } else {
                0
            };

            if gap_ms < 100 {
                current_burst.push(pair[1].index);
            } else {
                if current_burst.len() >= 3 {
                    bursts.push(current_burst.clone());
                }
                current_burst = vec![pair[1].index];
            }
        }
        if current_burst.len() >= 3 {
            bursts.push(current_burst);
        }

        println!(
            "    Detected {} activity bursts (3+ frames within 100ms):",
            bursts.len()
        );
        for (bi, burst) in bursts.iter().take(10).enumerate() {
            let burst_frames: Vec<_> = burst
                .iter()
                .filter_map(|&idx| capture.frames.iter().find(|f| f.index == idx))
                .collect();
            let total_bytes: usize = burst_frames.iter().map(|f| f.raw.len()).sum();
            let cs = burst_frames
                .iter()
                .filter(|f| f.direction == Direction::ClientToServer)
                .count();
            let sc = burst_frames.len() - cs;
            println!(
                "      burst {bi}: {} frames, {} total, C->S={cs} S->C={sc}, frames {:?}...",
                burst.len(),
                format_bytes(total_bytes as u64),
                &burst[..burst.len().min(5)],
            );
        }
    }

    // 5d: Summary
    println!("\n  5d: Protocol summary:");
    let total_frames = capture.frames.len();
    let enc_count = encrypted.len();
    let ctrl_count = capture
        .frames
        .iter()
        .filter(|f| matches!(f.frame_type, FrameType::Control { .. }))
        .count();
    let hs_count = capture
        .frames
        .iter()
        .filter(|f| f.frame_type == FrameType::Handshake)
        .count();
    let ss_count = capture
        .frames
        .iter()
        .filter(|f| matches!(f.frame_type, FrameType::ServerSelect { .. }))
        .count();
    let bulk_count = capture
        .frames
        .iter()
        .filter(|f| f.frame_type == FrameType::BulkEncrypted)
        .count();
    let bulk_bytes: usize = capture
        .frames
        .iter()
        .filter(|f| f.frame_type == FrameType::BulkEncrypted)
        .map(|f| f.raw.len())
        .sum();
    let unk_count = capture
        .frames
        .iter()
        .filter(|f| f.frame_type == FrameType::Unknown)
        .count();

    println!("    Total frames:    {total_frames}");
    println!("    Handshake:       {hs_count}");
    println!("    Encrypted:       {enc_count}");
    println!(
        "    Bulk encrypted:  {bulk_count} ({} bytes)",
        format_bytes(bulk_bytes as u64)
    );
    println!("    Server-select:   {ss_count}");
    println!("    Control/ACK:     {ctrl_count}");
    println!("    Unknown:         {unk_count}");
    println!();
    println!("    Protocol structure:");
    println!("      1. Plaintext protobuf handshake (small, ≤256 bytes)");
    println!("      2. Encrypted data frames: [05 0F 09 0C] + [3 x LE u32 header] + [ciphertext]");
    println!(
        "      3. Bulk encrypted stream: continuous ciphertext after initial exchange (no framing)"
    );
    println!("      4. Server-select frames: [01 00 00] + [BE u16 len] + [cmd] + [session + data]");
    println!("      5. Control frames: [0B XX 00] (3-byte ACKs)");
    println!(
        "      6. Header uint32s: h0 = total_size (h0-16 = payload), h2 likely sequence number"
    );
}

// ══════════════════════════════════════════════════════════════
// CLI Argument Parsing
// ══════════════════════════════════════════════════════════════

struct CliArgs {
    file_path: String,
    force_raw: bool,
    port: u16,
    server_ip: Option<Ipv4Addr>,
}

fn parse_args() -> CliArgs {
    let args: Vec<String> = std::env::args().collect();

    if args.len() < 2 {
        eprintln!("Usage: tcp-dissect <capture_file> [--raw] [--port N] [--server IP]");
        eprintln!();
        eprintln!("Formats (auto-detected by magic bytes):");
        eprintln!("  TCAP binary      — from tcp_capture.py");
        eprintln!("  pcap / pcapng    — from Wireshark / tcpdump");
        eprintln!("  raw mitmproxy    — use --raw flag");
        eprintln!();
        eprintln!("Options:");
        eprintln!("  --raw            Force raw mitmproxy dump parsing");
        eprintln!("  --port N         Filter pcap to TCP port N (default: {DEFAULT_GAME_PORT})");
        eprintln!(
            "  --server IP      Server IP for direction detection (auto-detected if omitted)"
        );
        eprintln!();
        eprintln!("Examples:");
        eprintln!("  cargo run --bin tcp-dissect --release -- capture.pcapng");
        eprintln!("  cargo run --bin tcp-dissect --release -- capture.pcapng --port 10012");
        eprintln!(
            "  cargo run --bin tcp-dissect --release -- capture.pcapng --server 172.65.253.17"
        );
        eprintln!("  cargo run --bin tcp-dissect --release -- tcp_capture.bin");
        std::process::exit(1);
    }

    let file_path = args[1].clone();
    let force_raw = args.iter().any(|a| a == "--raw");

    let mut port = DEFAULT_GAME_PORT;
    let mut server_ip = None;

    let mut i = 2;
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
            _ => {} // ignore unknown flags (--raw handled above)
        }
        i += 1;
    }

    CliArgs {
        file_path,
        force_raw,
        port,
        server_ip,
    }
}

// ══════════════════════════════════════════════════════════════
// Main
// ══════════════════════════════════════════════════════════════

fn main() {
    let cli = parse_args();

    println!("╔══════════════════════════════════════════════════════════╗");
    println!("║  Phase R3: TCP Protocol Dissector                       ║");
    println!("╚══════════════════════════════════════════════════════════╝");
    println!();
    println!("  Input: {}", cli.file_path);

    let data = match fs::read(&cli.file_path) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("  [!] Failed to read file: {e}");
            std::process::exit(1);
        }
    };

    println!("  Size:  {}", format_bytes(data.len() as u64));

    let capture = if cli.force_raw {
        println!("  Format: raw mitmproxy dump (--raw forced)");
        parse_raw_dump(&data)
    } else {
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
                    CaptureFormat::PcapLe => "pcap (little-endian)",
                    CaptureFormat::PcapBe => "pcap (big-endian)",
                    CaptureFormat::PcapNg => "pcapng",
                    _ => unreachable!(),
                };
                println!("  Format: {fmt_name}");
                println!("  Filter port: {}", cli.port);
                if let Some(ip) = cli.server_ip {
                    println!("  Server IP override: {ip}");
                }
                parse_pcap_capture(&data, &format, cli.port, cli.server_ip)
            }
            CaptureFormat::Unknown => {
                println!("  Format: raw mitmproxy dump (no recognized magic)");
                parse_raw_dump(&data)
            }
        }
    };

    println!("  Parsed {} frames.", capture.frames.len());

    if capture.frames.is_empty() {
        println!("\n  [!] No frames parsed. Check the input file format.");
        println!("  Hint: use --raw if this is a mitmproxy binary dump.");
        println!(
            "  Hint: use --port to change the TCP port filter (default: {DEFAULT_GAME_PORT})."
        );

        // Fallback: treat entire file as one big blob and show preview
        println!("\n  File preview:");
        println!("    {}", hex_preview(&data, 128));
        std::process::exit(0);
    }

    // Run all passes
    pass1_frame_parser(&capture);
    pass2_statistics(&capture);
    pass3_crypto(&capture);
    pass4_key_hunt(&capture);
    pass5_protocol(&capture);

    println!("\n{}", "=".repeat(70));
    println!(
        "  Analysis complete. {} frames processed.",
        capture.frames.len()
    );
    println!("{}", "=".repeat(70));
}
