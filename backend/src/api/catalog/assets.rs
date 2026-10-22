//! File-system lookups for catalog entries: UI textures by naming convention, static mesh
//! folders (operators, NPCs, monsters, weapons), and rigged GLBs from `models/index.json`.
//! Everything here is blocking I/O; call it from `spawn_blocking`.
//!
//! The lookups go through [`Files`], so a root that lives in the bucket is answered from
//! its last listing (and the local disk), and every other root from the local disk.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use serde::Serialize;

use crate::api::paths::{Root, is_hidden};
use crate::api::storage::{BucketView, Listing};

/// The files of one root, as the catalog sees them. The paths are relative to the root,
/// with `/` between the directories.
pub trait Files {
    /// The size of a file, or `None` when there is no such file.
    fn file_size(&self, rel: &str) -> Option<u64>;

    /// Whether a file exists.
    fn has_file(&self, rel: &str) -> bool {
        self.file_size(rel).is_some()
    }

    /// The visible entries of a directory: the name and whether it is a directory.
    fn entries(&self, rel: &str) -> Vec<(String, bool)>;

    /// The contents of a file.
    fn read_file(&self, rel: &str) -> Option<Vec<u8>>;
}

/// A root on the local disk.
impl Files for Path {
    fn file_size(&self, rel: &str) -> Option<u64> {
        let meta = std::fs::metadata(self.join(rel)).ok()?;
        meta.is_file().then_some(meta.len())
    }

    fn entries(&self, rel: &str) -> Vec<(String, bool)> {
        let Ok(rd) = std::fs::read_dir(self.join(rel)) else {
            return Vec::new();
        };
        rd.flatten()
            .filter_map(|e| {
                let name = e.file_name().into_string().ok()?;
                let ft = e.file_type().ok()?;
                (!is_hidden(&name)).then(|| (name, ft.is_dir()))
            })
            .collect()
    }

    fn read_file(&self, rel: &str) -> Option<Vec<u8>> {
        std::fs::read(self.join(rel)).ok()
    }
}

impl Files for PathBuf {
    fn file_size(&self, rel: &str) -> Option<u64> {
        self.as_path().file_size(rel)
    }

    fn entries(&self, rel: &str) -> Vec<(String, bool)> {
        self.as_path().entries(rel)
    }

    fn read_file(&self, rel: &str) -> Option<Vec<u8>> {
        self.as_path().read_file(rel)
    }
}

/// The files of one root: the local directory, or the last listing of the bucket for a
/// root that lives there.
#[derive(Clone)]
pub enum RootFiles {
    Disk(PathBuf),
    /// The listing first, then the local directory: a file that is not published yet is
    /// still served from the disk.
    Bucket {
        listing: Arc<Listing>,
        disk: PathBuf,
    },
}

impl Files for RootFiles {
    fn file_size(&self, rel: &str) -> Option<u64> {
        match self {
            Self::Disk(dir) => dir.file_size(rel),
            Self::Bucket { listing, disk } => listing.size(rel).or_else(|| disk.file_size(rel)),
        }
    }

    fn entries(&self, rel: &str) -> Vec<(String, bool)> {
        match self {
            Self::Disk(dir) => dir.entries(rel),
            Self::Bucket { listing, disk } => {
                let mut all: BTreeMap<String, bool> = listing.entries(rel).into_iter().collect();
                for (name, is_dir) in disk.entries(rel) {
                    *all.entry(name).or_default() |= is_dir;
                }
                all.into_iter().collect()
            }
        }
    }

    /// From the bucket, the listing has the file: it is read from a blocking thread (the
    /// catalog builders run on one) and remembered until the next listing.
    fn read_file(&self, rel: &str) -> Option<Vec<u8>> {
        match self {
            Self::Disk(dir) => dir.read_file(rel),
            // A failed read of the bucket falls back to the local copy, if any.
            Self::Bucket { listing, disk } => listing
                .size(rel)
                .and_then(|_| listing.read(rel))
                .map(|bytes| bytes.to_vec())
                .or_else(|| disk.read_file(rel)),
        }
    }
}

/// Where the files of each root of an output tree are.
pub trait Roots {
    fn root_files(&self, root: Root) -> RootFiles;
}

/// An output directory with every root on the local disk.
impl Roots for Path {
    fn root_files(&self, root: Root) -> RootFiles {
        RootFiles::Disk(root.dir(self))
    }
}

impl Roots for PathBuf {
    fn root_files(&self, root: Root) -> RootFiles {
        self.as_path().root_files(root)
    }
}

/// The output directory plus the last listing of the bucket: the roots that live in the
/// bucket are looked up in it, the others on the local disk.
#[derive(Clone)]
pub struct OutputFiles {
    pub dir: PathBuf,
    pub view: Arc<BucketView>,
}

impl Roots for OutputFiles {
    fn root_files(&self, root: Root) -> RootFiles {
        let disk = root.dir(&self.dir);
        match self.view.listing(root) {
            Some(listing) => RootFiles::Bucket {
                listing: listing.clone(),
                disk,
            },
            None => RootFiles::Disk(disk),
        }
    }
}

/// UI sprite folder, relative to the textures root.
pub const UI: &str = "beyond/dynamicassets/gameplay/ui/sprites";

const ENTITY: &str = "beyond/arts/entity";
const PREFAB_WEAPONS: &str = "beyond/dynamicassets/gameplay/prefabs/weapons";
const ENEMY_POSTMODELS: &str = "beyond/dynamicassets/gameplay/actors/postmodels/enemies";

/// Texture-root-relative path of `UI/<dir>/<name>.png` when the file exists.
pub fn ui_png(textures: &(impl Files + ?Sized), dir: &str, name: &str) -> Option<String> {
    if name.is_empty() || name.contains("..") {
        return None;
    }
    let rel = format!("{UI}/{dir}/{name}.png");
    textures.has_file(&rel).then_some(rel)
}

/// Sprite reference like `TermIcon/icon_term_ba_noguard` → existing PNG under `UI/` (the
/// folder name is lowercased the way the extractor writes it).
pub fn ui_sprite_ref(textures: &(impl Files + ?Sized), reference: &str) -> Option<String> {
    let (dir, name) = reference.rsplit_once('/')?;
    ui_png(textures, &dir.to_ascii_lowercase(), name)
}

fn list_dirs(files: &(impl Files + ?Sized), dir: &str) -> Vec<String> {
    let mut names: Vec<String> = files
        .entries(dir)
        .into_iter()
        .filter_map(|(name, is_dir)| is_dir.then_some(name))
        .collect();
    names.sort();
    names
}

/// Mesh folders by entity code, relative to the meshes root.
#[derive(Debug, Default)]
pub struct MeshIndex {
    /// Operator/major-NPC body code (`chen`) → folder.
    pub actors: HashMap<String, String>,
    /// Monster code (`mimicw`) → folder.
    pub monsters: HashMap<String, String>,
    /// Enemy template id (`eny_0113_jzogre`) → postmodel folder.
    pub enemy_postmodels: HashMap<String, String>,
    /// Weapon id → folders (entity folder first, then the prefab folder).
    pub weapons: HashMap<String, Vec<String>>,
    /// Crowd NPC body (`npc_gentleman_body_riverbandits_a`) → folder.
    pub crowd: HashMap<String, String>,
}

impl MeshIndex {
    pub fn build(meshes: &(impl Files + ?Sized)) -> Self {
        let mut ix = Self::default();
        // Playable actors first so `actor/<type>/<code>` wins over NPC alternates.
        for base in ["actor", "npc/major"] {
            for body in list_dirs(meshes, &format!("{ENTITY}/{base}")) {
                for code in list_dirs(meshes, &format!("{ENTITY}/{base}/{body}")) {
                    ix.actors
                        .entry(code.clone())
                        .or_insert_with(|| format!("{ENTITY}/{base}/{body}/{code}"));
                }
            }
        }
        for tier in ["boss", "elite", "enemy"] {
            for code in list_dirs(meshes, &format!("{ENTITY}/monster/{tier}")) {
                ix.monsters
                    .entry(code.clone())
                    .or_insert_with(|| format!("{ENTITY}/monster/{tier}/{code}"));
            }
        }
        for name in list_dirs(meshes, ENEMY_POSTMODELS) {
            if let Some(id) = name.strip_suffix("_postmodel") {
                ix.enemy_postmodels
                    .insert(id.to_string(), format!("{ENEMY_POSTMODELS}/{name}"));
            }
        }
        for ty in list_dirs(meshes, &format!("{ENTITY}/wpn")) {
            for id in list_dirs(meshes, &format!("{ENTITY}/wpn/{ty}")) {
                ix.weapons
                    .entry(id.clone())
                    .or_default()
                    .push(format!("{ENTITY}/wpn/{ty}/{id}"));
            }
        }
        for id in list_dirs(meshes, PREFAB_WEAPONS) {
            ix.weapons
                .entry(id.clone())
                .or_default()
                .push(format!("{PREFAB_WEAPONS}/{id}"));
        }
        let crowd = format!("{ENTITY}/npc/pedestrain");
        for body in list_dirs(meshes, &crowd) {
            for name in list_dirs(meshes, &format!("{crowd}/{body}/body")) {
                ix.crowd.insert(
                    name.clone(),
                    format!("{ENTITY}/npc/pedestrain/{body}/body/{name}"),
                );
            }
        }
        ix
    }

    /// Mesh folder for an operator id `chr_NNNN_<code>`.
    pub fn operator(&self, char_id: &str) -> Option<(&str, String)> {
        let code = char_id.splitn(3, '_').nth(2)?;
        let code = match code {
            "purrche" => "purrchena",
            "endmin" => "endminm",
            c => c,
        };
        self.actors
            .get(code)
            .map(|d| (d.as_str(), code.to_string()))
    }

    /// Mesh folder for an enemy template `eny_NNNN_<code>`.
    pub fn enemy(&self, template_id: &str) -> Option<(&str, String)> {
        if let Some(d) = self.enemy_postmodels.get(template_id) {
            let code = template_id.splitn(3, '_').nth(2).unwrap_or(template_id);
            return Some((d.as_str(), code.to_string()));
        }
        let code = template_id.splitn(3, '_').nth(2)?;
        if let Some(d) = self.monsters.get(code) {
            return Some((d.as_str(), code.to_string()));
        }
        let trimmed = code.trim_end_matches(|c: char| c.is_ascii_digit());
        self.monsters
            .get(trimmed)
            .map(|d| (d.as_str(), trimmed.to_string()))
    }

    /// Mesh folder for a weapon id (falls back to the base id for `_suffix` variants).
    pub fn weapon(&self, id: &str) -> Option<(&str, String)> {
        let pick = |id: &str| {
            self.weapons
                .get(id)
                .and_then(|v| v.first())
                .map(|d| (d.as_str(), id.to_string()))
        };
        pick(id).or_else(|| {
            let mut parts: Vec<&str> = id.split('_').collect();
            (parts.len() > 3)
                .then(|| {
                    parts.truncate(3);
                    parts.join("_")
                })
                .and_then(|base| pick(&base).map(|(d, _)| (d, base)))
        })
    }

    /// Mesh folder for an NPC `dataKey` (`npc_spl_qinjc_01` → major `qinjc`;
    /// `npc_gentleman_riverbandits_a_02` → crowd body `npc_gentleman_body_riverbandits_a`).
    pub fn npc(&self, data_key: &str) -> Option<(&str, String)> {
        let parts: Vec<&str> = data_key.split('_').collect();
        if parts.len() >= 3 && parts[0] == "npc" {
            if let Some(d) = self.actors.get(parts[2]) {
                return Some((d.as_str(), parts[2].to_string()));
            }
            // Crowd: npc_<body>_<outfit…>_<NN>
            let outfit_end = if parts
                .last()
                .is_some_and(|p| p.chars().all(|c| c.is_ascii_digit()))
            {
                parts.len() - 1
            } else {
                parts.len()
            };
            let key = format!("npc_{}_body_{}", parts[1], parts[2..outfit_end].join("_"));
            if let Some(d) = self.crowd.get(&key) {
                return Some((d.as_str(), parts[2..outfit_end].join("_")));
            }
        }
        None
    }
}

/// One mesh part: the best (lowest) LOD present.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ModelPart {
    pub name: String,
    /// Path relative to the `meshes` root.
    pub path: String,
    pub lod: u32,
}

/// Split `S_actor_chen_body_01_lod0.glb` into (`S_actor_chen_body_01`, 0).
pub fn split_lod(file: &str) -> Option<(&str, u32)> {
    let stem = file.strip_suffix(".glb")?;
    match stem.rfind("_lod") {
        Some(i) => match stem[i + 4..].parse() {
            Ok(n) => Some((&stem[..i], n)),
            Err(_) => Some((stem, 0)),
        },
        None => Some((stem, 0)),
    }
}

/// Every part under `rel_dir` (recursive, `animations/` skipped) at its best LOD, with
/// display names stripped of the `S_` / kind / `code` prefix.
pub fn model_parts(meshes: &(impl Files + ?Sized), rel_dir: &str, code: &str) -> Vec<ModelPart> {
    fn walk<F: Files + ?Sized>(meshes: &F, rel: &str, depth: u32, out: &mut Vec<(String, String)>) {
        for (name, is_dir) in meshes.entries(rel) {
            let child = format!("{rel}/{name}");
            if is_dir {
                if depth < 3 && name != "animations" {
                    walk(meshes, &child, depth + 1, out);
                }
            } else if name.ends_with(".glb") {
                out.push((name, child));
            }
        }
    }
    let mut files = Vec::new();
    walk(meshes, rel_dir, 0, &mut files);
    let mut best: HashMap<String, (u32, String)> = HashMap::new();
    for (file, path) in files {
        let Some((part, lod)) = split_lod(&file) else {
            continue;
        };
        let slot = best
            .entry(part.to_string())
            .or_insert((u32::MAX, String::new()));
        if lod < slot.0 || (lod == slot.0 && path < slot.1) {
            *slot = (lod, path);
        }
    }
    let needle = format!("{code}_");
    let mut parts: Vec<ModelPart> = best
        .into_iter()
        .map(|(part, (lod, path))| {
            let bare = part.strip_prefix("S_").unwrap_or(&part);
            let name = if code.is_empty() {
                bare.to_string()
            } else {
                match bare.find(&needle) {
                    Some(i) if i + needle.len() < bare.len() => {
                        bare[i + needle.len()..].to_string()
                    }
                    _ => bare.to_string(),
                }
            };
            ModelPart { name, path, lod }
        })
        .collect();
    parts.sort_by(|a, b| a.name.cmp(&b.name).then_with(|| a.path.cmp(&b.path)));
    parts
}

/// Rigged GLBs listed by `<output>/models/index.json`: file stem → models-root path.
#[derive(Debug, Default)]
pub struct RigIndex {
    pub by_stem: HashMap<String, String>,
    pub stamp: Option<SystemTime>,
}

impl RigIndex {
    /// Parse the index loosely: any string value ending in `.glb` anywhere in the JSON is
    /// taken as a models-root-relative path (a leading `models/` or an absolute prefix up
    /// to `/models/` is stripped) and kept only if the file exists.
    pub fn load(output: &Path) -> Self {
        let root = Root::Models.dir(output);
        let Ok(meta) = std::fs::metadata(root.join("index.json")) else {
            return Self::default();
        };
        Self::load_from(&root, meta.modified().ok())
    }

    /// The same from the files of the models root (the bucket's, when it lives there),
    /// stamped with `stamp`. An index that cannot be read or parsed gives an empty one.
    pub fn load_from(models: &(impl Files + ?Sized), stamp: Option<SystemTime>) -> Self {
        Self::try_load_from(models, stamp).unwrap_or(Self {
            by_stem: HashMap::new(),
            stamp,
        })
    }

    /// The same, or `None` when `index.json` cannot be read or parsed, so that the caller
    /// can try again instead of keeping an empty index.
    pub fn try_load_from(
        models: &(impl Files + ?Sized),
        stamp: Option<SystemTime>,
    ) -> Option<Self> {
        let bytes = models.read_file("index.json")?;
        let value = serde_json::from_slice::<serde_json::Value>(&bytes).ok()?;
        let mut ix = Self {
            by_stem: HashMap::new(),
            stamp,
        };
        let mut strings = Vec::new();
        collect_glb_strings(&value, &mut strings);
        for s in strings {
            let s = s.replace('\\', "/");
            let rel = match s.rfind("/models/") {
                Some(i) if s.starts_with('/') || s.contains(":/") => s[i + 8..].to_string(),
                _ => s.strip_prefix("models/").unwrap_or(&s).to_string(),
            };
            if rel.split('/').any(|seg| seg == ".." || seg.is_empty()) {
                continue;
            }
            if !models.has_file(&rel) {
                continue;
            }
            if let Some(stem) = Path::new(&rel).file_stem().and_then(|s| s.to_str()) {
                ix.by_stem.entry(stem.to_string()).or_insert(rel);
            }
        }
        Some(ix)
    }

    /// Rigged GLB for `id` (models-root relative): from the index, else
    /// `<kind>/<id>.glb` when the index exists.
    pub fn find(&self, output: &(impl Roots + ?Sized), kind: &str, id: &str) -> Option<String> {
        self.stamp?;
        let dirs = kind_dirs(kind);
        let in_dirs = |path: &str| dirs.iter().any(|d| path.split('/').next() == Some(d));
        // The kind's own folder first: operators and their NPC variants share stems
        // (`operator/chr_0005_chen.glb`, `npc/chr_0005_chen.glb`).
        let root = output.root_files(Root::Models);
        dirs.iter()
            .map(|k| format!("{k}/{id}.glb"))
            .find(|rel| root.has_file(rel))
            .or_else(|| self.by_stem.get(id).filter(|p| in_dirs(p)).cloned())
            .or_else(|| self.find_variant(id, &in_dirs))
    }

    /// Rigged GLB for an NPC: by catalog id, else through its `dataKey`. `npc_chr_<chrId>`
    /// is an operator's NPC-mode prefab (`npc/<chrId>.glb`, else the operator's own GLB),
    /// `npc_eny_<enemyId>` an enemy, `npc_spl_<code>_NN` a named NPC (`npc/spl_<code>.glb`).
    pub fn find_npc(
        &self,
        output: &(impl Roots + ?Sized),
        id: &str,
        data_key: &str,
    ) -> Option<String> {
        self.find(output, "npcs", id).or_else(|| {
            if let Some(chr) = data_key.strip_prefix("npc_chr_") {
                let chr = format!("chr_{chr}");
                self.find(output, "npcs", &chr)
                    .or_else(|| self.find(output, "operators", &chr))
            } else if let Some(eny) = data_key.strip_prefix("npc_eny_") {
                self.find(output, "enemies", &format!("eny_{eny}"))
            } else if let Some(spl) = data_key.strip_prefix("npc_spl_") {
                // Named NPCs: `npc_spl_qinjc_01` -> the Avatar-built `npc/spl_qinjc.glb`.
                let code = spl.trim_end_matches(|c: char| c.is_ascii_digit());
                let code = code.strip_suffix('_').unwrap_or(code);
                self.find(output, "npcs", &format!("spl_{code}"))
            } else {
                None
            }
        })
    }

    /// A variant prefab of a catalog id: templates such as `eny_0054_hsmino` only have
    /// `eny_0054_hsmino1` / `eny_0054_hsmino_chrdg`, and some weapons only `<id>_refined`.
    /// Digit suffixes count only after a letter (so `wpn_x_0009` never matches `wpn_x_00091`);
    /// digit variants win over `_` variants, then the shortest, then the first by name.
    fn find_variant(&self, id: &str, in_dirs: &dyn Fn(&str) -> bool) -> Option<String> {
        let digit_ok = id.ends_with(|c: char| c.is_ascii_alphabetic());
        self.by_stem
            .iter()
            .filter(|(_, path)| in_dirs(path))
            .filter_map(|(stem, path)| {
                let rest = stem.strip_prefix(id)?;
                let rank = match rest.chars().next()? {
                    c if c.is_ascii_digit() && digit_ok => 0,
                    '_' => 1,
                    _ => return None,
                };
                Some(((rank, stem.len(), stem.clone()), path))
            })
            .min_by(|a, b| a.0.cmp(&b.0))
            .map(|(_, path)| path.clone())
    }
}

/// Folders of `models/` that hold a catalog kind's GLBs (the exporter writes singular kinds).
/// The plural kind itself is accepted too.
fn kind_dirs(kind: &str) -> Vec<String> {
    let singular = match kind {
        "operators" => "operator",
        "enemies" => "enemy",
        "weapons" => "weapon",
        "npcs" => "npc",
        other => other.trim_end_matches('s'),
    };
    let mut dirs = vec![singular.to_string(), kind.to_string()];
    if kind == "operators" {
        dirs.push("characters".into());
    }
    dirs
}

fn collect_glb_strings(v: &serde_json::Value, out: &mut Vec<String>) {
    match v {
        serde_json::Value::String(s) if s.to_ascii_lowercase().ends_with(".glb") => {
            out.push(s.clone())
        }
        serde_json::Value::Array(a) => a.iter().for_each(|x| collect_glb_strings(x, out)),
        serde_json::Value::Object(o) => o.values().for_each(|x| collect_glb_strings(x, out)),
        _ => {}
    }
}

/// Model block of a detail response.
#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ModelInfo {
    /// Mesh folder (meshes-root relative).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dir: Option<String>,
    pub parts: Vec<ModelPart>,
    /// Rigged GLB under the `models` root, when the exporter produced one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rigged: Option<String>,
    /// Clip index of the rigged GLB under the `models` root (`clips/<kind>/<id>/index.json`,
    /// written by `endfield-models clips`): one animation-only GLB per clip next to it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub clips: Option<String>,
    /// Menu lighting of the rigged GLB under the `models` root (`lights/<id>.json`, written by
    /// `endfield-models export` for operators): key-light volume and additional NPR lights.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lights: Option<String>,
    /// Character-screen state machine next to the clip index (`clips/<kind>/<id>/menu.json`,
    /// written by `endfield-models charinfo`): states with clip files, exits and moments.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub menu: Option<String>,
    /// Default weapon and how it mounts per clip (`charinfo/weapons/<id>.json`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub weapon: Option<String>,
    /// Deco props with per-clip visibility (`charinfo/deco/<id>/index.json`; prop GLBs beside
    /// it).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deco: Option<String>,
    /// Default character-screen framing from the lights file:
    /// `{position, target, fov, near, far, screenOffset?, referenceAspect?}`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub camera: Option<serde_json::Value>,
}

/// Clip index (models-root relative) of a rigged GLB, when `endfield-models clips` wrote a
/// non-empty one: `operator/chr_0005_chen.glb` → `clips/operator/chr_0005_chen/index.json`.
pub fn clips_index(output: &(impl Roots + ?Sized), rigged: Option<&str>) -> Option<String> {
    let stem = rigged?.strip_suffix(".glb")?;
    let rel = format!("clips/{stem}/index.json");
    let len = output.root_files(Root::Models).file_size(&rel)?;
    // An entity without clips gets `[]` (2 bytes).
    (len > 2).then_some(rel)
}

/// Light data (models-root relative) of a rigged GLB, when the exporter wrote one:
/// `operator/chr_0004_pelica.glb` → `lights/chr_0004_pelica.json`.
pub fn lights_file(output: &(impl Roots + ?Sized), rigged: Option<&str>) -> Option<String> {
    let stem = rigged?.strip_suffix(".glb")?;
    let id = stem.rsplit('/').next()?;
    models_file(output, format!("lights/{id}.json"))
}

/// Id of a rigged GLB path: `operator/chr_0004_pelica.glb` → `chr_0004_pelica`.
fn rigged_id(rigged: Option<&str>) -> Option<&str> {
    rigged?.strip_suffix(".glb")?.rsplit('/').next()
}

/// A models-root relative file, when it exists.
fn models_file(output: &(impl Roots + ?Sized), rel: String) -> Option<String> {
    output
        .root_files(Root::Models)
        .has_file(&rel)
        .then_some(rel)
}

/// Character-screen state machine of a rigged GLB, when `endfield-models charinfo` wrote one:
/// `operator/chr_0004_pelica.glb` → `clips/operator/chr_0004_pelica/menu.json`.
pub fn menu_file(output: &(impl Roots + ?Sized), rigged: Option<&str>) -> Option<String> {
    let stem = rigged?.strip_suffix(".glb")?;
    models_file(output, format!("clips/{stem}/menu.json"))
}

/// Default weapon mounting of a rigged GLB: `charinfo/weapons/<id>.json`.
pub fn weapon_file(output: &(impl Roots + ?Sized), rigged: Option<&str>) -> Option<String> {
    models_file(
        output,
        format!("charinfo/weapons/{}.json", rigged_id(rigged)?),
    )
}

/// Deco props of a rigged GLB: `charinfo/deco/<id>/index.json`.
pub fn deco_file(output: &(impl Roots + ?Sized), rigged: Option<&str>) -> Option<String> {
    models_file(
        output,
        format!("charinfo/deco/{}/index.json", rigged_id(rigged)?),
    )
}

/// The `camera` block of a lights file (models-root relative), when set.
pub fn camera_of(
    output: &(impl Roots + ?Sized),
    lights: Option<&str>,
) -> Option<serde_json::Value> {
    let bytes = output.root_files(Root::Models).read_file(lights?)?;
    let v: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    v.get("camera").filter(|c| c.is_object()).cloned()
}

pub fn model_info(
    output: &(impl Roots + ?Sized),
    rig: &RigIndex,
    kind: &str,
    id: &str,
    folder: Option<(&str, String)>,
) -> ModelInfo {
    let meshes = output.root_files(Root::Meshes);
    let (dir, parts) = match folder {
        Some((dir, code)) => (Some(dir.to_string()), model_parts(&meshes, dir, &code)),
        None => (None, Vec::new()),
    };
    let rigged = rig.find(output, kind, id);
    let r = rigged.as_deref();
    let lights = lights_file(output, r);
    ModelInfo {
        dir,
        parts,
        clips: clips_index(output, r),
        camera: camera_of(output, lights.as_deref()),
        lights,
        menu: menu_file(output, r),
        weapon: weapon_file(output, r),
        deco: deco_file(output, r),
        rigged,
    }
}
