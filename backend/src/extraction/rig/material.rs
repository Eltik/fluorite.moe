//! Unity Material → glTF material, with the shader's name and saved properties in `extras`
//! and textures embedded from the extracted PNG tree (or decoded from bundle data).
use super::export::{Exporter, ObjRef};
use crate::extraction::texture::{bgra_to_rgba_flipped, decode_texture, resolve_stream_data};
use crate::extraction::texture_layout::plan_textures;
use crate::extraction::unity::value_bytes;
use image::{DynamicImage, ImageFormat, RgbaImage};
use serde_json::{Map, Value, json};
use std::collections::HashMap;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::sync::Arc;

const CLASS_TEXTURE2D: i32 = 28;
/// Largest texture side decoded from bundle data.
pub const MAX_TEXTURE_SIDE: usize = 16384;
const CLASS_SHADER: i32 = 48;
pub const CLASS_MATERIAL: i32 = 21;

/// What a texture is used for, which decides how it is processed before embedding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TextureRole {
    BaseColor,
    Normal,
    Emission,
    /// A non-colour map (masks, P maps, SDFs), embedded as lossless PNG at full size.
    Data,
    /// A ramp or LUT sampled at exact texels: like [`Self::Data`], with a clamped,
    /// non-mipmapped sampler.
    Ramp,
}

impl TextureRole {
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::BaseColor => "baseColor",
            Self::Normal => "normal",
            Self::Emission => "emission",
            Self::Data => "data",
            Self::Ramp => "ramp",
        }
    }

    /// Whether the image is shading data that must stay lossless and full size.
    #[must_use]
    pub fn is_data(&self) -> bool {
        matches!(self, Self::Data | Self::Ramp)
    }
}

/// Texture env names bound to glTF slots, in priority order.
pub const BASE_COLOR_NAMES: &[&str] = &[
    "_BaseMap",
    "_MainTex",
    "_DiffuseMap",
    "_BaseColorMap",
    "_AlbedoMap",
    "_Albedo",
    "_BaseTex",
];
pub const NORMAL_NAMES: &[&str] = &["_BumpMap", "_NormalMap", "_NormalTex", "_BumpTex"];
pub const EMISSION_NAMES: &[&str] = &["_EmissionMap", "_EmissiveMap", "_EmissiveTex"];

/// Data maps the game's NPR character shaders (`HGRP/CharacterNPR*`) sample, embedded by
/// default: ramps, the P (metal/gloss) map, outline and SDF masks, the skin shadow LUT,
/// fur dye and UV2 colour masks, parallax.
pub const NPR_DATA_SLOTS: &[&str] = &[
    "_DiffRampMap",
    "_SpecRampMap",
    "_MetallicGlossMap",
    "_OutlineMask",
    "_SDFLightmap",
    "_SDFMask",
    "_HighlightMap",
    "_ShadowLutTex",
    "_ExtraAlphaMask",
    "_FurDyeMap",
    "_ParallaxTex",
];

/// Slots sampled at exact texels (ramp rows, the 2D-unwrapped 3D LUT).
pub const RAMP_SLOTS: &[&str] = &["_DiffRampMap", "_SpecRampMap", "_ShadowLutTex"];

/// Extras texture slots holding colour (sRGB) data; every other slot is linear data.
pub const SRGB_DATA_SLOTS: &[&str] = &["_FurDyeMap", "_ParallaxTex"];

/// Whether a texture slot holds sRGB colour (albedo, emission, dye) rather than linear data.
#[must_use]
pub fn is_srgb_slot(name: &str) -> bool {
    BASE_COLOR_NAMES.contains(&name)
        || EMISSION_NAMES.contains(&name)
        || SRGB_DATA_SLOTS.contains(&name)
}

/// Whether a shader is one of the game's NPR character shaders.
#[must_use]
pub fn is_npr_shader(shader: &str) -> bool {
    shader.starts_with("HGRP/CharacterNPR")
}

/// glTF alpha mode of an NPR character material, mirroring warfarin's exporter (checked on
/// all 456 materials of their 34 operators): `_EnableAlphaTest` → MASK at
/// `_AlphaClipThreshold`; `_SurfaceType` 1 with SrcAlpha blending (`_SrcBlend` 5) → BLEND;
/// anything else (including premultiplied One/OneMinusSrcAlpha, which the client blends
/// itself) → OPAQUE. Render queue and `_AlphaClip` do not matter.
#[must_use]
pub fn npr_alpha_mode(floats: &Value) -> (&'static str, Option<f32>) {
    let f = |k: &str| floats.get(k).and_then(Value::as_f64);
    if f("_EnableAlphaTest") == Some(1.0) {
        let cutoff = f("_AlphaClipThreshold").unwrap_or(0.5).clamp(0.0, 1.0) as f32;
        return ("MASK", Some(cutoff));
    }
    if f("_SurfaceType") == Some(1.0) && f("_SrcBlend") == Some(5.0) {
        return ("BLEND", None);
    }
    ("OPAQUE", None)
}

/// `{name: {r,g,b,a}}` → `{name: [r,g,b,a]}`.
#[must_use]
pub fn color_arrays(colors: Option<&Value>) -> Value {
    let Some(map) = colors.and_then(Value::as_object) else {
        return colors.cloned().unwrap_or(Value::Null);
    };
    let out: Map<String, Value> = map
        .iter()
        .map(|(k, v)| {
            let arr = color(Some(v)).map_or_else(|| v.clone(), |c| json!(c));
            (k.clone(), arr)
        })
        .collect();
    Value::Object(out)
}

/// Role of a texture env name, if it maps to a glTF slot.
#[must_use]
pub fn role_for(name: &str) -> Option<TextureRole> {
    if BASE_COLOR_NAMES.contains(&name) {
        Some(TextureRole::BaseColor)
    } else if NORMAL_NAMES.contains(&name) {
        Some(TextureRole::Normal)
    } else if EMISSION_NAMES.contains(&name) {
        Some(TextureRole::Emission)
    } else {
        None
    }
}

/// Extracted textures on disk: `<root>/<relative>.png`, plus a name → path index.
#[derive(Debug, Default)]
pub struct TextureLibrary {
    pub root: PathBuf,
    /// Texture name (lowercase) → (relative path, format).
    pub by_name: HashMap<String, (PathBuf, String)>,
}

impl TextureLibrary {
    /// Read every `<root>/.index/*.jsonl` written by texture extraction.
    #[must_use]
    pub fn load(root: &Path) -> Self {
        let mut lib = Self {
            root: root.to_path_buf(),
            by_name: HashMap::new(),
        };
        let Ok(entries) = std::fs::read_dir(root.join(".index")) else {
            return lib;
        };
        for entry in entries.filter_map(Result::ok) {
            let Ok(text) = std::fs::read_to_string(entry.path()) else {
                continue;
            };
            for line in text.lines() {
                let Ok(v) = serde_json::from_str::<Value>(line) else {
                    continue;
                };
                let (Some(name), Some(path)) = (
                    v.get("name").and_then(Value::as_str),
                    v.get("path").and_then(Value::as_str),
                ) else {
                    continue;
                };
                let format = v.get("format").and_then(Value::as_str).unwrap_or("");
                lib.by_name
                    .entry(name.to_ascii_lowercase())
                    .or_insert_with(|| (PathBuf::from(path), format.to_string()));
            }
        }
        lib
    }
}

/// Material assets of the store by name, for models shipped without renderers (named NPC
/// `sk_*.fbx` imports): their parts' materials are separate `.mat` assets next to the model
/// (`<entity>/materials/m_<part>.mat`), or loose Materials inside the part mesh bundles.
#[derive(Debug, Default, Clone)]
pub struct MaterialAssets {
    /// Material name (lowercase `.mat` stem) → `(cab, path_id, container)`.
    pub by_name: HashMap<String, Vec<(String, i64, String)>>,
    /// `(container, cab)` of stored bundles holding Material objects, for scans by directory.
    pub bundles: Vec<(String, String)>,
}

impl MaterialAssets {
    /// Index the `.mat` containers and Material-holding bundles of stored records.
    #[must_use]
    pub fn from_records(
        records: &[super::store::CabRecord],
        stored: &dyn Fn(&str) -> bool,
    ) -> Self {
        let mut out = Self::default();
        for rec in records {
            if !rec.classes.contains_key(&CLASS_MATERIAL) || !stored(&rec.cab) {
                continue;
            }
            for (path, _, path_id) in &rec.containers {
                let lower = path.to_ascii_lowercase();
                if let Some(stem) = lower
                    .strip_suffix(".mat")
                    .and_then(|p| p.rsplit('/').next())
                {
                    out.by_name.entry(stem.to_string()).or_default().push((
                        rec.cab.clone(),
                        *path_id,
                        lower.clone(),
                    ));
                }
                out.bundles.push((lower, rec.cab.clone()));
            }
        }
        out
    }

    /// The `.mat` asset named `name`, preferring one whose container starts with `near`.
    #[must_use]
    pub fn lookup(&self, name: &str, near: &str) -> Option<(&str, i64)> {
        let found = self.by_name.get(&name.to_ascii_lowercase())?;
        found
            .iter()
            .find(|(_, _, c)| c.starts_with(near))
            .or_else(|| found.first())
            .map(|(cab, id, _)| (cab.as_str(), *id))
    }

    /// Stored Material bundles whose container starts with `near`, deduplicated.
    #[must_use]
    pub fn bundles_near(&self, near: &str) -> Vec<&str> {
        let mut cabs: Vec<&str> = self
            .bundles
            .iter()
            .filter(|(c, _)| c.starts_with(near))
            .map(|(_, cab)| cab.as_str())
            .collect();
        cabs.sort_unstable();
        cabs.dedup();
        cabs
    }
}

/// Material name of a model part: `S_npc_major_x_face_02_lod0` → `m_npc_major_x_face_02`.
#[must_use]
pub fn material_name_for_mesh(mesh: &str) -> String {
    let (base, _) = super::export::split_lod(mesh);
    match base.strip_prefix("s_") {
        Some(rest) => format!("m_{rest}"),
        None => format!("m_{base}"),
    }
}

/// Pick the material of a model part among an entity's material names (lowercase): the
/// exact `m_<part>` name, else the only non-LOD material ending in the part's `<kind>_<nn>`
/// (`S_actor_death_cloth_02` → `M_npc_major_death_cloth_02`; `brow`/`eyebrow` and
/// `iris`/`eye` are interchangeable).
#[must_use]
pub fn choose_part_material(mesh: &str, names: &[String]) -> Option<usize> {
    let want = material_name_for_mesh(mesh);
    if let Some(i) = names.iter().position(|n| *n == want) {
        return Some(i);
    }
    let (base, _) = super::export::split_lod(mesh);
    let mut segs = base.rsplitn(3, '_');
    let (num, kind) = (segs.next()?, segs.next()?);
    if num.is_empty() || !num.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    // Part kinds named differently by meshes and materials of one model.
    let alias = match kind {
        "brow" => Some("eyebrow"),
        "eyebrow" => Some("brow"),
        "iris" => Some("eye"),
        "eye" => Some("iris"),
        _ => None,
    };
    let suffixes: Vec<String> = std::iter::once(kind)
        .chain(alias)
        .map(|k| format!("_{k}_{num}"))
        .collect();
    let hits: Vec<usize> = names
        .iter()
        .enumerate()
        .filter(|(_, n)| !n.contains("_lod_") && suffixes.iter().any(|s| n.ends_with(s.as_str())))
        .map(|(i, _)| i)
        .collect();
    (hits.len() == 1).then(|| hits[0])
}

/// Shared overlay-shadow material of a shadow part: `S_x_hairshadow_01` → `m_hairshadow_common_01`.
#[must_use]
pub fn common_shadow_material(mesh: &str) -> Option<String> {
    let (base, _) = super::export::split_lod(mesh);
    let kind = base.rsplit('_').nth(1)?;
    kind.ends_with("shadow")
        .then(|| format!("m_{kind}_common_01"))
}

/// Entity directory of a model container: `.../george/models/sk_x.fbx` → `.../george/`.
#[must_use]
pub fn entity_dir(container: &str) -> String {
    let lower = container.to_ascii_lowercase();
    match lower.rfind("/models/") {
        Some(i) => lower[..=i].to_string(),
        None => lower
            .rfind('/')
            .map_or(String::new(), |i| lower[..=i].to_string()),
    }
}

/// How embedded images are encoded.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ImageEncoding {
    /// JPEG (quality 90) for images without alpha, PNG for the rest: ~5–10× smaller than PNG.
    #[default]
    Auto,
    /// Lossless PNG for everything.
    Png,
    /// Lossless WebP for everything (`EXT_texture_webp`).
    WebpLossless,
}

impl std::str::FromStr for ImageEncoding {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "auto" => Ok(Self::Auto),
            "png" => Ok(Self::Png),
            "webp" => Ok(Self::WebpLossless),
            other => Err(format!(
                "unknown image encoding {other:?} (auto, png, webp)"
            )),
        }
    }
}

/// Material and texture export settings.
#[derive(Debug, Clone, Default)]
pub struct MaterialOptions {
    pub library: Option<Arc<TextureLibrary>>,
    /// Embed every referenced texture (masks, ramps…), not only glTF-bound ones.
    pub all_textures: bool,
    pub encoding: ImageEncoding,
    /// Downscale images whose larger side exceeds this.
    pub max_texture_size: Option<u32>,
}

fn color(value: Option<&Value>) -> Option<[f32; 4]> {
    let v = value?;
    let get = |k: &str| v.get(k).and_then(Value::as_f64).map(|x| x as f32);
    Some([get("r")?, get("g")?, get("b")?, get("a")?])
}

fn float(props: &Value, name: &str) -> Option<f32> {
    props
        .pointer(&format!("/m_Floats/{name}"))
        .and_then(Value::as_f64)
        .map(|x| x as f32)
}

/// Cache key of an exported material.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum MaterialKey {
    Unity(ObjRef),
    /// A neutral/name-matched fallback for a part whose material could not be read.
    Fallback(String),
}

/// glTF material index for a Unity material (cached). `None` gets a neutral default.
pub fn material_index(exp: &mut Exporter, material: Option<ObjRef>, name_hint: &str) -> usize {
    // Fallbacks are textured by part name, so they are cached per part, not per (missing) ref.
    let key = match material {
        Some(m) => MaterialKey::Unity(m),
        None => MaterialKey::Fallback(super::export::split_lod(name_hint).0),
    };
    if let Some(&i) = exp.material_cache.get(&key) {
        return i;
    }
    let built = match material.and_then(|m| build_material(exp, m)) {
        Some(built) => built,
        None => fallback_material(exp, name_hint),
    };
    exp.builder.materials.push(built);
    let i = exp.builder.materials.len() - 1;
    exp.material_cache.insert(key, i);
    i
}

/// Neutral material, textured by name (`S_x_lod0` mesh → `T_x_D`) when the library has it.
fn fallback_material(exp: &mut Exporter, name_hint: &str) -> Value {
    let mut pbr = json!({"baseColorFactor": [0.8, 0.8, 0.8, 1.0], "metallicFactor": 0.0, "roughnessFactor": 1.0});
    let (base, _) = super::export::split_lod(name_hint);
    let guess = base.strip_prefix("s_").map(|rest| format!("t_{rest}_d"));
    if let Some(guess) = guess
        && let Some(tex) = library_texture(exp, &guess, TextureRole::BaseColor)
    {
        pbr["baseColorFactor"] = json!([1.0, 1.0, 1.0, 1.0]);
        pbr["baseColorTexture"] = json!({"index": tex});
        exp.warn(format!(
            "{name_hint}: material unresolved; texture {guess} matched by name"
        ));
    } else {
        exp.warn(format!(
            "{name_hint}: material unresolved; neutral fallback"
        ));
    }
    json!({"name": format!("fallback_{base}"), "pbrMetallicRoughness": pbr, "extras": {"fallback": true}})
}

fn shader_name(exp: &mut Exporter, shader: ObjRef) -> Option<String> {
    if let Some(name) = exp.shader_cache.get(&shader) {
        return Some(name.clone());
    }
    if exp.class_of(shader)? != CLASS_SHADER {
        return None;
    }
    let name = exp
        .peek_name(shader)
        .filter(|n| !n.is_empty())
        .or_else(|| {
            exp.read_prefix(shader, &["m_ParsedForm"]).and_then(|f| {
                f.get("m_ParsedForm")?
                    .get("m_Name")?
                    .as_str()
                    .map(str::to_string)
            })
        })?;
    exp.shader_cache.insert(shader, name.clone());
    Some(name)
}

fn build_material(exp: &mut Exporter, mref: ObjRef) -> Option<Value> {
    let (class, mat) = exp.read(mref)?;
    if class != 21 {
        return None;
    }
    let name = mat
        .get("m_Name")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let shader_ref = exp.resolve(mref.file, mat.get("m_Shader"));
    let shader = shader_ref.and_then(|s| shader_name(exp, s));
    let props = mat.get("m_SavedProperties").cloned().unwrap_or(Value::Null);
    let render_queue = mat
        .get("m_CustomRenderQueue")
        .and_then(Value::as_i64)
        .unwrap_or(-1);
    let tags = mat.get("stringTagMap").cloned().unwrap_or(json!({}));
    let keywords = mat.get("m_ValidKeywords").cloned().unwrap_or(json!([]));

    let npr = shader.as_deref().is_some_and(is_npr_shader);
    let all_textures = exp_all_textures(exp);
    let emission_color = color(props.pointer("/m_Colors/_EmissionColor")).unwrap_or([0.0; 4]);
    // NPR characters: warfarin embeds no emission map for any of their 47 `_UseEmission`
    // materials, so the Studio would glow where theirs does not. Mirror that unless every
    // texture is asked for.
    let emission_on = keywords
        .as_array()
        .is_some_and(|k| k.iter().any(|k| k.as_str() == Some("_EMISSION")))
        && emission_color[..3].iter().any(|c| *c > 0.0)
        && (!npr || all_textures);
    let mut tex_extras = Map::new();
    let mut bound: HashMap<TextureRole, (usize, u64)> = HashMap::new();
    let envs: Vec<(String, Value)> = props
        .get("m_TexEnvs")
        .and_then(Value::as_object)
        .map(|m| m.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
        .unwrap_or_default();
    for (env_name, env) in &envs {
        let Some(tref) = exp.resolve(mref.file, env.get("m_Texture")) else {
            continue;
        };
        let tex_name = exp.peek_name(tref);
        let role = role_for(env_name);
        let uv = env.get("m_UVSetIndex").and_then(Value::as_u64).unwrap_or(0);
        let data_role = if RAMP_SLOTS.contains(&env_name.as_str()) {
            TextureRole::Ramp
        } else {
            TextureRole::Data
        };
        let embed = match role {
            Some(TextureRole::Emission) => emission_on || all_textures,
            Some(_) => true,
            None => all_textures || NPR_DATA_SLOTS.contains(&env_name.as_str()),
        };
        let index = if embed {
            texture_index(exp, tref, role.unwrap_or(data_role))
        } else {
            None
        };
        if let (Some(role), Some(index)) = (role, index) {
            bound.entry(role).or_insert((index, uv));
        }
        tex_extras.insert(
            env_name.clone(),
            json!({
                "name": tex_name,
                "texture": index,
                "scale": env.get("m_Scale"),
                "offset": env.get("m_Offset"),
                "uvSet": uv,
            }),
        );
    }
    // Prefer the first base/normal name in priority order when several are bound.
    for (role, names) in [
        (TextureRole::BaseColor, BASE_COLOR_NAMES),
        (TextureRole::Normal, NORMAL_NAMES),
        (TextureRole::Emission, EMISSION_NAMES),
    ] {
        for n in names {
            if let Some(t) = tex_extras.get(*n)
                && let Some(i) = t.get("texture").and_then(Value::as_u64)
            {
                let uv = t.get("uvSet").and_then(Value::as_u64).unwrap_or(0);
                bound.insert(role, (i as usize, uv));
                break;
            }
        }
    }

    let base_factor = color(props.pointer("/m_Colors/_BaseColor"))
        .or_else(|| color(props.pointer("/m_Colors/_Color")))
        .unwrap_or([1.0; 4])
        .map(|c| c.clamp(0.0, 1.0));
    let mut pbr = json!({"metallicFactor": 0.0, "roughnessFactor": 1.0});
    match bound.get(&TextureRole::BaseColor) {
        Some((i, uv)) => {
            pbr["baseColorTexture"] = json!({"index": i, "texCoord": uv});
            pbr["baseColorFactor"] = json!(base_factor);
        }
        None => {
            // No albedo map: a neutral tint rather than black or white.
            let neutral = if base_factor == [1.0; 4] {
                [0.8, 0.8, 0.8, 1.0]
            } else {
                base_factor
            };
            pbr["baseColorFactor"] = json!(neutral);
        }
    }
    let shader_lower = shader.clone().unwrap_or_default().to_ascii_lowercase();
    // Untextured effect shells (energy shields, glows) would read as solid grey blobs:
    // keep them as faint tinted glass instead.
    let effect = shader_lower.contains("vfx") || shader_lower.contains("effect");
    let faint_effect = effect && !bound.contains_key(&TextureRole::BaseColor);
    if faint_effect {
        let tint = color(props.pointer("/m_Colors/_TintColor"))
            .or_else(|| color(props.pointer("/m_Colors/_BaseColor")))
            .unwrap_or([1.0; 4]);
        pbr["baseColorFactor"] = json!([
            tint[0].clamp(0.0, 1.0),
            tint[1].clamp(0.0, 1.0),
            tint[2].clamp(0.0, 1.0),
            0.15
        ]);
    }
    let mut out = json!({"name": name, "pbrMetallicRoughness": pbr});
    if let Some((i, uv)) = bound.get(&TextureRole::Normal) {
        out["normalTexture"] = json!({"index": i, "texCoord": uv});
    }
    if emission_on && let Some((i, uv)) = bound.get(&TextureRole::Emission) {
        out["emissiveTexture"] = json!({"index": i, "texCoord": uv});
        out["emissiveFactor"] = json!(
            emission_color[..3]
                .iter()
                .map(|c| c.clamp(0.0, 1.0))
                .collect::<Vec<_>>()
        );
    }
    let render_type = tags.get("RenderType").and_then(Value::as_str).unwrap_or("");
    let transparent = render_queue >= 2900
        || render_type.eq_ignore_ascii_case("Transparent")
        || float(&props, "_Surface") == Some(1.0)
        || shader_lower.contains("transparent");
    let cutout = (2450..2900).contains(&render_queue)
        || float(&props, "_AlphaClip") == Some(1.0)
        || render_type.eq_ignore_ascii_case("TransparentCutout");
    let cull = float(&props, "_Cull").or_else(|| float(&props, "_CullMode"));
    if npr {
        let floats = props.get("m_Floats").cloned().unwrap_or(Value::Null);
        match npr_alpha_mode(&floats) {
            ("OPAQUE", _) => {}
            (mode, cutoff) => {
                out["alphaMode"] = json!(mode);
                if let Some(c) = cutoff {
                    out["alphaCutoff"] = json!(c);
                }
            }
        }
        // Shaders without `_Cull` (overlay shadows, VFX) draw both faces.
        if cull.is_none_or(|c| c == 0.0) {
            out["doubleSided"] = json!(true);
        }
    } else {
        if transparent || faint_effect {
            out["alphaMode"] = json!("BLEND");
        } else if cutout && bound.contains_key(&TextureRole::BaseColor) {
            out["alphaMode"] = json!("MASK");
            out["alphaCutoff"] = json!(float(&props, "_Cutoff").unwrap_or(0.5).clamp(0.0, 1.0));
        }
        if cull == Some(0.0) {
            out["doubleSided"] = json!(true);
        }
    }
    let srgb: Vec<&String> = tex_extras
        .iter()
        .filter(|(slot, t)| is_srgb_slot(slot) && t.get("texture").is_some_and(|i| !i.is_null()))
        .map(|(slot, _)| slot)
        .collect();
    out["extras"] = json!({
        "shader": shader,
        "effect": effect,
        "renderQueue": render_queue,
        "keywords": keywords,
        "tags": tags,
        "textures": tex_extras,
        "floats": props.get("m_Floats"),
        "colors": color_arrays(props.get("m_Colors")),
        "ints": props.get("m_Ints"),
        "srgb": srgb,
    });
    Some(out)
}

fn exp_all_textures(exp: &Exporter) -> bool {
    exp.material_options().all_textures
}

/// Relative PNG path of a Texture2D as texture extraction laid it out.
fn planned_path(exp: &mut Exporter, tref: ObjRef) -> Option<PathBuf> {
    if !exp.texture_paths.contains_key(&tref.file) {
        let sf = &exp.src.files.files[tref.file].sf;
        let plans = plan_textures(sf)
            .into_iter()
            .map(|p| (p.path_id, p.relative))
            .collect();
        exp.texture_paths.insert(tref.file, plans);
    }
    exp.texture_paths
        .get(&tref.file)?
        .get(&tref.path_id)
        .cloned()
}

/// glTF texture index for a Texture2D (cached per role).
pub fn texture_index(exp: &mut Exporter, tref: ObjRef, role: TextureRole) -> Option<usize> {
    if let Some(&cached) = exp.texture_cache.get(&(tref, role)) {
        return cached;
    }
    let result = load_texture(exp, tref, role);
    exp.texture_cache.insert((tref, role), result);
    result
}

fn load_texture(exp: &mut Exporter, tref: ObjRef, role: TextureRole) -> Option<usize> {
    if exp.class_of(tref)? != CLASS_TEXTURE2D {
        return None;
    }
    let name = exp.peek_name(tref).unwrap_or_default();
    let opts = exp.material_options().clone();
    let mut image: Option<(DynamicImage, String)> = None;
    if let Some(lib) = &opts.library
        && let Some(rel) = planned_path(exp, tref)
        && let Ok(bytes) = std::fs::read(lib.root.join(&rel))
        && let Ok(img) = image::load_from_memory_with_format(&bytes, ImageFormat::Png)
    {
        let format = lib
            .by_name
            .get(&name.to_ascii_lowercase())
            .map(|(_, f)| f.clone())
            .unwrap_or_default();
        image = Some((img, format));
    }
    if image.is_none() {
        image = decode_from_bundle(exp, tref);
    }
    if image.is_none()
        && let Some(lib) = &opts.library
        && let Some((rel, format)) = lib.by_name.get(&name.to_ascii_lowercase())
        && let Ok(bytes) = std::fs::read(lib.root.join(rel))
        && let Ok(img) = image::load_from_memory_with_format(&bytes, ImageFormat::Png)
    {
        image = Some((img, format.clone()));
    }
    let Some((img, format)) = image else {
        exp.warn(format!(
            "texture {name}: no pixels (not in texture tree, no bundle data)"
        ));
        return None;
    };
    embed(exp, img, &format, &name, role, &opts)
}

/// Embed a texture found by name in the library (used by the fallback material).
pub fn library_texture(exp: &mut Exporter, name: &str, role: TextureRole) -> Option<usize> {
    let opts = exp.material_options().clone();
    let lib = opts.library.as_ref()?;
    let (rel, format) = lib.by_name.get(&name.to_ascii_lowercase())?;
    let bytes = std::fs::read(lib.root.join(rel)).ok()?;
    let img = image::load_from_memory_with_format(&bytes, ImageFormat::Png).ok()?;
    embed(exp, img, &format.clone(), name, role, &opts)
}

fn decode_from_bundle(exp: &mut Exporter, tref: ObjRef) -> Option<(DynamicImage, String)> {
    let (_, tex) = exp.read(tref)?;
    let width = tex.get("m_Width")?.as_u64()? as usize;
    let height = tex.get("m_Height")?.as_u64()? as usize;
    let format = tex.get("m_TextureFormat")?.as_i64()?;
    let data = tex
        .get("image data")
        .and_then(value_bytes)
        .filter(|b| !b.is_empty())
        .or_else(|| resolve_stream_data(&tex, &exp.src.files.resources))?;
    // Unity caps textures at 16384 per side; larger values are corrupt headers.
    if width == 0 || height == 0 || width > MAX_TEXTURE_SIDE || height > MAX_TEXTURE_SIDE {
        return None;
    }
    let mut pixels = vec![0u32; width * height];
    decode_texture(format, &data, width, height, &mut pixels).ok()?;
    let rgba = bgra_to_rgba_flipped(&pixels, width, height);
    let img = RgbaImage::from_raw(width as u32, height as u32, rgba)?;
    let name = crate::extraction::texture::texture_format_name(format).to_string();
    Some((DynamicImage::ImageRgba8(img), name))
}

/// Mean of each RGBA channel over a sparse sample, in 0..1.
fn channel_means(img: &RgbaImage) -> [f32; 4] {
    let mut sum = [0f64; 4];
    let mut n = 0f64;
    let step = ((img.width() * img.height()) / 4096).max(1) as usize;
    for p in img.pixels().step_by(step) {
        for (acc, &v) in sum.iter_mut().zip(&p.0) {
            *acc += f64::from(v);
        }
        n += 1.0;
    }
    sum.map(|s| (s / n.max(1.0) / 255.0) as f32)
}

/// Rebuild a tangent-space normal map from two channels, reconstructing Z.
///
/// Returns `None` when the image does not look like a normal map.
#[must_use]
pub fn normal_map(img: &RgbaImage, format: &str) -> Option<RgbaImage> {
    let [r, g, b, _a] = channel_means(img);
    let two_channel = format == "BC5" || format == "RG16" || b < 0.2;
    if !two_channel {
        // Already a full XYZ normal map when blue (Z) dominates.
        return (b > 0.6 && (r - 0.5).abs() < 0.25 && (g - 0.5).abs() < 0.25).then(|| img.clone());
    }
    if (r - 0.5).abs() > 0.3 || (g - 0.5).abs() > 0.3 {
        return None;
    }
    let mut out = img.clone();
    for p in out.pixels_mut() {
        let x = f32::from(p.0[0]) / 255.0 * 2.0 - 1.0;
        let y = f32::from(p.0[1]) / 255.0 * 2.0 - 1.0;
        let z = (1.0 - x * x - y * y).max(0.0).sqrt();
        p.0[2] = ((z * 0.5 + 0.5) * 255.0).round() as u8;
        p.0[3] = 255;
    }
    Some(out)
}

fn embed(
    exp: &mut Exporter,
    img: DynamicImage,
    format: &str,
    name: &str,
    role: TextureRole,
    opts: &MaterialOptions,
) -> Option<usize> {
    let mut rgba = img.to_rgba8();
    if role == TextureRole::Normal {
        match normal_map(&rgba, format) {
            Some(n) => rgba = n,
            None => {
                exp.warn(format!(
                    "texture {name}: not a recognizable normal map; skipped"
                ));
                return None;
            }
        }
    }
    if let Some(max) = opts.max_texture_size.filter(|_| !role.is_data())
        && (rgba.width() > max || rgba.height() > max)
    {
        let scale = f64::from(max) / f64::from(rgba.width().max(rgba.height()));
        let w = ((f64::from(rgba.width()) * scale).round() as u32).max(1);
        let h = ((f64::from(rgba.height()) * scale).round() as u32).max(1);
        rgba = image::imageops::resize(&rgba, w, h, image::imageops::FilterType::Triangle);
    }
    // Opaque images drop the alpha channel to save space.
    let opaque = rgba.pixels().all(|p| p.0[3] == 255);
    let dynamic = if opaque {
        DynamicImage::ImageRgb8(DynamicImage::ImageRgba8(rgba).to_rgb8())
    } else {
        DynamicImage::ImageRgba8(rgba)
    };
    let mut bytes = Vec::new();
    let encoding = if role.is_data() && opts.encoding == ImageEncoding::Auto {
        // Data is never JPEG: ramps and LUTs are sampled at exact texels.
        ImageEncoding::Png
    } else {
        opts.encoding
    };
    let encoded = match (encoding, opaque) {
        (ImageEncoding::Auto, true) => {
            let encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut bytes, 90);
            dynamic.write_with_encoder(encoder).map(|()| "image/jpeg")
        }
        (ImageEncoding::WebpLossless, _) => dynamic
            .write_to(&mut Cursor::new(&mut bytes), ImageFormat::WebP)
            .map(|()| "image/webp"),
        _ => dynamic
            .write_to(&mut Cursor::new(&mut bytes), ImageFormat::Png)
            .map(|()| "image/png"),
    };
    let mime = match encoded {
        Ok(mime) => mime,
        Err(e) => {
            exp.warn(format!("texture {name}: encode failed: {e}"));
            return None;
        }
    };
    Some(if role == TextureRole::Ramp {
        exp.builder.clamped_texture(&bytes, mime, name)
    } else {
        exp.builder.texture(&bytes, mime, name)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extraction::rig::store::CabRecord;
    use std::collections::BTreeMap;

    fn record(cab: &str, container: &str, path_id: i64) -> CabRecord {
        CabRecord {
            cab: cab.into(),
            chunk: "c".into(),
            sf_size: 0,
            resources: Vec::new(),
            resource_sizes: BTreeMap::new(),
            classes: BTreeMap::from([(CLASS_MATERIAL, 1)]),
            containers: vec![(container.to_string(), 0, path_id)],
            externals: Vec::new(),
        }
    }

    #[test]
    fn part_material_names() {
        assert_eq!(
            material_name_for_mesh("S_npc_major_andrew_face_02_lod0"),
            "m_npc_major_andrew_face_02"
        );
        assert_eq!(material_name_for_mesh("Body"), "m_body");
        assert_eq!(
            entity_dir("assets/npc/major/gentleman/george/models/sk_npc_major_george_01.fbx"),
            "assets/npc/major/gentleman/george/"
        );
    }

    #[test]
    fn part_materials_match_by_name_then_part_suffix() {
        let names: Vec<String> = [
            "m_npc_major_death_cloth_02",
            "m_npc_major_lod_death_cloth_03",
            "m_npc_major_death_cloth_03",
            "m_npc_major_death_eyebrow_01",
            "m_npc_major_andrew_face_02",
            "m_npc_major_x_face_01",
            "m_npc_major_y_face_01",
        ]
        .map(String::from)
        .to_vec();
        let pick = |mesh: &str| choose_part_material(mesh, &names).map(|i| names[i].as_str());
        assert_eq!(
            pick("S_npc_major_andrew_face_02_lod0"),
            Some("m_npc_major_andrew_face_02")
        );
        assert_eq!(
            pick("S_actor_death_cloth_03_lod0"),
            Some("m_npc_major_death_cloth_03")
        );
        assert_eq!(
            pick("S_actor_death_brow_01_lod0"),
            Some("m_npc_major_death_eyebrow_01")
        );
        let qinjc = vec![
            "m_npc_major_qinjc_brow_01".to_string(),
            "m_npc_major_qinjc_eye_01".into(),
        ];
        assert_eq!(
            choose_part_material("S_npc_major_qinjc_eyebrow_01_lod0", &qinjc),
            Some(0)
        );
        assert_eq!(
            choose_part_material("S_npc_major_qinjc_iris_01_lod0", &qinjc),
            Some(1)
        );
        // Ambiguous or absent parts are left to the caller.
        assert_eq!(pick("S_actor_z_face_01_lod0"), None);
        assert_eq!(pick("S_actor_death_cloth_04_lod0"), None);
        assert_eq!(
            common_shadow_material("S_npc_major_x_hairshadow_01_lod0").as_deref(),
            Some("m_hairshadow_common_01")
        );
        assert_eq!(common_shadow_material("S_npc_major_x_hair_01_lod0"), None);
    }

    #[test]
    fn material_assets_prefer_the_entity_directory() {
        let records = vec![
            record(
                "CAB-a",
                "a/andrew/materials/m_npc_major_andrew_face_01.mat",
                1,
            ),
            record(
                "CAB-b",
                "a/andrewhq/materials/M_npc_major_andrew_face_01.mat",
                2,
            ),
            record(
                "CAB-c",
                "a/andrewhq/models/s_npc_major_andrew_body_01_lod0.asset",
                3,
            ),
            record(
                "CAB-gone",
                "a/andrew/materials/m_npc_major_andrew_iris_01.mat",
                4,
            ),
        ];
        let assets = MaterialAssets::from_records(&records, &|cab: &str| cab != "CAB-gone");
        let face = "m_npc_major_andrew_face_01";
        assert_eq!(assets.lookup(face, "a/andrewhq/"), Some(("CAB-b", 2)));
        assert_eq!(assets.lookup(face, "a/andrew/"), Some(("CAB-a", 1)));
        assert_eq!(assets.lookup(face, "a/other/"), Some(("CAB-a", 1)));
        assert_eq!(
            assets.lookup("m_npc_major_andrew_iris_01", "a/andrew/"),
            None
        );
        assert_eq!(assets.bundles_near("a/andrewhq/"), vec!["CAB-b", "CAB-c"]);
    }
}
