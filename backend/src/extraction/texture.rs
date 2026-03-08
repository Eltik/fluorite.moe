/// Texture2D extraction: decode compressed textures and export as PNG.
use std::collections::HashMap;
use std::path::Path;
use unity_asset_binary::asset::SerializedFile;
use unity_asset_binary::object::ObjectHandle;
use unity_asset_core::UnityValue;

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
        1 => {
            if data.len() < width * height {
                return Err("Alpha8 data too short");
            }
            for (i, &a) in data.iter().take(width * height).enumerate() {
                out[i] = u32::from_be_bytes([a, 255, 255, 255]);
            }
            Ok(())
        }
        // RGB24 (3)
        3 => {
            if data.len() < width * height * 3 {
                return Err("RGB24 data too short");
            }
            for i in 0..(width * height) {
                let r = data[i * 3];
                let g = data[i * 3 + 1];
                let b = data[i * 3 + 2];
                out[i] = u32::from_le_bytes([b, g, r, 255]);
            }
            Ok(())
        }
        // RGBA32 (4)
        4 => {
            if data.len() < width * height * 4 {
                return Err("RGBA32 data too short");
            }
            for i in 0..(width * height) {
                let r = data[i * 4];
                let g = data[i * 4 + 1];
                let b = data[i * 4 + 2];
                let a = data[i * 4 + 3];
                out[i] = u32::from_le_bytes([b, g, r, a]);
            }
            Ok(())
        }
        // ARGB32 (5)
        5 => {
            if data.len() < width * height * 4 {
                return Err("ARGB32 data too short");
            }
            for i in 0..(width * height) {
                let a = data[i * 4];
                let r = data[i * 4 + 1];
                let g = data[i * 4 + 2];
                let b = data[i * 4 + 3];
                out[i] = u32::from_le_bytes([b, g, r, a]);
            }
            Ok(())
        }
        // R8 (30)
        30 => {
            if data.len() < width * height {
                return Err("R8 data too short");
            }
            for (i, &r) in data.iter().take(width * height).enumerate() {
                out[i] = u32::from_le_bytes([0, 0, r, 255]);
            }
            Ok(())
        }
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

/// Resolve streamed texture data from .resS resources.
/// m_StreamData.path can be "archive:/CAB-<hash>/CAB-<hash>.resS" or just "CAB-<hash>.resS".
pub fn resolve_stream_data(
    obj: &unity_asset_binary::object::UnityObject,
    resources: &HashMap<String, Vec<u8>>,
) -> Option<Vec<u8>> {
    let sd = match obj.get("m_StreamData") {
        Some(UnityValue::Object(o)) => o,
        _ => return None,
    };

    let path = match sd.get("path") {
        Some(UnityValue::String(s)) if !s.is_empty() => s.as_str(),
        _ => return None,
    };
    let offset = match sd.get("offset") {
        Some(UnityValue::Integer(i)) => *i as usize,
        _ => 0,
    };
    let size = match sd.get("size") {
        Some(UnityValue::Integer(i)) => *i as usize,
        _ => return None,
    };
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

    let end = (offset + size).min(res_data.len());
    if offset >= res_data.len() {
        return None;
    }

    Some(res_data[offset..end].to_vec())
}

/// Get an integer field from a UnityObject.
fn get_int(obj: &unity_asset_binary::object::UnityObject, key: &str) -> i64 {
    obj.get(key).and_then(|v| v.as_i64()).unwrap_or(0)
}

/// Extract all Texture2D objects from SerializedFiles as PNG files.
/// Returns (extracted_count, failed_count, unsupported_count, no_data_count).
pub fn extract_textures(
    sf: &SerializedFile,
    resources: &HashMap<String, Vec<u8>>,
    output_dir: &Path,
) -> (u64, u64, u64, u64) {
    let mut extracted = 0u64;
    let mut failed = 0u64;
    let mut skipped_format = 0u64;
    let mut no_data = 0u64;

    for obj in sf.objects_of_type(28) {
        let handle = ObjectHandle::new(sf, obj);
        let unity_obj = match handle.read() {
            Ok(o) => o,
            Err(_) => continue,
        };

        let name = unity_obj.name().unwrap_or_default();
        if name.is_empty() {
            continue;
        }

        let width = get_int(&unity_obj, "m_Width") as usize;
        let height = get_int(&unity_obj, "m_Height") as usize;
        let format = get_int(&unity_obj, "m_TextureFormat");

        if width == 0 || height == 0 {
            continue;
        }

        // Get pixel data: inline or from .resS stream
        let pixel_data: Option<Vec<u8>> = {
            let inline = unity_obj
                .get("image data")
                .and_then(|v| v.as_bytes())
                .filter(|b| !b.is_empty());

            if let Some(data) = inline {
                Some(data.to_vec())
            } else {
                resolve_stream_data(&unity_obj, resources)
            }
        };

        let Some(compressed) = pixel_data else {
            no_data += 1;
            continue;
        };

        // Decode compressed texture → RGBA32
        let mut rgba_buf = vec![0u32; width * height];
        match decode_texture(format, &compressed, width, height, &mut rgba_buf) {
            Ok(()) => {
                // Convert u32 BGRA → u8 RGBA and save PNG
                let mut rgba_bytes = vec![0u8; width * height * 4];
                for (i, &pixel) in rgba_buf.iter().enumerate() {
                    let b = (pixel & 0xFF) as u8;
                    let g = ((pixel >> 8) & 0xFF) as u8;
                    let r = ((pixel >> 16) & 0xFF) as u8;
                    let a = ((pixel >> 24) & 0xFF) as u8;
                    rgba_bytes[i * 4] = r;
                    rgba_bytes[i * 4 + 1] = g;
                    rgba_bytes[i * 4 + 2] = b;
                    rgba_bytes[i * 4 + 3] = a;
                }

                let safe_name = crate::extraction::utils::sanitize_filename(&name);
                let path = output_dir.join(format!("{safe_name}.png"));
                match image::save_buffer(
                    &path,
                    &rgba_bytes,
                    width as u32,
                    height as u32,
                    image::ColorType::Rgba8,
                ) {
                    Ok(()) => {
                        extracted += 1;
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

    (extracted, failed, skipped_format, no_data)
}
