//! Texture2D extraction: decode compressed textures and export as PNG.
use crate::extraction::texture_layout::{self, IndexEntry, PlannedTexture};
use crate::extraction::unity::{SerializedFile, value_bytes};
use serde_json::Value;
use std::collections::HashMap;
use std::path::Path;

/// Unity TextureFormat enum names (subset of common values).
pub fn texture_format_name(id: i64) -> &'static str {
    match id {
        1 => "Alpha8",
        2 => "ARGB4444",
        3 => "RGB24",
        4 => "RGBA32",
        5 => "ARGB32",
        7 => "RGB565",
        9 => "R16",
        10 => "DXT1",
        12 => "DXT5",
        13 => "RGBA4444",
        14 => "BGRA32",
        15 => "RHalf",
        16 => "RGHalf",
        17 => "RGBAHalf",
        18 => "RFloat",
        19 => "RGFloat",
        20 => "RGBAFloat",
        22 => "RGB9e5Float",
        24 => "BC6H",
        25 => "BC7",
        26 => "BC4",
        27 => "BC5",
        29 => "RG16",
        30 => "R8",
        34 => "ETC2_RGBA8",
        41 => "ETC_RGB4Crunched",
        42 => "ETC2_RGBA8Crunched",
        45 => "ASTC_RGB_4x4",
        46 => "ASTC_RGB_5x5",
        47 => "ASTC_RGB_6x6",
        48 => "ASTC_RGB_8x8",
        49 => "ASTC_RGB_10x10",
        50 => "ASTC_RGB_12x12",
        51 => "ASTC_RGBA_4x4",
        52 => "ASTC_RGBA_5x5",
        53 => "ASTC_RGBA_6x6",
        54 => "ASTC_RGBA_8x8",
        55 => "ASTC_RGBA_10x10",
        56 => "ASTC_RGBA_12x12",
        62 => "RG32",
        63 => "RGB48",
        64 => "RGBA64",
        _ => "Unknown",
    }
}

/// Decode a compressed texture into RGBA32 (u32 per pixel, BGRA layout from texture2ddecoder).
pub fn decode_texture(
    format: i64,
    data: &[u8],
    width: usize,
    height: usize,
    out: &mut [u32],
) -> Result<(), &'static str> {
    match format {
        // Alpha8 (1) — single-channel grayscale
        1 => convert_pixels(data, width, height, out, 1, "Alpha8 data too short", |p| {
            [255, 255, 255, p[0]]
        }),
        // RGB24 (3)
        3 => convert_pixels(data, width, height, out, 3, "RGB24 data too short", |p| {
            [p[2], p[1], p[0], 255]
        }),
        // RGBA32 (4)
        4 => convert_pixels(data, width, height, out, 4, "RGBA32 data too short", |p| {
            [p[2], p[1], p[0], p[3]]
        }),
        // ARGB32 (5)
        5 => convert_pixels(data, width, height, out, 4, "ARGB32 data too short", |p| {
            [p[3], p[2], p[1], p[0]]
        }),
        // R8 (30)
        30 => convert_pixels(data, width, height, out, 1, "R8 data too short", |p| {
            [0, 0, p[0], 255]
        }),
        // DXT1 / BC1 (10)
        10 => texture2ddecoder::decode_bc1(data, width, height, out),
        // DXT5 / BC3 (12)
        12 => texture2ddecoder::decode_bc3(data, width, height, out),
        // BC4 (26)
        26 => texture2ddecoder::decode_bc4(data, width, height, out),
        // BC5 (27)
        27 => texture2ddecoder::decode_bc5(data, width, height, out),
        // BC6H (24)
        24 => texture2ddecoder::decode_bc6(data, width, height, out, false),
        // BC7 (25)
        25 => texture2ddecoder::decode_bc7(data, width, height, out),
        // ETC2_RGBA8 (34)
        34 => texture2ddecoder::decode_etc2_rgba8(data, width, height, out),
        // ASTC variants (45-56): block_width x block_height
        45 | 51 => texture2ddecoder::decode_astc(data, width, height, 4, 4, out),
        46 | 52 => texture2ddecoder::decode_astc(data, width, height, 5, 5, out),
        47 | 53 => texture2ddecoder::decode_astc(data, width, height, 6, 6, out),
        48 | 54 => texture2ddecoder::decode_astc(data, width, height, 8, 8, out),
        49 | 55 => texture2ddecoder::decode_astc(data, width, height, 10, 10, out),
        50 | 56 => texture2ddecoder::decode_astc(data, width, height, 12, 12, out),
        _ => Err("unsupported"),
    }
}

/// Convert an uncompressed format with `bytes_per_pixel`-byte pixels into packed BGRA, using
/// `to_bgra` to reorder one source pixel's channels.
fn convert_pixels(
    data: &[u8],
    width: usize,
    height: usize,
    out: &mut [u32],
    bytes_per_pixel: usize,
    too_short: &'static str,
    to_bgra: impl Fn(&[u8]) -> [u8; 4],
) -> Result<(), &'static str> {
    let pixels = width * height;
    if data.len() < pixels * bytes_per_pixel {
        return Err(too_short);
    }
    for (i, pixel) in data.chunks_exact(bytes_per_pixel).take(pixels).enumerate() {
        out[i] = u32::from_le_bytes(to_bgra(pixel));
    }
    Ok(())
}

/// Resolve streamed texture data from .resS resources.
/// m_StreamData.path can be "archive:/CAB-<hash>/CAB-<hash>.resS" or just "CAB-<hash>.resS".
pub fn resolve_stream_data(obj: &Value, resources: &HashMap<String, Vec<u8>>) -> Option<Vec<u8>> {
    let sd = obj.get("m_StreamData")?.as_object()?;

    let path = sd.get("path")?.as_str().filter(|s| !s.is_empty())?;
    let offset = sd.get("offset").and_then(Value::as_u64).unwrap_or(0) as usize;
    let size = sd.get("size")?.as_u64()? as usize;
    if size == 0 {
        return None;
    }

    let bare_name = path.rsplit('/').next().unwrap_or(path);

    let res_data = resources
        .get(path)
        .or_else(|| resources.get(bare_name))
        .or_else(|| {
            resources
                .iter()
                .find(|(k, _)| k.ends_with(bare_name))
                .map(|(_, v)| v)
        })?;

    // A stream running past the end of its .resS is truncated: report it as missing
    // rather than decoding partial bytes.
    let end = offset.checked_add(size)?;
    Some(res_data.get(offset..end)?.to_vec())
}

/// Get an integer field from a deserialized object.
pub(crate) fn get_int(obj: &Value, key: &str) -> i64 {
    obj.get(key).and_then(|v| v.as_i64()).unwrap_or(0)
}

fn index_entry(plan: &PlannedTexture, width: usize, height: usize, format: i64) -> IndexEntry {
    IndexEntry::new(
        &plan.relative,
        &plan.name,
        plan.placement.as_ref(),
        width as u32,
        height as u32,
        texture_format_name(format),
    )
}

/// Texture extraction counts and index entries produced for one SerializedFile.
pub struct TextureExtractResult {
    pub extracted: u64,
    /// Already on disk and skipped (only with `skip_existing`).
    pub existing: u64,
    pub failed: u64,
    /// PNGs that decoded but could not be written (included in `failed`).
    pub write_errors: u64,
    pub unsupported: u64,
    pub no_data: u64,
    pub entries: Vec<IndexEntry>,
}

/// Convert decoded BGRA pixels to RGBA bytes, flipping rows top-to-bottom.
///
/// Unity stores texture rows bottom-up, so the first decoded row is the bottom of the image.
pub fn bgra_to_rgba_flipped(pixels: &[u32], width: usize, height: usize) -> Vec<u8> {
    let mut rgba = vec![0u8; width * height * 4];
    for (y, row) in pixels.chunks_exact(width).take(height).enumerate() {
        let out_row = &mut rgba[(height - 1 - y) * width * 4..(height - y) * width * 4];
        for (out, &pixel) in out_row.as_chunks_mut::<4>().0.iter_mut().zip(row) {
            let [b, g, r, a] = pixel.to_le_bytes();
            out.copy_from_slice(&[r, g, b, a]);
        }
    }
    rgba
}

/// Save RGBA pixels as a PNG, written beside the target and renamed so a crash never leaves a
/// truncated PNG that a later run would keep as "already extracted".
fn save_png(path: &Path, rgba: &[u8], width: usize, height: usize) -> Result<(), String> {
    let tmp = path.with_extension("png.tmp");
    image::save_buffer_with_format(
        &tmp,
        rgba,
        width as u32,
        height as u32,
        image::ColorType::Rgba8,
        image::ImageFormat::Png,
    )
    .map_err(|error| error.to_string())
    .and_then(|()| std::fs::rename(&tmp, path).map_err(|error| error.to_string()))
}

/// Extract all Texture2D objects from a SerializedFile as PNG files.
///
/// With `skip_existing`, textures whose PNG is already on disk are indexed without re-decoding.
pub fn extract_textures(
    sf: &SerializedFile,
    resources: &HashMap<String, Vec<u8>>,
    output_dir: &Path,
    skip_existing: bool,
) -> TextureExtractResult {
    let mut extracted = 0u64;
    let mut failed = 0u64;
    let mut skipped_format = 0u64;
    let mut no_data = 0u64;
    let mut existing = 0u64;
    let mut write_errors = 0u64;
    let mut entries = Vec::new();
    let planned = texture_layout::plan_textures(sf);
    let planned_by_id: HashMap<_, _> = planned.iter().map(|plan| (plan.path_id, plan)).collect();

    for obj in sf.objects_of_type(28) {
        let Some(plan) = planned_by_id.get(&obj.path_id) else {
            continue;
        };
        let Ok(unity_obj) = sf.read_object(obj) else {
            continue;
        };

        let name = &plan.name;

        let width = get_int(&unity_obj, "m_Width") as usize;
        let height = get_int(&unity_obj, "m_Height") as usize;
        let format = get_int(&unity_obj, "m_TextureFormat");

        if width == 0 || height == 0 {
            continue;
        }

        let path = output_dir.join(&plan.relative);
        if skip_existing && path.is_file() {
            existing += 1;
            entries.push(index_entry(plan, width, height, format));
            continue;
        }

        // Pixel data is stored inline or in a .resS stream.
        let pixel_data = unity_obj
            .get("image data")
            .and_then(value_bytes)
            .filter(|b| !b.is_empty())
            .or_else(|| resolve_stream_data(&unity_obj, resources));

        let Some(compressed) = pixel_data else {
            no_data += 1;
            continue;
        };

        // Decode compressed texture → RGBA32
        let mut rgba_buf = vec![0u32; width * height];
        match decode_texture(format, &compressed, width, height, &mut rgba_buf) {
            Ok(()) => {
                let rgba_bytes = bgra_to_rgba_flipped(&rgba_buf, width, height);
                if let Some(parent) = path.parent()
                    && let Err(error) = std::fs::create_dir_all(parent)
                {
                    eprintln!("  FAIL create directory for {}: {}", name, error);
                    failed += 1;
                    write_errors += 1;
                    continue;
                }
                match save_png(&path, &rgba_bytes, width, height) {
                    Ok(()) => {
                        extracted += 1;
                        entries.push(index_entry(plan, width, height, format));
                        if extracted <= 20 || extracted.is_multiple_of(100) {
                            println!(
                                "  [{}] {} ({}x{}, {}) → {}",
                                extracted,
                                name,
                                width,
                                height,
                                texture_format_name(format),
                                path.display()
                            );
                        }
                    }
                    Err(e) => {
                        eprintln!("  FAIL save {}: {}", name, e);
                        failed += 1;
                        write_errors += 1;
                    }
                }
            }
            Err(msg) => {
                if msg == "unsupported" {
                    skipped_format += 1;
                } else {
                    eprintln!(
                        "  FAIL decode {} ({}x{}, fmt={}): {}",
                        name, width, height, format, msg
                    );
                    failed += 1;
                }
            }
        }
    }

    TextureExtractResult {
        extracted,
        existing,
        failed,
        write_errors,
        unsupported: skipped_format,
        no_data,
        entries,
    }
}
