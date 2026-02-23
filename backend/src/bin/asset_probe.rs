/// Asset investigation probe: catalogs all Unity objects in VFS directories,
/// surveys Texture2D/Mesh objects, dumps TypeTree structure, and searches for character assets.
///
/// Pipeline: config → ZIP → .chk → VFS decrypt → UnityFS → SerializedFile → object survey
///
/// Environment variables:
///   SCAN_ALL=1    — process ALL .chk files (default: smallest per directory)
///   CHAR_SCAN=1   — run character asset search (Pass 5)
///   EXTRACT_TEX=1 — extract Texture2D objects as PNG files
use backend::core::endfield::{
    config::{ResolveParams, resolve_game_config},
    constants::{app_code, channel, sub_channel},
};
use backend::core::hypergryph::launcher::Platform;
use backend::extraction::unityfs_header;
use backend::extraction::vfs_decrypt;
use backend::extraction::zip_range::{SplitArchivePart, ZipCentralDirectory};
use reqwest::Client;
use std::collections::{BTreeMap, HashMap};
use unity_asset_binary::asset::{SerializedFile, SerializedFileParser};
use unity_asset_binary::object::ObjectHandle;
use unity_asset_core::UnityValue;

/// VFS container directories.
const VFS_DIRS: &[(&str, &str)] = &[("7064D8E2", "bundles/configs"), ("0CE8FA57", "textures")];

fn format_bytes(bytes: u64) -> String {
    const UNITS: &[&str] = &["B", "KB", "MB", "GB", "TB"];
    if bytes == 0 {
        return "0 B".into();
    }
    let mut size = bytes as f64;
    let mut i = 0;
    while size >= 1024.0 && i < UNITS.len() - 1 {
        size /= 1024.0;
        i += 1;
    }
    format!("{:.2} {}", size, UNITS[i])
}

/// Get Unity class name from class ID.
fn class_name(id: i32) -> String {
    unity_asset_core::get_class_name(id).unwrap_or_else(|| format!("Class_{}", id))
}

/// Texture2D survey result.
struct Tex2DInfo {
    name: String,
    width: i64,
    height: i64,
    format: i64,
    mip_count: i64,
    data_size: usize,
    is_readable: bool,
    stream_path: String,
    stream_size: i64,
}

/// Mesh survey result.
struct MeshInfo {
    name: String,
    submesh_count: usize,
    vertex_count: i64,
    index_buf_size: usize,
    vertex_data_size: i64,
    mesh_compression: i64,
}

/// All extracted contents from a .chk file: SerializedFile data + shared resource streams.
struct ChkContents {
    /// Individual SerializedFile blobs.
    sf_data: Vec<Vec<u8>>,
    /// Resource streams keyed by entry path (e.g., "CAB-<hash>.resS").
    /// Shared across all SerializedFiles from the same .chk.
    resources: HashMap<String, Vec<u8>>,
}

/// Extract all SerializedFile + resource stream data from a .chk file.
///
/// Resource streams (.resS) can come from two places:
/// 1. As directory entries INSIDE a UnityFS bundle (alongside the SerializedFile)
/// 2. As separate VFS files alongside the UnityFS bundle (NOT wrapped in UnityFS)
fn extract_chk_contents(data: &[u8]) -> Option<ChkContents> {
    if !vfs_decrypt::is_valid_header(data) {
        return None;
    }
    let assets = match unityfs_header::process_chk_file(data) {
        Ok(a) => a,
        Err(_) => return None,
    };

    let mut resources: HashMap<String, Vec<u8>> = HashMap::new();
    let mut sf_data: Vec<Vec<u8>> = Vec::new();

    for asset in assets {
        if asset.is_unityfs {
            for entry in &asset.bundle_entries {
                let start = entry.offset as usize;
                let end = (start + entry.size as usize).min(asset.data.len());
                if start >= asset.data.len() {
                    continue;
                }
                if entry.path.ends_with(".resS") || entry.path.ends_with(".resource") {
                    resources.insert(entry.path.clone(), asset.data[start..end].to_vec());
                } else {
                    sf_data.push(asset.data[start..end].to_vec());
                }
            }
        } else if asset.vfs_name.ends_with(".resS") || asset.vfs_name.ends_with(".resource") {
            resources.insert(asset.vfs_name, asset.data);
        } else {
            sf_data.push(asset.data);
        }
    }

    Some(ChkContents { sf_data, resources })
}

/// Get an integer field from a UnityObject.
fn get_int(obj: &unity_asset_binary::object::UnityObject, key: &str) -> i64 {
    obj.get(key).and_then(|v| v.as_i64()).unwrap_or(0)
}

/// Survey a Texture2D object.
fn survey_texture2d(sf: &SerializedFile, obj: &unity_asset_binary::asset::ObjectInfo) -> Option<Tex2DInfo> {
    let handle = ObjectHandle::new(sf, obj);
    let unity_obj = handle.read().ok()?;

    let name = unity_obj.name().unwrap_or_default();
    let width = get_int(&unity_obj, "m_Width");
    let height = get_int(&unity_obj, "m_Height");
    let format = get_int(&unity_obj, "m_TextureFormat");
    let mip_count = get_int(&unity_obj, "m_MipCount");
    let is_readable = unity_obj.get("m_IsReadable")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    // Image data — may be inline bytes or empty (streamed externally)
    let data_size = unity_obj.get("image data")
        .and_then(|v| v.as_bytes())
        .map(|b| b.len())
        .unwrap_or(0);

    // Streaming data
    let (stream_path, stream_size) = match unity_obj.get("m_StreamData") {
        Some(UnityValue::Object(sd)) => {
            let path = match sd.get("path") {
                Some(UnityValue::String(s)) => s.clone(),
                _ => String::new(),
            };
            let size = match sd.get("size") {
                Some(UnityValue::Integer(i)) => *i,
                _ => 0,
            };
            (path, size)
        }
        _ => (String::new(), 0),
    };

    Some(Tex2DInfo {
        name,
        width,
        height,
        format,
        mip_count,
        data_size,
        is_readable,
        stream_path,
        stream_size,
    })
}

/// Survey a Mesh object.
fn survey_mesh(sf: &SerializedFile, obj: &unity_asset_binary::asset::ObjectInfo) -> Option<MeshInfo> {
    let handle = ObjectHandle::new(sf, obj);
    let unity_obj = handle.read().ok()?;

    let name = unity_obj.name().unwrap_or_default();

    let submesh_count = unity_obj.get("m_SubMeshes")
        .and_then(|v| v.as_array())
        .map(|a| a.len())
        .unwrap_or(0);

    // Try m_VertexCount first, then look inside m_VertexData
    let vertex_count = {
        let vc = get_int(&unity_obj, "m_VertexCount");
        if vc > 0 {
            vc
        } else {
            match unity_obj.get("m_VertexData") {
                Some(UnityValue::Object(vd)) => match vd.get("m_VertexCount") {
                    Some(UnityValue::Integer(i)) => *i,
                    _ => 0,
                },
                _ => 0,
            }
        }
    };

    let index_buf_size = match unity_obj.get("m_IndexBuffer") {
        Some(UnityValue::Bytes(b)) => b.len(),
        _ => 0,
    };

    let vertex_data_size = match unity_obj.get("m_VertexData") {
        Some(UnityValue::Object(vd)) => match vd.get("m_DataSize") {
            Some(UnityValue::Integer(i)) => *i,
            _ => 0,
        },
        _ => 0,
    };

    let mesh_compression = get_int(&unity_obj, "m_MeshCompression");

    Some(MeshInfo {
        name,
        submesh_count,
        vertex_count,
        index_buf_size,
        vertex_data_size,
        mesh_compression,
    })
}

/// Dump TypeTree structure for a given class ID.
fn dump_type_tree(sf: &SerializedFile, class_id: i32) {
    // Find the SerializedType for this class ID
    let st = sf.types.iter().find(|t| t.class_id == class_id);
    let Some(st) = st else {
        println!("  (no type entry for classID={})", class_id);
        return;
    };

    if st.type_tree.is_empty() {
        println!("  (TypeTree is empty — stripped build?)");
        return;
    }

    fn print_node(node: &unity_asset_binary::typetree::TypeTreeNode, indent: usize) {
        let pad = "  ".repeat(indent);
        let size_str = if node.byte_size >= 0 {
            format!("{} bytes", node.byte_size)
        } else {
            "variable".to_string()
        };
        let flags = if node.is_aligned() { " [aligned]" } else { "" };
        println!(
            "{}  {} {} : {} ({}{})",
            pad, node.level, node.name, node.type_name, size_str, flags
        );
        for child in &node.children {
            print_node(child, indent + 1);
        }
    }

    for node in &st.type_tree.nodes {
        print_node(node, 0);
    }
}

/// Unity TextureFormat enum names (subset of common values).
fn texture_format_name(id: i64) -> &'static str {
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

/// Resolve streamed texture data from .resS resources.
/// m_StreamData.path can be "archive:/CAB-<hash>/CAB-<hash>.resS" or just "CAB-<hash>.resS".
fn resolve_stream_data(
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

    // Try multiple path resolution strategies:
    // 1. Exact match
    // 2. Bare filename (strip "archive:/.../" prefix)
    // 3. Partial match on filename
    let bare_name = path.rsplit('/').next().unwrap_or(path);

    let res_data = resources
        .get(path)
        .or_else(|| resources.get(bare_name))
        .or_else(|| {
            // Fuzzy match: find a resource whose key ends with bare_name
            resources.iter().find(|(k, _)| k.ends_with(bare_name)).map(|(_, v)| v)
        })?;

    let end = (offset + size).min(res_data.len());
    if offset >= res_data.len() {
        return None;
    }

    Some(res_data[offset..end].to_vec())
}

/// Decode a compressed texture into RGBA32 (u32 per pixel, BGRA layout from texture2ddecoder).
fn decode_texture(
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
                out[i] = u32::from_be_bytes([a, 255, 255, 255]); // BGRA: B=255,G=255,R=255,A=a
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
                out[i] = u32::from_le_bytes([b, g, r, 255]); // BGRA
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
                out[i] = u32::from_le_bytes([b, g, r, a]); // BGRA
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
                out[i] = u32::from_le_bytes([b, g, r, a]); // BGRA
            }
            Ok(())
        }
        // R8 (30)
        30 => {
            if data.len() < width * height {
                return Err("R8 data too short");
            }
            for (i, &r) in data.iter().take(width * height).enumerate() {
                out[i] = u32::from_le_bytes([0, 0, r, 255]); // BGRA
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

/// Sanitize a string for use as a filename.
fn sanitize_filename(name: &str) -> String {
    name.chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            _ => c,
        })
        .collect()
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("=== Asset Probe ===\n");

    let scan_all = std::env::var("SCAN_ALL").is_ok();
    let char_scan = std::env::var("CHAR_SCAN").is_ok();

    if scan_all {
        println!("SCAN_ALL: processing ALL .chk files\n");
    }

    let client = Client::new();
    let params = ResolveParams {
        app_code: app_code::game::OS_WIN_REL,
        launcher_app_code: app_code::launcher::OS_WIN_REL,
        channel: channel::OS_WIN_REL,
        sub_channel: sub_channel::OS_WIN_REL,
        launcher_sub_channel: sub_channel::OS_WIN_REL,
        platform: Platform::Windows,
    };

    println!("[1] Resolving game config...");
    let config = resolve_game_config(&client, &params).await?;
    println!("    Version: {}", config.version);

    println!("\n[2] Parsing ZIP central directory...");
    let parts: Vec<SplitArchivePart> = config
        .packs
        .iter()
        .map(|p| SplitArchivePart {
            url: p.url.clone(),
            size: p.package_size.parse::<u64>().expect("invalid package_size"),
        })
        .collect();
    let cd = ZipCentralDirectory::from_split_archive(&client, parts).await?;
    println!("    Total ZIP entries: {}", cd.len());

    // Collect all contents across both VFS directories.
    let mut all_chks: Vec<ChkContents> = Vec::new();
    let mut dir_sf_counts: Vec<(String, usize)> = Vec::new();

    println!("\n[3] Downloading and extracting VFS data...\n");

    for (dir, desc) in VFS_DIRS {
        println!("--- {} ({}) ---", dir, desc);

        let mut candidates: Vec<(String, u64)> = cd
            .filenames()
            .filter(|f| f.ends_with(".chk") && f.contains(dir))
            .map(|f| {
                let size = cd.get(f).map_or(0, |e| e.uncompressed_size);
                (f.to_string(), size)
            })
            .collect();
        candidates.sort_by_key(|(_n, s)| *s);

        if !scan_all {
            candidates.truncate(1);
        }

        println!("  Processing {} .chk file(s)", candidates.len());

        let mut dir_count = 0usize;
        for (name, size) in &candidates {
            println!("  {} ({})", name, format_bytes(*size));

            let data = match cd.extract(&client, name).await {
                Ok(d) => d,
                Err(e) => {
                    println!("    Download error: {}", e);
                    continue;
                }
            };

            if let Some(chk) = extract_chk_contents(&data) {
                let sf_count = chk.sf_data.len();
                let res_count = chk.resources.len();
                let res_bytes: u64 = chk.resources.values().map(|v| v.len() as u64).sum();
                println!(
                    "    → {} SerializedFile(s), {} .resS stream(s) ({})",
                    sf_count,
                    res_count,
                    format_bytes(res_bytes)
                );
                dir_count += sf_count;
                all_chks.push(chk);
            }
        }
        dir_sf_counts.push((format!("{} ({})", dir, desc), dir_count));
        println!();
    }

    let total_sfs: usize = all_chks.iter().map(|c| c.sf_data.len()).sum();
    println!("  Total SerializedFiles: {}\n", total_sfs);
    for (dir, count) in &dir_sf_counts {
        println!("    {}: {} files", dir, count);
    }

    // ══════════════════════════════════════════════════════════════
    // Pass 1: Class ID Census
    // ══════════════════════════════════════════════════════════════
    println!("\n╔══════════════════════════════════════════╗");
    println!("║  Pass 1: Class ID Census                 ║");
    println!("╚══════════════════════════════════════════╝\n");

    let mut class_counts: BTreeMap<i32, u64> = BTreeMap::new();
    let mut class_total_bytes: BTreeMap<i32, u64> = BTreeMap::new();
    let mut total_objects = 0u64;

    for chk in &all_chks {
        for sf_bytes in &chk.sf_data {
            let sf = match SerializedFileParser::from_bytes(sf_bytes.clone()) {
                Ok(sf) => sf,
                Err(_) => continue,
            };

            for obj in &sf.objects {
                *class_counts.entry(obj.type_id).or_insert(0) += 1;
                *class_total_bytes.entry(obj.type_id).or_insert(0) += obj.byte_size as u64;
                total_objects += 1;
            }
        }
    }

    println!("  Total objects: {}\n", total_objects);
    println!("  {:>8} | {:>6} | {:>10} | Name", "ClassID", "Count", "Total Size");
    println!("  {}", "-".repeat(55));

    let mut sorted_classes: Vec<_> = class_counts.iter().collect();
    sorted_classes.sort_by_key(|(_, count)| std::cmp::Reverse(*count));

    for (class_id, count) in &sorted_classes {
        let total = class_total_bytes.get(class_id).copied().unwrap_or(0);
        println!(
            "  {:>8} | {:>6} | {:>10} | {}",
            class_id,
            count,
            format_bytes(total),
            class_name(**class_id)
        );
    }

    // ══════════════════════════════════════════════════════════════
    // Pass 2: Texture2D Survey
    // ══════════════════════════════════════════════════════════════
    println!("\n╔══════════════════════════════════════════╗");
    println!("║  Pass 2: Texture2D Survey                ║");
    println!("╚══════════════════════════════════════════╝\n");

    let mut textures: Vec<Tex2DInfo> = Vec::new();
    let mut format_counts: BTreeMap<i64, u64> = BTreeMap::new();

    for chk in &all_chks {
        for sf_bytes in &chk.sf_data {
            let sf = match SerializedFileParser::from_bytes(sf_bytes.clone()) {
                Ok(sf) => sf,
                Err(_) => continue,
            };

            for obj in sf.objects_of_type(28) {
                if let Some(info) = survey_texture2d(&sf, obj) {
                    *format_counts.entry(info.format).or_insert(0) += 1;
                    textures.push(info);
                }
            }
        }
    }

    println!("  Found {} Texture2D objects\n", textures.len());

    // Format distribution
    println!("  Format distribution:");
    let mut sorted_formats: Vec<_> = format_counts.iter().collect();
    sorted_formats.sort_by_key(|(_, count)| std::cmp::Reverse(*count));
    for (fmt_id, count) in &sorted_formats {
        println!(
            "    {:>3} ({:<20}) : {}",
            fmt_id,
            texture_format_name(**fmt_id),
            count
        );
    }

    // Character textures
    let char_textures: Vec<&Tex2DInfo> = textures
        .iter()
        .filter(|t| {
            let n = t.name.to_lowercase();
            n.contains("chr_") || n.contains("actor_") || n.contains("t_actor_")
        })
        .collect();

    if !char_textures.is_empty() {
        println!("\n  Character textures ({}):", char_textures.len());
        println!(
            "  {:>40} | {:>10} | {:>3} | {:>10} | {:>10} | Readable",
            "Name", "Size", "Fmt", "Data", "Stream"
        );
        println!("  {}", "-".repeat(100));
        for t in &char_textures {
            let streamed = if t.stream_path.is_empty() {
                "-".to_string()
            } else {
                format_bytes(t.stream_size as u64)
            };
            println!(
                "  {:>40} | {:>4}x{:<4} | {:>3} | {:>10} | {:>10} | {}",
                if t.name.len() > 40 { &t.name[..40] } else { &t.name },
                t.width,
                t.height,
                t.format,
                format_bytes(t.data_size as u64),
                streamed,
                t.is_readable
            );
        }
    }

    // All textures summary table (first 50)
    println!("\n  All textures (first 50):");
    println!(
        "  {:>50} | {:>10} | {:>20} | {:>10} | Mips",
        "Name", "Size", "Format", "Data"
    );
    println!("  {}", "-".repeat(110));
    for t in textures.iter().take(50) {
        let data_info = if t.data_size > 0 {
            format!("{} inline", format_bytes(t.data_size as u64))
        } else if t.stream_size > 0 {
            format!("{} stream", format_bytes(t.stream_size as u64))
        } else {
            "NONE".to_string()
        };
        println!(
            "  {:>50} | {:>4}x{:<4} | {:>20} | {:>10} | {}",
            if t.name.len() > 50 { &t.name[..50] } else { &t.name },
            t.width,
            t.height,
            texture_format_name(t.format),
            data_info,
            t.mip_count
        );
    }

    // ══════════════════════════════════════════════════════════════
    // Pass 3: Mesh Survey
    // ══════════════════════════════════════════════════════════════
    println!("\n╔══════════════════════════════════════════╗");
    println!("║  Pass 3: Mesh Survey                     ║");
    println!("╚══════════════════════════════════════════╝\n");

    let mut meshes: Vec<MeshInfo> = Vec::new();

    for chk in &all_chks {
        for sf_bytes in &chk.sf_data {
            let sf = match SerializedFileParser::from_bytes(sf_bytes.clone()) {
                Ok(sf) => sf,
                Err(_) => continue,
            };

            for obj in sf.objects_of_type(43) {
                if let Some(info) = survey_mesh(&sf, obj) {
                    meshes.push(info);
                }
            }
        }
    }

    println!("  Found {} Mesh objects\n", meshes.len());

    // Character meshes
    let char_meshes: Vec<&MeshInfo> = meshes
        .iter()
        .filter(|m| {
            let n = m.name.to_lowercase();
            n.contains("s_actor_") || n.contains("chr_") || n.contains("lod")
        })
        .collect();

    if !char_meshes.is_empty() {
        println!("  Character meshes ({}):", char_meshes.len());
        println!(
            "  {:>50} | {:>8} | {:>4} | {:>10} | {:>10} | Compress",
            "Name", "Verts", "Sub", "VtxData", "IdxBuf"
        );
        println!("  {}", "-".repeat(100));
        for m in &char_meshes {
            println!(
                "  {:>50} | {:>8} | {:>4} | {:>10} | {:>10} | {}",
                if m.name.len() > 50 { &m.name[..50] } else { &m.name },
                m.vertex_count,
                m.submesh_count,
                format_bytes(m.vertex_data_size as u64),
                format_bytes(m.index_buf_size as u64),
                m.mesh_compression
            );
        }
    }

    // All meshes summary (first 50)
    println!("\n  All meshes (first 50):");
    println!(
        "  {:>50} | {:>8} | {:>4} | {:>10} | {:>10} | Compress",
        "Name", "Verts", "Sub", "VtxData", "IdxBuf"
    );
    println!("  {}", "-".repeat(100));
    for m in meshes.iter().take(50) {
        println!(
            "  {:>50} | {:>8} | {:>4} | {:>10} | {:>10} | {}",
            if m.name.len() > 50 { &m.name[..50] } else { &m.name },
            m.vertex_count,
            m.submesh_count,
            format_bytes(m.vertex_data_size as u64),
            format_bytes(m.index_buf_size as u64),
            m.mesh_compression
        );
    }

    // ══════════════════════════════════════════════════════════════
    // Pass 4: TypeTree Structure Dump
    // ══════════════════════════════════════════════════════════════
    println!("\n╔══════════════════════════════════════════╗");
    println!("║  Pass 4: TypeTree Structure              ║");
    println!("╚══════════════════════════════════════════╝\n");

    let mut dumped_classes: std::collections::HashSet<i32> = std::collections::HashSet::new();

    for class_id in [28, 43] {
        if dumped_classes.contains(&class_id) {
            continue;
        }

        println!("  --- {} (classID={}) ---", class_name(class_id), class_id);

        'tree_search: for chk in &all_chks {
            for sf_bytes in &chk.sf_data {
                let sf = match SerializedFileParser::from_bytes(sf_bytes.clone()) {
                    Ok(sf) => sf,
                    Err(_) => continue,
                };

                if sf.types.iter().any(|t| t.class_id == class_id) {
                    dump_type_tree(&sf, class_id);
                    dumped_classes.insert(class_id);
                    break 'tree_search;
                }
            }
        }

        if !dumped_classes.contains(&class_id) {
            println!("  (classID={} not found in any SerializedFile)", class_id);
        }
        println!();
    }

    // Also dump property names for a sample Texture2D and Mesh via ObjectHandle::read()
    println!("  --- Sample Texture2D properties ---");
    'tex_sample: for chk in &all_chks {
        for sf_bytes in &chk.sf_data {
            let sf = match SerializedFileParser::from_bytes(sf_bytes.clone()) {
                Ok(sf) => sf,
                Err(_) => continue,
            };
            for obj in sf.objects_of_type(28) {
                let handle = ObjectHandle::new(&sf, obj);
                if let Ok(unity_obj) = handle.read() {
                    for name in unity_obj.property_names() {
                        let val = unity_obj.get(name).unwrap();
                        let type_str = match val {
                            UnityValue::Null => "null".to_string(),
                            UnityValue::Bool(b) => format!("bool = {}", b),
                            UnityValue::Integer(i) => format!("int = {}", i),
                            UnityValue::Float(f) => format!("float = {}", f),
                            UnityValue::String(s) => format!("string = {:?}", if s.len() > 50 { &s[..50] } else { s }),
                            UnityValue::Array(a) => format!("array[{}]", a.len()),
                            UnityValue::Bytes(b) => format!("bytes[{}]", b.len()),
                            UnityValue::Object(o) => format!("object{{{} fields}}", o.len()),
                        };
                        println!("    {} : {}", name, type_str);
                    }
                    break 'tex_sample;
                }
            }
        }
    }

    println!("\n  --- Sample Mesh properties ---");
    'mesh_sample: for chk in &all_chks {
        for sf_bytes in &chk.sf_data {
            let sf = match SerializedFileParser::from_bytes(sf_bytes.clone()) {
                Ok(sf) => sf,
                Err(_) => continue,
            };
            for obj in sf.objects_of_type(43) {
                let handle = ObjectHandle::new(&sf, obj);
                if let Ok(unity_obj) = handle.read() {
                    for name in unity_obj.property_names() {
                        let val = unity_obj.get(name).unwrap();
                        let type_str = match val {
                            UnityValue::Null => "null".to_string(),
                            UnityValue::Bool(b) => format!("bool = {}", b),
                            UnityValue::Integer(i) => format!("int = {}", i),
                            UnityValue::Float(f) => format!("float = {}", f),
                            UnityValue::String(s) => format!("string = {:?}", if s.len() > 50 { &s[..50] } else { s }),
                            UnityValue::Array(a) => format!("array[{}]", a.len()),
                            UnityValue::Bytes(b) => format!("bytes[{}]", b.len()),
                            UnityValue::Object(o) => format!("object{{{} fields}}", o.len()),
                        };
                        println!("    {} : {}", name, type_str);
                    }
                    break 'mesh_sample;
                }
            }
        }
    }

    // ══════════════════════════════════════════════════════════════
    // Pass 5: Character Asset Search (optional)
    // ══════════════════════════════════════════════════════════════
    if char_scan {
        println!("\n╔══════════════════════════════════════════╗");
        println!("║  Pass 5: Character Asset Search          ║");
        println!("╚══════════════════════════════════════════╝\n");

        let mut char_assets: HashMap<String, Vec<(i32, String)>> = HashMap::new();

        for chk in &all_chks {
            for sf_bytes in &chk.sf_data {
                let sf = match SerializedFileParser::from_bytes(sf_bytes.clone()) {
                    Ok(sf) => sf,
                    Err(_) => continue,
                };

                for obj in &sf.objects {
                    let handle = ObjectHandle::new(&sf, obj);
                    let name = match handle.peek_name() {
                        Ok(Some(n)) => n,
                        _ => continue,
                    };

                    for id in 2..=29 {
                        let pattern = format!("chr_{:04}", id);
                        if name.to_lowercase().contains(&pattern) {
                            char_assets
                                .entry(pattern)
                                .or_default()
                                .push((obj.type_id, name.clone()));
                            break;
                        }
                    }
                }
            }
        }

        if char_assets.is_empty() {
            println!("  No character assets found in scanned files.");
            println!("  (Try SCAN_ALL=1 CHAR_SCAN=1 to search all .chk files)");
        } else {
            let mut sorted: Vec<_> = char_assets.iter().collect();
            sorted.sort_by_key(|(id, _)| (*id).clone());

            for (char_id, assets) in &sorted {
                println!("  {} ({} assets):", char_id, assets.len());
                for (class_id, name) in assets.iter() {
                    println!("    [{}] {}", class_name(*class_id), name);
                }
            }
        }
    }

    // ══════════════════════════════════════════════════════════════
    // Pass 6: Texture2D Extraction (EXTRACT_TEX=1)
    // ══════════════════════════════════════════════════════════════
    let extract_tex = std::env::var("EXTRACT_TEX").is_ok();
    if extract_tex {
        println!("\n╔══════════════════════════════════════════╗");
        println!("║  Pass 6: Texture2D Extraction            ║");
        println!("╚══════════════════════════════════════════╝\n");

        let out_dir = std::path::Path::new("./output/textures");
        std::fs::create_dir_all(out_dir)?;

        let mut extracted = 0u64;
        let mut failed = 0u64;
        let mut skipped_format = 0u64;
        let mut no_data = 0u64;

        for chk in &all_chks {
            for sf_bytes in &chk.sf_data {
                let sf = match SerializedFileParser::from_bytes(sf_bytes.clone()) {
                    Ok(sf) => sf,
                    Err(_) => continue,
                };

                for obj in sf.objects_of_type(28) {
                    let handle = ObjectHandle::new(&sf, obj);
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
                            resolve_stream_data(&unity_obj, &chk.resources)
                        }
                    };

                let Some(compressed) = pixel_data else {
                    no_data += 1;
                    continue;
                };

                // Decode compressed texture → RGBA32
                let mut rgba_buf = vec![0u32; width * height];
                let decode_result = decode_texture(format, &compressed, width, height, &mut rgba_buf);

                match decode_result {
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

                        let path = out_dir.join(format!("{}.png", sanitize_filename(&name)));
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
                            eprintln!("  FAIL decode {} ({}x{}, fmt={}): {}", name, width, height, format, msg);
                            failed += 1;
                        }
                    }
                }
                }
            }
        }

        println!("\n  Extraction results:");
        println!("    Extracted:       {}", extracted);
        println!("    Decode failed:   {}", failed);
        println!("    Unsupported fmt: {}", skipped_format);
        println!("    No data:         {}", no_data);
    }

    // ══════════════════════════════════════════════════════════════
    // Summary
    // ══════════════════════════════════════════════════════════════
    println!("\n╔══════════════════════════════════════════╗");
    println!("║  Summary                                 ║");
    println!("╚══════════════════════════════════════════╝\n");

    println!("  SerializedFiles parsed: {}", total_sfs);
    println!("  Total objects:          {}", total_objects);
    println!("  Unique class IDs:       {}", class_counts.len());
    println!("  Texture2D objects:      {}", textures.len());
    println!("  Mesh objects:           {}", meshes.len());
    println!(
        "  Character textures:     {}",
        textures.iter().filter(|t| {
            let n = t.name.to_lowercase();
            n.contains("chr_") || n.contains("actor_") || n.contains("t_actor_")
        }).count()
    );
    println!(
        "  Character meshes:       {}",
        meshes.iter().filter(|m| {
            let n = m.name.to_lowercase();
            n.contains("s_actor_") || n.contains("chr_") || n.contains("lod")
        }).count()
    );

    // Inline data vs streamed
    let inline_tex = textures.iter().filter(|t| t.data_size > 0).count();
    let streamed_tex = textures.iter().filter(|t| t.stream_size > 0 && t.data_size == 0).count();
    let no_data_tex = textures.iter().filter(|t| t.data_size == 0 && t.stream_size == 0).count();
    println!("\n  Texture data availability:");
    println!("    Inline data:    {}", inline_tex);
    println!("    Streamed only:  {}", streamed_tex);
    println!("    No data:        {}", no_data_tex);

    // Resource stream stats
    let total_res: usize = all_chks.iter().map(|c| c.resources.len()).sum();
    let total_res_bytes: u64 = all_chks
        .iter()
        .flat_map(|c| c.resources.values())
        .map(|v| v.len() as u64)
        .sum();
    println!("\n  Resource streams:");
    println!("    Total .resS files: {}", total_res);
    println!("    Total .resS data:  {}", format_bytes(total_res_bytes));

    // Sample stream paths
    let mut unique_paths: std::collections::HashSet<String> = std::collections::HashSet::new();
    for t in &textures {
        if !t.stream_path.is_empty() {
            unique_paths.insert(t.stream_path.clone());
        }
    }
    if !unique_paths.is_empty() {
        println!("\n  Sample m_StreamData.path values (first 10):");
        for (i, path) in unique_paths.iter().take(10).enumerate() {
            println!("    [{}] {}", i, path);
        }
    }

    println!("\n=== Done ===");

    Ok(())
}
