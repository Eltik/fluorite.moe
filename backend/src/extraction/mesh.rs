//! Unity Mesh decoding and glTF binary export.
use crate::extraction::texture::resolve_stream_data;
use crate::extraction::texture_layout::{self, Placement};
use crate::extraction::unity::{SerializedFile, value_bytes};
use crate::extraction::utils::{flat_filename, write_file};
use serde_json::{Map, Value, json};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

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

fn object<'a>(value: Option<&'a Value>, name: &str) -> Result<&'a Map<String, Value>, String> {
    value
        .and_then(Value::as_object)
        .ok_or_else(|| format!("missing or invalid {name}"))
}
fn integer(map: &Map<String, Value>, name: &str) -> Result<i64, String> {
    map.get(name)
        .and_then(Value::as_i64)
        .ok_or_else(|| format!("missing or invalid {name}"))
}
/// A non-negative integer field, rejected as `invalid <name>` when negative.
fn index_field(map: &Map<String, Value>, name: &str) -> Result<usize, String> {
    usize::try_from(integer(map, name)?).map_err(|_| format!("invalid {name}"))
}
fn bytes(value: Option<&Value>) -> Result<Vec<u8>, String> {
    let value = value.ok_or("missing or invalid byte data")?;
    value_bytes(value).ok_or_else(|| {
        if value.is_array() {
            "invalid byte array".into()
        } else {
            "missing or invalid byte data".into()
        }
    })
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
/// Channel `dimension` high nibble marking a normal packed into one 32-bit word.
const PACKED_NORMAL: usize = 3;

/// Decode a normal packed into one 32-bit word: an octahedral encoding with x in bits 0–9
/// and y in bits 10–19, both signed-normalized. The upper bits hold the tangent frame
/// (bit 31 is the bitangent sign) and are ignored.
///
/// Verified against Endfield meshes: decoded normals agree with averaged face normals to a
/// median of under 1°. Reading the word as three 10-bit components instead is 35–50° off.
pub(crate) fn packed_normal(data: &[u8]) -> Result<[f32; 3], String> {
    let word = u32::from_le_bytes(read_at(data, 0, 4)?.try_into().unwrap());
    let snorm = |v: u32| {
        let v = ((v & 0x3ff) as i32 ^ 0x200) - 0x200;
        (v as f32 / 511.0).max(-1.0)
    };
    Ok(octahedral_unit(snorm(word), snorm(word >> 10)))
}

/// The unit vector of an octahedral encoding with both coordinates in [-1, 1].
pub(crate) fn octahedral_unit(x: f32, y: f32) -> [f32; 3] {
    let z = 1.0 - x.abs() - y.abs();
    let [x, y] = if z < 0.0 {
        [(1.0 - y.abs()).copysign(x), (1.0 - x.abs()).copysign(y)]
    } else {
        [x, y]
    };
    let length = (x * x + y * y + z * z).sqrt();
    if length < 1e-6 {
        return [0.0, 0.0, 1.0];
    }
    [x / length, y / length, z / length]
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

/// Bytes per index of `m_IndexFormat` (0: UInt16, 1: UInt32).
fn index_size(mesh: &Value) -> Result<usize, String> {
    match mesh
        .get("m_IndexFormat")
        .and_then(Value::as_i64)
        .unwrap_or(0)
    {
        0 => Ok(2),
        1 => Ok(4),
        _ => Err("unsupported index format".into()),
    }
}

/// Triangle lists of `m_SubMeshes`, resolved against the mesh's whole index list.
///
/// `firstByte` counts bytes of an `index_size`-wide index buffer, also for compressed meshes
/// whose indices are bit-packed.
fn submeshes(
    mesh: &Value,
    indices: &[u32],
    index_size: usize,
    count: usize,
) -> Result<Vec<Vec<u32>>, String> {
    let submeshes = mesh
        .get("m_SubMeshes")
        .and_then(Value::as_array)
        .ok_or_else(|| "missing m_SubMeshes".to_string())?
        .iter()
        .filter_map(|v| v.as_object())
        .filter(|o| integer(o, "topology").ok() == Some(0))
        .map(|o| {
            let first_byte = index_field(o, "firstByte")?;
            let n = index_field(o, "indexCount")?;
            let base = integer(o, "baseVertex")?;
            if n % 3 != 0 {
                return Err("triangle index count is not divisible by three".into());
            }
            if !first_byte.is_multiple_of(index_size) {
                return Err("misaligned submesh firstByte".into());
            }
            let first = first_byte / index_size;
            let end = first
                .checked_add(n)
                .ok_or_else(|| "index buffer overflow".to_string())?;
            let slice = indices
                .get(first..end)
                .ok_or_else(|| "index buffer is too short".to_string())?;
            slice
                .iter()
                .map(|&raw| {
                    let index = i64::from(raw)
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
    Ok(submeshes)
}

/// Whether `m_CompressedMesh` actually carries vertices.
///
/// Endfield sets m_MeshCompression to values stock Unity never writes (4, 5) on meshes whose
/// m_CompressedMesh is empty and whose vertices are in m_VertexData as usual, so the payload
/// decides, not the flag.
fn has_compressed_vertices(mesh: &Value) -> bool {
    mesh.pointer("/m_CompressedMesh/m_Vertices/m_NumItems")
        .and_then(Value::as_i64)
        .unwrap_or(0)
        > 0
}

/// A Unity `PackedBitVector`: `num_items` unsigned values of `bit_size` bits each, packed
/// least-significant bit first. Float vectors map each value linearly onto
/// `[start, start + range]`.
pub(crate) struct PackedBitVector {
    pub num_items: usize,
    pub bit_size: u32,
    pub data: Vec<u8>,
    pub range: f32,
    pub start: f32,
}

impl PackedBitVector {
    /// Parse a `PackedBitVector` value; `Ok(None)` when it is absent or empty.
    fn from_value(value: Option<&Value>, name: &str) -> Result<Option<Self>, String> {
        let Some(map) = value.and_then(Value::as_object) else {
            return Ok(None);
        };
        let num_items = usize::try_from(map.get("m_NumItems").and_then(Value::as_i64).unwrap_or(0))
            .map_err(|_| format!("invalid {name}"))?;
        if num_items == 0 {
            return Ok(None);
        }
        let bit_size = map
            .get("m_BitSize")
            .and_then(Value::as_u64)
            .and_then(|b| u32::try_from(b).ok())
            .ok_or_else(|| format!("invalid {name}"))?;
        let float = |key: &str| map.get(key).and_then(Value::as_f64).unwrap_or(0.0) as f32;
        Ok(Some(Self {
            num_items,
            bit_size,
            data: bytes(map.get("m_Data")).map_err(|e| format!("{name}: {e}"))?,
            range: float("m_Range"),
            start: float("m_Start"),
        }))
    }

    /// The raw packed values.
    pub fn unpack_ints(&self) -> Result<Vec<u32>, String> {
        if self.bit_size > 32 {
            return Err("packed bit size exceeds 32".into());
        }
        let bits = self.bit_size as usize;
        let needed = self
            .num_items
            .checked_mul(bits)
            .ok_or_else(|| "packed vector overflow".to_string())?
            .div_ceil(8);
        if self.data.len() < needed {
            return Err("packed vector data is too short".into());
        }
        let mut out = Vec::with_capacity(self.num_items);
        let mut pos = 0usize;
        for _ in 0..self.num_items {
            let mut value = 0u64;
            let mut filled = 0usize;
            while filled < bits {
                let shift = pos % 8;
                let take = (bits - filled).min(8 - shift);
                let chunk = (u64::from(self.data[pos / 8]) >> shift) & ((1u64 << take) - 1);
                value |= chunk << filled;
                filled += take;
                pos += take;
            }
            out.push(value as u32);
        }
        Ok(out)
    }

    /// The values dequantized to floats.
    pub fn unpack_floats(&self) -> Result<Vec<f32>, String> {
        let max = (1u64 << self.bit_size.min(32)) - 1;
        Ok(self
            .unpack_ints()?
            .into_iter()
            .map(|x| {
                if max == 0 {
                    self.start
                } else {
                    (f64::from(x) * f64::from(self.range) / max as f64) as f32 + self.start
                }
            })
            .collect())
    }
}

/// Rebuild a unit vector from its x and y and the sign of its z.
///
/// `x² + y² > 1` (quantization error) leaves z at 0 and renormalizes x and y instead.
pub(crate) fn reconstruct_unit(x: f32, y: f32, z_positive: bool) -> [f32; 3] {
    let z_squared = 1.0 - x * x - y * y;
    let [x, y, z] = if z_squared >= 0.0 {
        [x, y, z_squared.sqrt()]
    } else {
        let length = (x * x + y * y).sqrt();
        if length < 1e-6 {
            [0.0, 0.0, 1.0]
        } else {
            [x / length, y / length, 0.0]
        }
    };
    [x, y, if z_positive { z } else { -z }]
}

/// Bits per texture coordinate channel in `m_UVInfo`.
const UV_INFO_BITS: u32 = 4;
/// `m_UVInfo` bits holding a channel's dimension minus one.
const UV_DIMENSION_MASK: i64 = 3;
/// `m_UVInfo` bit marking a channel as present.
const UV_CHANNEL_EXISTS: i64 = 4;

/// Split the flat `m_UV` floats into per-channel `[u, v]` lists.
///
/// With `uv_info` set (Unity 2017.4+), channels are stored one after another, each
/// `dimension * count` floats; channels of dimension 1 get v = 0 and components past the
/// second are dropped. Without it, the first `2 * count` floats are uv0 and the next uv1.
pub(crate) fn split_uvs(floats: &[f32], uv_info: i64, count: usize) -> Vec<Vec<[f32; 2]>> {
    let mut uvs = Vec::new();
    let mut offset = 0usize;
    let channels: Vec<usize> = if uv_info == 0 {
        (0..2)
            .take_while(|i| floats.len() >= (i + 1) * 2 * count)
            .map(|_| 2)
            .collect()
    } else {
        (0..8)
            .map(|i| (uv_info >> (i * UV_INFO_BITS)) & 0xf)
            .filter(|bits| bits & UV_CHANNEL_EXISTS != 0)
            .map(|bits| 1 + (bits & UV_DIMENSION_MASK) as usize)
            .collect()
    };
    for dimension in channels {
        let Some(values) = floats.get(offset..offset + dimension * count) else {
            break;
        };
        uvs.push(
            values
                .chunks_exact(dimension)
                .map(|c| [c[0], c.get(1).copied().unwrap_or(0.0)])
                .collect(),
        );
        offset += dimension * count;
    }
    uvs
}

fn compressed_field<'a>(mesh: &'a Value, name: &str) -> Option<&'a Value> {
    mesh.get("m_CompressedMesh").and_then(|cm| cm.get(name))
}

fn packed_field(mesh: &Value, name: &str) -> Result<Option<PackedBitVector>, String> {
    PackedBitVector::from_value(compressed_field(mesh, name), name)
}

/// Reconstructed unit vectors and the raw sign bits they were read with.
type UnitsWithSigns = (Vec<[f32; 3]>, Vec<u32>);

/// Unit vectors stored as packed (x, y) pairs plus one z-sign bit per vector, where the sign
/// of vector `i` is `signs[i * sign_stride]`. `None` when absent or not one per vertex.
fn compressed_units(
    mesh: &Value,
    name: &str,
    signs_name: &str,
    sign_stride: usize,
    count: usize,
) -> Result<Option<UnitsWithSigns>, String> {
    let Some(packed) = packed_field(mesh, name)? else {
        return Ok(None);
    };
    let signs = packed_field(mesh, signs_name)?
        .map(|s| s.unpack_ints())
        .transpose()?
        .unwrap_or_default();
    if packed.num_items != count * 2 || signs.len() < count * sign_stride {
        return Ok(None);
    }
    let units = packed
        .unpack_floats()?
        .as_chunks::<2>()
        .0
        .iter()
        .enumerate()
        .map(|(i, [x, y])| reconstruct_unit(*x, *y, signs[i * sign_stride] != 0))
        .collect();
    Ok(Some((units, signs)))
}

/// Decode a mesh whose geometry lives in `m_CompressedMesh` (the import-time "Mesh
/// Compression" setting), as AssetStudio and UnityPy do.
fn decode_compressed_mesh(mesh: &Value) -> Result<DecodedMesh, String> {
    let vertices =
        packed_field(mesh, "m_Vertices")?.ok_or_else(|| "mesh has no vertices".to_string())?;
    if !vertices.num_items.is_multiple_of(3) {
        return Err("compressed vertex count is not divisible by three".into());
    }
    let count = vertices.num_items / 3;
    let positions = vertices.unpack_floats()?.as_chunks::<3>().0.to_vec();
    let uvs = match packed_field(mesh, "m_UV")? {
        Some(uv) => {
            let uv_info = compressed_field(mesh, "m_UVInfo")
                .and_then(Value::as_i64)
                .unwrap_or(0);
            split_uvs(&uv.unpack_floats()?, uv_info, count)
        }
        None => Vec::new(),
    };
    let normals = compressed_units(mesh, "m_Normals", "m_NormalSigns", 1, count)?.map(|(n, _)| n);
    // Tangent signs alternate: z sign, then handedness (w).
    let tangents =
        compressed_units(mesh, "m_Tangents", "m_TangentSigns", 2, count)?.map(|(units, signs)| {
            units
                .iter()
                .enumerate()
                .map(|(i, t)| {
                    [
                        t[0],
                        t[1],
                        t[2],
                        if signs[i * 2 + 1] != 0 { 1.0 } else { -1.0 },
                    ]
                })
                .collect()
        });
    let colors = match packed_field(mesh, "m_FloatColors")? {
        Some(c) if c.num_items == count * 4 => Some(c.unpack_floats()?.as_chunks::<4>().0.to_vec()),
        _ => None,
    };
    let indices = packed_field(mesh, "m_Triangles")?
        .ok_or_else(|| "mesh has no triangles".to_string())?
        .unpack_ints()?;
    let submeshes = submeshes(mesh, &indices, index_size(mesh)?, count)?;
    Ok(DecodedMesh {
        name: mesh
            .get("m_Name")
            .and_then(Value::as_str)
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

/// Skin influences of a compressed mesh: per vertex up to four bone indices and weights.
///
/// Weights are 5-bit (sum 31) and listed per vertex until they reach 31; a vertex with three
/// listed weights short of 31 gets the remainder as an implicit fourth. Every listed weight,
/// implicit ones included, consumes one entry of `m_BoneIndices`. `Ok(None)` when the mesh
/// has no compressed weights.
pub fn decode_compressed_skin(mesh: &Value) -> Result<Option<CompressedSkin>, String> {
    if !has_compressed_vertices(mesh) {
        return Ok(None);
    }
    let Some(weights) = packed_field(mesh, "m_Weights")? else {
        return Ok(None);
    };
    let weights = weights.unpack_ints()?;
    let bones = packed_field(mesh, "m_BoneIndices")?
        .map(|b| b.unpack_ints())
        .transpose()?
        .unwrap_or_default();
    let mut bones = bones.into_iter();
    let mut next_bone = || -> Result<u16, String> {
        let bone = bones
            .next()
            .ok_or_else(|| "compressed bone indices are too short".to_string())?;
        u16::try_from(bone).map_err(|_| format!("bone index {bone} out of range"))
    };
    let mut skin = CompressedSkin::default();
    let (mut joints, mut values, mut slot, mut sum) = ([0u16; 4], [0f32; 4], 0usize, 0u32);
    for weight in weights {
        joints[slot] = next_bone()?;
        values[slot] = weight as f32 / 31.0;
        slot += 1;
        sum += weight;
        if sum >= 31 || slot == 3 {
            if sum < 31 {
                joints[3] = next_bone()?;
                values[3] = (31 - sum) as f32 / 31.0;
            }
            skin.joints.push(joints);
            skin.weights.push(values);
            (joints, values, slot, sum) = ([0; 4], [0.0; 4], 0, 0);
        }
    }
    let vertices = packed_field(mesh, "m_Vertices")?.map_or(0, |v| v.num_items / 3);
    if skin.joints.len() != vertices {
        return Err(format!(
            "compressed skin covers {} of {vertices} vertices",
            skin.joints.len()
        ));
    }
    Ok(Some(skin))
}

/// Per-vertex skin influences decoded from `m_CompressedMesh`.
#[derive(Debug, Default, PartialEq)]
pub struct CompressedSkin {
    pub joints: Vec<[u16; 4]>,
    pub weights: Vec<[f32; 4]>,
}

/// Decode a Unity Mesh object, including streamed vertex data.
pub fn decode_mesh(
    mesh: &Value,
    resources: &HashMap<String, Vec<u8>>,
) -> Result<DecodedMesh, String> {
    if has_compressed_vertices(mesh) {
        return decode_compressed_mesh(mesh);
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
        .and_then(Value::as_array)
        .ok_or_else(|| "missing m_Channels".to_string())?
        .iter()
        .map(|v| {
            let o = v.as_object().ok_or_else(|| "invalid channel".to_string())?;
            let raw_dimension = index_field(o, "dimension")?;
            Ok(Channel {
                stream: index_field(o, "stream")?,
                offset: index_field(o, "offset")?,
                format: index_field(o, "format")?,
                dimension: raw_dimension & 0x0f,
                packed: raw_dimension >> 4 == PACKED_NORMAL,
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
    let index_size = index_size(mesh)?;
    let indices: Vec<u32> = if index_size == 2 {
        index_data
            .as_chunks::<2>()
            .0
            .iter()
            .map(|b| u32::from(u16::from_le_bytes(*b)))
            .collect()
    } else {
        index_data
            .as_chunks::<4>()
            .0
            .iter()
            .map(|b| u32::from_le_bytes(*b))
            .collect()
    };
    let submeshes = submeshes(mesh, &indices, index_size, count)?;
    Ok(DecodedMesh {
        name: mesh
            .get("m_Name")
            .and_then(Value::as_str)
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

const COMPONENT_FLOAT: u32 = 5126;
const COMPONENT_UNSIGNED_SHORT: u32 = 5123;
const COMPONENT_UNSIGNED_INT: u32 = 5125;

/// Binary chunk, buffer views, and accessors of a glTF document under construction.
#[derive(Default)]
struct GlbBuffers {
    bin: Vec<u8>,
    views: Vec<Value>,
    accessors: Vec<Value>,
}

impl GlbBuffers {
    fn pad(&mut self) {
        while !self.bin.len().is_multiple_of(4) {
            self.bin.push(0);
        }
    }

    /// Start a buffer view at the next aligned offset and return its index.
    fn view(&mut self, byte_length: usize) -> usize {
        self.pad();
        self.views
            .push(json!({"buffer":0,"byteOffset":self.bin.len(),"byteLength":byte_length}));
        self.views.len() - 1
    }

    fn accessor(
        &mut self,
        view: usize,
        component: u32,
        count: usize,
        kind: &str,
        bounds: Option<([f32; 3], [f32; 3])>,
    ) -> usize {
        let mut a = json!({"bufferView":view,"componentType":component,"count":count,"type":kind});
        if let Some((min, max)) = bounds {
            a["min"] = json!(min);
            a["max"] = json!(max);
        }
        self.accessors.push(a);
        self.accessors.len() - 1
    }

    /// Append `count` float vectors of `components` each as a new view and return its accessor.
    fn float_accessor(
        &mut self,
        count: usize,
        components: usize,
        kind: &str,
        values: impl IntoIterator<Item = f32>,
        bounds: Option<([f32; 3], [f32; 3])>,
    ) -> usize {
        let view = self.view(count * components * 4);
        for value in values {
            self.bin.extend_from_slice(&value.to_le_bytes());
        }
        self.accessor(view, COMPONENT_FLOAT, count, kind, bounds)
    }
}

/// Write decoded mesh data as a self-contained glTF 2.0 binary document.
///
/// Unity is left-handed: X is mirrored and triangle winding reversed to convert to glTF.
pub fn write_glb(mesh: &DecodedMesh) -> Vec<u8> {
    let mut buffers = GlbBuffers::default();
    let positions = || mesh.positions.iter().map(|p| [-p[0], p[1], p[2]]);
    let mut min = [f32::INFINITY; 3];
    let mut max = [f32::NEG_INFINITY; 3];
    for p in positions() {
        for i in 0..3 {
            min[i] = min[i].min(p[i]);
            max[i] = max[i].max(p[i]);
        }
    }
    let position = buffers.float_accessor(
        mesh.positions.len(),
        3,
        "VEC3",
        positions().flatten(),
        Some((min, max)),
    );
    let mut attributes = serde_json::Map::new();
    attributes.insert("POSITION".into(), json!(position));
    if let Some(ns) = &mesh.normals {
        let values = ns.iter().flat_map(|n| [-n[0], n[1], n[2]]);
        let a = buffers.float_accessor(ns.len(), 3, "VEC3", values, None);
        attributes.insert("NORMAL".into(), json!(a));
    }
    if let Some(ts) = &mesh.tangents {
        let values = ts.iter().flat_map(|t| [-t[0], t[1], t[2], -t[3]]);
        let a = buffers.float_accessor(ts.len(), 4, "VEC4", values, None);
        attributes.insert("TANGENT".into(), json!(a));
    }
    if let Some(cs) = &mesh.colors {
        let a = buffers.float_accessor(cs.len(), 4, "VEC4", cs.iter().flatten().copied(), None);
        attributes.insert("COLOR_0".into(), json!(a));
    }
    for (i, uv) in mesh.uvs.iter().enumerate() {
        let values = uv.iter().flat_map(|u| [u[0], 1.0 - u[1]]);
        let a = buffers.float_accessor(uv.len(), 2, "VEC2", values, None);
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
        let v = buffers.view(byte_len);
        for tri in sub.as_chunks::<3>().0 {
            for index in [tri[0], tri[2], tri[1]] {
                if short {
                    buffers.bin.extend_from_slice(&(index as u16).to_le_bytes());
                } else {
                    buffers.bin.extend_from_slice(&index.to_le_bytes());
                }
            }
        }
        let component = if short {
            COMPONENT_UNSIGNED_SHORT
        } else {
            COMPONENT_UNSIGNED_INT
        };
        let a = buffers.accessor(v, component, sub.len(), "SCALAR", None);
        primitives.push(json!({"attributes":attributes,"indices":a,"mode":4}));
    }
    buffers.pad();
    let GlbBuffers {
        bin,
        views,
        accessors,
    } = buffers;
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
pub fn mesh_to_glb(mesh: &Value, resources: &HashMap<String, Vec<u8>>) -> Result<Vec<u8>, String> {
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
        let mesh = match sf.read_object(obj) {
            Ok(mesh) => mesh,
            // Endfield writes some meshes (seen on collision-only ones) without the
            // m_CompressedMesh bytes their type tree declares, so the full read fails. Every
            // field decode_mesh needs, short of streamed vertex data, precedes it.
            Err(_) => match sf.read_prefix(obj, &["m_VertexData"]) {
                Ok(Some(fields)) => Value::Object(fields),
                _ => {
                    result.failed += 1;
                    continue;
                }
            },
        };
        let mesh_name = mesh.get("m_Name").and_then(Value::as_str);
        let glb = match mesh_to_glb(&mesh, resources) {
            Ok(glb) => glb,
            // Empty placeholder meshes are common and not worth reporting.
            Err(error) if error == "mesh has no vertices" => continue,
            Err(error) => {
                let name = mesh_name.unwrap_or("");
                eprintln!("  FAIL mesh {name} ({}): {error}", obj.path_id);
                result.failed += 1;
                continue;
            }
        };
        let mut name = mesh_name.map(flat_filename).unwrap_or_default();
        if name.is_empty() {
            name = format!("mesh_{}", obj.path_id);
        }
        let Some(path) = texture_layout::claim_object_path(
            out_dir,
            containers.get(&obj.path_id),
            PathBuf::from("_unsorted"),
            &format!("{name}.glb"),
            obj.path_id,
            seen,
        ) else {
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
