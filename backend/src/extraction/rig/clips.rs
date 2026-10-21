//! Per-clip animation GLBs for an exported model (`endfield-models clips`).
//!
//! Each clip becomes an animation-only GLB: the model nodes its channels target (plus their
//! ancestors, for a valid hierarchy) and one animation, with no meshes or textures. Channels
//! bind by node name, so a viewer plays the clip on the already-loaded model with three.js'
//! `AnimationMixer` on the model root.
//!
//! three.js names loaded nodes `sanitizeNodeName(name)` and makes them unique in load order
//! (`name_1`, `name_2`…; see `GLTFParser.createUniqueName`). Endfield models repeat names
//! (a skinned mesh node shares its renderer GameObject's name), so a clip node is written
//! under the name three.js gives the model node: [`three_names`] replays the loader's naming.
use super::acl;
use super::anim::{self, AnimStats, DecodedClip};
use super::convert;
use super::gltf::{self, Builder};
use crate::extraction::unity::value_bytes;
use serde::Serialize;
use serde_json::{Map, Value, json};
use std::collections::{HashMap, HashSet};

/// Clip categories, by animation folder name.
pub const SOURCES: [&str; 5] = ["3c", "battle", "dialog", "interact", "ui"];

/// `PropertyBinding.sanitizeNodeName`: whitespace becomes `_`, `[ ] . : /` are removed.
#[must_use]
pub fn sanitize_node_name(name: &str) -> String {
    name.chars()
        .filter(|c| !matches!(c, '[' | ']' | '.' | ':' | '/'))
        .map(|c| if c.is_whitespace() { '_' } else { c })
        .collect()
}

/// Names three.js' `GLTFLoader` gives each node (`None` for unnamed or unloaded nodes).
///
/// The loader names a node when it first reaches it: scene roots in order, each subtree
/// pre-order, and a skinned node's joints right after its children. Mesh names are taken
/// later (asynchronously), so they never displace a node's name.
#[must_use]
pub fn three_names(doc: &Value) -> Vec<Option<String>> {
    let nodes = doc
        .get("nodes")
        .and_then(Value::as_array)
        .map_or(&[][..], Vec::as_slice);
    let skins = doc
        .get("skins")
        .and_then(Value::as_array)
        .map_or(&[][..], Vec::as_slice);
    let mut used: HashMap<String, usize> = HashMap::new();
    let mut names: Vec<Option<String>> = vec![None; nodes.len()];
    let mut seen = vec![false; nodes.len()];
    let mut shallow = |i: usize, names: &mut Vec<Option<String>>, seen: &mut Vec<bool>| {
        if seen[i] {
            return;
        }
        seen[i] = true;
        let Some(raw) = nodes[i].get("name").and_then(Value::as_str) else {
            return;
        };
        if raw.is_empty() {
            return;
        }
        let base = sanitize_node_name(raw);
        names[i] = Some(match used.get_mut(&base) {
            Some(n) => {
                *n += 1;
                format!("{base}_{n}")
            }
            None => {
                used.insert(base.clone(), 0);
                base
            }
        });
    };
    let indices = |v: Option<&Value>| -> Vec<usize> {
        v.and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|x| x.as_u64().map(|x| x as usize))
            .filter(|&x| x < nodes.len())
            .collect()
    };
    // Explicit stack of (node, phase): phase 0 names the node and queues its children,
    // phase 1 (after the children) names unloaded skin joints.
    let scene = doc
        .get("scene")
        .and_then(Value::as_u64)
        .map_or(0, |s| s as usize);
    let roots = indices(doc.pointer(&format!("/scenes/{scene}/nodes")));
    let mut loaded = vec![false; nodes.len()];
    let mut stack: Vec<(usize, bool)> = roots.iter().rev().map(|&r| (r, false)).collect();
    while let Some((i, after)) = stack.pop() {
        if after {
            if let Some(skin) = nodes[i].get("skin").and_then(Value::as_u64) {
                for j in indices(skins.get(skin as usize).and_then(|s| s.get("joints"))) {
                    shallow(j, &mut names, &mut seen);
                }
            }
            continue;
        }
        if loaded[i] {
            continue;
        }
        loaded[i] = true;
        shallow(i, &mut names, &mut seen);
        stack.push((i, true));
        for c in indices(nodes[i].get("children")).into_iter().rev() {
            stack.push((c, false));
        }
    }
    names
}

/// The node hierarchy of an exported model GLB, as clip export needs it.
pub struct ModelRig {
    pub nodes: Vec<Value>,
    pub parent: Vec<Option<usize>>,
    /// Node clip binding paths are relative to (the Animator's node, else the scene root).
    pub anim_root: usize,
    /// Nodes named like the Animator's node.
    pub roots: Vec<usize>,
    /// A generic Avatar's root motion bone: clips move it themselves.
    pub motion_bone: Option<usize>,
    /// Skin joints of the model's meshes.
    pub joints: Vec<usize>,
    /// Name to write for each node in a clip GLB (resolves to the node on the loaded model).
    pub clip_names: Vec<Option<String>>,
    /// Binding path hash → node.
    pub hashes: HashMap<u32, usize>,
    /// Nodes whose three.js name is not unique on the loaded model (never targeted).
    pub ambiguous: HashSet<usize>,
}

impl ModelRig {
    /// Read the hierarchy of a model GLB document written by the rig exporter.
    pub fn from_document(doc: &Value) -> Result<Self, String> {
        let nodes = doc
            .get("nodes")
            .and_then(Value::as_array)
            .cloned()
            .ok_or("model has no nodes")?;
        let mut parent = vec![None; nodes.len()];
        for (i, n) in nodes.iter().enumerate() {
            for c in n
                .get("children")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                if let Some(c) = c.as_u64().map(|c| c as usize).filter(|&c| c < nodes.len()) {
                    parent[c] = Some(i);
                }
            }
        }
        let scene_root = doc
            .pointer("/scenes/0/nodes/0")
            .and_then(Value::as_u64)
            .map(|r| r as usize)
            .filter(|&r| r < nodes.len())
            .ok_or("model has no scene root")?;
        // Clips bind below the Animator's node, known by name only (see `choose_root`).
        let animator = doc
            .pointer("/asset/extras/animators/0/node")
            .and_then(Value::as_str);
        let roots: Vec<usize> = animator
            .map(|name| {
                (0..nodes.len())
                    .filter(|&i| nodes[i].get("name").and_then(Value::as_str) == Some(name))
                    .collect()
            })
            .unwrap_or_default();
        let anim_root = roots.first().copied().unwrap_or(scene_root);
        let three = three_names(doc);
        let mut count: HashMap<&str, usize> = HashMap::new();
        for name in three.iter().flatten() {
            *count.entry(name).or_default() += 1;
        }
        let ambiguous: HashSet<usize> = three
            .iter()
            .enumerate()
            .filter(|(_, n)| n.as_deref().is_some_and(|n| count[n] > 1))
            .map(|(i, _)| i)
            .collect();
        // The raw name when three.js keeps it as is (modulo sanitizing), else three's name.
        let clip_names = three
            .iter()
            .enumerate()
            .map(|(i, t)| {
                let t = t.as_ref()?;
                let raw = nodes[i].get("name").and_then(Value::as_str).unwrap_or("");
                Some(if sanitize_node_name(raw) == *t {
                    raw.to_string()
                } else {
                    t.clone()
                })
            })
            .collect();
        let hashes = anim::node_path_hashes(&nodes, anim_root);
        let mut joints: Vec<usize> = doc
            .get("skins")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|s| s.get("joints").and_then(Value::as_array))
            .flatten()
            .filter_map(|j| j.as_u64().map(|j| j as usize))
            .filter(|&j| j < nodes.len())
            .collect();
        joints.sort_unstable();
        joints.dedup();
        Ok(Self {
            nodes,
            parent,
            anim_root,
            roots,
            motion_bone: None,
            joints,
            clip_names,
            hashes,
            ambiguous,
        })
    }

    /// Set the root motion bone by its path hash below the clip root.
    pub fn set_motion_bone(&mut self, path_hash: Option<u32>) {
        self.motion_bone = path_hash.and_then(|h| self.hashes.get(&h).copied());
    }

    /// The skeleton's root bone: the clip root's child above most skin joints (`Root` above
    /// `Bip001`), unless that child is a joint itself (then it is the body, not a placement).
    #[must_use]
    pub fn skeleton_root(&self) -> Option<usize> {
        let mut count: HashMap<usize, usize> = HashMap::new();
        for &j in &self.joints {
            let mut n = j;
            while let Some(p) = self.parent[n] {
                if p == self.anim_root {
                    *count.entry(n).or_default() += 1;
                    break;
                }
                n = p;
            }
        }
        count
            .into_iter()
            .max_by_key(|&(n, c)| (c, std::cmp::Reverse(n)))
            .map(|(n, _)| n)
            .filter(|n| self.joints.binary_search(n).is_err())
    }

    /// Among the nodes named like the Animator (an NPC wrapper can repeat its model's name),
    /// bind below the one whose subtree holds the most of the Avatar's skeleton paths.
    pub fn choose_root(&mut self, skeleton_ids: &[u32]) {
        let best = self
            .roots
            .iter()
            .map(|&r| {
                let hashes = anim::node_path_hashes(&self.nodes, r);
                let hits = skeleton_ids
                    .iter()
                    .filter(|h| hashes.contains_key(h))
                    .count();
                (hits, std::cmp::Reverse(r), hashes)
            })
            .max_by_key(|(hits, r, _)| (*hits, *r));
        if let Some((hits, std::cmp::Reverse(root), hashes)) = best
            && hits > 0
        {
            self.anim_root = root;
            self.hashes = hashes;
        }
    }
}

/// Read only the JSON chunk of a GLB file (models can be tens of MB of textures).
pub fn read_glb_json(path: &std::path::Path) -> Result<Value, String> {
    use std::io::Read;
    let mut f = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut head = [0u8; 20];
    f.read_exact(&mut head).map_err(|e| e.to_string())?;
    if &head[0..4] != b"glTF" || &head[16..20] != b"JSON" {
        return Err(format!("{}: not a GLB", path.display()));
    }
    let len = u32::from_le_bytes(head[12..16].try_into().unwrap_or_default()) as usize;
    if len > 512 << 20 {
        return Err("GLB JSON chunk too large".into());
    }
    let mut json = vec![0u8; len];
    f.read_exact(&mut json).map_err(|e| e.to_string())?;
    serde_json::from_slice(&json).map_err(|e| e.to_string())
}

/// Root motion of a clip in Unity space, one entry per sample.
pub struct RootMotion {
    pub positions: Vec<[f32; 3]>,
    pub rotations: Vec<[f32; 4]>,
}

/// Decode `m_AclCompressedBuffer.RootMotionBufferData`: float tracks whose first seven are
/// the root's position (xyz) and rotation (xyzw). `None` when absent or inconsistent.
#[must_use]
pub fn decode_root_motion(clip: &Value, num_samples: usize) -> Option<RootMotion> {
    let buffer = clip
        .pointer("/m_AclCompressedBuffer/RootMotionBufferData")
        .and_then(value_bytes)
        .filter(|b| !b.is_empty())?;
    let (info, values) = acl::floats(&buffer).ok()?;
    let tracks = info.num_tracks as usize;
    if tracks < 7 || info.num_samples as usize != num_samples || values.len() < tracks * num_samples
    {
        return None;
    }
    let mut out = RootMotion {
        positions: Vec::with_capacity(num_samples),
        rotations: Vec::with_capacity(num_samples),
    };
    for s in 0..num_samples {
        let v = &values[s * tracks..s * tracks + 7];
        let q = [v[3], v[4], v[5], v[6]];
        let norm = q.iter().map(|x| x * x).sum::<f32>().sqrt();
        if !(0.9..1.1).contains(&norm) || v.iter().any(|x| !x.is_finite()) {
            return None;
        }
        out.positions.push([v[0], v[1], v[2]]);
        out.rotations.push(q);
    }
    Some(out)
}

fn quat_mul(a: [f32; 4], b: [f32; 4]) -> [f32; 4] {
    let [ax, ay, az, aw] = a;
    let [bx, by, bz, bw] = b;
    [
        aw * bx + ax * bw + ay * bz - az * by,
        aw * by - ax * bz + ay * bw + az * bx,
        aw * bz + ax * by - ay * bx + az * bw,
        aw * bw - ax * bx - ay * by - az * bz,
    ]
}

fn quat_rotate(q: [f32; 4], v: [f32; 3]) -> [f32; 3] {
    let p = quat_mul(
        quat_mul(q, [v[0], v[1], v[2], 0.0]),
        [-q[0], -q[1], -q[2], q[3]],
    );
    [p[0], p[1], p[2]]
}

fn vec_of<const N: usize>(v: Option<&Value>, default: [f32; N]) -> [f32; N] {
    let mut out = default;
    if let Some(a) = v.and_then(Value::as_array) {
        for (o, x) in out.iter_mut().zip(a) {
            *o = x.as_f64().map_or(*o, |x| x as f32);
        }
    }
    out
}

/// One written clip.
pub struct ClipGlb {
    pub glb: Vec<u8>,
    pub stats: AnimStats,
    /// Tracks dropped because their node's name is ambiguous on the loaded model.
    pub ambiguous: usize,
    /// glTF-space translation of the root at the first / last sample.
    pub root_start: Option<[f32; 3]>,
    pub root_end: Option<[f32; 3]>,
    /// Whether the clip moves or turns the root ([`ClipGlb::root_node`]'s channels).
    pub root_motion: bool,
    /// The node carrying the root's placement: the clip root when root-motion channels were
    /// added, else the node of a root translation track (constant ones included).
    pub root_node: Option<String>,
}

/// Where a root translation track puts its node (see `clip_glb`).
struct Placement {
    start: Option<[f32; 3]>,
    end: Option<[f32; 3]>,
    moves: bool,
    node: Option<String>,
}

/// Largest error a dropped key may introduce: metres, radians, scale units.
#[derive(Debug, Clone, Copy)]
pub struct Tolerance {
    pub position: f32,
    pub rotation: f32,
    pub scale: f32,
}

/// Default key reduction tolerances: 0.1 mm, ~0.06°, 1e-4 (ACL's own error is ~1e-4).
pub const TOLERANCE: Tolerance = Tolerance {
    position: 1e-4,
    rotation: 1e-3,
    scale: 1e-4,
};

/// Longest run of keys one linear segment may replace (bounds the reduction's cost).
const MAX_SEGMENT: usize = 120;

/// Angle between two unit quaternions (radians, sign-agnostic).
fn angle_between(a: [f32; 4], b: [f32; 4]) -> f32 {
    let dot: f32 = (0..4).map(|k| a[k] * b[k]).sum();
    2.0 * dot.abs().min(1.0).acos()
}

/// Spherical interpolation, as three.js' `QuaternionLinearInterpolant` does.
fn slerp(a: [f32; 4], b: [f32; 4], t: f32) -> [f32; 4] {
    let mut dot: f32 = (0..4).map(|k| a[k] * b[k]).sum();
    let b = if dot < 0.0 {
        dot = -dot;
        b.map(|x| -x)
    } else {
        b
    };
    let (wa, wb) = if dot > 0.9995 {
        (1.0 - t, t)
    } else {
        let theta = dot.min(1.0).acos();
        let sin = theta.sin();
        (((1.0 - t) * theta).sin() / sin, (t * theta).sin() / sin)
    };
    let q: [f32; 4] = std::array::from_fn(|k| wa * a[k] + wb * b[k]);
    let len = q.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-12);
    q.map(|x| x / len)
}

/// Key strides tried by [`grid_keys`], coarsest first.
const STRIDES: [usize; 10] = [32, 24, 16, 12, 8, 6, 4, 3, 2, 1];

/// Keys `0, k, 2k, …` plus the last, for the coarsest stride `k` whose linear interpolation
/// stays within `tol` of every key. Channels sharing a stride share their key times.
pub fn grid_keys<T: Copy>(
    keys: &[T],
    tol: f32,
    interp: impl Fn(T, T, f32) -> T,
    error: impl Fn(T, T) -> f32,
) -> Vec<usize> {
    let n = keys.len();
    for stride in STRIDES {
        let mut grid: Vec<usize> = (0..n).step_by(stride).collect();
        if grid.last() != Some(&(n - 1)) {
            grid.push(n - 1);
        }
        if stride == 1 {
            return grid;
        }
        let fits = grid.windows(2).all(|w| {
            let (a, b) = (w[0], w[1]);
            (a + 1..b).all(|k| {
                let t = (k - a) as f32 / (b - a) as f32;
                error(interp(keys[a], keys[b], t), keys[k]) <= tol
            })
        });
        if fits {
            return grid;
        }
    }
    (0..n).collect()
}

/// Indices of the keys to keep so that interpolating between kept keys stays within `tol`
/// of every dropped key (greedy, segments of at most [`MAX_SEGMENT`] keys).
pub fn reduce_keys<T: Copy>(
    keys: &[T],
    tol: f32,
    interp: impl Fn(T, T, f32) -> T,
    error: impl Fn(T, T) -> f32,
) -> Vec<usize> {
    let n = keys.len();
    if n <= 2 {
        return (0..n).collect();
    }
    let mut kept = vec![0];
    let mut last = 0;
    let mut end = 2;
    while end < n {
        let fits = end - last <= MAX_SEGMENT
            && (last + 1..end).all(|k| {
                let t = (k - last) as f32 / (end - last) as f32;
                error(interp(keys[last], keys[end], t), keys[k]) <= tol
            });
        if fits {
            end += 1;
        } else {
            last = end - 1;
            kept.push(last);
            end = last + 2;
        }
    }
    kept.push(n - 1);
    kept
}

/// Accumulates key-reduced channels of one animation. Key times are shared per key set and
/// all outputs share two buffer views (floats, normalized shorts) to keep the JSON small.
struct AnimWriter<'a> {
    builder: &'a mut Builder,
    tol: Tolerance,
    times: Vec<f32>,
    inputs: HashMap<Vec<usize>, usize>,
    samplers: Vec<Value>,
    channels: Vec<Value>,
    floats: Vec<u8>,
    shorts: Vec<u8>,
    /// Accessors to point at the float (`false`) or short (`true`) view on finish.
    pending: Vec<(usize, bool)>,
    /// A constant channel equal to the rest pose, kept only if nothing else is written.
    fallback: Option<(usize, &'static str, Vec<f32>)>,
}

impl<'a> AnimWriter<'a> {
    fn new(builder: &'a mut Builder, count: usize, rate: f32, tol: Tolerance) -> Self {
        let times = (0..count)
            .map(|s| if s == 0 { 0.0 } else { s as f32 / rate })
            .collect();
        Self {
            builder,
            tol,
            times,
            inputs: HashMap::new(),
            samplers: Vec::new(),
            channels: Vec::new(),
            floats: Vec::new(),
            shorts: Vec::new(),
            pending: Vec::new(),
            fallback: None,
        }
    }

    fn accessor(&mut self, short: bool, values: &[f32], kind: &str, count: usize) -> usize {
        let offset = if short {
            let offset = self.shorts.len();
            for &x in values {
                let v = (x.clamp(-1.0, 1.0) * 32767.0).round() as i16;
                self.shorts.extend_from_slice(&v.to_le_bytes());
            }
            offset
        } else {
            let offset = self.floats.len();
            for &x in values {
                self.floats.extend_from_slice(&x.to_le_bytes());
            }
            offset
        };
        let mut a = json!({
            "bufferView": 0, "byteOffset": offset,
            "componentType": if short { 5122 } else { gltf::FLOAT }, "count": count, "type": kind
        });
        if short {
            a["normalized"] = json!(true);
        }
        if kind == "SCALAR" {
            let min = values.iter().copied().fold(f32::INFINITY, f32::min);
            let max = values.iter().copied().fold(f32::NEG_INFINITY, f32::max);
            a["min"] = json!([min]);
            a["max"] = json!([max]);
        }
        self.builder.accessors.push(a);
        let index = self.builder.accessors.len() - 1;
        self.pending.push((index, short));
        index
    }

    fn push(&mut self, node: usize, path: &str, kept: Vec<usize>, output: usize) {
        let input = match self.inputs.get(&kept) {
            Some(&i) => i,
            None => {
                let times: Vec<f32> = kept.iter().map(|&k| self.times[k]).collect();
                let i = self.accessor(false, &times, "SCALAR", times.len());
                self.inputs.insert(kept, i);
                i
            }
        };
        self.samplers
            .push(json!({"input": input, "output": output}));
        self.channels.push(
            json!({"sampler": self.samplers.len() - 1, "target": {"node": node, "path": path}}),
        );
    }

    /// Rotation keys, written as normalized shorts (glTF allows them for rotations). A
    /// constant channel equal to `rest` is left out: the mixer falls back to the rest pose.
    fn rotation(&mut self, node: usize, mut keys: Vec<[f32; 4]>, rest: [f32; 4]) {
        anim::make_continuous(&mut keys);
        let tol = self.tol.rotation;
        let kept = if keys.iter().all(|q| angle_between(*q, keys[0]) <= tol) {
            if angle_between(keys[0], rest) <= tol {
                self.fallback
                    .get_or_insert((node, "rotation", keys[0].to_vec()));
                return;
            }
            vec![0]
        } else {
            reduce_keys(&keys, tol, slerp, angle_between)
        };
        let values: Vec<f32> = kept.iter().flat_map(|&k| keys[k]).collect();
        let output = self.accessor(true, &values, "VEC4", kept.len());
        self.push(node, "rotation", kept, output);
    }

    fn vec3(
        &mut self,
        node: usize,
        path: &'static str,
        keys: Vec<[f32; 3]>,
        tol: f32,
        rest: [f32; 3],
    ) {
        let distance = |a: [f32; 3], b: [f32; 3]| {
            ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
        };
        let kept = if keys.iter().all(|k| distance(*k, keys[0]) <= tol) {
            if distance(keys[0], rest) <= tol {
                self.fallback.get_or_insert((node, path, keys[0].to_vec()));
                return;
            }
            vec![0]
        } else {
            reduce_keys(
                &keys,
                tol,
                |a, b, t| std::array::from_fn(|k| a[k] + (b[k] - a[k]) * t),
                distance,
            )
        };
        let values: Vec<f32> = kept.iter().flat_map(|&k| keys[k]).collect();
        let output = self.accessor(false, &values, "VEC3", kept.len());
        self.push(node, path, kept, output);
    }

    fn finish(mut self) -> (Vec<Value>, Vec<Value>) {
        if self.channels.is_empty()
            && let Some((node, path, value)) = self.fallback.take()
        {
            let short = path == "rotation";
            let kind = if short { "VEC4" } else { "VEC3" };
            let output = self.accessor(short, &value, kind, 1);
            self.push(node, path, vec![0], output);
        }
        let float_view = self.builder.view(&self.floats, None);
        let short_view = (!self.shorts.is_empty()).then(|| self.builder.view(&self.shorts, None));
        for (a, short) in self.pending {
            let view = if short {
                short_view.unwrap_or(float_view)
            } else {
                float_view
            };
            self.builder.accessors[a]["bufferView"] = json!(view);
        }
        (self.samplers, self.channels)
    }
}

/// Unity-space curves of one node, one key per sample.
#[derive(Debug, Clone, Default)]
pub struct NodeTrack {
    pub path_hash: u32,
    pub rotation: Option<Vec<[f32; 4]>>,
    pub translation: Option<Vec<[f32; 3]>>,
    pub scale: Option<Vec<[f32; 3]>>,
}

/// The Transform tracks of a decoded clip.
pub fn transform_tracks(clip: &DecodedClip) -> Result<Vec<NodeTrack>, String> {
    let count = clip.num_samples;
    let n = clip.tracks.len();
    if count == 0
        || clip.samples.len() < count * n * acl::TRANSFORM_STRIDE
        || anim::validate_timing(count as u32, clip.sample_rate).is_err()
    {
        return Err("clip samples are inconsistent".into());
    }
    let value =
        |s: usize, t: usize, k: usize| clip.samples[(s * n + t) * acl::TRANSFORM_STRIDE + k];
    Ok(clip
        .tracks
        .iter()
        .enumerate()
        .map(|(t, track)| NodeTrack {
            path_hash: track.path_hash,
            rotation: track.rotation.then(|| {
                (0..count)
                    .map(|s| {
                        [
                            value(s, t, 0),
                            value(s, t, 1),
                            value(s, t, 2),
                            value(s, t, 3),
                        ]
                    })
                    .collect()
            }),
            translation: track.position.then(|| {
                (0..count)
                    .map(|s| [value(s, t, 4), value(s, t, 5), value(s, t, 6)])
                    .collect()
            }),
            scale: track.scale.then(|| {
                (0..count)
                    .map(|s| [value(s, t, 7), value(s, t, 8), value(s, t, 9)])
                    .collect()
            }),
        })
        .collect())
}

/// Body tracks of a humanoid clip (see [`super::humanoid`]) and its root motion. Each human
/// bone track replaces any Transform track of the same node, as in Unity.
pub fn humanoid_tracks(
    human: &super::humanoid::Humanoid,
    curves: &[f32],
    count: usize,
) -> (Vec<NodeTrack>, RootMotion) {
    use super::humanoid::CURVES;
    let mut by_hash: HashMap<u32, NodeTrack> = HashMap::new();
    let mut order = Vec::new();
    let mut motion = RootMotion {
        positions: Vec::with_capacity(count),
        rotations: Vec::with_capacity(count),
    };
    for s in 0..count {
        let sample = &curves[s * CURVES..(s + 1) * CURVES];
        for (hash, t, q) in human.pose(sample) {
            let track = by_hash.entry(hash).or_insert_with(|| {
                order.push(hash);
                NodeTrack {
                    path_hash: hash,
                    ..NodeTrack::default()
                }
            });
            track.rotation.get_or_insert_with(Vec::new).push(q);
            if let Some(t) = t {
                track.translation.get_or_insert_with(Vec::new).push(t);
            }
        }
        let (t, q) = human.motion(sample);
        motion.positions.push(t);
        motion.rotations.push(q);
    }
    let tracks = order
        .into_iter()
        .filter_map(|h| by_hash.remove(&h))
        .collect();
    (tracks, motion)
}

/// Replace (or add) tracks by node: humanoid bones override Transform curves.
#[must_use]
pub fn merge_tracks(mut base: Vec<NodeTrack>, overrides: Vec<NodeTrack>) -> Vec<NodeTrack> {
    for o in overrides {
        match base.iter_mut().find(|t| t.path_hash == o.path_hash) {
            Some(t) => {
                t.rotation = o.rotation.or(t.rotation.take());
                t.translation = o.translation.or(t.translation.take());
            }
            None => base.push(o),
        }
    }
    base
}

/// Share of tracks whose node exists in the model (1.0 without tracks).
#[must_use]
pub fn matched_share(tracks: &[NodeTrack], rig: &ModelRig) -> f32 {
    if tracks.is_empty() {
        return 1.0;
    }
    let matched = tracks
        .iter()
        .filter(|t| rig.hashes.contains_key(&t.path_hash))
        .count();
    matched as f32 / tracks.len() as f32
}

/// Timing of a clip.
#[derive(Debug, Clone, Copy)]
pub struct ClipTiming {
    pub samples: usize,
    pub rate: f32,
}

/// Build the animation-only GLB of one clip for `rig`: the targeted nodes and their ancestors
/// below the clip root (`rig.anim_root`, kept only when animated), and one animation. Root motion, when given and not
/// already animated by a root transform track, becomes translation/rotation channels on the
/// clip root (`rig.anim_root`), composed with the node's rest transform.
pub fn clip_glb(
    rig: &ModelRig,
    name: &str,
    timing: ClipTiming,
    tracks: &[NodeTrack],
    root_motion: Option<&RootMotion>,
    tol: Tolerance,
    extras: Value,
) -> Result<ClipGlb, String> {
    let count = timing.samples;
    if count == 0
        || anim::validate_timing(count as u32, timing.rate).is_err()
        || tracks.iter().any(|t| {
            t.rotation.as_ref().is_some_and(|k| k.len() != count)
                || t.translation.as_ref().is_some_and(|k| k.len() != count)
                || t.scale.as_ref().is_some_and(|k| k.len() != count)
        })
    {
        return Err("clip samples are inconsistent".into());
    }
    // Targeted nodes (by path hash), minus ambiguous names; then their ancestors.
    let mut targets: HashMap<u32, usize> = HashMap::new();
    let mut ambiguous = 0;
    for t in tracks {
        if let Some(&node) = rig.hashes.get(&t.path_hash) {
            if rig.ambiguous.contains(&node) || rig.clip_names[node].is_none() {
                ambiguous += 1;
            } else {
                targets.insert(t.path_hash, node);
            }
        }
    }
    // Travel already in a Transform track: the clip root's own, or a generic Avatar's root
    // motion bone. Root motion curves are then not added on top.
    let track_of = |node: usize| {
        tracks
            .iter()
            .find(|t| rig.hashes.get(&t.path_hash) == Some(&node))
    };
    let clip_root_track =
        track_of(rig.anim_root).filter(|t| t.translation.is_some() || t.rotation.is_some());
    let bone_track = rig
        .motion_bone
        .and_then(|bone| track_of(bone).map(|t| (bone, t)))
        .filter(|(_, t)| t.translation.is_some());
    // The node whose translation track places the character: the clip root's (humanoid
    // travel), else the motion bone's (generic travel, or a pose parked at a lineup slot),
    // even when the clip root has a rotation-only track of its own.
    let root_translation = clip_root_track
        .and_then(|t| t.translation.as_ref().map(|k| (rig.anim_root, k)))
        .or_else(|| bone_track.and_then(|(n, t)| t.translation.as_ref().map(|k| (n, k))));
    // Without either, a generic clip may still place the skeleton root (a humanoid model's
    // `Root` parked at a team lineup slot); named, but root motion curves still apply.
    let skeleton_track = rig
        .skeleton_root()
        .filter(|_| root_translation.is_none())
        .and_then(|n| {
            track_of(n)
                .and_then(|t| t.translation.as_ref())
                .map(|k| (n, k))
        });
    let use_root_motion = root_motion.filter(|rm| {
        rm.positions.len() == count
            && rm.rotations.len() == count
            && clip_root_track.is_none()
            && bone_track.is_none()
    });
    let rest_of = |model_node: usize| {
        let n = &rig.nodes[model_node];
        (
            vec_of(n.get("translation"), [0.0; 3]),
            vec_of(n.get("rotation"), [0.0, 0.0, 0.0, 1.0]),
            vec_of(n.get("scale"), [1.0; 3]),
        )
    };
    // Channels first, targeting model node indices; the clip's nodes follow from them.
    let mut builder = Builder::default();
    let mut writer = AnimWriter::new(&mut builder, count, timing.rate, tol);
    let mut stats = AnimStats {
        name: name.to_string(),
        duration: writer.times.last().copied().unwrap_or(0.0),
        ..AnimStats::default()
    };
    for track in tracks {
        let Some(&node) = targets.get(&track.path_hash) else {
            if !rig.hashes.contains_key(&track.path_hash) {
                stats.unmatched_tracks += 1;
            }
            continue;
        };
        stats.matched_tracks += 1;
        let (rest_t, rest_r, rest_s) = rest_of(node);
        if let Some(keys) = &track.rotation {
            let keys = keys.iter().map(|&q| convert::rotation(q)).collect();
            writer.rotation(node, keys, rest_r);
        }
        if let Some(keys) = &track.translation {
            let keys = keys.iter().map(|&p| convert::position(p)).collect();
            writer.vec3(node, "translation", keys, tol.position, rest_t);
        }
        if let Some(keys) = &track.scale {
            writer.vec3(node, "scale", keys.clone(), tol.scale, rest_s);
        }
    }
    // A root translation track, in glTF space. It names its node unless it only restates the
    // rest pose (no channel then): a pose parked off rest still needs its root held in place.
    let placement = |node: usize, keys: &[[f32; 3]]| {
        let keys: Vec<[f32; 3]> = keys.iter().map(|&p| convert::position(p)).collect();
        let moves = !anim::is_constant(&keys, tol.position);
        let rest = rest_of(node).0;
        let off_rest = keys.first().is_some_and(|k| {
            k.iter()
                .zip(&rest)
                .any(|(a, b)| (a - b).abs() > tol.position)
        });
        Placement {
            start: keys.first().copied(),
            end: keys.last().copied(),
            moves,
            node: rig.clip_names[node].clone().filter(|_| moves || off_rest),
        }
    };
    let (rest_t, rest_r, _) = rest_of(rig.anim_root);
    let mut out = ClipGlb {
        glb: Vec::new(),
        stats,
        ambiguous,
        root_start: None,
        root_end: None,
        root_motion: false,
        root_node: None,
    };
    if let Some(rm) = use_root_motion {
        let positions: Vec<[f32; 3]> = rm
            .positions
            .iter()
            .map(|&p| {
                let p = quat_rotate(rest_r, convert::position(p));
                [p[0] + rest_t[0], p[1] + rest_t[1], p[2] + rest_t[2]]
            })
            .collect();
        let rotations: Vec<[f32; 4]> = rm
            .rotations
            .iter()
            .map(|&q| quat_mul(rest_r, convert::rotation(q)))
            .collect();
        out.root_start = positions.first().copied();
        out.root_end = positions.last().copied();
        let moves = !anim::is_constant(&positions, tol.position);
        let turns = rotations
            .iter()
            .any(|q| angle_between(*q, rotations[0]) > tol.rotation);
        if (moves || turns) && count > 1 {
            if moves {
                writer.vec3(
                    rig.anim_root,
                    "translation",
                    positions,
                    tol.position,
                    [f32::NAN; 3],
                );
            }
            if turns {
                writer.rotation(rig.anim_root, rotations, [f32::NAN; 4]);
            }
            out.root_motion = true;
            out.root_node = rig.clip_names[rig.anim_root].clone();
        }
    } else if let Some((node, keys)) = root_translation {
        // Root placement carried by a Transform track (local translation of `node`).
        let p = placement(node, keys);
        (out.root_start, out.root_end) = (p.start, p.end);
        out.root_motion = p.moves;
        out.root_node = p.node;
    }
    if let Some((node, keys)) = skeleton_track.filter(|_| !out.root_motion) {
        let p = placement(node, keys);
        if p.node.is_some() {
            (out.root_start, out.root_end) = (p.start, p.end);
            out.root_motion = p.moves;
            out.root_node = p.node;
        }
    }
    let (samplers, mut channels) = writer.finish();
    out.stats.channels = channels.len();
    if channels.is_empty() {
        return Err("clip has no channels on this model".into());
    }
    // Animated nodes and their ancestors below the clip root (the root itself only when
    // animated): nothing above affects playback, and leaving it out makes clips of models
    // sharing a rig byte-identical (an NPC wrapper renames the root `<name>_1`).
    let mut keep: Vec<bool> = vec![false; rig.nodes.len()];
    for c in &channels {
        let Some(mut n) = c["target"]["node"].as_u64().map(|n| n as usize) else {
            continue;
        };
        while !keep[n] {
            keep[n] = true;
            match rig.parent[n] {
                Some(p) if p != rig.anim_root && n != rig.anim_root => n = p,
                _ => break,
            }
        }
    }
    // Names only: the clip never needs the rest pose. Renumbered in model order.
    let mut remap: Vec<Option<usize>> = vec![None; rig.nodes.len()];
    for i in 0..rig.nodes.len() {
        if !keep[i] {
            continue;
        }
        let mut node = Map::new();
        if let Some(name) = &rig.clip_names[i] {
            node.insert("name".into(), json!(name));
        }
        remap[i] = Some(builder.node(Value::Object(node)));
    }
    let mut roots = Vec::new();
    for i in 0..rig.nodes.len() {
        let Some(new) = remap[i] else { continue };
        match rig.parent[i].and_then(|p| remap[p]) {
            Some(p) => {
                let mut children = builder.nodes[p]["children"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default();
                children.push(json!(new));
                builder.nodes[p]["children"] = json!(children);
            }
            None => roots.push(new),
        }
    }
    for c in &mut channels {
        let n = c["target"]["node"].as_u64().map(|n| n as usize);
        if let Some(r) = n.and_then(|n| remap.get(n).copied().flatten()) {
            c["target"]["node"] = json!(r);
        }
    }
    builder.animations = vec![json!({"name": name, "samplers": samplers, "channels": channels})];
    let (doc, glb) = builder.finish(roots, extras);
    let (_, bin) = gltf::parse_glb(&glb)?;
    let errors = gltf::check_document(&doc, bin);
    if !errors.is_empty() {
        return Err(format!("invalid clip document: {}", errors.join("; ")));
    }
    out.glb = glb;
    Ok(out)
}

/// Clip category from its container path: the first folder below `/animations/` that is a
/// known category (`…/chen/animations/battle/…` → `battle`), else `other`.
#[must_use]
pub fn clip_source(container: &str) -> &'static str {
    let lower = container.to_ascii_lowercase();
    let Some(at) = lower.find("/animations/") else {
        return "other";
    };
    let rest = &lower[at + "/animations/".len()..];
    let dirs: Vec<&str> = rest.split('/').collect();
    for dir in &dirs[..dirs.len().saturating_sub(1)] {
        if let Some(s) = SOURCES.iter().find(|s| *s == dir) {
            return s;
        }
    }
    "other"
}

/// Display name of a clip: lowercase, the `a_<kind>_<code>_` prefix stripped, `_` → space
/// (`A_actor_chen_battle_attack_01` → `battle attack 01`).
#[must_use]
pub fn pretty_name(clip: &str, code: &str) -> String {
    let lower = clip.to_ascii_lowercase();
    let parts: Vec<&str> = lower.split('_').filter(|p| !p.is_empty()).collect();
    let mut start = 0;
    if parts.first() == Some(&"a") {
        start = 1;
        if parts
            .get(1)
            .is_some_and(|k| matches!(*k, "actor" | "monster" | "npc" | "boss" | "chr"))
        {
            start = 2;
        }
        let base = code.trim_end_matches(|c: char| c.is_ascii_digit());
        if parts
            .get(start)
            .is_some_and(|c| !code.is_empty() && (*c == code || *c == base))
        {
            start += 1;
        }
    }
    if start >= parts.len() {
        start = 0;
    }
    parts[start..].join(" ")
}

/// File stem for a clip: its lowercase name with anything outside `[a-z0-9_-]` replaced.
#[must_use]
pub fn clip_file_stem(clip: &str) -> String {
    let stem: String = clip
        .to_ascii_lowercase()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if stem.is_empty() { "clip".into() } else { stem }
}

/// `m_MuscleClip.m_LoopTime` (the clip's "Loop Time" import setting).
#[must_use]
pub fn clip_loops(clip: &Value) -> bool {
    clip.pointer("/m_MuscleClip/m_LoopTime")
        .and_then(|v| v.as_bool().or_else(|| v.as_i64().map(|n| n != 0)))
        .unwrap_or(false)
}

/// One entry of `clips/<kind>/<id>/index.json`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClipEntry {
    /// GLB file name in the same folder.
    pub file: String,
    /// Display name (prefix stripped).
    pub name: String,
    /// The AnimationClip's own name.
    pub clip: String,
    /// `3c`, `battle`, `dialog`, `interact`, `ui` or `other`.
    pub source: &'static str,
    /// Seconds.
    pub duration: f32,
    pub samples: usize,
    pub fps: f32,
    #[serde(rename = "loop")]
    pub looping: bool,
    /// Muscle-driven (Unity Humanoid) clip, retargeted through the model's Avatar.
    pub humanoid: bool,
    /// Animated Transforms in the clip.
    pub tracks: usize,
    /// Of those, the ones bound to a node of the model.
    pub matched: usize,
    pub size: u64,
    /// Whether the clip moves/turns `rootNode` (humanoid: the clip root's root-motion channels;
    /// generic: the Avatar's root motion bone). Hold its position at `rootStart` to play in place.
    pub root_motion: bool,
    /// The node placing the character: set whenever the root carries a translation track,
    /// even a constant one (a generic pose parked at a lineup slot), not only when it travels.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub root_node: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub root_start: Option<[f32; 3]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub root_end: Option<[f32; 3]>,
}

/// Round to 4 decimals for compact JSON.
#[must_use]
pub fn round4(v: [f32; 3]) -> [f32; 3] {
    v.map(|x| (x * 1e4).round() / 1e4)
}

/// A clip that was found but not written.
#[derive(Debug, Clone, Serialize)]
pub struct SkippedClip {
    pub clip: String,
    pub container: String,
    pub reason: String,
}

/// Outcome of [`export_entity`].
#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EntityClips {
    pub clips: usize,
    pub bytes: u64,
    /// Clips without an ACL buffer (plain Mecanim curves are not decoded).
    pub non_acl: usize,
    pub additive: usize,
    pub other_skeleton: usize,
    pub duplicates: usize,
    pub failed: usize,
    /// Clips hard-linked to an identical file of another entity.
    pub linked: usize,
    /// Humanoid clips skipped because the model's Avatar was not found.
    pub no_avatar: usize,
    /// Humanoid clips written (retargeted through the Avatar).
    pub humanoid: usize,
    /// Tracks dropped for an ambiguous node name, over all clips.
    pub ambiguous_tracks: usize,
    pub skipped: Vec<SkippedClip>,
    #[serde(skip)]
    pub entries: Vec<ClipEntry>,
}

/// Options of [`export_entity`].
pub struct EntityJob<'a> {
    pub id: &'a str,
    pub kind: &'a str,
    /// Clip naming code (`chen`), stripped from display names.
    pub code: &'a str,
    pub rig: &'a ModelRig,
    /// `(container path, cab, path_id)` of every candidate clip object.
    pub sources: &'a [(String, String, i64)],
    pub out_dir: &'a std::path::Path,
    pub root_motion: bool,
    pub tolerance: Tolerance,
    /// The model's human description, for humanoid clips.
    pub human: Option<&'a super::humanoid::Humanoid>,
    /// Files written in this run by content, so identical clips of entities that share a
    /// skeleton (NPC variants of operators, enemy postmodels) become hard links.
    pub written: Option<&'a std::sync::Mutex<WrittenFiles>>,
}

/// `(content hash, length)` → first path written with that content.
pub type WrittenFiles = HashMap<(u64, usize), std::path::PathBuf>;

/// Write `data` to `path`, hard-linking an identical file written earlier when possible.
/// Returns whether a link was made.
pub fn write_dedup(
    path: &std::path::Path,
    data: &[u8],
    written: Option<&std::sync::Mutex<WrittenFiles>>,
) -> std::io::Result<bool> {
    use std::hash::{Hash, Hasher};
    let Some(written) = written else {
        super::store::write_atomic(path, data)?;
        return Ok(false);
    };
    let mut h = std::collections::hash_map::DefaultHasher::new();
    data.hash(&mut h);
    let key = (h.finish(), data.len());
    let existing = written
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(&key)
        .cloned();
    if let Some(existing) = existing.filter(|e| e != path)
        && std::fs::read(&existing).is_ok_and(|b| b == data)
    {
        let tmp = super::store::temp_path(path);
        let _ = std::fs::remove_file(&tmp);
        if std::fs::hard_link(&existing, &tmp).is_ok() {
            std::fs::rename(&tmp, path)?;
            return Ok(true);
        }
    }
    super::store::write_atomic(path, data)?;
    written
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .entry(key)
        .or_insert_with(|| path.to_path_buf());
    Ok(false)
}

/// Decode and write every clip of one entity into `out_dir` (one GLB per clip plus
/// `index.json`), removing GLBs of earlier runs that were not written again.
pub fn export_entity(
    src: &mut super::store::CabSource,
    job: &EntityJob<'_>,
) -> Result<EntityClips, String> {
    const CLASS_ANIMATION_CLIP: i32 = 74;
    std::fs::create_dir_all(job.out_dir).map_err(|e| e.to_string())?;
    let mut report = EntityClips::default();
    let mut written: HashMap<String, (usize, usize)> = HashMap::new();
    let mut seen_objects: HashSet<(String, i64)> = HashSet::new();
    let mut order: Vec<&(String, String, i64)> = job.sources.iter().collect();
    // Load each bundle once.
    order.sort_by(|a, b| (&a.1, &a.0, a.2).cmp(&(&b.1, &b.0, b.2)));
    for (container, cab, path_id) in order {
        if !seen_objects.insert((cab.to_ascii_lowercase(), *path_id)) {
            continue;
        }
        if src.files.files.len() > 48 {
            src.clear();
        }
        let skip = |report: &mut EntityClips, clip: &str, reason: String| {
            report.skipped.push(SkippedClip {
                clip: clip.to_string(),
                container: container.clone(),
                reason,
            });
        };
        let Some(file) = src.get(cab) else {
            report.failed += 1;
            skip(&mut report, "", format!("{cab} not in store"));
            continue;
        };
        let sf = &src.files.files[file].sf;
        let Some(obj) = sf.objects.iter().find(|o| o.path_id == *path_id) else {
            continue;
        };
        if obj.class_id != CLASS_ANIMATION_CLIP {
            continue;
        }
        let value = match sf.read_object(obj) {
            Ok(v) => v,
            Err(e) => {
                report.failed += 1;
                skip(&mut report, "", format!("unreadable clip: {e}"));
                continue;
            }
        };
        let name = value
            .get("m_Name")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let file_name = container.rsplit('/').next().unwrap_or(container);
        if name.to_ascii_lowercase().contains("additive") || file_name.contains("additive") {
            report.additive += 1;
            skip(&mut report, &name, "additive".into());
            continue;
        }
        let decoded = match anim::decode_clip(&value) {
            Ok(d) => d,
            Err(e) => {
                if e.contains("no ACL") {
                    report.non_acl += 1;
                } else {
                    report.failed += 1;
                }
                skip(&mut report, &name, e);
                continue;
            }
        };
        let mut tracks = match transform_tracks(&decoded) {
            Ok(t) => t,
            Err(e) => {
                report.failed += 1;
                skip(&mut report, &name, e);
                continue;
            }
        };
        let humanoid = super::humanoid::is_humanoid(&value);
        let mut root_motion: Option<RootMotion>;
        if humanoid {
            let Some(human) = job.human else {
                report.no_avatar += 1;
                skip(
                    &mut report,
                    &name,
                    "humanoid clip, but the model's Avatar was not found".into(),
                );
                continue;
            };
            match super::humanoid::decode_curves(&value, decoded.num_samples) {
                Ok(curves) => {
                    let (body, motion) = humanoid_tracks(human, &curves, decoded.num_samples);
                    tracks = merge_tracks(tracks, body);
                    root_motion = Some(motion);
                }
                Err(e) => {
                    report.failed += 1;
                    skip(&mut report, &name, format!("humanoid curves: {e}"));
                    continue;
                }
            }
        } else {
            root_motion = decode_root_motion(&value, decoded.num_samples);
        }
        if !job.root_motion {
            root_motion = None;
        }
        let fraction = matched_share(&tracks, job.rig);
        if fraction < 0.5 {
            report.other_skeleton += 1;
            skip(
                &mut report,
                &name,
                format!(
                    "{:.0}% of {} tracks match the model (other skeleton)",
                    fraction * 100.0,
                    tracks.len()
                ),
            );
            continue;
        }
        let mut stem = clip_file_stem(&name);
        if let Some(&(samples, tracks)) = written.get(&stem) {
            if samples == decoded.num_samples && tracks == decoded.tracks.len() {
                report.duplicates += 1;
                skip(&mut report, &name, format!("duplicate of {stem}.glb"));
                continue;
            }
            let mut n = 2;
            while written.contains_key(&format!("{stem}_{n}")) {
                n += 1;
            }
            stem = format!("{stem}_{n}");
        }
        let source = clip_source(container);
        let looping = clip_loops(&value);
        // Nothing entity-specific, so clips shared by several models dedupe to one file.
        let extras = json!({
            "clip": {"name": name, "container": container, "source": source, "loop": looping},
            "coordinateSystem": "Unity left-handed converted by negating X (rotations (x,-y,-z,w))",
        });
        let timing = ClipTiming {
            samples: decoded.num_samples,
            rate: decoded.sample_rate,
        };
        let built = match clip_glb(
            job.rig,
            &name,
            timing,
            &tracks,
            root_motion.as_ref(),
            job.tolerance,
            extras,
        ) {
            Ok(b) => b,
            Err(e) => {
                report.failed += 1;
                skip(&mut report, &name, e);
                continue;
            }
        };
        let file = format!("{stem}.glb");
        if write_dedup(&job.out_dir.join(&file), &built.glb, job.written)
            .map_err(|e| format!("{file}: {e}"))?
        {
            report.linked += 1;
        }
        written.insert(stem, (decoded.num_samples, decoded.tracks.len()));
        report.clips += 1;
        report.humanoid += usize::from(humanoid);
        report.bytes += built.glb.len() as u64;
        report.ambiguous_tracks += built.ambiguous;
        let duration = if decoded.num_samples > 1 {
            (decoded.num_samples - 1) as f32 / decoded.sample_rate
        } else {
            0.0
        };
        report.entries.push(ClipEntry {
            file,
            name: pretty_name(&name, job.code),
            clip: name,
            source,
            duration: (duration * 1e4).round() / 1e4,
            samples: decoded.num_samples,
            fps: decoded.sample_rate,
            looping,
            tracks: tracks.len(),
            matched: built.stats.matched_tracks,
            humanoid,
            size: built.glb.len() as u64,
            root_motion: built.root_motion,
            root_node: built.root_node,
            root_start: built.root_start.map(round4),
            root_end: built.root_end.map(round4),
        });
    }
    report
        .entries
        .sort_by(|a, b| a.name.cmp(&b.name).then_with(|| a.file.cmp(&b.file)));
    let keep: HashSet<&str> = report.entries.iter().map(|e| e.file.as_str()).collect();
    if let Ok(rd) = std::fs::read_dir(job.out_dir) {
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            if name.ends_with(".glb") && !keep.contains(name.as_str()) {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
    let index = serde_json::to_vec(&report.entries).map_err(|e| e.to_string())?;
    super::store::write_atomic(&job.out_dir.join("index.json"), &index)
        .map_err(|e| format!("index.json: {e}"))?;
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extraction::rig::anim::{TrackBinding, crc32};

    #[test]
    fn sanitizes_like_three() {
        assert_eq!(sanitize_node_name("Bip001 L.Thigh [x]"), "Bip001_LThigh_x");
        assert_eq!(sanitize_node_name("a/b:c"), "abc");
    }

    fn model() -> Value {
        // 0 root ─ 1 Bip001 ─ 2 Body (renderer) ; 3 Body (skinned, scene root, joints [1])
        json!({
            "scene": 0,
            "scenes": [{"nodes": [0, 3]}],
            "asset": {"extras": {"animators": [{"node": "root"}]}},
            "nodes": [
                {"name": "root", "children": [1]},
                {"name": "Bip001", "translation": [0.0, 1.0, 0.0], "children": [2]},
                {"name": "Body"},
                {"name": "Body", "skin": 0, "mesh": 0}
            ],
            "skins": [{"joints": [1]}]
        })
    }

    #[test]
    fn three_names_follow_load_order() {
        let names = three_names(&model());
        assert_eq!(
            names,
            vec![
                Some("root".into()),
                Some("Bip001".into()),
                Some("Body".into()),
                Some("Body_1".into())
            ]
        );
        let rig = ModelRig::from_document(&model()).unwrap();
        assert!(rig.ambiguous.is_empty());
        assert_eq!(rig.clip_names[3].as_deref(), Some("Body_1"));
        assert_eq!(rig.anim_root, 0);
        assert_eq!(rig.hashes[&crc32(b"Bip001/Body")], 2);
    }

    #[test]
    fn ambiguous_suffix_collisions_are_detected() {
        let doc = json!({
            "scenes": [{"nodes": [0]}],
            "nodes": [
                {"name": "r", "children": [1, 2, 3]},
                {"name": "X"}, {"name": "X"}, {"name": "X_1"}
            ]
        });
        let rig = ModelRig::from_document(&doc).unwrap();
        // node 2 becomes X_1, and so does node 3.
        assert!(rig.ambiguous.contains(&2) && rig.ambiguous.contains(&3));
        assert!(!rig.ambiguous.contains(&1));
    }

    #[test]
    fn clip_holds_only_targeted_nodes() {
        let rig = ModelRig::from_document(&model()).unwrap();
        let mut samples = Vec::new();
        for s in 0..3 {
            // rotation xyzw, translation, scale
            samples.extend_from_slice(&[0.0, 0.0, 0.0, 1.0, s as f32, 0.0, 0.0, 1.0, 1.0, 1.0]);
        }
        let clip = DecodedClip {
            name: "A_actor_x_walk".into(),
            sample_rate: 30.0,
            num_samples: 3,
            tracks: vec![TrackBinding {
                path_hash: crc32(b"Bip001"),
                position: true,
                rotation: true,
                scale: false,
            }],
            samples,
        };
        let rm = RootMotion {
            positions: vec![[0.0, 0.0, 0.0], [0.0, 0.0, 1.0], [0.0, 0.0, 2.0]],
            rotations: vec![[0.0, 0.0, 0.0, 1.0]; 3],
        };
        let tracks = transform_tracks(&clip).unwrap();
        let timing = ClipTiming {
            samples: 3,
            rate: 30.0,
        };
        let out = clip_glb(
            &rig,
            &clip.name,
            timing,
            &tracks,
            Some(&rm),
            TOLERANCE,
            json!({}),
        )
        .unwrap();
        let (doc, _) = gltf::parse_glb(&out.glb).unwrap();
        let names: Vec<&str> = doc["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|n| n["name"].as_str().unwrap())
            .collect();
        assert_eq!(names, ["root", "Bip001"]);
        assert_eq!(doc["scenes"][0]["nodes"], json!([0]));
        assert!(doc.get("meshes").is_none());
        let channels = doc["animations"][0]["channels"].as_array().unwrap();
        // Bip001 translation (its rotation never leaves the rest pose: no channel) + root
        // translation (root motion).
        let targets: Vec<(u64, &str)> = channels
            .iter()
            .map(|c| {
                (
                    c["target"]["node"].as_u64().unwrap(),
                    c["target"]["path"].as_str().unwrap(),
                )
            })
            .collect();
        assert_eq!(targets.len(), 2);
        assert!(targets.contains(&(1, "translation")) && targets.contains(&(0, "translation")));
        assert!(out.root_motion);
        assert_eq!(out.root_start, Some([0.0, 0.0, 0.0]));
        assert_eq!(out.root_end, Some([0.0, 0.0, 2.0]));
        assert_eq!(out.root_node.as_deref(), Some("root"));
        assert_eq!(out.stats.matched_tracks, 1);
    }

    #[test]
    fn clip_root_is_left_out_unless_animated() {
        let rig = ModelRig::from_document(&model()).unwrap();
        let track = NodeTrack {
            path_hash: crc32(b"Bip001"),
            rotation: Some(vec![
                [0.0, 0.0, 0.0, 1.0],
                [0.0, 0.0, 0.5f32.sin(), 0.5f32.cos()],
            ]),
            ..NodeTrack::default()
        };
        let timing = ClipTiming {
            samples: 2,
            rate: 30.0,
        };
        let out = clip_glb(&rig, "x", timing, &[track], None, TOLERANCE, json!({})).unwrap();
        let (doc, bin) = gltf::parse_glb(&out.glb).unwrap();
        assert_eq!(doc["nodes"], json!([{"name": "Bip001"}]));
        assert_eq!(
            doc["animations"][0]["channels"][0]["target"],
            json!({"node": 0, "path": "rotation"})
        );
        // Rotations are normalized shorts.
        let output = doc["animations"][0]["samplers"][0]["output"]
            .as_u64()
            .unwrap() as usize;
        assert_eq!(doc["accessors"][output]["componentType"], 5122);
        assert!(gltf::check_document(&doc, bin).is_empty());
        assert!(!out.root_motion);
    }

    #[test]
    fn motion_bone_travel_is_not_doubled() {
        let mut rig = ModelRig::from_document(&model()).unwrap();
        rig.set_motion_bone(Some(crc32(b"Bip001")));
        let track = NodeTrack {
            path_hash: crc32(b"Bip001"),
            translation: Some(vec![[0.0, 1.0, 0.0], [0.0, 1.0, 2.0]]),
            ..NodeTrack::default()
        };
        let rm = RootMotion {
            positions: vec![[0.0; 3], [0.0, 0.0, 2.0]],
            rotations: vec![[0.0, 0.0, 0.0, 1.0]; 2],
        };
        let timing = ClipTiming {
            samples: 2,
            rate: 30.0,
        };
        let out = clip_glb(&rig, "x", timing, &[track], Some(&rm), TOLERANCE, json!({})).unwrap();
        let (doc, _) = gltf::parse_glb(&out.glb).unwrap();
        // Only Bip001 is animated; the clip root gets no root-motion channel.
        assert_eq!(doc["nodes"], json!([{"name": "Bip001"}]));
        assert_eq!(
            doc["animations"][0]["channels"].as_array().unwrap().len(),
            1
        );
        assert!(out.root_motion);
        assert_eq!(out.root_node.as_deref(), Some("Bip001"));
        assert_eq!(out.root_end, Some([0.0, 1.0, 2.0]));
    }

    #[test]
    fn parked_constant_root_is_named() {
        let mut rig = ModelRig::from_document(&model()).unwrap();
        rig.set_motion_bone(Some(crc32(b"Bip001")));
        let track = NodeTrack {
            path_hash: crc32(b"Bip001"),
            translation: Some(vec![[-0.9, 0.0, -1.2]; 2]),
            ..NodeTrack::default()
        };
        let timing = ClipTiming {
            samples: 2,
            rate: 30.0,
        };
        let out = clip_glb(&rig, "x", timing, &[track], None, TOLERANCE, json!({})).unwrap();
        assert!(!out.root_motion);
        assert_eq!(out.root_node.as_deref(), Some("Bip001"));
        let parked = convert::position([-0.9, 0.0, -1.2]);
        assert_eq!(out.root_start, Some(parked));
        assert_eq!(out.root_end, Some(parked));
        // Constant at the rest pose: nothing to hold, no channel, no node.
        let track = NodeTrack {
            path_hash: crc32(b"Bip001"),
            translation: Some(vec![[0.0, 1.0, 0.0]; 2]),
            rotation: Some(vec![
                [0.0, 0.0, 0.0, 1.0],
                [0.0, 0.0, 0.5f32.sin(), 0.5f32.cos()],
            ]),
            ..NodeTrack::default()
        };
        let out = clip_glb(&rig, "x", timing, &[track], None, TOLERANCE, json!({})).unwrap();
        assert!(!out.root_motion && out.root_node.is_none());
    }

    #[test]
    fn motion_bone_is_found_beside_clip_root_rotation() {
        let mut rig = ModelRig::from_document(&model()).unwrap();
        rig.set_motion_bone(Some(crc32(b"Bip001")));
        let turn = [0.0, 0.0, 0.5f32.sin(), 0.5f32.cos()];
        let tracks = [
            // The clip root's own rotation-only track.
            NodeTrack {
                path_hash: rig
                    .hashes
                    .iter()
                    .find(|&(_, &n)| n == rig.anim_root)
                    .map(|(&h, _)| h)
                    .unwrap(),
                rotation: Some(vec![[0.0, 0.0, 0.0, 1.0], turn]),
                ..NodeTrack::default()
            },
            NodeTrack {
                path_hash: crc32(b"Bip001"),
                translation: Some(vec![[1.9, 0.0, -1.5]; 2]),
                ..NodeTrack::default()
            },
        ];
        let timing = ClipTiming {
            samples: 2,
            rate: 30.0,
        };
        let out = clip_glb(&rig, "x", timing, &tracks, None, TOLERANCE, json!({})).unwrap();
        assert!(!out.root_motion);
        assert_eq!(out.root_node.as_deref(), Some("Bip001"));
        assert_eq!(out.root_start, Some(convert::position([1.9, 0.0, -1.5])));
    }

    #[test]
    fn parked_skeleton_root_is_named_without_a_motion_bone() {
        // A humanoid model (no motion bone): clip root ─ Root ─ Bip001 (the skin's joint).
        let doc = json!({
            "scene": 0,
            "scenes": [{"nodes": [0, 3]}],
            "asset": {"extras": {"animators": [{"node": "root"}]}},
            "nodes": [
                {"name": "root", "children": [1]},
                {"name": "Root", "children": [2]},
                {"name": "Bip001", "translation": [0.0, 1.0, 0.0]},
                {"name": "Body", "skin": 0, "mesh": 0}
            ],
            "skins": [{"joints": [2]}]
        });
        let rig = ModelRig::from_document(&doc).unwrap();
        assert_eq!(rig.skeleton_root(), Some(1));
        let track = NodeTrack {
            path_hash: crc32(b"Root"),
            translation: Some(vec![[-0.9, 0.0, -1.2]; 2]),
            ..NodeTrack::default()
        };
        // Still root motion curves do not hide the parked bone.
        let rm = RootMotion {
            positions: vec![[0.0; 3]; 2],
            rotations: vec![[0.0, 0.0, 0.0, 1.0]; 2],
        };
        let timing = ClipTiming {
            samples: 2,
            rate: 30.0,
        };
        let out = clip_glb(&rig, "x", timing, &[track], Some(&rm), TOLERANCE, json!({})).unwrap();
        assert!(!out.root_motion);
        assert_eq!(out.root_node.as_deref(), Some("Root"));
        assert_eq!(out.root_start, Some(convert::position([-0.9, 0.0, -1.2])));
        // A joint directly below the clip root is the body, never a placement.
        assert_eq!(
            ModelRig::from_document(&model()).unwrap().skeleton_root(),
            None
        );
    }

    #[test]
    fn still_root_motion_curves_name_no_root() {
        let rig = ModelRig::from_document(&model()).unwrap();
        let track = NodeTrack {
            path_hash: crc32(b"Bip001"),
            rotation: Some(vec![
                [0.0, 0.0, 0.0, 1.0],
                [0.0, 0.0, 0.5f32.sin(), 0.5f32.cos()],
            ]),
            ..NodeTrack::default()
        };
        let rm = RootMotion {
            positions: vec![[0.0; 3]; 2],
            rotations: vec![[0.0, 0.0, 0.0, 1.0]; 2],
        };
        let timing = ClipTiming {
            samples: 2,
            rate: 30.0,
        };
        let out = clip_glb(&rig, "x", timing, &[track], Some(&rm), TOLERANCE, json!({})).unwrap();
        assert!(!out.root_motion);
        assert!(out.root_node.is_none());
        assert!(out.root_start.is_some());
    }

    #[test]
    fn key_reduction_keeps_corners() {
        let keys: Vec<f32> = vec![0.0, 1.0, 2.0, 3.0, 2.0, 1.0, 1.0, 1.0];
        let kept = reduce_keys(&keys, 1e-4, |a, b, t| a + (b - a) * t, |a, b| (a - b).abs());
        assert_eq!(kept, vec![0, 3, 5, 7]);
        assert_eq!(
            reduce_keys(&[1.0f32, 2.0], 0.1, |a, _, _| a, |a, b| (a - b).abs()),
            vec![0, 1]
        );
    }

    #[test]
    fn names_and_sources() {
        assert_eq!(
            pretty_name("A_actor_chen_battle_attack_01", "chen"),
            "battle attack 01"
        );
        assert_eq!(pretty_name("A_monster_hsmino_idle", "hsmino1"), "idle");
        assert_eq!(pretty_name("custom_clip", "chen"), "custom clip");
        assert_eq!(
            clip_source("assets/a/chen/animations/battle/a_actor_chen_attack.fbx"),
            "battle"
        );
        assert_eq!(
            clip_source("assets/a/monster/animations/hsmino/3c/a.fbx"),
            "3c"
        );
        assert_eq!(
            clip_source("assets/a/chen/animations/customized/x.fbx"),
            "other"
        );
        assert_eq!(clip_source("assets/a/chen/animations/ui.fbx"), "other");
        assert_eq!(
            clip_file_stem("A_actor_chen idle.01"),
            "a_actor_chen_idle_01"
        );
        assert!(clip_loops(&json!({"m_MuscleClip": {"m_LoopTime": true}})));
        assert!(!clip_loops(&json!({})));
    }
}
