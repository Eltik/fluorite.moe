//! Prefab → glTF: Transform hierarchy, skinned/static meshes, skins and materials.
use super::anim;
use super::avatar;
use super::convert;
use super::gltf::{self, ARRAY_BUFFER, Builder};
use super::material::{self, MaterialOptions};
use super::skin;
use super::store::CabSource;
use crate::extraction::mesh::{DecodedMesh, decode_mesh};
use serde_json::{Map, Value, json};
use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;

pub const CLASS_GAME_OBJECT: i32 = 1;
pub const CLASS_TRANSFORM: i32 = 4;
pub const CLASS_MESH_RENDERER: i32 = 23;
pub const CLASS_MESH_FILTER: i32 = 33;
pub const CLASS_MESH: i32 = 43;
pub const CLASS_ANIMATION_CLIP: i32 = 74;
pub const CLASS_AVATAR: i32 = 90;
pub const CLASS_ANIMATOR_CONTROLLER: i32 = 91;
pub const CLASS_ANIMATOR: i32 = 95;
pub const CLASS_SKINNED_MESH_RENDERER: i32 = 137;
pub const CLASS_LOD_GROUP: i32 = 205;
pub const CLASS_RECT_TRANSFORM: i32 = 224;

/// An object in a loaded file of the [`CabSource`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ObjRef {
    pub file: usize,
    pub path_id: i64,
}

/// Cache key of an exported mesh: source mesh, renderer materials, whether it is skinned, and
/// the joint count its `JOINTS_0` indices were checked against.
type MeshKey = (ObjRef, Vec<Option<ObjRef>>, bool, usize);

/// Export settings.
#[derive(Debug, Clone, Default)]
pub struct ExportOptions {
    pub material: MaterialOptions,
    /// Keep every LOD instead of only the best one per part.
    pub all_lods: bool,
}

/// Counts reported per exported entity.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct ExportStats {
    pub nodes: usize,
    pub meshes: usize,
    pub primitives: usize,
    pub materials: usize,
    pub textures: usize,
    pub skins: usize,
    pub joints: usize,
    pub skinned_renderers: usize,
    pub static_renderers: usize,
    pub skipped_lod_renderers: usize,
    pub inactive_renderers: usize,
    pub vertices: usize,
    pub triangles: usize,
    pub unresolved_refs: usize,
    pub animators: usize,
}

/// One exported entity.
pub struct EntityExport {
    pub glb: Vec<u8>,
    pub document: Value,
    pub stats: ExportStats,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone)]
struct Renderer {
    skinned: bool,
    renderer: ObjRef,
    node: usize,
    name: String,
    active: bool,
    mesh: Option<ObjRef>,
    materials: Vec<Option<ObjRef>>,
    bones: Vec<Option<ObjRef>>,
    lod: Option<usize>,
}

/// Exporter state for one entity.
pub struct Exporter<'a> {
    pub src: &'a mut CabSource,
    index: HashMap<usize, HashMap<i64, usize>>,
    pub builder: Builder,
    transform_nodes: HashMap<ObjRef, usize>,
    renderers: Vec<Renderer>,
    lod_levels: HashMap<ObjRef, usize>,
    mesh_cache: HashMap<MeshKey, Option<usize>>,
    pub texture_paths: HashMap<usize, HashMap<i64, PathBuf>>,
    /// Copies of materials with `texCoord` clamped, by (material, UV set count).
    uv_clamped: HashMap<(usize, usize), usize>,
    pub material_cache: HashMap<material::MaterialKey, usize>,
    pub texture_cache: HashMap<(ObjRef, material::TextureRole), Option<usize>>,
    pub shader_cache: HashMap<ObjRef, String>,
    pub warnings: BTreeSet<String>,
    unresolved: BTreeSet<String>,
    pub stats: ExportStats,
    opts: ExportOptions,
    animators: Vec<Value>,
    /// Node of the first Animator found: clip binding paths are relative to it.
    animator_node: Option<usize>,
    /// AnimationClips (`(cab, path_id)`) to embed as glTF animations.
    pub clip_sources: Vec<(String, i64)>,
}

fn pptr(value: Option<&Value>) -> Option<(i64, i64)> {
    let v = value?;
    let file = v.get("m_FileID")?.as_i64()?;
    let path = v.get("m_PathID")?.as_i64()?;
    (path != 0).then_some((file, path))
}

fn vec3(value: Option<&Value>, default: f32) -> [f32; 3] {
    let get = |k: &str| {
        value
            .and_then(|v| v.get(k))
            .and_then(Value::as_f64)
            .map_or(default, |x| x as f32)
    };
    [get("x"), get("y"), get("z")]
}

fn quat(value: Option<&Value>) -> [f32; 4] {
    let get = |k: &str, d: f32| {
        value
            .and_then(|v| v.get(k))
            .and_then(Value::as_f64)
            .map_or(d, |x| x as f32)
    };
    [get("x", 0.0), get("y", 0.0), get("z", 0.0), get("w", 1.0)]
}

/// Whether a renderer's GameObject is a shadow-casting proxy (`S_x_shadowProxyDesktop`).
#[must_use]
pub fn is_shadow_proxy(name: &str) -> bool {
    name.to_ascii_lowercase().contains("shadowproxy")
}

/// Parse a trailing `_lodN` of a name: `(base, Some(N))`.
#[must_use]
pub fn split_lod(name: &str) -> (String, Option<usize>) {
    let lower = name.to_ascii_lowercase();
    if let Some(pos) = lower.rfind("_lod") {
        let digits = &lower[pos + 4..];
        if !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit()) {
            return (lower[..pos].to_string(), digits.parse().ok());
        }
    }
    (lower, None)
}

impl<'a> Exporter<'a> {
    #[must_use]
    pub fn new(src: &'a mut CabSource, opts: ExportOptions) -> Self {
        Self {
            src,
            index: HashMap::new(),
            builder: Builder::default(),
            transform_nodes: HashMap::new(),
            renderers: Vec::new(),
            lod_levels: HashMap::new(),
            mesh_cache: HashMap::new(),
            texture_paths: HashMap::new(),
            uv_clamped: HashMap::new(),
            material_cache: HashMap::new(),
            texture_cache: HashMap::new(),
            shader_cache: HashMap::new(),
            warnings: BTreeSet::new(),
            unresolved: BTreeSet::new(),
            stats: ExportStats::default(),
            opts,
            animators: Vec::new(),
            animator_node: None,
            clip_sources: Vec::new(),
        }
    }

    #[must_use]
    pub fn material_options(&self) -> &MaterialOptions {
        &self.opts.material
    }

    pub fn warn(&mut self, message: impl Into<String>) {
        self.warnings.insert(message.into());
    }

    /// Resolve a PPtr read from an object in `from`.
    pub fn resolve(&mut self, from: usize, value: Option<&Value>) -> Option<ObjRef> {
        let (file_id, path_id) = pptr(value)?;
        if file_id == 0 {
            return Some(ObjRef {
                file: from,
                path_id,
            });
        }
        let cab = {
            let sf = &self.src.files.files.get(from)?.sf;
            let ext = usize::try_from(file_id - 1)
                .ok()
                .and_then(|i| sf.externals.get(i));
            match ext {
                Some(ext) => ext.cab_name().to_string(),
                None => {
                    self.unresolved.insert(format!("bad m_FileID {file_id}"));
                    return None;
                }
            }
        };
        match self.src.get(&cab) {
            Some(file) => Some(ObjRef { file, path_id }),
            None => {
                self.unresolved.insert(cab);
                None
            }
        }
    }

    fn object_index(&mut self, r: ObjRef) -> Option<usize> {
        let sf = &self.src.files.files.get(r.file)?.sf;
        let map = self.index.entry(r.file).or_insert_with(|| {
            sf.objects
                .iter()
                .enumerate()
                .map(|(i, o)| (o.path_id, i))
                .collect()
        });
        map.get(&r.path_id).copied()
    }

    /// Class ID and deserialized value of an object.
    pub fn read(&mut self, r: ObjRef) -> Option<(i32, Value)> {
        let Some(i) = self.object_index(r) else {
            let cab = self.src.files.files[r.file].cab.clone();
            self.unresolved.insert(format!("{cab}:{}", r.path_id));
            return None;
        };
        let sf = &self.src.files.files[r.file].sf;
        let obj = &sf.objects[i];
        let class = obj.class_id;
        match sf.read_object(obj) {
            Ok(v) => Some((class, v)),
            Err(e) => {
                // Some meshes lack bytes their type tree declares after the vertex data.
                if class == CLASS_MESH
                    && let Ok(Some(fields)) = sf.read_prefix(obj, &["m_VertexData"])
                {
                    return Some((class, Value::Object(fields)));
                }
                self.warn(format!(
                    "read failed for class {class} object {}: {e}",
                    r.path_id
                ));
                None
            }
        }
    }

    /// Read only the root fields up to `last` (for huge objects such as shaders).
    pub fn read_prefix(&mut self, r: ObjRef, last: &[&str]) -> Option<Map<String, Value>> {
        let i = self.object_index(r)?;
        let sf = &self.src.files.files[r.file].sf;
        sf.read_prefix(&sf.objects[i], last).ok().flatten()
    }

    pub fn peek_name(&mut self, r: ObjRef) -> Option<String> {
        let i = self.object_index(r)?;
        let sf = &self.src.files.files[r.file].sf;
        sf.peek_name(&sf.objects[i]).ok().flatten()
    }

    pub fn class_of(&mut self, r: ObjRef) -> Option<i32> {
        let i = self.object_index(r)?;
        Some(self.src.files.files[r.file].sf.objects[i].class_id)
    }

    /// The Transform of a GameObject (or the object itself if it is a Transform).
    pub fn transform_of(&mut self, r: ObjRef) -> Option<ObjRef> {
        match self.class_of(r)? {
            CLASS_TRANSFORM | CLASS_RECT_TRANSFORM => Some(r),
            CLASS_GAME_OBJECT => {
                let (_, go) = self.read(r)?;
                let comps = go.get("m_Component")?.as_array()?.clone();
                for c in &comps {
                    let cref = self.resolve(r.file, c.get("component"))?;
                    if matches!(
                        self.class_of(cref),
                        Some(CLASS_TRANSFORM | CLASS_RECT_TRANSFORM)
                    ) {
                        return Some(cref);
                    }
                }
                None
            }
            _ => None,
        }
    }

    /// Walk a Transform subtree, creating nodes and collecting renderers.
    fn visit(&mut self, t: ObjRef, depth: usize) -> Option<usize> {
        if depth > 256 {
            self.warn("hierarchy deeper than 256 levels; truncated");
            return None;
        }
        if self.transform_nodes.contains_key(&t) {
            self.warn("transform visited twice; cycle skipped");
            return None;
        }
        let (_, tr) = self.read(t)?;
        let go_ref = self.resolve(t.file, tr.get("m_GameObject"));
        let go = go_ref.and_then(|g| self.read(g)).map(|(_, v)| v);
        let name = go
            .as_ref()
            .and_then(|g| g.get("m_Name"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let active = go
            .as_ref()
            .and_then(|g| g.get("m_IsActive"))
            .and_then(Value::as_bool)
            .unwrap_or(true);
        let t_unity = vec3(tr.get("m_LocalPosition"), 0.0);
        let r_unity = quat(tr.get("m_LocalRotation"));
        let s = vec3(tr.get("m_LocalScale"), 1.0);
        let mut node = Map::new();
        node.insert("name".into(), json!(name));
        let tt = convert::position(t_unity);
        if tt != [0.0; 3] {
            node.insert("translation".into(), json!(tt));
        }
        let rr = convert::rotation(r_unity);
        if rr != [0.0, 0.0, 0.0, 1.0] {
            node.insert("rotation".into(), json!(rr));
        }
        if s != [1.0; 3] {
            if s.iter().any(|x| *x == 0.0 || !x.is_finite()) {
                self.warn(format!("node {name} has degenerate scale {s:?}"));
            }
            node.insert("scale".into(), json!(s));
        }
        if !active {
            node.insert("extras".into(), json!({"active": false}));
        }
        let index = self.builder.node(Value::Object(node));
        self.transform_nodes.insert(t, index);
        if let (Some(go_ref), Some(go)) = (go_ref, go.as_ref()) {
            self.collect_components(go_ref, go, index, &name, active);
        }
        let children: Vec<Value> = tr
            .get("m_Children")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let mut child_nodes = Vec::new();
        for c in &children {
            if let Some(cref) = self.resolve(t.file, Some(c))
                && let Some(n) = self.visit(cref, depth + 1)
            {
                child_nodes.push(n);
            }
        }
        if !child_nodes.is_empty() {
            self.builder.nodes[index]["children"] = json!(child_nodes);
        }
        Some(index)
    }

    fn collect_components(
        &mut self,
        go_ref: ObjRef,
        go: &Value,
        node: usize,
        name: &str,
        active: bool,
    ) {
        let comps: Vec<Value> = go
            .get("m_Component")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let mut mesh_filter_mesh = None;
        let mut static_renderer = None;
        for c in &comps {
            let Some(cref) = self.resolve(go_ref.file, c.get("component")) else {
                continue;
            };
            match self.class_of(cref) {
                Some(CLASS_SKINNED_MESH_RENDERER) => {
                    if let Some((_, smr)) = self.read(cref) {
                        let mesh = self.resolve(cref.file, smr.get("m_Mesh"));
                        let materials = self.material_refs(cref.file, &smr);
                        let bones = smr
                            .get("m_Bones")
                            .and_then(Value::as_array)
                            .cloned()
                            .unwrap_or_default()
                            .iter()
                            .map(|b| self.resolve(cref.file, Some(b)))
                            .collect();
                        let enabled = smr
                            .get("m_Enabled")
                            .and_then(Value::as_bool)
                            .unwrap_or(true);
                        self.renderers.push(Renderer {
                            skinned: true,
                            renderer: cref,
                            node,
                            name: name.to_string(),
                            active: active && enabled,
                            mesh,
                            materials,
                            bones,
                            lod: None,
                        });
                    }
                }
                Some(CLASS_MESH_RENDERER) => {
                    if let Some((_, mr)) = self.read(cref) {
                        let enabled = mr.get("m_Enabled").and_then(Value::as_bool).unwrap_or(true);
                        static_renderer = Some((cref, self.material_refs(cref.file, &mr), enabled));
                    }
                }
                Some(CLASS_MESH_FILTER) => {
                    if let Some((_, mf)) = self.read(cref) {
                        mesh_filter_mesh = self.resolve(cref.file, mf.get("m_Mesh"));
                    }
                }
                Some(CLASS_LOD_GROUP) => {
                    if let Some((_, lg)) = self.read(cref) {
                        let lods = lg
                            .get("m_LODs")
                            .and_then(Value::as_array)
                            .cloned()
                            .unwrap_or_default();
                        for (level, lod) in lods.iter().enumerate() {
                            for r in lod
                                .get("renderers")
                                .and_then(Value::as_array)
                                .into_iter()
                                .flatten()
                            {
                                if let Some(rref) = self.resolve(cref.file, r.get("renderer")) {
                                    self.lod_levels.entry(rref).or_insert(level);
                                }
                            }
                        }
                    }
                }
                Some(CLASS_ANIMATOR) => {
                    self.animator_node.get_or_insert(node);
                    if let Some((_, an)) = self.read(cref) {
                        let avatar = self.resolve(cref.file, an.get("m_Avatar"));
                        let controller = self.resolve(cref.file, an.get("m_Controller"));
                        let avatar_name = avatar.and_then(|a| self.peek_name(a));
                        let controller_name = controller.and_then(|c| self.peek_name(c));
                        self.animators.push(json!({
                            "node": name,
                            "avatar": avatar_name,
                            "controller": controller_name,
                        }));
                    }
                }
                _ => {}
            }
        }
        if let Some((renderer, materials, enabled)) = static_renderer {
            self.renderers.push(Renderer {
                skinned: false,
                renderer,
                node,
                name: name.to_string(),
                active: active && enabled,
                mesh: mesh_filter_mesh,
                materials,
                bones: Vec::new(),
                lod: None,
            });
        }
    }

    fn material_refs(&mut self, file: usize, renderer: &Value) -> Vec<Option<ObjRef>> {
        renderer
            .get("m_Materials")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|m| self.resolve(file, Some(m)))
            .collect()
    }

    /// Keep the best LOD of each part: LODGroup level, else a `_lodN` name suffix. Shadow
    /// proxies (`*_shadowProxy*`, low-LOD casters the game never draws) are dropped too, as
    /// are renderers drawing a `_lod1+` mesh whose `_lod0` another renderer draws.
    fn select_lods(&mut self) {
        for r in &mut self.renderers {
            let (_, from_name) = split_lod(&r.name);
            r.lod = self.lod_levels.get(&r.renderer).copied().or(from_name);
        }
        if self.opts.all_lods {
            return;
        }
        let before = self.renderers.len();
        let renderers = std::mem::take(&mut self.renderers);
        let mesh_names: Vec<Option<String>> = renderers
            .iter()
            .map(|r| r.mesh.and_then(|m| self.peek_name(m)))
            .collect();
        let drawn_lod0: BTreeSet<String> = mesh_names
            .iter()
            .flatten()
            .filter_map(|n| match split_lod(n) {
                (base, Some(0)) => Some(base),
                _ => None,
            })
            .collect();
        self.renderers = renderers
            .into_iter()
            .zip(mesh_names)
            .filter(|(r, mesh_name)| {
                if is_shadow_proxy(&r.name) {
                    return false;
                }
                !mesh_name.as_deref().is_some_and(|n| match split_lod(n) {
                    (base, Some(level)) => level > 0 && drawn_lod0.contains(&base),
                    _ => false,
                })
            })
            .map(|(r, _)| r)
            .collect();
        let dropped = before - self.renderers.len();
        let mut best: HashMap<String, usize> = HashMap::new();
        for r in &self.renderers {
            let (base, _) = split_lod(&r.name);
            let level = r.lod.unwrap_or(0);
            best.entry(base)
                .and_modify(|b| *b = (*b).min(level))
                .or_insert(level);
        }
        let group_has_lod0 = self.renderers.iter().any(|r| r.lod == Some(0));
        let before = self.renderers.len();
        self.renderers.retain(|r| {
            let (base, _) = split_lod(&r.name);
            let level = r.lod.unwrap_or(0);
            if level > best[&base] {
                return false;
            }
            // A LODGroup-only level (no name suffix) is dropped when the group has a LOD0.
            !(level > 0 && split_lod(&r.name).1.is_none() && group_has_lod0)
        });
        self.stats.skipped_lod_renderers = before - self.renderers.len() + dropped;
    }

    fn decode(&mut self, mesh_ref: ObjRef) -> Option<(Value, DecodedMesh)> {
        let (class, mesh) = self.read(mesh_ref)?;
        if class != CLASS_MESH {
            self.warn(format!(
                "renderer mesh {} is class {class}",
                mesh_ref.path_id
            ));
            return None;
        }
        match decode_mesh(&mesh, &self.src.files.resources) {
            Ok(decoded) => Some((mesh, decoded)),
            Err(e) => {
                let name = mesh.get("m_Name").and_then(Value::as_str).unwrap_or("");
                self.warn(format!("mesh {name}: {e}"));
                None
            }
        }
    }

    /// glTF mesh for a renderer; `None` when it cannot be decoded.
    fn mesh_for(
        &mut self,
        r: &Renderer,
        joint_count: usize,
    ) -> Option<(usize, Option<skin::SkinWeights>, Value)> {
        let mesh_ref = r.mesh?;
        let (mesh_value, decoded) = self.decode(mesh_ref)?;
        if r.skinned && r.bones.is_empty() {
            self.warn(format!(
                "{}: skinned renderer without bones; exported static",
                r.name
            ));
        }
        // Without bones there is no skin, so JOINTS_0/WEIGHTS_0 would dangle: drop them.
        let weights = if r.skinned && !r.bones.is_empty() {
            match skin::decode_skin_weights(&mesh_value, &self.src.files.resources) {
                Ok(Some(w)) => Some(w),
                Ok(None) => {
                    self.warn(format!(
                        "{}: skinned mesh without blend indices; exported static",
                        r.name
                    ));
                    None
                }
                Err(e) => {
                    self.warn(format!("{}: skin weights: {e}", r.name));
                    None
                }
            }
        } else {
            None
        };
        let key = (
            mesh_ref,
            r.materials.clone(),
            weights.is_some(),
            joint_count,
        );
        if let Some(cached) = self.mesh_cache.get(&key) {
            return cached.map(|m| (m, weights, mesh_value));
        }
        let mut attributes = Map::new();
        let positions: Vec<[f32; 3]> = decoded
            .positions
            .iter()
            .map(|p| convert::position(*p))
            .collect();
        let b = &mut self.builder;
        attributes.insert(
            "POSITION".into(),
            json!(b.floats(&positions, "VEC3", Some(ARRAY_BUFFER), true)),
        );
        if let Some(normals) = &decoded.normals {
            let ns: Vec<[f32; 3]> = normals
                .iter()
                .map(|n| {
                    let c = convert::position(*n);
                    let len = (c[0] * c[0] + c[1] * c[1] + c[2] * c[2]).sqrt();
                    if len > 1e-6 && len.is_finite() {
                        c.map(|x| x / len)
                    } else {
                        [0.0, 1.0, 0.0]
                    }
                })
                .collect();
            attributes.insert(
                "NORMAL".into(),
                json!(b.floats(&ns, "VEC3", Some(ARRAY_BUFFER), false)),
            );
        }
        if let Some(tangents) = &decoded.tangents {
            let ts: Vec<[f32; 4]> = tangents
                .iter()
                .map(|t| {
                    let c = [-t[0], t[1], t[2]];
                    let len = (c[0] * c[0] + c[1] * c[1] + c[2] * c[2]).sqrt();
                    let w = if -t[3] < 0.0 { -1.0 } else { 1.0 };
                    if len > 1e-6 && len.is_finite() {
                        [c[0] / len, c[1] / len, c[2] / len, w]
                    } else {
                        [1.0, 0.0, 0.0, 1.0]
                    }
                })
                .collect();
            attributes.insert(
                "TANGENT".into(),
                json!(b.floats(&ts, "VEC4", Some(ARRAY_BUFFER), false)),
            );
        }
        for (i, uv) in decoded.uvs.iter().enumerate() {
            let flipped: Vec<[f32; 2]> = uv.iter().map(|u| [u[0], 1.0 - u[1]]).collect();
            attributes.insert(
                format!("TEXCOORD_{i}"),
                json!(b.floats(&flipped, "VEC2", Some(ARRAY_BUFFER), false)),
            );
        }
        if let Some(colors) = &decoded.colors {
            // NPR vertex colors carry outline/shading data, not albedo: keep them under an
            // application-specific name so viewers don't tint the model with them.
            attributes.insert(
                "_COLOR_0".into(),
                json!(b.floats(colors, "VEC4", Some(ARRAY_BUFFER), false)),
            );
        }
        if let Some(w) = &weights {
            let mut joints = w.joints.clone();
            let mut weights = w.weights.clone();
            let mut bad = 0usize;
            for (j, wt) in joints.iter_mut().zip(weights.iter_mut()) {
                let mut changed = false;
                for k in 0..4 {
                    if usize::from(j[k]) >= joint_count.max(1) {
                        // Drop the influence rather than re-pointing it (which could duplicate joint 0).
                        j[k] = 0;
                        wt[k] = 0.0;
                        bad += 1;
                        changed = true;
                    }
                }
                if changed {
                    convert::normalize_influences(j, wt, 0);
                }
            }
            if bad > 0 {
                self.warn(format!(
                    "{}: {bad} influences on bone indices beyond {joint_count} bones dropped",
                    r.name
                ));
            }
            let b = &mut self.builder;
            attributes.insert("JOINTS_0".into(), json!(b.joints(&joints, joint_count)));
            attributes.insert(
                "WEIGHTS_0".into(),
                json!(b.floats(&weights, "VEC4", Some(ARRAY_BUFFER), false)),
            );
            if w.renormalized > 0 {
                self.warn(format!(
                    "{}: {} vertices had weights not summing to 1 (renormalized)",
                    r.name, w.renormalized
                ));
            }
            if w.unweighted > 0 {
                self.warn(format!(
                    "{}: {} vertices without weights bound to joint 0",
                    r.name, w.unweighted
                ));
            }
        }
        let attributes = Value::Object(attributes);
        let mut primitives = Vec::new();
        let last_sub = decoded.submeshes.len().checked_sub(1);
        for (i, sub) in decoded.submeshes.iter().enumerate() {
            if sub.is_empty() {
                continue;
            }
            let mut flipped = Vec::with_capacity(sub.len());
            for tri in sub.as_chunks::<3>().0 {
                flipped.extend_from_slice(&[tri[0], tri[2], tri[1]]);
            }
            self.stats.triangles += flipped.len() / 3;
            let indices = self.builder.indices(&flipped);
            let mat_ref = r
                .materials
                .get(i)
                .copied()
                .flatten()
                .or_else(|| r.materials.last().copied().flatten());
            let material = material::material_index(self, mat_ref, &decoded.name);
            let material = self.material_for_uvs(material, decoded.uvs.len());
            primitives.push(json!({"attributes": attributes, "indices": indices, "material": material, "mode": 4}));
            // Unity draws the last submesh once more with each material beyond the submesh
            // count (transparent hair passes, layered eye shadows): extra primitives sharing
            // the same accessors.
            if Some(i) == last_sub {
                let extra: Vec<ObjRef> = r
                    .materials
                    .iter()
                    .skip(decoded.submeshes.len())
                    .copied()
                    .flatten()
                    .collect();
                for (pass, mref) in extra.into_iter().enumerate() {
                    let material = material::material_index(self, Some(mref), &decoded.name);
                    let material = self.material_for_uvs(material, decoded.uvs.len());
                    self.stats.triangles += flipped.len() / 3;
                    primitives.push(json!({
                        "attributes": attributes,
                        "indices": indices,
                        "material": material,
                        "mode": 4,
                        "extras": {"extraMaterialPass": pass + 1},
                    }));
                }
            }
        }
        if primitives.is_empty() {
            self.mesh_cache.insert(key, None);
            return None;
        }
        self.stats.vertices += decoded.positions.len();
        self.stats.primitives += primitives.len();
        self.builder
            .meshes
            .push(json!({"name": decoded.name, "primitives": primitives}));
        let index = self.builder.meshes.len() - 1;
        self.mesh_cache.insert(key, Some(index));
        Some((index, weights, mesh_value))
    }

    /// `material`, or a copy whose `texCoord`s are clamped to UV sets the mesh actually has
    /// (a texture on a missing TEXCOORD_n would be invalid glTF).
    fn material_for_uvs(&mut self, material: usize, uv_sets: usize) -> usize {
        let slots = [
            "/pbrMetallicRoughness/baseColorTexture",
            "/normalTexture",
            "/emissiveTexture",
        ];
        let fits = |m: &Value| {
            slots.iter().all(|s| {
                m.pointer(&format!("{s}/texCoord"))
                    .and_then(Value::as_u64)
                    .is_none_or(|t| (t as usize) < uv_sets.max(1))
            })
        };
        if fits(&self.builder.materials[material]) {
            return material;
        }
        if let Some(&i) = self.uv_clamped.get(&(material, uv_sets)) {
            return i;
        }
        let mut copy = self.builder.materials[material].clone();
        for s in slots {
            if let Some(t) = copy.pointer_mut(&format!("{s}/texCoord")) {
                *t = json!(
                    uv_sets
                        .saturating_sub(1)
                        .min(t.as_u64().unwrap_or(0) as usize)
                );
            }
        }
        let name = copy["name"].as_str().unwrap_or("").to_string();
        self.warn(format!(
            "material {name}: texCoord clamped to the mesh's {uv_sets} UV set(s)"
        ));
        self.builder.materials.push(copy);
        let i = self.builder.materials.len() - 1;
        self.uv_clamped.insert((material, uv_sets), i);
        i
    }

    /// An identity-transform child of `parent`, used as a unique stand-in joint.
    fn placeholder_joint(&mut self, parent: usize, name: &str) -> usize {
        let node = self
            .builder
            .node(json!({"name": name, "extras": {"placeholderJoint": true}}));
        let mut children = self.builder.nodes[parent]["children"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        children.push(json!(node));
        self.builder.nodes[parent]["children"] = json!(children);
        node
    }

    /// Skin for a skinned renderer: joints from `m_Bones`, IBMs from the mesh's bind poses.
    fn skin_for(&mut self, r: &Renderer, mesh_value: &Value, root: usize) -> usize {
        let poses = skin::bind_poses(mesh_value);
        if poses.len() != r.bones.len() {
            self.warn(format!(
                "{}: {} bind poses for {} bones",
                r.name,
                poses.len(),
                r.bones.len()
            ));
        }
        // glTF forbids repeated joints: a missing bone, or a bone listed twice, gets an
        // identity child of its node (the root for missing ones), which has the same world
        // transform and so skins identically.
        let mut joints = Vec::with_capacity(r.bones.len());
        let mut used = std::collections::HashSet::new();
        let (mut missing, mut repeated) = (0, 0);
        for (i, bone) in r.bones.iter().enumerate() {
            let node = bone.and_then(|b| self.transform_nodes.get(&b).copied());
            let joint = match node {
                Some(n) if used.insert(n) => n,
                Some(n) => {
                    repeated += 1;
                    self.placeholder_joint(n, &format!("{}_bone{i}_dup", r.name))
                }
                None => {
                    missing += 1;
                    self.placeholder_joint(root, &format!("{}_bone{i}_missing", r.name))
                }
            };
            used.insert(joint);
            joints.push(joint);
        }
        if missing > 0 {
            self.warn(format!(
                "{}: {missing} bones outside the hierarchy bound to placeholder joints at the root",
                r.name
            ));
        }
        if repeated > 0 {
            self.warn(format!(
                "{}: {repeated} repeated bones given placeholder joints",
                r.name
            ));
        }
        let ibms: Vec<[f32; 16]> = (0..joints.len())
            .map(|i| {
                let m = poses.get(i).copied().unwrap_or([
                    [1.0, 0.0, 0.0, 0.0],
                    [0.0, 1.0, 0.0, 0.0],
                    [0.0, 0.0, 1.0, 0.0],
                    [0.0, 0.0, 0.0, 1.0],
                ]);
                convert::column_major(convert::matrix(m))
            })
            .collect();
        let ibm = self.builder.floats(&ibms, "MAT4", None, false);
        self.stats.joints = self.stats.joints.max(joints.len());
        self.builder.skins.push(
            json!({"name": r.name, "joints": joints, "inverseBindMatrices": ibm, "skeleton": root}),
        );
        self.builder.skins.len() - 1
    }

    /// Decode `clip_sources` and add them as glTF animations, bound by path hash below the
    /// Animator's node (or the prefab root).
    fn embed_clips(&mut self, root_node: usize) -> Vec<anim::AnimStats> {
        let sources = std::mem::take(&mut self.clip_sources);
        if sources.is_empty() {
            return Vec::new();
        }
        let hashes =
            anim::node_path_hashes(&self.builder.nodes, self.animator_node.unwrap_or(root_node));
        let mut out = Vec::new();
        for (cab, path_id) in sources {
            let Some(file) = self.src.get(&cab) else {
                self.warn(format!("clip {cab}:{path_id}: bundle not in store"));
                continue;
            };
            let Some((class, clip)) = self.read(ObjRef { file, path_id }) else {
                continue;
            };
            if class != CLASS_ANIMATION_CLIP {
                continue;
            }
            match anim::decode_clip(&clip) {
                // A clip binding mostly unknown paths belongs to another skeleton.
                Ok(decoded) if anim::matched_fraction(&decoded, &hashes) < 0.5 => {
                    self.warn(format!(
                        "clip {}: skipped, most tracks match no node (other skeleton)",
                        decoded.name
                    ));
                }
                Ok(decoded) => {
                    let mut animations = std::mem::take(&mut self.builder.animations);
                    let stats =
                        anim::add_animation(&mut self.builder, &mut animations, &decoded, &hashes);
                    self.builder.animations = animations;
                    if stats.unmatched_tracks > 0 {
                        self.warn(format!(
                            "clip {}: {} of {} tracks match no node",
                            stats.name,
                            stats.unmatched_tracks,
                            stats.unmatched_tracks + stats.matched_tracks
                        ));
                    }
                    out.push(stats);
                }
                Err(e) => {
                    let name = clip.get("m_Name").and_then(Value::as_str).unwrap_or("");
                    self.warn(format!("clip {name}: {e}"));
                }
            }
        }
        out
    }

    /// Build the document for the prefab whose root GameObject (or Transform) is `root`.
    pub fn export(mut self, root: ObjRef, asset_extras: Value) -> Result<EntityExport, String> {
        let root_t = self
            .transform_of(root)
            .ok_or("prefab root has no Transform")?;
        let root_node = self
            .visit(root_t, 0)
            .ok_or("prefab root could not be read")?;
        // The prefab's own placement is irrelevant in a standalone file.
        for key in ["translation", "rotation", "scale"] {
            if let Some(obj) = self.builder.nodes[root_node].as_object_mut() {
                obj.remove(key);
            }
        }
        self.finish(root_node, asset_extras)
    }

    /// Build a model shipped without GameObjects (named NPC `sk_*.fbx` imports): the node
    /// hierarchy comes from the Avatar's skeleton (TOS paths, parents, skeleton pose) and every
    /// mesh is skinned through its `m_BoneNameHashes` (path hashes, resolved via the TOS).
    /// Materials are not shipped either, so each part gets the name-matched fallback.
    pub fn export_avatar(
        mut self,
        avatar: ObjRef,
        meshes: &[ObjRef],
        name: &str,
        asset_extras: Value,
    ) -> Result<EntityExport, String> {
        let (_, value) = self.read(avatar).ok_or("avatar could not be read")?;
        let skeleton = avatar::Skeleton::from_avatar(&value)?;
        let root_node = self.builder.node(json!({"name": name}));
        // Skeleton node 0 is the model root itself (empty path).
        let mut nodes = vec![root_node];
        for i in 1..skeleton.len() {
            let (t, r, s) = skeleton.local[i];
            let mut node = Map::new();
            node.insert("name".into(), json!(skeleton.name(i)));
            let tt = convert::position(t);
            if tt != [0.0; 3] {
                node.insert("translation".into(), json!(tt));
            }
            let rr = convert::rotation(r);
            if rr != [0.0, 0.0, 0.0, 1.0] {
                node.insert("rotation".into(), json!(rr));
            }
            if s != [1.0; 3] {
                node.insert("scale".into(), json!(s));
            }
            nodes.push(self.builder.node(Value::Object(node)));
        }
        for i in 1..skeleton.len() {
            let parent = nodes[skeleton.parent(i).unwrap_or(0)];
            let mut children = self.builder.nodes[parent]["children"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            children.push(json!(nodes[i]));
            self.builder.nodes[parent]["children"] = json!(children);
        }
        // Bones resolve by path hash: register each skeleton node under a synthetic ref.
        for (i, &hash) in skeleton.ids.iter().enumerate() {
            self.transform_nodes
                .insert(avatar::bone_ref(hash), nodes[i]);
        }
        for &mesh in meshes {
            let Some(fields) = self.read_prefix(mesh, &["m_BoneNameHashes"]) else {
                continue;
            };
            let mesh_name = fields
                .get("m_Name")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let bones: Vec<Option<ObjRef>> = fields
                .get("m_BoneNameHashes")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .map(|h| h.as_u64().map(|h| avatar::bone_ref(h as u32)))
                .collect();
            self.renderers.push(Renderer {
                skinned: !bones.is_empty(),
                renderer: mesh,
                node: root_node,
                name: mesh_name,
                active: true,
                mesh: Some(mesh),
                materials: vec![None],
                bones,
                lod: None,
            });
        }
        self.finish(root_node, asset_extras)
    }

    /// Shared tail of both exports: LOD choice, meshes, skins, clips, document.
    fn finish(mut self, root_node: usize, asset_extras: Value) -> Result<EntityExport, String> {
        self.select_lods();
        let renderers = std::mem::take(&mut self.renderers);
        let mut scene = vec![root_node];
        let mut skinned_nodes = Vec::new();
        for r in &renderers {
            if !r.active {
                self.stats.inactive_renderers += 1;
            }
            let Some((mesh, weights, mesh_value)) = self.mesh_for(r, r.bones.len()) else {
                continue;
            };
            let mut extras = json!({"active": r.active, "lod": r.lod, "renderer": if r.skinned {"SkinnedMeshRenderer"} else {"MeshRenderer"}});
            // Only kept with `all_lods`: the client hides them.
            if is_shadow_proxy(&r.name) {
                extras["role"] = json!("shadowProxy");
            }
            if r.skinned && weights.is_some() && !r.bones.is_empty() {
                let skin = self.skin_for(r, &mesh_value, root_node);
                // Skinned meshes sit at the scene root: glTF ignores their node transform.
                let node = self
                    .builder
                    .node(json!({"name": r.name, "mesh": mesh, "skin": skin, "extras": extras}));
                skinned_nodes.push(node);
                self.stats.skinned_renderers += 1;
            } else {
                if self.builder.nodes[r.node].get("mesh").is_some() {
                    let node = self.builder.node(
                        json!({"name": format!("{}_mesh", r.name), "mesh": mesh, "extras": extras}),
                    );
                    let children = self.builder.nodes[r.node]["children"]
                        .as_array()
                        .cloned()
                        .unwrap_or_default();
                    let mut children = children;
                    children.push(json!(node));
                    self.builder.nodes[r.node]["children"] = json!(children);
                } else {
                    self.builder.nodes[r.node]["mesh"] = json!(mesh);
                    let mut node_extras = self.builder.nodes[r.node]
                        .get("extras")
                        .cloned()
                        .unwrap_or(json!({}));
                    node_extras["renderer"] = extras["renderer"].clone();
                    node_extras["lod"] = extras["lod"].clone();
                    if let Some(role) = extras.get("role") {
                        node_extras["role"] = role.clone();
                    }
                    self.builder.nodes[r.node]["extras"] = node_extras;
                }
                self.stats.static_renderers += 1;
            }
        }
        scene.extend(skinned_nodes);
        if self.stats.skinned_renderers + self.stats.static_renderers == 0 {
            let mut reason = format!("no renderable meshes ({} renderers", renderers.len());
            if !self.unresolved.is_empty() {
                let sample: Vec<_> = self.unresolved.iter().take(3).cloned().collect();
                reason += &format!(
                    ", {} unresolved refs e.g. {}",
                    self.unresolved.len(),
                    sample.join(", ")
                );
            }
            if let Some(w) = self.warnings.iter().next() {
                reason += &format!("; {w}");
            }
            return Err(reason + ")");
        }
        self.stats.nodes = self.builder.nodes.len();
        self.stats.meshes = self.builder.meshes.len();
        self.stats.skins = self.builder.skins.len();
        self.stats.materials = self.builder.materials.len();
        self.stats.textures = self.builder.textures.len();
        self.stats.unresolved_refs = self.unresolved.len();
        self.stats.animators = self.animators.len();
        if !self.unresolved.is_empty() {
            let sample: Vec<_> = self.unresolved.iter().take(5).cloned().collect();
            self.warn(format!(
                "{} unresolved references (e.g. {})",
                self.unresolved.len(),
                sample.join(", ")
            ));
        }
        let animations = self.embed_clips(root_node);
        let mut extras = asset_extras;
        if !animations.is_empty() {
            extras["animations"] = serde_json::to_value(&animations).unwrap_or_default();
        }
        extras["counts"] = serde_json::to_value(&self.stats).unwrap_or_default();
        extras["animators"] = json!(self.animators);
        extras["coordinateSystem"] =
            json!("Unity left-handed converted by negating X (rotations (x,-y,-z,w))");
        let warnings: Vec<String> = self.warnings.into_iter().collect();
        let (document, glb) = self.builder.finish(scene, extras);
        let (_, bin) = gltf::parse_glb(&glb)?;
        let errors = gltf::check_document(&document, bin);
        if !errors.is_empty() {
            return Err(format!("invalid document: {}", errors.join("; ")));
        }
        Ok(EntityExport {
            glb,
            document,
            stats: self.stats,
            warnings,
        })
    }
}
