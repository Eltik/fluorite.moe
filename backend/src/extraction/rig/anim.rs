//! AnimationClip → glTF animation.
//!
//! Endfield clips keep their curves in `m_AclCompressedBuffer` (stock ACL v2.1.0 buffers) and
//! leave Unity's dense/streamed clip arrays empty. The clip's generic bindings name the
//! curves: one position (attribute 1), rotation (2) or scale (3) binding per animated
//! Transform, keyed by the CRC32 of the Transform's path below the Animator. ACL transform
//! track `i` is the `i`-th distinct Transform path in binding order. `m_ConstantIndexs` /
//! `m_ConstantValues` repeat values already in the ACL output (verified to ~1e-4), so they
//! are not needed.
use super::acl::{self, TRANSFORM_STRIDE};
use super::convert;
use super::gltf::Builder;
use crate::extraction::unity::value_bytes;
use serde_json::{Value, json};
use std::collections::HashMap;

const TYPE_TRANSFORM: i64 = 4;
const ATTR_POSITION: i64 = 1;
const ATTR_ROTATION: i64 = 2;
const ATTR_SCALE: i64 = 3;

/// Which TRS components a Transform path is animated on, in ACL track order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrackBinding {
    pub path_hash: u32,
    pub position: bool,
    pub rotation: bool,
    pub scale: bool,
}

/// Group a clip's generic bindings into one entry per Transform path, in first-seen order
/// (which is ACL's transform track order).
#[must_use]
pub fn transform_bindings(bindings: &[Value]) -> Vec<TrackBinding> {
    let mut out: Vec<TrackBinding> = Vec::new();
    let mut index: HashMap<u32, usize> = HashMap::new();
    for b in bindings {
        if b.get("typeID").and_then(Value::as_i64) != Some(TYPE_TRANSFORM) {
            continue;
        }
        let Some(hash) = b.get("path").and_then(Value::as_u64).map(|h| h as u32) else {
            continue;
        };
        let i = *index.entry(hash).or_insert_with(|| {
            out.push(TrackBinding {
                path_hash: hash,
                position: false,
                rotation: false,
                scale: false,
            });
            out.len() - 1
        });
        match b.get("attribute").and_then(Value::as_i64) {
            Some(ATTR_POSITION) => out[i].position = true,
            Some(ATTR_ROTATION) => out[i].rotation = true,
            Some(ATTR_SCALE) => out[i].scale = true,
            _ => {}
        }
    }
    out
}

/// CRC-32 (IEEE), as Unity uses for binding path hashes.
#[must_use]
pub fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &b in bytes {
        crc ^= u32::from(b);
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

/// Decoded transform curves of one clip, in Unity space.
pub struct DecodedClip {
    pub name: String,
    pub sample_rate: f32,
    pub num_samples: usize,
    pub tracks: Vec<TrackBinding>,
    /// `samples[(sample * tracks + track) * TRANSFORM_STRIDE + k]`.
    pub samples: Vec<f32>,
}

/// Decode an AnimationClip object's ACL transform tracks.
pub fn decode_clip(clip: &Value) -> Result<DecodedClip, String> {
    let name = clip
        .get("m_Name")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let buffer = clip
        .pointer("/m_AclCompressedBuffer/TransformBufferData")
        .and_then(value_bytes)
        .filter(|b| !b.is_empty())
        .ok_or("clip has no ACL transform buffer (plain Mecanim clips are not supported)")?;
    let bindings = clip
        .pointer("/m_ClipBindingConstant/genericBindings")
        .and_then(Value::as_array)
        .ok_or("clip has no generic bindings")?;
    let tracks = transform_bindings(bindings);
    let (info, samples) = acl::transforms(&buffer)?;
    if info.num_tracks as usize != tracks.len() {
        return Err(format!(
            "ACL has {} transform tracks but bindings name {} paths",
            info.num_tracks,
            tracks.len()
        ));
    }
    Ok(DecodedClip {
        name,
        sample_rate: info.sample_rate,
        num_samples: info.num_samples as usize,
        tracks,
        samples,
    })
}

/// Flip quaternions so consecutive keys stay in the same hemisphere (glTF slerps the short
/// way only if neighbours agree in sign).
pub fn make_continuous(keys: &mut [[f32; 4]]) {
    for i in 1..keys.len() {
        let dot: f32 = (0..4).map(|k| keys[i][k] * keys[i - 1][k]).sum();
        if dot < 0.0 {
            keys[i] = keys[i].map(|x| -x);
        }
    }
}

/// True when every key equals the first within `eps`.
#[must_use]
pub fn is_constant<const N: usize>(keys: &[[f32; N]], eps: f32) -> bool {
    keys.iter()
        .all(|k| k.iter().zip(&keys[0]).all(|(a, b)| (a - b).abs() <= eps))
}

/// Counts reported for one embedded animation.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct AnimStats {
    pub name: String,
    pub duration: f32,
    pub channels: usize,
    pub matched_tracks: usize,
    pub unmatched_tracks: usize,
}

/// Append a decoded clip to the document as a glTF animation targeting `nodes` (path hash →
/// node index). Channels whose keys never change are reduced to a single key.
pub fn add_animation(
    builder: &mut Builder,
    animations: &mut Vec<Value>,
    clip: &DecodedClip,
    nodes: &HashMap<u32, usize>,
) -> AnimStats {
    let n = clip.tracks.len();
    let count = clip.num_samples.max(1);
    let times: Vec<[f32; 1]> = (0..count).map(|s| [s as f32 / clip.sample_rate]).collect();
    let mut stats = AnimStats {
        name: clip.name.clone(),
        duration: times.last().map_or(0.0, |t| t[0]),
        ..AnimStats::default()
    };
    let full_input = builder.floats(&times, "SCALAR", None, true);
    let single_input = builder.floats(&[[0.0f32]], "SCALAR", None, true);
    let mut samplers = Vec::new();
    let mut channels = Vec::new();
    let value = |s: usize, t: usize, k: usize| clip.samples[(s * n + t) * TRANSFORM_STRIDE + k];
    for (t, track) in clip.tracks.iter().enumerate() {
        let Some(&node) = nodes.get(&track.path_hash) else {
            stats.unmatched_tracks += 1;
            continue;
        };
        stats.matched_tracks += 1;
        let mut push = |path: &str, output: usize, constant: bool| {
            samplers.push(json!({"input": if constant { single_input } else { full_input }, "output": output, "interpolation": "LINEAR"}));
            channels.push(
                json!({"sampler": samplers.len() - 1, "target": {"node": node, "path": path}}),
            );
        };
        if track.rotation {
            let mut keys: Vec<[f32; 4]> = (0..count)
                .map(|s| {
                    convert::rotation([
                        value(s, t, 0),
                        value(s, t, 1),
                        value(s, t, 2),
                        value(s, t, 3),
                    ])
                })
                .collect();
            make_continuous(&mut keys);
            let constant = is_constant(&keys, 1e-6);
            let out = builder.floats(
                if constant { &keys[..1] } else { &keys },
                "VEC4",
                None,
                false,
            );
            push("rotation", out, constant);
        }
        if track.position {
            let keys: Vec<[f32; 3]> = (0..count)
                .map(|s| convert::position([value(s, t, 4), value(s, t, 5), value(s, t, 6)]))
                .collect();
            let constant = is_constant(&keys, 1e-6);
            let out = builder.floats(
                if constant { &keys[..1] } else { &keys },
                "VEC3",
                None,
                false,
            );
            push("translation", out, constant);
        }
        if track.scale {
            let keys: Vec<[f32; 3]> = (0..count)
                .map(|s| [value(s, t, 7), value(s, t, 8), value(s, t, 9)])
                .collect();
            let constant = is_constant(&keys, 1e-6);
            let out = builder.floats(
                if constant { &keys[..1] } else { &keys },
                "VEC3",
                None,
                false,
            );
            push("scale", out, constant);
        }
    }
    stats.channels = channels.len();
    if !channels.is_empty() {
        animations.push(json!({"name": clip.name, "samplers": samplers, "channels": channels}));
    }
    stats
}

/// Map path hashes to nodes: the path of each node below `root`, joined with `/` (the root
/// itself is the empty path), hashed like Unity's `TransformBinding` paths.
#[must_use]
pub fn node_path_hashes(nodes: &[Value], root: usize) -> HashMap<u32, usize> {
    let mut out = HashMap::new();
    let mut stack = vec![(root, String::new())];
    while let Some((node, path)) = stack.pop() {
        out.insert(crc32(path.as_bytes()), node);
        for c in nodes[node]
            .get("children")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let Some(c) = c.as_u64().map(|c| c as usize) else {
                continue;
            };
            let name = nodes[c].get("name").and_then(Value::as_str).unwrap_or("");
            let child = if path.is_empty() {
                name.to_string()
            } else {
                format!("{path}/{name}")
            };
            stack.push((c, child));
        }
    }
    out
}
