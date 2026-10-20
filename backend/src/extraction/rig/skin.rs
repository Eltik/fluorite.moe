//! Skinning data of a Unity Mesh: bone weights/indices (vertex channels 12/13) and bind poses.
use super::convert::{RowMajor, normalize_influences};
use crate::extraction::mesh::{CompressedSkin, decode_compressed_skin, decode_scalar};
use crate::extraction::texture::resolve_stream_data;
use crate::extraction::unity::value_bytes;
use serde_json::Value;
use std::collections::HashMap;

/// Vertex channel holding blend weights.
const CHANNEL_WEIGHTS: usize = 12;
/// Vertex channel holding blend indices.
const CHANNEL_INDICES: usize = 13;

/// Per-vertex influences (at most four) with weights normalized to sum to 1.
#[derive(Debug, Clone, PartialEq)]
pub struct SkinWeights {
    pub joints: Vec<[u16; 4]>,
    pub weights: Vec<[f32; 4]>,
    /// Influences per vertex stored in the mesh (1–4).
    pub influences: usize,
    /// Vertices whose weights did not sum to ~1 before normalization.
    pub renormalized: usize,
    /// Vertices with no influence at all (bound to joint 0).
    pub unweighted: usize,
}

#[derive(Clone, Copy, Debug)]
struct Channel {
    stream: usize,
    offset: usize,
    format: usize,
    dimension: usize,
}

fn format_size(format: usize) -> Option<usize> {
    match format {
        0 | 10 | 11 => Some(4),
        1 | 4 | 5 | 8 | 9 => Some(2),
        2 | 3 | 6 | 7 => Some(1),
        _ => None,
    }
}

fn channels(vertex: &Value) -> Option<Vec<Channel>> {
    vertex
        .get("m_Channels")?
        .as_array()?
        .iter()
        .map(|c| {
            Some(Channel {
                stream: usize::try_from(c.get("stream")?.as_u64()?).ok()?,
                offset: usize::try_from(c.get("offset")?.as_u64()?).ok()?,
                format: usize::try_from(c.get("format")?.as_u64()?).ok()?,
                dimension: usize::try_from(c.get("dimension")?.as_u64()?).ok()? & 0x0f,
            })
        })
        .collect()
}

/// The raw vertex buffer of a mesh: inline `m_DataSize` bytes, or its streamed data.
pub fn vertex_bytes(mesh: &Value, resources: &HashMap<String, Vec<u8>>) -> Option<Vec<u8>> {
    let inline = mesh
        .pointer("/m_VertexData/m_DataSize")
        .and_then(value_bytes)
        .unwrap_or_default();
    if !inline.is_empty() {
        return Some(inline);
    }
    resolve_stream_data(mesh, resources)
}

/// Decode channels 12/13 into normalized four-influence skin data.
///
/// `Ok(None)` when the mesh has no blend index channel (not skinned through vertex data).
pub fn decode_skin_weights(
    mesh: &Value,
    resources: &HashMap<String, Vec<u8>>,
) -> Result<Option<SkinWeights>, String> {
    if let Some(skin) = decode_compressed_skin(mesh)? {
        return Ok(Some(from_compressed(skin)));
    }
    let vertex = mesh.get("m_VertexData").ok_or("missing m_VertexData")?;
    let count = vertex
        .get("m_VertexCount")
        .and_then(Value::as_u64)
        .ok_or("missing m_VertexCount")? as usize;
    let chans = channels(vertex).ok_or("invalid m_Channels")?;
    let Some(indices) = chans
        .get(CHANNEL_INDICES)
        .copied()
        .filter(|c| c.dimension > 0)
    else {
        return Ok(None);
    };
    let weights = chans
        .get(CHANNEL_WEIGHTS)
        .copied()
        .filter(|c| c.dimension > 0);
    let data = vertex_bytes(mesh, resources).ok_or("missing vertex data")?;
    decode_with_layout(&chans, indices, weights, &data, count).map(Some)
}

/// Normalize influences decoded from `m_CompressedMesh` (always four slots, 5-bit weights).
fn from_compressed(skin: CompressedSkin) -> SkinWeights {
    let mut out = SkinWeights {
        joints: Vec::with_capacity(skin.joints.len()),
        weights: Vec::with_capacity(skin.weights.len()),
        influences: 4,
        renormalized: 0,
        unweighted: 0,
    };
    for (mut j, mut w) in skin.joints.into_iter().zip(skin.weights) {
        let sum: f32 = w.iter().sum();
        if sum <= 0.0 {
            out.unweighted += 1;
        } else if (sum - 1.0).abs() > 1e-3 {
            out.renormalized += 1;
        }
        normalize_influences(&mut j, &mut w, 0);
        out.joints.push(j);
        out.weights.push(w);
    }
    out
}

fn decode_with_layout(
    chans: &[Channel],
    indices: Channel,
    weights: Option<Channel>,
    data: &[u8],
    count: usize,
) -> Result<SkinWeights, String> {
    let overflow = || "vertex layout overflow".to_string();
    // Unity has at most four vertex streams; anything else is corrupt data.
    let stream_count = chans.iter().map(|c| c.stream + 1).max().unwrap_or(0);
    if stream_count > 4 {
        return Err(format!("invalid vertex stream count {stream_count}"));
    }
    let mut strides = vec![0usize; stream_count];
    for c in chans {
        let size = format_size(c.format).ok_or("unsupported vertex format")?;
        let bytes = size.checked_mul(c.dimension).ok_or_else(overflow)?;
        strides[c.stream] = strides[c.stream].checked_add(bytes).ok_or_else(overflow)?;
    }
    let mut starts = Vec::with_capacity(stream_count);
    let mut running = 0usize;
    for stride in &strides {
        running = running.checked_next_multiple_of(16).ok_or_else(overflow)?;
        starts.push(running);
        let len = count.checked_mul(*stride).ok_or_else(overflow)?;
        running = running.checked_add(len).ok_or_else(overflow)?;
    }
    if data.len() < running {
        return Err("vertex buffer is too short".into());
    }
    let read = |c: Channel, vertex: usize, component: usize| -> Result<f32, String> {
        let size = format_size(c.format).ok_or("unsupported vertex format")?;
        let off = vertex
            .checked_mul(strides[c.stream])
            .and_then(|o| o.checked_add(starts[c.stream]))
            .and_then(|o| o.checked_add(c.offset))
            .and_then(|o| o.checked_add(component.checked_mul(size)?))
            .ok_or_else(overflow)?;
        decode_scalar(
            c.format,
            data.get(off..).ok_or("vertex buffer is too short")?,
        )
    };
    let influences = indices.dimension.min(4);
    let mut out = SkinWeights {
        joints: Vec::with_capacity(count),
        weights: Vec::with_capacity(count),
        influences,
        renormalized: 0,
        unweighted: 0,
    };
    for v in 0..count {
        let mut j = [0u16; 4];
        let mut w = [0f32; 4];
        for (k, slot) in j.iter_mut().enumerate().take(influences) {
            let raw = read(indices, v, k)?;
            if !(0.0..=f32::from(u16::MAX)).contains(&raw) {
                return Err(format!("bone index {raw} out of range"));
            }
            *slot = raw as u16;
        }
        match weights {
            Some(wc) => {
                for (k, slot) in w.iter_mut().enumerate().take(wc.dimension.min(4)) {
                    *slot = read(wc, v, k)?;
                }
                // Unity may drop the last weight; it is whatever the others leave.
                if wc.dimension < influences {
                    let partial: f32 = w.iter().take(wc.dimension).sum();
                    w[wc.dimension] = (1.0 - partial).max(0.0);
                }
            }
            // A single influence needs no weight channel.
            None => w[0] = 1.0,
        }
        let sum: f32 = w.iter().sum();
        if sum <= 0.0 {
            out.unweighted += 1;
        } else if (sum - 1.0).abs() > 1e-3 {
            out.renormalized += 1;
        }
        normalize_influences(&mut j, &mut w, 0);
        out.joints.push(j);
        out.weights.push(w);
    }
    Ok(out)
}

/// A mesh's `m_BindPose` as row-major matrices.
#[must_use]
pub fn bind_poses(mesh: &Value) -> Vec<RowMajor> {
    mesh.get("m_BindPose")
        .and_then(Value::as_array)
        .map(|items| items.iter().map(matrix_value).collect())
        .unwrap_or_default()
}

/// Parse a Unity `Matrix4x4f` (`e00`..`e33`, row then column).
#[must_use]
pub fn matrix_value(value: &Value) -> RowMajor {
    let mut m = [[0.0f32; 4]; 4];
    for (r, row) in m.iter_mut().enumerate() {
        for (c, cell) in row.iter_mut().enumerate() {
            *cell = value
                .get(format!("e{r}{c}"))
                .and_then(Value::as_f64)
                .unwrap_or(if r == c { 1.0 } else { 0.0 }) as f32;
        }
    }
    m
}

#[cfg(test)]
pub(crate) fn decode_for_test(
    chans: &[(usize, usize, usize, usize)],
    data: &[u8],
    count: usize,
) -> Result<SkinWeights, String> {
    let chans: Vec<Channel> = chans
        .iter()
        .map(|&(stream, offset, format, dimension)| Channel {
            stream,
            offset,
            format,
            dimension,
        })
        .collect();
    let indices = chans[CHANNEL_INDICES];
    let weights = Some(chans[CHANNEL_WEIGHTS]).filter(|c| c.dimension > 0);
    decode_with_layout(&chans, indices, weights, data, count)
}
