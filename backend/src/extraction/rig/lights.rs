//! Per-character menu lighting, the same data as warfarin's `lights.json`: the key-light
//! volume of the character screen and its additional (NPR wrap/rim/fog) lights.
//!
//! Sources, all read from the scan store (native Light/Transform/GameObject components
//! included):
//! - `prefabs/charinfo/cameratracks/track_<chr>.prefab`: a `refDict` maps `volume_overview` to
//!   a component holding `charLightVolumeData` (an `HGCharacterVolume` block). Fields whose
//!   `overrideState` is 0 fall back to the global `prefabs/charinfo/charinfo_volume.asset`.
//! - `prefabs/charinfo/additionallights/light_<chr>.prefab`: a `refDict` maps `light_overview`
//!   to a light group (`m_LightList` of Light components, `m_LightInfoList` intensities). Each
//!   light's GameObject carries HG light data (`m_lightNPRType`…) and optionally a follower
//!   (`followType`, `followableNodeType`, offsets) that pins it to `Bip001` or `Head_Local`.
//! - The camera track's `vcam_overview` (a Cinemachine virtual camera under `DollyCart`):
//!   its Transform, `m_LookAt` target and `m_Lens` give the default framing (warfarin's
//!   `cameras.json`).
//!
//! Output (`<models>/lights/<id>.json`), coordinates converted to glTF handedness (x negated,
//! quaternions `(x, -y, -z, w)`):
//!
//! ```json
//! {"id": "chr_0004_pelica",
//!  "volume": {"multiplier", "envMul", "envShadowMul", "rangeBias", "minPitch", "yawBias",
//!             "lightColor": [r,g,b], "skinLightColor": [r,g,b],
//!             "shadowTint": [r,g,b], "skinShadowTint": [r,g,b], "mainLightShadow": bool},
//!  "lights": [{"name", "active", "type": "point|spot|directional|area",
//!              "color": [r,g,b] (sRGB), "colorLinear": [r,g,b], "intensity",
//!              "falloffExponent", "specularIntensity", "tubeLength", "shadows", "range",
//!              "spot": [outer°, inner°], "position": [x,y,z] (prefab space),
//!              "forward": [x,y,z], "npr": "wrap|ramped|specular|rim|fog|unknown",
//!              "nprData": [x,y,z,w], "rimWidth", "rimAlbedo", "rampBias", "characterOnly",
//!              "follower": null | {"on", "self", "type": "fixedOffset|parent",
//!                                  "node": "Bip001|Head_Local", "positionOffset": [x,y,z],
//!                                  "localPosition": [x,y,z], "localRotation": [x,y,z,w]}}],
//!  "camera": null | {"position": [x,y,z], "target": [x,y,z], "fov", "near", "far",
//!                     "screenOffset"?: [x,y], "referenceAspect"?},
//!  "source": {"lights": container, "track": container}}
//! ```
//!
//! Shadow tints are normalized by `(max + min) / 2` of their RGB, as the game's shader does.
//! A followed light sits relative to its bone: `fixedOffset` at `node + positionOffset` (in
//! model orientation), `parent` at `node + localPosition` rotated by the bone, facing
//! `localRotation`. Unfollowed lights use `position`/`forward` in model space.
use super::store::{CabRecord, CabSource};
use serde_json::{Map, Value, json};
use std::collections::HashMap;

const LIGHTS_DIR: &str = "assets/beyond/dynamicassets/gameplay/prefabs/charinfo/additionallights/";
const TRACKS_DIR: &str = "assets/beyond/dynamicassets/gameplay/prefabs/charinfo/cameratracks/";
const DEFAULT_VOLUME: &str =
    "assets/beyond/dynamicassets/gameplay/prefabs/charinfo/charinfo_volume.asset";

const CLASS_GAME_OBJECT: i32 = 1;
const CLASS_TRANSFORM: i32 = 4;
const CLASS_LIGHT: i32 = 108;
const CLASS_MONO_BEHAVIOUR: i32 = 114;

/// Objects of one SerializedFile by path id, with class.
struct Objects {
    by_id: HashMap<i64, (i32, Value)>,
}

impl Objects {
    fn load(src: &mut CabSource, cab: &str, classes: &[i32]) -> Option<Self> {
        let i = src.get(cab)?;
        let sf = &src.files.files[i].sf;
        let by_id = sf
            .objects
            .iter()
            .filter(|o| classes.contains(&o.class_id))
            .filter_map(|o| Some((o.path_id, (o.class_id, sf.read_object(o).ok()?))))
            .collect();
        Some(Self { by_id })
    }

    fn get(&self, id: i64) -> Option<&Value> {
        self.by_id.get(&id).map(|(_, v)| v)
    }

    fn of_class(&self, class: i32) -> impl Iterator<Item = (i64, &Value)> {
        self.by_id
            .iter()
            .filter(move |(_, (c, _))| *c == class)
            .map(|(id, (_, v))| (*id, v))
    }

    /// The object a local PPtr points at (`m_FileID` 0 only).
    fn deref(&self, pptr: Option<&Value>) -> Option<(i64, &Value)> {
        let p = pptr?;
        if p.get("m_FileID").and_then(Value::as_i64) != Some(0) {
            return None;
        }
        let id = p.get("m_PathID").and_then(Value::as_i64)?;
        Some((id, self.get(id)?))
    }

    /// The component a `refDict` (`_keys` / `_values`) of any MonoBehaviour maps `key` to.
    fn ref_dict(&self, key: &str) -> Option<(i64, &Value)> {
        self.of_class(CLASS_MONO_BEHAVIOUR).find_map(|(_, mb)| {
            let dict = mb.get("refDict")?;
            let keys = dict.get("_keys")?.as_array()?;
            let i = keys.iter().position(|k| k.as_str() == Some(key))?;
            self.deref(dict.get("_values")?.as_array()?.get(i))
        })
    }

    /// MonoBehaviours on a GameObject.
    fn behaviours_of(&self, go: i64) -> impl Iterator<Item = &Value> {
        self.of_class(CLASS_MONO_BEHAVIOUR)
            .filter(move |(_, mb)| game_object(mb) == Some(go))
            .map(|(_, mb)| mb)
    }
}

fn game_object(component: &Value) -> Option<i64> {
    component.pointer("/m_GameObject/m_PathID")?.as_i64()
}

fn f(v: Option<&Value>) -> Option<f64> {
    v.and_then(Value::as_f64)
}

fn vec3(v: Option<&Value>) -> [f64; 3] {
    let g = |k| f(v.and_then(|v| v.get(k))).unwrap_or(0.0);
    [g("x"), g("y"), g("z")]
}

fn quat(v: Option<&Value>) -> [f64; 4] {
    let g = |k, d| f(v.and_then(|v| v.get(k))).unwrap_or(d);
    [g("x", 0.0), g("y", 0.0), g("z", 0.0), g("w", 1.0)]
}

fn vec4(v: Option<&Value>) -> [f64; 4] {
    let g = |k| f(v.and_then(|v| v.get(k))).unwrap_or(0.0);
    [g("x"), g("y"), g("z"), g("w")]
}

fn rgb(v: Option<&Value>) -> [f64; 3] {
    let g = |k| f(v.and_then(|v| v.get(k))).unwrap_or(1.0);
    [g("r"), g("g"), g("b")]
}

fn round(x: f64, digits: i32) -> f64 {
    let p = 10f64.powi(digits);
    let r = (x * p).round() / p;
    if r == 0.0 { 0.0 } else { r }
}

fn round_all<const N: usize>(v: [f64; N], digits: i32) -> Value {
    json!(v.iter().map(|x| round(*x, digits)).collect::<Vec<_>>())
}

fn srgb_to_linear(c: f64) -> f64 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

fn quat_mul(a: [f64; 4], b: [f64; 4]) -> [f64; 4] {
    let [ax, ay, az, aw] = a;
    let [bx, by, bz, bw] = b;
    [
        aw * bx + ax * bw + ay * bz - az * by,
        aw * by - ax * bz + ay * bw + az * bx,
        aw * bz + ax * by - ay * bx + az * bw,
        aw * bw - ax * bx - ay * by - az * bz,
    ]
}

fn rotate(q: [f64; 4], v: [f64; 3]) -> [f64; 3] {
    let p = quat_mul(
        quat_mul(q, [v[0], v[1], v[2], 0.0]),
        [-q[0], -q[1], -q[2], q[3]],
    );
    [p[0], p[1], p[2]]
}

/// Unity Euler angles (degrees; applied Z, then X, then Y) → quaternion.
#[must_use]
pub fn euler_to_quat(e: [f64; 3]) -> [f64; 4] {
    let axis = |i: usize, deg: f64| {
        let h = deg.to_radians() / 2.0;
        let mut q = [0.0, 0.0, 0.0, h.cos()];
        q[i] = h.sin();
        q
    };
    quat_mul(quat_mul(axis(1, e[1]), axis(0, e[0])), axis(2, e[2]))
}

/// Unity position / direction → glTF (x negated).
fn flip(v: [f64; 3]) -> [f64; 3] {
    [-v[0], v[1], v[2]]
}

/// Unity rotation → glTF `(x, -y, -z, w)`.
fn flip_quat(q: [f64; 4]) -> [f64; 4] {
    [q[0], -q[1], -q[2], q[3]]
}

/// Shadow tint normalized by the mid-range of its channels.
#[must_use]
pub fn normalize_tint(c: [f64; 3]) -> [f64; 3] {
    let max = c.iter().copied().fold(f64::MIN, f64::max);
    let min = c.iter().copied().fold(f64::MAX, f64::min);
    let mid = (max + min) / 2.0;
    if mid <= 0.0 { c } else { c.map(|x| x / mid) }
}

/// `volume` block from a character's `charLightVolumeData`, falling back field by field
/// (`overrideState` 0) to the global character volume.
#[must_use]
pub fn volume(data: &Value, defaults: Option<&Value>) -> Value {
    let field = |k: &str| -> Option<&Value> {
        let own = data.get(k);
        let overridden = own
            .and_then(|o| o.get("overrideState"))
            .and_then(Value::as_i64)
            .is_some_and(|s| s != 0);
        if overridden || defaults.is_none() {
            own?.get("m_Value")
        } else {
            defaults?
                .get(k)?
                .get("m_Value")
                .or_else(|| own?.get("m_Value"))
        }
    };
    let num = |k: &str, d: f64| round(f(field(k)).unwrap_or(d), 4);
    let bias = field("charCameraFollowMainLightBias");
    let ignore_shadow = field("charIgnoreMainLightShadow")
        .and_then(|v| v.as_i64().or_else(|| v.as_bool().map(i64::from)))
        .unwrap_or(0);
    json!({
        "multiplier": num("charMainLightMultiplier", 0.9),
        "envMul": num("charEnvLightMultiplier", 0.8),
        "envShadowMul": num("charEnvShadowMultiplier", 0.8),
        "rangeBias": num("charMainLightRangeBias", -0.4),
        "minPitch": round(f(bias.and_then(|b| b.get("x"))).unwrap_or(32.0), 4),
        "yawBias": round(f(bias.and_then(|b| b.get("y"))).unwrap_or(10.0), 4),
        "lightColor": round_all(rgb(field("charMainLightOverrideColor")), 4),
        "skinLightColor": round_all(rgb(field("charSkinMainLightOverrideColor")), 4),
        "shadowTint": round_all(normalize_tint(rgb(field("charShadowTintColor"))), 4),
        "skinShadowTint": round_all(normalize_tint(rgb(field("charSkinShadowTintColor"))), 4),
        "mainLightShadow": ignore_shadow == 0,
    })
}

fn light_type(code: i64) -> &'static str {
    match code {
        0 => "spot",
        1 => "directional",
        2 => "point",
        _ => "area",
    }
}

fn npr_type(code: i64) -> &'static str {
    match code {
        0 => "wrap",
        1 => "ramped",
        2 => "specular",
        3 => "rim",
        4 => "fog",
        _ => "unknown",
    }
}

/// World TRS of a Transform in prefab space (Unity handedness).
fn world(objs: &Objects, transform: i64) -> ([f64; 3], [f64; 4]) {
    let mut chain = Vec::new();
    let mut cur = Some(transform);
    while let Some(id) = cur {
        let Some(t) = objs.get(id) else { break };
        if chain.len() > 64 {
            break;
        }
        chain.push(t);
        cur = objs.deref(t.get("m_Father")).map(|(id, _)| id);
    }
    let mut pos = [0.0; 3];
    let mut rot = [0.0, 0.0, 0.0, 1.0];
    let mut scale = [1.0; 3];
    for t in chain.iter().rev() {
        let lp = vec3(t.get("m_LocalPosition"));
        let scaled = [lp[0] * scale[0], lp[1] * scale[1], lp[2] * scale[2]];
        let moved = rotate(rot, scaled);
        pos = [pos[0] + moved[0], pos[1] + moved[1], pos[2] + moved[2]];
        rot = quat_mul(rot, quat(t.get("m_LocalRotation")));
        let ls = t.get("m_LocalScale").map_or([1.0; 3], |s| vec3(Some(s)));
        scale = [scale[0] * ls[0], scale[1] * ls[1], scale[2] * ls[2]];
    }
    (pos, rot)
}

fn follower(on: &str, mb: &Value) -> Value {
    let kind = match mb.get("followType").and_then(Value::as_i64) {
        Some(0) => "fixedOffset",
        Some(1) => "parent",
        _ => "unknown",
    };
    let node = match mb.get("followableNodeType").and_then(Value::as_i64) {
        Some(0) => "Bip001",
        Some(1) => "Head_Local",
        _ => "unknown",
    };
    let local_rotation = flip_quat(euler_to_quat(vec3(mb.get("localRotationEuler"))));
    json!({
        "on": on,
        "self": true,
        "type": kind,
        "node": node,
        "positionOffset": round_all(flip(vec3(mb.get("positionOffset"))), 4),
        "localPosition": round_all(flip(vec3(mb.get("localPosition"))), 4),
        "localRotation": round_all(local_rotation, 6),
    })
}

/// The `lights[]` entries of a light prefab's `light_overview` group.
fn lights(objs: &Objects) -> Vec<Value> {
    let Some((_, group)) = objs.ref_dict("light_overview") else {
        return Vec::new();
    };
    // Group intensities by the GameObject of their HG light data.
    let mut infos: HashMap<i64, &Value> = HashMap::new();
    for info in group
        .get("m_LightInfoList")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        if let Some(go) = objs
            .deref(info.get("lightData"))
            .and_then(|(_, d)| game_object(d))
        {
            infos.insert(go, info);
        }
    }
    let mut out = Vec::new();
    for lref in group
        .get("m_LightList")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let Some((_, light)) = objs.deref(Some(lref)) else {
            continue;
        };
        let Some(go_id) = game_object(light) else {
            continue;
        };
        let go = objs.get(go_id);
        let name = go
            .and_then(|g| g.get("m_Name"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let active = go
            .and_then(|g| g.get("m_IsActive"))
            .and_then(|v| v.as_bool().or_else(|| v.as_i64().map(|i| i != 0)))
            .unwrap_or(true)
            && light.get("m_Enabled").and_then(Value::as_i64).unwrap_or(1) != 0;
        let transform = objs
            .of_class(CLASS_TRANSFORM)
            .find(|(_, t)| game_object(t) == Some(go_id))
            .map(|(id, _)| id);
        let (pos, rot) = transform.map_or(([0.0; 3], [0.0, 0.0, 0.0, 1.0]), |t| world(objs, t));
        let forward = rotate(rot, [0.0, 0.0, 1.0]);
        let hg = objs
            .behaviours_of(go_id)
            .find(|mb| mb.get("m_lightNPRType").is_some());
        let follow = objs
            .behaviours_of(go_id)
            .find(|mb| mb.get("followType").is_some());
        let color = rgb(light.get("m_Color"));
        let intensity = infos
            .get(&go_id)
            .and_then(|i| f(i.get("intensity")))
            .or_else(|| f(light.get("m_Intensity")))
            .unwrap_or(1.0);
        let hgf = |k: &str, d: f64| hg.and_then(|h| f(h.get(k))).unwrap_or(d);
        let npr_data = hg.and_then(|h| h.get("m_lightNPRData"));
        // Shadow type (0 none, 1 hard, 2 soft); this Unity build keeps it per platform.
        let shadows = light
            .pointer("/m_Shadows/m_Type")
            .or_else(|| light.pointer("/m_Shadows/m_PlatformSpecificType/defaultParam"))
            .and_then(Value::as_i64)
            .unwrap_or(0)
            != 0;
        out.push(json!({
            "name": name,
            "active": active,
            "type": light_type(light.get("m_Type").and_then(Value::as_i64).unwrap_or(2)),
            "color": round_all(color, 4),
            "colorLinear": round_all(color.map(srgb_to_linear), 4),
            "intensity": round(intensity, 4),
            "falloffExponent": round(hgf("m_falloffExponent", -1.0), 4),
            "specularIntensity": round(f(light.get("m_SpecularIntensity")).unwrap_or(1.0), 4),
            "tubeLength": round(f(light.get("m_LinearLightLength")).unwrap_or(-1.0), 4),
            "shadows": shadows,
            "range": round(f(light.get("m_Range")).unwrap_or(1.0), 4),
            "spot": [
                round(f(light.get("m_SpotAngle")).unwrap_or(30.0), 4),
                round(f(light.get("m_InnerSpotAngle")).unwrap_or(21.8), 4),
            ],
            "position": round_all(flip(pos), 4),
            "forward": round_all(flip(forward), 4),
            "npr": npr_type(hg.and_then(|h| h.get("m_lightNPRType")).and_then(Value::as_i64).unwrap_or(-1)),
            "nprData": round_all(vec4(npr_data), 4),
            "rimWidth": round(hgf("m_lightNPRRimWidth", 0.4), 4),
            "rimAlbedo": round(hgf("m_lightNPRRimAlbedoAlpha", 1.0), 4),
            "rampBias": round(hgf("m_lightNPRRampBias", 0.0), 4),
            "characterOnly": hg.and_then(|h| h.get("m_LightCharacterOnly")).and_then(Value::as_i64).unwrap_or(0),
            "follower": follow.map_or(Value::Null, |mb| follower(&name, mb)),
        }));
    }
    out
}

/// Stored CAB of the container `<dir><stem>.prefab` that names character `id`: an exact
/// `<prefix>_<id>` match, else the longest stem `id` starts with (`chr_0003_endmin` for
/// `chr_0003_endminf`).
fn find_prefab<'r>(
    records: &'r [CabRecord],
    dir: &str,
    prefix: &str,
    id: &str,
    stored: &dyn Fn(&str) -> bool,
) -> Option<(&'r str, &'r str)> {
    let mut best: Option<(usize, &str, &str)> = None;
    for rec in records {
        for (path, _, _) in &rec.containers {
            let Some(stem) = path
                .strip_prefix(dir)
                .and_then(|p| p.strip_suffix(".prefab"))
                .and_then(|p| p.strip_prefix(prefix))
            else {
                continue;
            };
            let score = if stem == id {
                usize::MAX
            } else if id.starts_with(stem) && stem.len() > "chr_0000".len() {
                stem.len()
            } else {
                continue;
            };
            if best.is_none_or(|(s, _, _)| score > s) && stored(&rec.cab) {
                best = Some((score, rec.cab.as_str(), path.as_str()));
            }
        }
    }
    best.map(|(_, cab, path)| (cab, path))
}

/// The global `HGCharacterVolume` component of `charinfo_volume.asset`.
fn default_volume(src: &mut CabSource, records: &[CabRecord]) -> Option<Value> {
    let rec = records
        .iter()
        .find(|r| r.containers.iter().any(|(p, _, _)| p == DEFAULT_VOLUME))?;
    let objs = Objects::load(src, &rec.cab, &[CLASS_MONO_BEHAVIOUR])?;
    objs.of_class(CLASS_MONO_BEHAVIOUR)
        .find(|(_, mb)| mb.get("charMainLightMultiplier").is_some())
        .map(|(_, mb)| mb.clone())
}

/// Default framing of the character screen: the `vcam_overview` virtual camera of the camera
/// track (the `cameraGroup` → `DollyCart` refDict), its look-at target and lens.
///
/// ```json
/// {"position": [x,y,z], "target": [x,y,z], "fov": 20, "near": 0.1, "far": 50,
///  "screenOffset": [x, y], "referenceAspect": 1.7778}
/// ```
///
/// Positions are world space of the track prefab, converted to glTF (x negated); the model
/// stands at the origin. `fov` is Cinemachine's vertical field of view in degrees.
/// `screenOffset` is the track's framing offset of the target in viewport units, when set.
fn camera(objs: &Objects) -> Option<Value> {
    let (vcam_go, cinemachine) = objs.of_class(CLASS_MONO_BEHAVIOUR).find_map(|(_, mb)| {
        let go = game_object(mb)?;
        let name = objs.get(go)?.get("m_Name")?.as_str()?;
        (name == "vcam_overview" && mb.get("m_Lens").is_some()).then_some((go, mb))
    })?;
    let transform_of = |go: i64| {
        objs.of_class(CLASS_TRANSFORM)
            .find(|(_, t)| game_object(t) == Some(go))
            .map(|(id, _)| id)
    };
    let (position, _) = world(objs, transform_of(vcam_go)?);
    let target = objs.deref(cinemachine.get("m_LookAt")).and_then(|(id, t)| {
        // `m_LookAt` is a Transform; tolerate a GameObject.
        let tid = if t.get("m_LocalPosition").is_some() {
            Some(id)
        } else {
            transform_of(id)
        };
        tid.map(|tid| world(objs, tid).0)
    });
    let lens = cinemachine.get("m_Lens");
    let lensf = |k: &str| f(lens.and_then(|l| l.get(k)));
    let mut out = Map::new();
    out.insert("position".into(), round_all(flip(position), 4));
    out.insert(
        "target".into(),
        target.map_or(Value::Null, |t| round_all(flip(t), 4)),
    );
    out.insert(
        "fov".into(),
        json!(round(lensf("FieldOfView").unwrap_or(20.0), 4)),
    );
    out.insert(
        "near".into(),
        json!(round(lensf("NearClipPlane").unwrap_or(0.1), 4)),
    );
    out.insert(
        "far".into(),
        json!(round(lensf("FarClipPlane").unwrap_or(50.0), 4)),
    );
    let behaviours: Vec<&Value> = objs.behaviours_of(vcam_go).collect();
    if let Some(offset) = behaviours.iter().find(|mb| mb.get("offsetX").is_some()) {
        out.insert(
            "screenOffset".into(),
            json!([
                round(f(offset.get("offsetX")).unwrap_or(0.0), 4),
                round(f(offset.get("offsetY")).unwrap_or(0.0), 4)
            ]),
        );
    }
    if let Some(aspect) = behaviours
        .iter()
        .find_map(|mb| f(mb.get("referenceAspect")))
    {
        out.insert("referenceAspect".into(), json!(round(aspect, 4)));
    }
    Some(Value::Object(out))
}

/// Light data of character `id`, or `None` when the store has neither its light rig nor its
/// camera track.
pub fn character_lights(
    src: &mut CabSource,
    records: &[CabRecord],
    id: &str,
    stored: &dyn Fn(&str) -> bool,
) -> Option<Value> {
    let defaults = default_volume(src, records);
    let track = find_prefab(records, TRACKS_DIR, "track_", id, stored);
    let rig = find_prefab(records, LIGHTS_DIR, "light_", id, stored);
    if track.is_none() && rig.is_none() {
        return None;
    }
    let track_objs = track.and_then(|(cab, _)| {
        Objects::load(
            src,
            cab,
            &[CLASS_GAME_OBJECT, CLASS_TRANSFORM, CLASS_MONO_BEHAVIOUR],
        )
    });
    let vol = track_objs.as_ref().and_then(|objs| {
        let (_, v) = objs.ref_dict("volume_overview")?;
        v.get("charLightVolumeData").cloned()
    });
    let camera = track_objs.as_ref().and_then(camera);
    let volume = match (&vol, &defaults) {
        (Some(v), d) => volume(v, d.as_ref()),
        (None, Some(d)) => volume(d, None),
        (None, None) => Value::Null,
    };
    let lights = rig
        .and_then(|(cab, _)| {
            Objects::load(
                src,
                cab,
                &[
                    CLASS_GAME_OBJECT,
                    CLASS_TRANSFORM,
                    CLASS_LIGHT,
                    CLASS_MONO_BEHAVIOUR,
                ],
            )
        })
        .map(|objs| lights(&objs))
        .unwrap_or_default();
    let mut source = Map::new();
    source.insert("lights".into(), json!(rig.map(|(_, p)| p)));
    source.insert("track".into(), json!(track.map(|(_, p)| p)));
    Some(json!({
        "id": id,
        "volume": volume,
        "lights": lights,
        "camera": camera,
        "source": source,
    }))
}
