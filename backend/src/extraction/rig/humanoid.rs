//! Unity Humanoid (Mecanim) clips → per-bone transforms.
//!
//! A humanoid clip does not animate the body's Transforms: it stores muscle curves
//! (`genericBindings` with `typeID` 95 / `customType` 8, decoded from the ACL float buffer)
//! and the Avatar's human description turns them into bone rotations. Curve attributes:
//! `0..7` MotionT/MotionQ (root motion), `7..14` RootT/RootQ (the body frame, in units of the
//! avatar's human scale), `14..42` IK goals (unused), then 61 body muscles and 40 finger
//! muscles (left hand, then right). Endfield's body muscles are Unity's 55 with three extra
//! DoFs after each leg's eight (body 9, head 12, legs 11 + 11, arms 9 + 9), as its T-pose
//! clips and `m_DeltaPose.m_DoFArray` (61 entries) show; the extras are not applied.
//!
//! Per human bone with axes `A` and muscles `m = (x, y, z)` (twist, then the two swing
//! DoFs), Mecanim computes, all in Unity space:
//!
//! ```text
//! angle_i = m_i * (m_i < 0 ? -A.min_i : A.max_i)        // LimitUnproject
//! v       = tan(0.5 * A.sgn * angle)                      // halfTan
//! zy      = normalize(v.x, v.y + v.x·v.z, v.z − v.x·v.y, 1)  // ZYRoll2Quat
//! local   = A.preQ · zy · conj(A.postQ)
//! ```
//!
//! The hips are placed from the body frame: `hips = body · inverse(human.rootX) · hips_T`
//! (`hips_T` being the hips in the avatar's T-pose), relative to the root motion frame.
//! Arm/leg twist redistribution and translation DoFs are not applied.
use super::anim::crc32;
use serde_json::Value;

/// First muscle curve attribute.
pub const MUSCLES: usize = 42;
/// First finger muscle curve attribute.
pub const FINGERS: usize = MUSCLES + 61;
/// Humanoid curve attributes this module reads.
pub const CURVES: usize = FINGERS + 40;

/// Muscle (index into the 61 body muscles) driving each human bone's x (twist), y, z axis,
/// by Mecanim bone order (`Hips, LeftUpperLeg, RightUpperLeg, LeftLowerLeg, …, Jaw`).
/// Blocks: body 0, head 9, left leg 21, right leg 32, left arm 43, right arm 52.
const BONE_DOF: [[i8; 3]; 25] = [
    [-1, -1, -1], // Hips
    [23, 22, 21], // LeftUpperLeg: twist, in-out, front-back
    [34, 33, 32], // RightUpperLeg
    [25, -1, 24], // LeftLowerLeg: twist, -, stretch
    [36, -1, 35], // RightLowerLeg
    [-1, 27, 26], // LeftFoot: -, twist in-out, up-down
    [-1, 38, 37], // RightFoot
    [2, 1, 0],    // Spine: twist, left-right, front-back
    [5, 4, 3],    // Chest
    [8, 7, 6],    // UpperChest
    [11, 10, 9],  // Neck
    [14, 13, 12], // Head
    [-1, 44, 43], // LeftShoulder: -, front-back, down-up
    [-1, 53, 52], // RightShoulder
    [47, 46, 45], // LeftUpperArm: twist, front-back, down-up
    [56, 55, 54], // RightUpperArm
    [49, -1, 48], // LeftLowerArm: twist, -, stretch
    [58, -1, 57], // RightLowerArm
    [-1, 51, 50], // LeftHand: -, in-out, down-up
    [-1, 60, 59], // RightHand
    [-1, -1, 28], // LeftToes
    [-1, -1, 39], // RightToes
    [-1, 16, 15], // LeftEye
    [-1, 18, 17], // RightEye
    [-1, 20, 19], // Jaw
];

/// Unity-space rigid transform.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Xform {
    pub t: [f32; 3],
    pub q: [f32; 4],
}

impl Xform {
    pub const IDENTITY: Self = Self {
        t: [0.0; 3],
        q: [0.0, 0.0, 0.0, 1.0],
    };

    #[must_use]
    pub fn mul(&self, b: &Self) -> Self {
        let r = qrot(self.q, b.t);
        Self {
            t: [self.t[0] + r[0], self.t[1] + r[1], self.t[2] + r[2]],
            q: qmul(self.q, b.q),
        }
    }

    #[must_use]
    pub fn inverse(&self) -> Self {
        let qi = qconj(self.q);
        let t = qrot(qi, self.t);
        Self {
            t: [-t[0], -t[1], -t[2]],
            q: qi,
        }
    }
}

#[must_use]
pub fn qmul(a: [f32; 4], b: [f32; 4]) -> [f32; 4] {
    let [ax, ay, az, aw] = a;
    let [bx, by, bz, bw] = b;
    [
        aw * bx + ax * bw + ay * bz - az * by,
        aw * by - ax * bz + ay * bw + az * bx,
        aw * bz + ax * by - ay * bx + az * bw,
        aw * bw - ax * bx - ay * by - az * bz,
    ]
}

#[must_use]
pub fn qconj(q: [f32; 4]) -> [f32; 4] {
    [-q[0], -q[1], -q[2], q[3]]
}

#[must_use]
pub fn qrot(q: [f32; 4], v: [f32; 3]) -> [f32; 3] {
    let p = qmul(qmul(q, [v[0], v[1], v[2], 0.0]), qconj(q));
    [p[0], p[1], p[2]]
}

fn qnormalize(q: [f32; 4]) -> [f32; 4] {
    let len = q.iter().map(|x| x * x).sum::<f32>().sqrt();
    if !len.is_finite() || len < 1e-8 {
        [0.0, 0.0, 0.0, 1.0]
    } else {
        q.map(|x| x / len)
    }
}

fn f(v: Option<&Value>, k: &str, d: f32) -> f32 {
    v.and_then(|v| v.get(k))
        .and_then(Value::as_f64)
        .map_or(d, |x| x as f32)
}

fn vec3(v: Option<&Value>, d: f32) -> [f32; 3] {
    [f(v, "x", d), f(v, "y", d), f(v, "z", d)]
}

fn quat(v: Option<&Value>) -> [f32; 4] {
    [
        f(v, "x", 0.0),
        f(v, "y", 0.0),
        f(v, "z", 0.0),
        f(v, "w", 1.0),
    ]
}

fn xform(v: Option<&Value>) -> Xform {
    Xform {
        t: vec3(v.and_then(|v| v.get("t")), 0.0),
        q: quat(v.and_then(|v| v.get("q"))),
    }
}

/// Mecanim `Axes` of a human bone.
#[derive(Debug, Clone, Copy)]
pub struct Axes {
    pub pre_q: [f32; 4],
    pub post_q: [f32; 4],
    pub sgn: [f32; 3],
    pub min: [f32; 3],
    pub max: [f32; 3],
}

impl Axes {
    /// Local rotation (relative to the parent node) for muscle values `m` (x, y, z).
    #[must_use]
    pub fn rotation(&self, m: [f32; 3]) -> [f32; 4] {
        let half = |i: usize| {
            let angle = m[i]
                * if m[i] < 0.0 {
                    -self.min[i]
                } else {
                    self.max[i]
                };
            let a = (0.5 * self.sgn[i] * angle).clamp(-1.5, 1.5);
            a.tan()
        };
        let (x, y, z) = (half(0), half(1), half(2));
        let zy = qnormalize([x, y + x * z, z - x * y, 1.0]);
        qnormalize(qmul(qmul(self.pre_q, zy), qconj(self.post_q)))
    }
}

/// One node of the human skeleton.
#[derive(Debug, Clone)]
pub struct HumanNode {
    /// Path hash below the Animator (same hashing as clip bindings).
    pub path_hash: u32,
    pub parent: Option<usize>,
    pub axes: Option<Axes>,
    /// T-pose local transform.
    pub pose: Xform,
}

/// The human description of an Avatar.
#[derive(Debug, Clone)]
pub struct Humanoid {
    pub scale: f32,
    pub nodes: Vec<HumanNode>,
    /// Human skeleton node of each Mecanim bone (`None` when unmapped).
    pub bones: Vec<Option<usize>>,
    /// Human skeleton node of each finger bone, left hand then right (15 each).
    pub fingers: Vec<Option<usize>>,
    /// Hips relative to the body frame: `inverse(rootX) · hips_T`.
    pub hips_offset: Xform,
    /// T-pose global transform of the hips' parent.
    pub hips_parent: Xform,
}

impl Humanoid {
    /// Read `m_Avatar.m_Human` of an Avatar object.
    pub fn from_avatar(avatar: &Value) -> Result<Self, String> {
        let human = avatar
            .pointer("/m_Avatar/m_Human/data")
            .ok_or("avatar has no human description")?;
        let skeleton = human
            .pointer("/m_Skeleton/data")
            .ok_or("human has no skeleton")?;
        let ids = skeleton
            .get("m_ID")
            .and_then(Value::as_array)
            .ok_or("human skeleton has no ids")?;
        let nodes_v = skeleton
            .get("m_Node")
            .and_then(Value::as_array)
            .ok_or("human skeleton has no nodes")?;
        let axes_v = skeleton
            .get("m_AxesArray")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let pose_v = human
            .pointer("/m_SkeletonPose/data/m_X")
            .and_then(Value::as_array)
            .ok_or("human has no skeleton pose")?;
        if nodes_v.is_empty() {
            return Err("generic Avatar (no human description)".into());
        }
        if ids.len() != nodes_v.len() || pose_v.len() != nodes_v.len() {
            return Err("human skeleton arrays disagree".into());
        }
        let mut nodes = Vec::with_capacity(nodes_v.len());
        for (i, n) in nodes_v.iter().enumerate() {
            let parent = n
                .get("m_ParentId")
                .and_then(Value::as_i64)
                .and_then(|p| usize::try_from(p).ok())
                .filter(|&p| p < i);
            let axes = n
                .get("m_AxesId")
                .and_then(Value::as_i64)
                .and_then(|a| usize::try_from(a).ok())
                .and_then(|a| axes_v.get(a))
                .map(|a| {
                    let limit = a.get("m_Limit");
                    Axes {
                        pre_q: quat(a.get("m_PreQ")),
                        post_q: quat(a.get("m_PostQ")),
                        sgn: vec3(a.get("m_Sgn"), 1.0),
                        min: vec3(limit.and_then(|l| l.get("m_Min")), 0.0),
                        max: vec3(limit.and_then(|l| l.get("m_Max")), 0.0),
                    }
                });
            nodes.push(HumanNode {
                path_hash: ids[i].as_u64().unwrap_or(0) as u32,
                parent,
                axes,
                pose: xform(Some(&pose_v[i])),
            });
        }
        let index = |v: Option<&Value>| -> Vec<Option<usize>> {
            v.and_then(Value::as_array)
                .into_iter()
                .flatten()
                .map(|x| {
                    x.as_i64()
                        .and_then(|i| usize::try_from(i).ok())
                        .filter(|&i| i < nodes.len())
                })
                .collect()
        };
        let mut bones = index(human.get("m_HumanBoneIndex"));
        bones.resize(BONE_DOF.len(), None);
        let hips = bones[0].ok_or("avatar maps no hips")?;
        let mut fingers = Vec::new();
        for (hand, has) in [
            ("m_LeftHand", "m_HasLeftHand"),
            ("m_RightHand", "m_HasRightHand"),
        ] {
            let mut f = if human.get(has).and_then(Value::as_bool).unwrap_or(false) {
                index(human.pointer(&format!("/{hand}/data/m_HandBoneIndex")))
            } else {
                Vec::new()
            };
            f.resize(15, None);
            fingers.extend(f);
        }
        let global = |mut i: usize| {
            let mut x = nodes[i].pose;
            while let Some(p) = nodes[i].parent {
                x = nodes[p].pose.mul(&x);
                i = p;
            }
            x
        };
        let hips_parent = nodes[hips].parent.map_or(Xform::IDENTITY, global);
        let mut root_x = xform(human.get("m_RootX"));
        let scale = human
            .get("m_Scale")
            .and_then(Value::as_f64)
            .map_or(1.0, |s| s as f32);
        if !(scale.is_finite() && scale > 0.0) {
            return Err(format!("human scale {scale}"));
        }
        root_x.q = qnormalize(root_x.q);
        Ok(Self {
            scale,
            hips_offset: root_x.inverse().mul(&global(hips)),
            hips_parent,
            nodes,
            bones,
            fingers,
        })
    }

    /// Local transforms (Unity space) of every driven human node for one sample of curves
    /// (indexed by curve attribute, see the module docs): `(path hash, translation, rotation)`;
    /// only the hips get a translation.
    #[must_use]
    pub fn pose(&self, curves: &[f32]) -> Vec<(u32, Option<[f32; 3]>, [f32; 4])> {
        let c = |i: usize| curves.get(i).copied().unwrap_or(0.0);
        let mut out = Vec::new();
        for (bone, dof) in BONE_DOF.iter().enumerate().skip(1) {
            let Some(node) = self.bones.get(bone).copied().flatten() else {
                continue;
            };
            let Some(axes) = &self.nodes[node].axes else {
                continue;
            };
            let m = dof.map(|d| usize::try_from(d).ok().map_or(0.0, |d| c(MUSCLES + d)));
            out.push((self.nodes[node].path_hash, None, axes.rotation(m)));
        }
        // Fingers: per finger [1 stretched, spread, 2 stretched, 3 stretched]; the proximal
        // phalanx takes spread (y) and stretch (z), the others stretch only.
        for (k, node) in self.fingers.iter().enumerate() {
            let Some(node) = *node else { continue };
            let Some(axes) = &self.nodes[node].axes else {
                continue;
            };
            let (hand, finger, phalanx) = (k / 15, (k % 15) / 3, k % 3);
            let base = FINGERS + hand * 20 + finger * 4;
            let m = match phalanx {
                0 => [0.0, c(base + 1), c(base)],
                1 => [0.0, 0.0, c(base + 2)],
                _ => [0.0, 0.0, c(base + 3)],
            };
            out.push((self.nodes[node].path_hash, None, axes.rotation(m)));
        }
        if let Some(hips) = self.bones[0] {
            let s = self.scale;
            let motion = Xform {
                t: [c(0) * s, c(1) * s, c(2) * s],
                q: qnormalize([c(3), c(4), c(5), c(6)]),
            };
            let body = Xform {
                t: [c(7) * s, c(8) * s, c(9) * s],
                q: qnormalize([c(10), c(11), c(12), c(13)]),
            };
            let local = self
                .hips_parent
                .inverse()
                .mul(&motion.inverse())
                .mul(&body)
                .mul(&self.hips_offset);
            out.push((self.nodes[hips].path_hash, Some(local.t), local.q));
        }
        out
    }

    /// Root motion (Unity space, scaled) of one sample of curves.
    #[must_use]
    pub fn motion(&self, curves: &[f32]) -> ([f32; 3], [f32; 4]) {
        let c = |i: usize| curves.get(i).copied().unwrap_or(0.0);
        let s = self.scale;
        (
            [c(0) * s, c(1) * s, c(2) * s],
            qnormalize([c(3), c(4), c(5), c(6)]),
        )
    }
}

/// Whether a clip's bindings include muscle curves. Generic clips can carry MotionT/Q and
/// RootT/Q curves too (root motion), but never muscles.
#[must_use]
pub fn is_humanoid(clip: &Value) -> bool {
    clip.pointer("/m_ClipBindingConstant/genericBindings")
        .and_then(Value::as_array)
        .is_some_and(|b| {
            b.iter().any(|b| {
                is_muscle_binding(b)
                    && b.get("attribute").and_then(Value::as_u64) >= Some(MUSCLES as u64)
            })
        })
}

fn is_muscle_binding(b: &Value) -> bool {
    b.get("typeID").and_then(Value::as_i64) == Some(95)
        && b.get("customType").and_then(Value::as_i64) == Some(8)
}

/// Humanoid curves of a clip, `[sample][attribute]` with `CURVES` attributes per sample.
/// The ACL float buffer holds one track per non-Transform binding, in binding order; its
/// sample count can differ from the Transform buffer's by one, so samples are clamped.
pub fn decode_curves(clip: &Value, num_samples: usize) -> Result<Vec<f32>, String> {
    let bindings = clip
        .pointer("/m_ClipBindingConstant/genericBindings")
        .and_then(Value::as_array)
        .ok_or("clip has no bindings")?;
    let floats: Vec<&Value> = bindings
        .iter()
        .filter(|b| b.get("typeID").and_then(Value::as_i64) != Some(4))
        .collect();
    let buffer = clip
        .pointer("/m_AclCompressedBuffer/FloatBufferData")
        .and_then(crate::extraction::unity::value_bytes)
        .filter(|b| !b.is_empty())
        .ok_or("clip has no float curves")?;
    let (info, values) = super::acl::floats(&buffer)?;
    let tracks = info.num_tracks as usize;
    let float_samples = info.num_samples as usize;
    if tracks != floats.len() || float_samples == 0 || float_samples.abs_diff(num_samples) > 1 {
        return Err(format!(
            "float buffer has {tracks} tracks x {float_samples} samples, bindings name {} curves x {num_samples}",
            floats.len()
        ));
    }
    let mut out = vec![0f32; num_samples * CURVES];
    // Identity quaternions for MotionQ / RootQ when absent.
    for s in 0..num_samples {
        out[s * CURVES + 6] = 1.0;
        out[s * CURVES + 13] = 1.0;
    }
    for (t, b) in floats.iter().enumerate() {
        if !is_muscle_binding(b) {
            continue;
        }
        let Some(attr) = b
            .get("attribute")
            .and_then(Value::as_u64)
            .map(|a| a as usize)
            .filter(|&a| a < CURVES)
        else {
            continue;
        };
        for s in 0..num_samples {
            out[s * CURVES + attr] = values[s.min(float_samples - 1) * tracks + t];
        }
    }
    Ok(out)
}

/// `(container path lowercased, cab)` of every stored bundle holding an Avatar.
#[must_use]
pub fn avatar_containers(
    records: &[super::store::CabRecord],
    store: &std::path::Path,
) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = records
        .iter()
        .filter(|r| r.classes.contains_key(&90) && store.join("sf").join(&r.cab).is_file())
        .flat_map(|r| {
            r.containers
                .iter()
                .map(|(p, _, _)| (p.to_ascii_lowercase(), r.cab.clone()))
        })
        .collect();
    out.sort();
    out.dedup();
    out
}

/// The Avatar of an exported model: the Animator's avatar (by name, in bundles whose
/// container is named after it), else an Avatar in the model's own bundle.
pub fn find_avatar(
    src: &mut super::store::CabSource,
    avatars: &[(String, String)],
    doc: &Value,
) -> Result<Value, String> {
    const CLASS_AVATAR: i32 = 90;
    let name = doc
        .pointer("/asset/extras/animators/0/avatar")
        .and_then(Value::as_str);
    let mut cabs: Vec<String> = Vec::new();
    if let Some(name) = name {
        let lower = name.to_ascii_lowercase();
        let stem = lower.strip_suffix("avatar").unwrap_or(&lower);
        for (path, cab) in avatars {
            let file = path.rsplit('/').next().unwrap_or(path);
            let file_stem = file.rsplit_once('.').map_or(file, |(s, _)| s);
            if (file_stem == stem || file_stem == lower) && !cabs.contains(cab) {
                cabs.push(cab.clone());
            }
        }
    }
    if let Some(cab) = doc
        .pointer("/asset/extras/source/cab")
        .and_then(Value::as_str)
    {
        cabs.push(cab.to_string());
    }
    for cab in cabs {
        let Some(file) = src.get(&cab) else { continue };
        let sf = &src.files.files[file].sf;
        for obj in sf.objects.iter().filter(|o| o.class_id == CLASS_AVATAR) {
            if name.is_some() && sf.peek_name(obj).ok().flatten().as_deref() != name {
                continue;
            }
            return sf.read_object(obj).map_err(|e| e.to_string());
        }
    }
    Err(format!("no Avatar named {name:?} in the store"))
}

/// Path hash of a generic Avatar's root motion bone (`m_RootMotionBoneIndex`), whose own
/// Transform track carries the clip's travel.
#[must_use]
pub fn root_motion_bone(avatar: &Value) -> Option<u32> {
    let index = avatar
        .pointer("/m_Avatar/m_RootMotionBoneIndex")
        .and_then(Value::as_i64)
        .and_then(|i| usize::try_from(i).ok())?;
    skeleton_ids(avatar).get(index).copied()
}

/// Path hashes of an Avatar's skeleton (relative to its Animator).
#[must_use]
pub fn skeleton_ids(avatar: &Value) -> Vec<u32> {
    avatar
        .pointer("/m_Avatar/m_AvatarSkeleton/data/m_ID")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|v| v.as_u64().map(|v| v as u32))
        .collect()
}

/// Path hash of an avatar-relative path (as human skeleton ids are).
#[must_use]
pub fn path_hash(path: &str) -> u32 {
    crc32(path.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: [f32; 4], b: [f32; 4]) -> bool {
        let d: f32 = (0..4).map(|k| a[k] * b[k]).sum();
        d.abs() > 0.9999
    }

    #[test]
    fn zero_muscles_give_pre_times_conj_post() {
        let axes = Axes {
            pre_q: qnormalize([0.1, 0.2, 0.3, 0.9]),
            post_q: qnormalize([-0.2, 0.1, 0.0, 0.95]),
            sgn: [1.0, -1.0, 1.0],
            min: [-1.0; 3],
            max: [1.0; 3],
        };
        assert!(close(
            axes.rotation([0.0; 3]),
            qnormalize(qmul(axes.pre_q, qconj(axes.post_q)))
        ));
    }

    #[test]
    fn single_axis_muscle_is_a_plain_rotation() {
        let axes = Axes {
            pre_q: [0.0, 0.0, 0.0, 1.0],
            post_q: [0.0, 0.0, 0.0, 1.0],
            sgn: [1.0; 3],
            min: [-1.0; 3],
            max: [2.0; 3],
        };
        // z muscle 0.5 → 1 rad about Z; negative muscles scale by -min.
        let q = axes.rotation([0.0, 0.0, 0.5]);
        assert!(close(q, [0.0, 0.0, 0.5f32.sin(), 0.5f32.cos()]));
        let q = axes.rotation([-0.5, 0.0, 0.0]);
        assert!(close(q, [(-0.25f32).sin(), 0.0, 0.0, (-0.25f32).cos()]));
    }

    #[test]
    fn xform_inverse_round_trips() {
        let a = Xform {
            t: [1.0, 2.0, 3.0],
            q: qnormalize([0.3, -0.2, 0.1, 0.9]),
        };
        let id = a.mul(&a.inverse());
        assert!(id.t.iter().all(|x| x.abs() < 1e-5));
        assert!(close(id.q, [0.0, 0.0, 0.0, 1.0]));
    }
}
