//! Unity Mesh decoding and glTF binary export.
use crate::extraction::gamedata::{claim_path, write_file};
use crate::extraction::texture::resolve_stream_data;
use crate::extraction::texture_layout::{self, Placement};
use crate::extraction::utils::sanitize_filename;
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use unity_asset_binary::asset::SerializedFile;
use unity_asset_binary::object::{ObjectHandle, UnityObject};
use unity_asset_core::UnityValue;

/// Mesh data decoded from Unity's vertex and index buffers.
pub struct DecodedMesh {
    pub name: String,
    pub positions: Vec<[f32; 3]>,
    pub normals: Option<Vec<[f32; 3]>>,
    pub tangents: Option<Vec<[f32; 4]>>,
    pub colors: Option<Vec<[f32; 4]>>,
    pub uvs: Vec<Vec<[f32; 2]>>,
    pub submeshes: Vec<Vec<u32>>,
}

#[derive(Clone, Copy)]
struct Channel {
    stream: usize,
    offset: usize,
    format: usize,
    dimension: usize,
    packed: bool,
}

fn object<'a>(
    value: Option<&'a UnityValue>,
    name: &str,
) -> Result<&'a indexmap::IndexMap<String, UnityValue>, String> {
    value
        .and_then(UnityValue::as_object)
        .ok_or_else(|| format!("missing or invalid {name}"))
}
fn integer(map: &indexmap::IndexMap<String, UnityValue>, name: &str) -> Result<i64, String> {
    map.get(name)
        .and_then(UnityValue::as_i64)
        .ok_or_else(|| format!("missing or invalid {name}"))
}
fn bytes(value: Option<&UnityValue>) -> Result<Vec<u8>, String> {
    match value {
        Some(UnityValue::Bytes(b)) => Ok(b.clone()),
        Some(UnityValue::Array(a)) => a
            .iter()
            .map(|v| {
                v.as_i64()
                    .filter(|n| (0..=255).contains(n))
                    .map(|n| n as u8)
                    .ok_or_else(|| "invalid byte array".to_string())
            })
            .collect(),
        _ => Err("missing or invalid byte data".into()),
    }
}
fn format_size(format: usize) -> Option<usize> {
    match format {
        0 | 10 | 11 => Some(4),
        1 | 4 | 5 | 8 | 9 => Some(2),
        2 | 3 | 6 | 7 => Some(1),
        _ => None,
    }
}
fn aligned(n: usize, alignment: usize) -> Result<usize, String> {
    n.checked_add(alignment - 1)
        .map(|v| v / alignment * alignment)
        .ok_or_else(|| "size overflow".into())
}
fn read_at(data: &[u8], off: usize, len: usize) -> Result<&[u8], String> {
    data.get(
        off..off
            .checked_add(len)
            .ok_or_else(|| "buffer offset overflow".to_string())?,
    )
    .ok_or_else(|| "vertex buffer is too short".into())
}

/// Convert an IEEE 754 binary16 bit pattern to f32.
pub(crate) fn half_to_f32(bits: u16) -> f32 {
    let sign = ((bits & 0x8000) as u32) << 16;
    let exponent = (bits >> 10) & 0x1f;
    let fraction = (bits & 0x03ff) as u32;
    let out = if exponent == 0 {
        if fraction == 0 {
            sign
        } else {
            let mut f = fraction;
            let mut e = -14i32;
            while f & 0x400 == 0 {
                f <<= 1;
                e -= 1;
            }
            sign | (((e + 127) as u32) << 23) | ((f & 0x3ff) << 13)
        }
    } else if exponent == 31 {
        sign | 0x7f80_0000 | (fraction << 13)
    } else {
        sign | ((exponent as u32 + 112) << 23) | (fraction << 13)
    };
    f32::from_bits(out)
}
/// Decode one scalar of a Unity VertexFormat.
pub(crate) fn decode_scalar(format: usize, data: &[u8]) -> Result<f32, String> {
    let need = format_size(format).ok_or_else(|| "unsupported vertex format".to_string())?;
    let b = read_at(data, 0, need)?;
    Ok(match format {
        0 => f32::from_le_bytes(b.try_into().unwrap()),
        1 => half_to_f32(u16::from_le_bytes(b.try_into().unwrap())),
        2 => b[0] as f32 / 255.0,
        3 => (b[0] as i8 as f32 / 127.0).max(-1.0),
        4 => u16::from_le_bytes(b.try_into().unwrap()) as f32 / 65535.0,
        5 => (i16::from_le_bytes(b.try_into().unwrap()) as f32 / 32767.0).max(-1.0),
        6 => b[0] as f32,
        7 => b[0] as i8 as f32,
        8 => u16::from_le_bytes(b.try_into().unwrap()) as f32,
        9 => i16::from_le_bytes(b.try_into().unwrap()) as f32,
        10 => u32::from_le_bytes(b.try_into().unwrap()) as f32,
        11 => i32::from_le_bytes(b.try_into().unwrap()) as f32,
        _ => unreachable!(),
    })
}
/// Channel `dimension` high nibble marking a vector packed into one 32-bit word.
const PACKED_10_10_10: usize = 3;

/// Decode a normal packed as three signed-normalized 10-bit fields (x low, z high).
///
/// Verified against fixtures: decoded normals are ~unit length and agree with averaged
/// face normals (median cosine 0.8–0.96), unlike octahedral or unsigned variants.
pub(crate) fn packed_normal(data: &[u8]) -> Result<[f32; 3], String> {
    let word = u32::from_le_bytes(read_at(data, 0, 4)?.try_into().unwrap());
    let snorm = |v: u32| {
        let v = ((v & 0x3ff) as i32 ^ 0x200) - 0x200;
        (v as f32 / 511.0).max(-1.0)
    };
    let n = [snorm(word), snorm(word >> 10), snorm(word >> 20)];
    let length = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
    if length < 1e-6 {
        return Ok([0.0, 1.0, 0.0]);
    }
    Ok(n.map(|c| c / length))
}
fn channel_values(
    data: &[u8],
    channel: Channel,
    starts: &[usize],
    stride: usize,
    count: usize,
) -> Result<Vec<Vec<f32>>, String> {
    let component_size =
        format_size(channel.format).ok_or_else(|| "unsupported vertex format".to_string())?;
    let needed = channel
        .dimension
        .checked_mul(component_size)
        .ok_or_else(|| "vertex layout overflow".to_string())?;
    if channel
        .offset
        .checked_add(needed)
        .ok_or_else(|| "vertex layout overflow".to_string())?
        > stride
    {
        return Err("channel exceeds stream stride".into());
    }
    let start = *starts
        .get(channel.stream)
        .ok_or_else(|| "invalid stream".to_string())?;
    (0..count)
        .map(|i| {
            let off = start
                .checked_add(
                    i.checked_mul(stride)
                        .ok_or_else(|| "vertex offset overflow".to_string())?,
                )
                .and_then(|x| x.checked_add(channel.offset))
                .ok_or_else(|| "vertex offset overflow".to_string())?;
            let src = read_at(data, off, needed)?;
            if channel.packed {
                Ok(packed_normal(src)?.to_vec())
            } else {
                (0..channel.dimension)
                    .map(|c| decode_scalar(channel.format, &src[c * component_size..]))
                    .collect()
            }
        })
        .collect()
}

/// Decode a Unity Mesh object, including streamed vertex data.
pub fn decode_mesh(
    mesh: &UnityObject,
    resources: &HashMap<String, Vec<u8>>,
) -> Result<DecodedMesh, String> {
    if mesh
        .get("m_MeshCompression")
        .and_then(UnityValue::as_i64)
        .unwrap_or(0)
        != 0
    {
        return Err("compressed mesh unsupported".into());
    }
    let vertex = object(mesh.get("m_VertexData"), "m_VertexData")?;
    let count =
        usize::try_from(integer(vertex, "m_VertexCount")?).map_err(|_| "invalid vertex count")?;
    if count == 0 {
        return Err("mesh has no vertices".into());
    }
    let mut data = bytes(vertex.get("m_DataSize")).unwrap_or_default();
    if data.is_empty() {
        data = resolve_stream_data(mesh, resources)
            .ok_or_else(|| "missing vertex data".to_string())?;
    }
    let channels = vertex
        .get("m_Channels")
        .and_then(UnityValue::as_array)
        .ok_or_else(|| "missing m_Channels".to_string())?
        .iter()
        .map(|v| {
            let o = v.as_object().ok_or_else(|| "invalid channel".to_string())?;
            let raw_dimension =
                usize::try_from(integer(o, "dimension")?).map_err(|_| "invalid dimension")?;
            Ok(Channel {
                stream: usize::try_from(integer(o, "stream")?).map_err(|_| "invalid stream")?,
                offset: usize::try_from(integer(o, "offset")?).map_err(|_| "invalid offset")?,
                format: usize::try_from(integer(o, "format")?).map_err(|_| "invalid format")?,
                dimension: raw_dimension & 0x0f,
                packed: raw_dimension >> 4 == PACKED_10_10_10,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    let stream_count = channels.iter().map(|c| c.stream + 1).max().unwrap_or(0);
    let mut strides = vec![0usize; stream_count];
    for c in &channels {
        let sz = format_size(c.format).ok_or_else(|| "unsupported vertex format".to_string())?;
        strides[c.stream] = strides[c.stream]
            .checked_add(
                sz.checked_mul(c.dimension)
                    .ok_or_else(|| "vertex layout overflow".to_string())?,
            )
            .ok_or_else(|| "vertex layout overflow".to_string())?;
    }
    let mut starts = Vec::with_capacity(stream_count);
    let mut running = 0usize;
    for stride in &strides {
        running = aligned(running, 16)?;
        starts.push(running);
        running = running
            .checked_add(
                count
                    .checked_mul(*stride)
                    .ok_or_else(|| "vertex buffer overflow".to_string())?,
            )
            .ok_or_else(|| "vertex buffer overflow".to_string())?;
    }
    if data.len() < running {
        return Err("vertex buffer is too short".into());
    }
    let values = |index: usize| -> Result<Option<Vec<Vec<f32>>>, String> {
        match channels.get(index).copied() {
            Some(c) if c.dimension > 0 => Ok(Some(channel_values(
                &data,
                c,
                &starts,
                strides[c.stream],
                count,
            )?)),
            _ => Ok(None),
        }
    };
    let pos = values(0)?.ok_or_else(|| "mesh has no position channel".to_string())?;
    if pos.iter().any(|v| v.len() < 3) {
        return Err("position channel has fewer than three components".into());
    }
    let positions = pos.into_iter().map(|v| [v[0], v[1], v[2]]).collect();
    let to3 = |v: Vec<Vec<f32>>| v.into_iter().map(|x| [x[0], x[1], x[2]]).collect();
    let normals = match values(1) {
        Ok(Some(v)) if v.iter().all(|x| x.len() >= 3) => Some(to3(v)),
        _ => None,
    };
    let tangents = match values(2)? {
        Some(v) if v.iter().all(|x| x.len() >= 4) => {
            Some(v.into_iter().map(|x| [x[0], x[1], x[2], x[3]]).collect())
        }
        _ => None,
    };
    let colors = match values(3)? {
        Some(v) if v.iter().all(|x| x.len() >= 3) => Some(
            v.into_iter()
                .map(|x| [x[0], x[1], x[2], if x.len() > 3 { x[3] } else { 1.0 }])
                .collect(),
        ),
        _ => None,
    };
    // Channels 4..12 are uv0..uv7; 12 and 13 hold skinning weights and bone indices.
    let mut uvs = Vec::new();
    for i in 4..channels.len().min(12) {
        if let Some(v) = values(i)?
            && v.iter().all(|x| x.len() >= 2)
        {
            uvs.push(v.into_iter().map(|x| [x[0], x[1]]).collect());
        }
    }
    let index_data = bytes(mesh.get("m_IndexBuffer"))?;
    let index_size = match mesh
        .get("m_IndexFormat")
        .and_then(UnityValue::as_i64)
        .unwrap_or(0)
    {
        0 => 2,
        1 => 4,
        _ => return Err("unsupported index format".into()),
    };
    let submeshes = mesh
        .get("m_SubMeshes")
        .and_then(UnityValue::as_array)
        .ok_or_else(|| "missing m_SubMeshes".to_string())?
        .iter()
        .filter_map(|v| v.as_object())
        .filter(|o| integer(o, "topology").ok() == Some(0))
        .map(|o| {
            let first =
                usize::try_from(integer(o, "firstByte")?).map_err(|_| "invalid firstByte")?;
            let n = usize::try_from(integer(o, "indexCount")?).map_err(|_| "invalid indexCount")?;
            let base = integer(o, "baseVertex")?;
            if n % 3 != 0 {
                return Err("triangle index count is not divisible by three".into());
            }
            let end = first
                .checked_add(
                    n.checked_mul(index_size)
                        .ok_or_else(|| "index buffer overflow".to_string())?,
                )
                .ok_or_else(|| "index buffer overflow".to_string())?;
            if end > index_data.len() {
                return Err("index buffer is too short".into());
            }
            (0..n)
                .map(|i| {
                    let off = first + i * index_size;
                    let raw = if index_size == 2 {
                        u16::from_le_bytes(index_data[off..off + 2].try_into().unwrap()) as i64
                    } else {
                        u32::from_le_bytes(index_data[off..off + 4].try_into().unwrap()) as i64
                    };
                    let index = raw
                        .checked_add(base)
                        .ok_or_else(|| "index overflow".to_string())?;
                    if index < 0 || index as usize >= count {
                        return Err("index out of range".into());
                    }
                    Ok(index as u32)
                })
                .collect()
        })
        .collect::<Result<Vec<Vec<u32>>, String>>()?;
    if submeshes.iter().all(Vec::is_empty) {
        return Err("mesh has no triangles".into());
    }
    Ok(DecodedMesh {
        name: mesh
            .get("m_Name")
            .and_then(UnityValue::as_str)
            .unwrap_or("")
            .to_string(),
        positions,
        normals,
        tangents,
        colors,
        uvs,
        submeshes,
    })
}

fn append_f32(bin: &mut Vec<u8>, values: impl IntoIterator<Item = f32>) {
    for value in values {
        bin.extend_from_slice(&value.to_le_bytes());
    }
}
fn pad(bin: &mut Vec<u8>) {
    while !bin.len().is_multiple_of(4) {
        bin.push(0);
    }
}
/// Start a buffer view at the next aligned offset and return its index.
fn view(bin: &mut Vec<u8>, views: &mut Vec<Value>, byte_length: usize) -> usize {
    pad(bin);
    views.push(json!({"buffer":0,"byteOffset":bin.len(),"byteLength":byte_length}));
    views.len() - 1
}
fn accessor(
    accessors: &mut Vec<Value>,
    view: usize,
    component: u32,
    count: usize,
    kind: &str,
    min: Option<[f32; 3]>,
    max: Option<[f32; 3]>,
) -> usize {
    let mut a = json!({"bufferView":view,"componentType":component,"count":count,"type":kind});
    if let Some(x) = min {
        a["min"] = json!(x);
    }
    if let Some(x) = max {
        a["max"] = json!(x);
    }
    accessors.push(a);
    accessors.len() - 1
}

/// Write decoded mesh data as a self-contained glTF 2.0 binary document.
pub fn write_glb(mesh: &DecodedMesh) -> Vec<u8> {
    let mut bin = Vec::new();
    let mut views = Vec::new();
    let mut accessors = Vec::new();
    let mut min = [f32::INFINITY; 3];
    let mut max = [f32::NEG_INFINITY; 3];
    for p in &mesh.positions {
        let p = [-p[0], p[1], p[2]];
        for i in 0..3 {
            min[i] = min[i].min(p[i]);
            max[i] = max[i].max(p[i]);
        }
    }
    let v = view(&mut bin, &mut views, mesh.positions.len() * 12);
    append_f32(
        &mut bin,
        mesh.positions.iter().flat_map(|p| [-p[0], p[1], p[2]]),
    );
    let position = accessor(
        &mut accessors,
        v,
        5126,
        mesh.positions.len(),
        "VEC3",
        Some(min),
        Some(max),
    );
    let mut attributes = serde_json::Map::new();
    attributes.insert("POSITION".into(), json!(position));
    if let Some(ns) = &mesh.normals {
        let v = view(&mut bin, &mut views, ns.len() * 12);
        append_f32(&mut bin, ns.iter().flat_map(|n| [-n[0], n[1], n[2]]));
        let a = accessor(&mut accessors, v, 5126, ns.len(), "VEC3", None, None);
        attributes.insert("NORMAL".into(), json!(a));
    }
    if let Some(ts) = &mesh.tangents {
        let v = view(&mut bin, &mut views, ts.len() * 16);
        append_f32(&mut bin, ts.iter().flat_map(|t| [-t[0], t[1], t[2], -t[3]]));
        let a = accessor(&mut accessors, v, 5126, ts.len(), "VEC4", None, None);
        attributes.insert("TANGENT".into(), json!(a));
    }
    if let Some(cs) = &mesh.colors {
        let v = view(&mut bin, &mut views, cs.len() * 16);
        append_f32(&mut bin, cs.iter().flatten().copied());
        let a = accessor(&mut accessors, v, 5126, cs.len(), "VEC4", None, None);
        attributes.insert("COLOR_0".into(), json!(a));
    }
    for (i, uv) in mesh.uvs.iter().enumerate() {
        let v = view(&mut bin, &mut views, uv.len() * 8);
        append_f32(&mut bin, uv.iter().flat_map(|u| [u[0], 1.0 - u[1]]));
        let a = accessor(&mut accessors, v, 5126, uv.len(), "VEC2", None, None);
        attributes.insert(format!("TEXCOORD_{i}"), json!(a));
    }
    let mut primitives = Vec::new();
    for sub in &mesh.submeshes {
        if sub.is_empty() {
            continue;
        }
        let max_index = *sub.iter().max().unwrap();
        let short = max_index <= u16::MAX as u32;
        let byte_len = sub.len() * if short { 2 } else { 4 };
        let v = view(&mut bin, &mut views, byte_len);
        for tri in sub.as_chunks::<3>().0 {
            for index in [tri[0], tri[2], tri[1]] {
                if short {
                    bin.extend_from_slice(&(index as u16).to_le_bytes());
                } else {
                    bin.extend_from_slice(&index.to_le_bytes());
                }
            }
        }
        let a = accessor(
            &mut accessors,
            v,
            if short { 5123 } else { 5125 },
            sub.len(),
            "SCALAR",
            None,
            None,
        );
        primitives.push(json!({"attributes":attributes,"indices":a,"mode":4}));
    }
    pad(&mut bin);
    let document = json!({"asset":{"version":"2.0","generator":"fluorite"},"buffers":[{"byteLength":bin.len()}],"bufferViews":views,"accessors":accessors,"meshes":[{"name":mesh.name,"primitives":primitives}],"nodes":[{"name":mesh.name,"mesh":0}],"scenes":[{"nodes":[0]}],"scene":0});
    let mut json_bytes = serde_json::to_vec(&document).unwrap();
    while !json_bytes.len().is_multiple_of(4) {
        json_bytes.push(b' ');
    }
    let total = 12 + 8 + json_bytes.len() + 8 + bin.len();
    let mut out = Vec::with_capacity(total);
    out.extend_from_slice(b"glTF");
    out.extend_from_slice(&2u32.to_le_bytes());
    out.extend_from_slice(&(total as u32).to_le_bytes());
    out.extend_from_slice(&(json_bytes.len() as u32).to_le_bytes());
    out.extend_from_slice(b"JSON");
    out.extend_from_slice(&json_bytes);
    out.extend_from_slice(&(bin.len() as u32).to_le_bytes());
    out.extend_from_slice(b"BIN\0");
    out.extend_from_slice(&bin);
    out
}

/// Decode a Unity Mesh and write it as glTF binary.
pub fn mesh_to_glb(
    mesh: &UnityObject,
    resources: &HashMap<String, Vec<u8>>,
) -> Result<Vec<u8>, String> {
    Ok(write_glb(&decode_mesh(mesh, resources)?))
}

pub struct MeshExportResult {
    pub exported: u64,
    /// Meshes that could not be decoded.
    pub failed: u64,
    /// Decoded meshes whose file could not be written.
    pub write_errors: u64,
}

/// Export every Mesh (class 43) in `sf` as `.glb` under `out_dir`, laid out by container path.
pub fn extract_meshes(
    sf: &SerializedFile,
    containers: &HashMap<i64, Placement>,
    resources: &HashMap<String, Vec<u8>>,
    out_dir: &Path,
    seen: &mut HashSet<PathBuf>,
) -> MeshExportResult {
    let mut result = MeshExportResult {
        exported: 0,
        failed: 0,
        write_errors: 0,
    };
    for obj in sf.objects_of_type(43) {
        let Ok(mesh) = ObjectHandle::new(sf, obj).read() else {
            result.failed += 1;
            continue;
        };
        let glb = match mesh_to_glb(&mesh, resources) {
            Ok(glb) => glb,
            // Empty placeholder meshes are common and not worth reporting.
            Err(error) if error == "mesh has no vertices" => continue,
            Err(error) => {
                let name = mesh
                    .get("m_Name")
                    .and_then(UnityValue::as_str)
                    .unwrap_or("");
                eprintln!("  FAIL mesh {name} ({}): {error}", obj.path_id);
                result.failed += 1;
                continue;
            }
        };
        let mut name = mesh
            .get("m_Name")
            .and_then(UnityValue::as_str)
            .map(|n| sanitize_filename(n).replace(['/', '\\'], "_"))
            .unwrap_or_default();
        if name.is_empty() {
            name = format!("mesh_{}", obj.path_id);
        }
        let mut file_name = format!("{name}.glb");
        let dir =
            texture_layout::placement_dir(containers.get(&obj.path_id)).unwrap_or_else(|| {
                // Unrelated chunks share `_unsorted/`; the path ID keeps names stable across runs.
                file_name = format!("{name}__{}.glb", obj.path_id);
                PathBuf::from("_unsorted")
            });
        let Some(path) = claim_path(out_dir.join(dir), &file_name, obj.path_id, seen) else {
            continue;
        };
        if write_file(&path, &glb) {
            result.exported += 1;
        } else {
            result.write_errors += 1;
        }
    }
    result
}
