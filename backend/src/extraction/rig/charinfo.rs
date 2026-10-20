//! Character-screen data of operators: the UI model's Animator state machine (`menu.json`),
//! default weapon mounting and deco props. The same data as warfarin's `clips/<id>/menu.json`,
//! `charinfo/weapons.json` and `deco/<id>/index.json`.
//!
//! Sources:
//! - `prefabs/uimodels/<id>_controller.controller`: the UI model's AnimatorController (class
//!   91, runtime form). States, transitions, AnyState transitions and the
//!   StateMachineBehaviours (UI VFX per state) are flattened by [`flatten_controller`].
//! - `gamedata/.../gameplayconfig/CharacterDisplayConfig.json`: per character the weapon
//!   mounts of the character screen (`weaponConfig.weaponData[].nodeUIIdle`), the weapon
//!   appear/disappear effects and the deco props (`decoItemConfig.decoItemData`: prefab and
//!   mount point).
//! - Clips of the UI controller animate the Animator parameter `WeaponHide` (weapon hidden
//!   while ≥ 0.5). Deco props run their own controller (`<deco>_controller.controller`) with
//!   the same state names, whose clips toggle `m_IsActive` of the prop's nodes.
//! - Tables: `GachaPoolCharPresetTable.perfectWeaponId` (the signature weapon, rule
//!   `signature`), else `CharacterTable.defaultWeaponId` (rule `basic`);
//!   `WeaponBasicTable` for type and rarity.
use super::anim::crc32;
use super::export::{ExportOptions, Exporter, ObjRef};
use super::store::{CabRecord, CabSource};
use serde_json::{Map, Value, json};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};

/// Unity's `AnimatorConditionMode`.
const COND_IF: i64 = 1;
const COND_IF_NOT: i64 = 2;
const COND_GREATER: i64 = 3;
const COND_LESS: i64 = 4;
const COND_EQUALS: i64 = 6;
const COND_NOT_EQUAL: i64 = 7;

/// Destination indices at or above this name a selector (sub-state machine entry/exit).
const SELECTOR_OFFSET: i64 = 30000;
/// `m_ClipID` of a blend tree node without a clip.
const NO_CLIP: u64 = u32::MAX as u64;

/// Unity's `typeID` of GameObject bindings and the `m_IsActive` attribute hash.
const TYPE_GAME_OBJECT: i64 = 1;
const TYPE_ANIMATOR: i64 = 95;

/// Animator parameter that hides the character-screen weapon while ≥ 0.5.
pub const WEAPON_HIDE: &str = "WeaponHide";

fn round4(x: f64) -> f64 {
    let r = (x * 1e4).round() / 1e4;
    if r == 0.0 { 0.0 } else { r }
}

/// An `OffsetPtr` wrapper (`{"data": …}`) or the value itself.
fn data(v: &Value) -> &Value {
    v.get("data").unwrap_or(v)
}

fn arr<'a>(v: &'a Value, key: &str) -> &'a [Value] {
    v.get(key)
        .and_then(Value::as_array)
        .map_or(&[], Vec::as_slice)
}

fn num(v: &Value, key: &str) -> Option<f64> {
    v.get(key).and_then(Value::as_f64)
}

fn int(v: &Value, key: &str) -> Option<i64> {
    v.get(key).and_then(Value::as_i64)
}

fn flag(v: &Value, key: &str) -> bool {
    v.get(key)
        .is_some_and(|b| b.as_bool().unwrap_or_else(|| b.as_i64().unwrap_or(0) != 0))
}

/// One entry of a clip index (`clips/<kind>/<id>/index.json`).
#[derive(Debug, Clone, PartialEq)]
pub struct ClipInfo {
    pub file: String,
    pub duration: f64,
    pub looping: bool,
}

/// Clip index entries by AnimationClip name (lowercase).
#[derive(Debug, Clone, Default)]
pub struct ClipIndex {
    by_name: HashMap<String, ClipInfo>,
}

impl ClipIndex {
    /// From the entries of a clip `index.json`. When two entries share a clip name the first
    /// one wins.
    #[must_use]
    pub fn from_entries(entries: &[Value]) -> Self {
        let mut by_name = HashMap::new();
        for e in entries {
            let (Some(clip), Some(file)) = (
                e.get("clip").and_then(Value::as_str),
                e.get("file").and_then(Value::as_str),
            ) else {
                continue;
            };
            by_name
                .entry(clip.to_ascii_lowercase())
                .or_insert_with(|| ClipInfo {
                    file: file.to_string(),
                    duration: num(e, "duration").unwrap_or(0.0),
                    looping: flag(e, "loop"),
                });
        }
        Self { by_name }
    }

    #[must_use]
    pub fn get(&self, clip_name: &str) -> Option<&ClipInfo> {
        self.by_name.get(&clip_name.to_ascii_lowercase())
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.by_name.is_empty()
    }

    /// Every clip file, sorted.
    pub fn files(&self) -> impl Iterator<Item = &str> {
        let mut files: Vec<&str> = self.by_name.values().map(|c| c.file.as_str()).collect();
        files.sort_unstable();
        files.dedup();
        files.into_iter()
    }
}

/// The controller's string table: `m_TOS` (`{hash: name}`) or, in this Unity build,
/// `m_TOSData` (names only, hashed with CRC32 like `Animator.StringToHash`).
#[must_use]
pub fn tos_map(controller: &Value) -> HashMap<u32, String> {
    let mut out = HashMap::new();
    if let Some(m) = controller.get("m_TOS").and_then(Value::as_object) {
        for (k, v) in m {
            if let (Ok(h), Some(s)) = (k.parse::<u32>(), v.as_str()) {
                out.insert(h, s.to_string());
            }
        }
    }
    for s in arr(controller, "m_TOSData")
        .iter()
        .filter_map(Value::as_str)
    {
        out.insert(crc32(s.as_bytes()), s.to_string());
    }
    out
}

/// A flattened state, before it is serialized.
struct State {
    name: String,
    full: String,
    path_id: u32,
    clip: Option<usize>,
    looping: bool,
    transitions: Vec<Value>,
}

/// Inputs of [`flatten_controller`] that live outside the controller object.
pub struct ControllerRefs<'a> {
    /// Name of each `m_AnimationClips` entry (`None` when unresolved).
    pub clip_names: &'a [Option<String>],
    /// Length in seconds of each `m_AnimationClips` entry (`m_StopTime − m_StartTime`), when
    /// read; preferred over the clip index duration (which counts sample intervals).
    pub clip_lengths: &'a [Option<f64>],
    /// Each `m_StateMachineBehaviours` entry, deserialized (`None` when unresolved).
    pub behaviours: &'a [Option<Value>],
    pub clips: &'a ClipIndex,
}

/// Flatten a runtime AnimatorController (layer 0) into warfarin's `menu.json` shape:
///
/// ```json
/// {"format": 1, "default": "Overview.OverviewIdle",
///  "states": {"Weapon.FromEquip": {"clip": "a_actor_x_ui_overview_to_weapon.glb",
///     "length": 1.45, "loop": false,
///     "exit": {"to": "Weapon.WeaponIdle", "at": 0.87, "fade": 0.5787, "offset": 0.0},
///     "on": [{"when": {"EnableSwitch": true, "ToIndex": 20}, "to", "at" (null: no exit time),
///             "fade", "offset"}],
///     "effects": [["P_fxui_…", "Funnel_Point_001"]]}},
///  "any": [{"when": {…}, "to", "fade" | "fadeOfSource", "offset"}],
///  "params": {"ToIndex": "int", "EnableSwitch": "trigger", …}}
/// ```
///
/// State names drop the layer prefix (`Base Layer.`). `clip` is the clip index `file` of the
/// state's (first) motion, `null` when the clip was not exported; `length`/`loop` then come
/// from the state. `exit` is the first unconditional exit-time transition: `at` in seconds of
/// the source clip (`atNormalized` when the clip length is unknown), `fade` in seconds
/// (normalized durations are scaled by the source length).
/// AnyState transitions have no source length, so a normalized duration is kept as
/// `fadeOfSource`. `offset` is the start time in seconds of the destination clip
/// (`offsetNormalized` when its length is unknown). A state without a motion has length 0.
/// Condition values: `true`/`false` for `If`/`IfNot`, numbers for `Equals`,
/// `{"gt"|"lt"|"ne": x}` otherwise.
#[must_use]
pub fn flatten_controller(controller: &Value, refs: &ControllerRefs<'_>) -> Option<Value> {
    let tos = tos_map(controller);
    let ctrl = data(controller.get("m_Controller")?);
    let layer = data(arr(ctrl, "m_LayerArray").first()?);
    let sm_index = usize::try_from(int(layer, "m_StateMachineIndex").unwrap_or(0)).ok()?;
    let sm = data(arr(ctrl, "m_StateMachineArray").get(sm_index)?);
    let name_of = |h: u64| -> String {
        tos.get(&(h as u32))
            .cloned()
            .unwrap_or_else(|| format!("#{h}"))
    };
    let strip_layer = |full: &str| -> String {
        full.split_once('.')
            .map_or(full.to_string(), |(_, rest)| rest.to_string())
    };
    // Parameter names and types.
    let mut params: BTreeMap<String, &'static str> = BTreeMap::new();
    let mut param_types: HashMap<u64, i64> = HashMap::new();
    if let Some(values) = ctrl.get("m_Values").map(data) {
        for v in arr(values, "m_ValueArray") {
            let (Some(id), Some(ty)) = (v.get("m_ID").and_then(Value::as_u64), int(v, "m_Type"))
            else {
                continue;
            };
            param_types.insert(id, ty);
            let kind = match ty {
                1 => "float",
                3 => "int",
                4 => "bool",
                9 => "trigger",
                _ => "unknown",
            };
            params.insert(name_of(id), kind);
        }
    }
    let states: Vec<State> = arr(sm, "m_StateConstantArray")
        .iter()
        .map(|s| {
            let s = data(s);
            let full_id = s.get("m_FullPathID").and_then(Value::as_u64).unwrap_or(0);
            let full = name_of(full_id);
            let clip = arr(s, "m_BlendTreeConstantArray")
                .iter()
                .flat_map(|bt| arr(data(bt), "m_NodeArray"))
                .filter_map(|n| data(n).get("m_ClipID").and_then(Value::as_u64))
                .find(|&id| id != NO_CLIP)
                .and_then(|id| usize::try_from(id).ok());
            State {
                name: strip_layer(&full),
                full,
                path_id: full_id as u32,
                clip,
                looping: flag(s, "m_Loop"),
                transitions: arr(s, "m_TransitionConstantArray")
                    .iter()
                    .map(|t| data(t).clone())
                    .collect(),
            }
        })
        .collect();
    let selectors: Vec<&Value> = arr(sm, "m_SelectorStateConstantArray")
        .iter()
        .map(data)
        .collect();
    // Destination state index of a transition, following selectors.
    let resolve = |mut dest: i64| -> Option<usize> {
        for _ in 0..16 {
            if dest < SELECTOR_OFFSET {
                return usize::try_from(dest).ok().filter(|&i| i < states.len());
            }
            let sel = selectors.get(usize::try_from(dest - SELECTOR_OFFSET).ok()?)?;
            let trs: Vec<&Value> = arr(sel, "m_TransitionConstantArray")
                .iter()
                .map(data)
                .collect();
            let pick = trs
                .iter()
                .find(|t| arr(t, "m_ConditionConstantArray").is_empty())
                .or_else(|| trs.first())?;
            dest = int(pick, "m_Destination")?;
        }
        None
    };
    let when = |t: &Value| -> Value {
        let mut m = Map::new();
        for c in arr(t, "m_ConditionConstantArray") {
            let c = data(c);
            let Some(id) = c.get("m_EventID").and_then(Value::as_u64) else {
                continue;
            };
            let thr = num(c, "m_EventThreshold").unwrap_or(0.0);
            let is_int = param_types.get(&id) == Some(&3);
            let value = |x: f64| {
                if is_int || x.fract() == 0.0 {
                    json!(x as i64)
                } else {
                    json!(round4(x))
                }
            };
            let v = match int(c, "m_ConditionMode").unwrap_or(0) {
                COND_IF => json!(true),
                COND_IF_NOT => json!(false),
                COND_EQUALS => value(thr),
                COND_GREATER => json!({"gt": value(thr)}),
                COND_LESS => json!({"lt": value(thr)}),
                COND_NOT_EQUAL => json!({"ne": value(thr)}),
                other => json!({"mode": other, "value": value(thr)}),
            };
            m.insert(name_of(id), v);
        }
        Value::Object(m)
    };
    let clip_info = |s: &State| -> Option<&ClipInfo> {
        let name = refs.clip_names.get(s.clip?)?.as_deref()?;
        refs.clips.get(name)
    };
    // A state without a motion lasts 0 s.
    let length_of = |s: &State| match s.clip {
        None => Some(0.0),
        Some(i) => refs
            .clip_lengths
            .get(i)
            .copied()
            .flatten()
            .or_else(|| clip_info(s).map(|c| c.duration)),
    };
    // Transition offsets are normalized to the destination clip: seconds when its length is
    // known.
    let offset = |m: &mut Map<String, Value>, t: &Value, to: &State| {
        let raw = num(t, "m_TransitionOffset").unwrap_or(0.0);
        match length_of(to) {
            Some(l) => m.insert("offset".into(), json!(round4(raw * l))),
            None => m.insert("offsetNormalized".into(), json!(round4(raw))),
        };
    };
    // StateMachineBehaviours (UI effects) by state full path id.
    let mut effects: HashMap<u32, Vec<Value>> = HashMap::new();
    if let Some(desc) = controller.get("m_StateMachineBehaviourVectorDescription") {
        let indices = arr(desc, "m_StateMachineBehaviourIndices");
        if let Some(ranges) = desc
            .get("m_StateMachineBehaviourRanges")
            .and_then(Value::as_object)
        {
            for (key, range) in ranges {
                let Some(state_id) = serde_json::from_str::<Value>(key)
                    .ok()
                    .and_then(|k| k.get("m_StateID").and_then(Value::as_u64))
                else {
                    continue;
                };
                let start = int(range, "m_StartIndex").unwrap_or(0);
                let count = int(range, "m_Count").unwrap_or(0);
                for i in start..start + count {
                    let Some(b) = indices
                        .get(usize::try_from(i).unwrap_or(usize::MAX))
                        .and_then(Value::as_u64)
                        .and_then(|b| refs.behaviours.get(usize::try_from(b).ok()?))
                        .and_then(Option::as_ref)
                    else {
                        continue;
                    };
                    for e in arr(b, "_effects") {
                        let name = e.get("effectName").and_then(Value::as_str).unwrap_or("");
                        let mount = e.get("mountPoint").and_then(Value::as_str).unwrap_or("");
                        effects
                            .entry(state_id as u32)
                            .or_default()
                            .push(json!([name, mount]));
                    }
                }
            }
        }
    }
    let fade = |t: &Value, source_len: Option<f64>| -> (&'static str, f64) {
        let d = num(t, "m_TransitionDuration").unwrap_or(0.0);
        if flag(t, "m_HasFixedDuration") {
            ("fade", round4(d))
        } else if let Some(len) = source_len {
            ("fade", round4(d * len))
        } else {
            ("fadeOfSource", round4(d))
        }
    };
    let mut out_states = Map::new();
    for s in &states {
        let info = clip_info(s);
        let len = length_of(s);
        let mut exit = Value::Null;
        let mut on = Vec::new();
        for t in &s.transitions {
            let Some(to) = int(t, "m_DestinationState").and_then(resolve) else {
                continue;
            };
            let has_exit = flag(t, "m_HasExitTime");
            let exit_time = num(t, "m_ExitTime").unwrap_or(0.0);
            let (fade_key, fade_value) = fade(t, len);
            let conditions = when(t);
            let unconditional = conditions.as_object().is_none_or(Map::is_empty);
            let mut m = Map::new();
            if !unconditional {
                m.insert("when".into(), conditions);
            }
            m.insert("to".into(), json!(states[to].name));
            if has_exit {
                // Seconds into the source clip; normalized when its length is unknown.
                match len {
                    Some(l) => m.insert("at".into(), json!(round4(exit_time * l))),
                    None => m.insert("atNormalized".into(), json!(round4(exit_time))),
                };
            } else {
                // Taken as soon as the conditions hold.
                m.insert("at".into(), Value::Null);
            }
            m.insert(fade_key.into(), json!(fade_value));
            offset(&mut m, t, &states[to]);
            if unconditional && has_exit && exit.is_null() {
                exit = Value::Object(m);
            } else {
                on.push(Value::Object(m));
            }
        }
        let mut st = Map::new();
        st.insert("clip".into(), json!(info.map(|c| c.file.clone())));
        st.insert("length".into(), json!(len.map(round4)));
        st.insert(
            "loop".into(),
            json!(info.map_or(s.looping && s.clip.is_some(), |c| c.looping)),
        );
        st.insert("exit".into(), exit);
        if info.is_none()
            && let Some(name) = s
                .clip
                .and_then(|i| refs.clip_names.get(i))
                .and_then(Option::as_deref)
        {
            st.insert("missingClip".into(), json!(name));
        }
        if !on.is_empty() {
            st.insert("on".into(), json!(on));
        }
        if let Some(fx) = effects.get(&s.path_id) {
            st.insert("effects".into(), json!(fx));
        }
        out_states.insert(s.name.clone(), Value::Object(st));
    }
    let any: Vec<Value> = arr(sm, "m_AnyStateTransitionConstantArray")
        .iter()
        .filter_map(|t| {
            let t = data(t);
            let to = int(t, "m_DestinationState").and_then(resolve)?;
            let (fade_key, fade_value) = fade(t, None);
            let mut m = Map::new();
            m.insert("when".into(), when(t));
            m.insert("to".into(), json!(states[to].name));
            m.insert(fade_key.into(), json!(fade_value));
            offset(&mut m, t, &states[to]);
            Some(Value::Object(m))
        })
        .collect();
    let default = int(sm, "m_DefaultState")
        .and_then(resolve)
        .map(|i| states[i].name.clone());
    let layer_name = states
        .first()
        .and_then(|s| s.full.split_once('.'))
        .map(|(l, _)| l.to_string());
    let mut out = Map::new();
    out.insert("format".into(), json!(1));
    out.insert("default".into(), json!(default));
    out.insert("states".into(), Value::Object(out_states));
    out.insert("any".into(), json!(any));
    out.insert("params".into(), json!(params));
    if let Some(l) = layer_name {
        out.insert("layer".into(), json!(l));
    }
    let moments = moments(&Value::Object(out.clone()));
    out.insert("moments".into(), moments);
    Some(Value::Object(out))
}

/// Display label of a character-screen state group or relax state (the in-game page names).
fn group_label(group: &str) -> String {
    match group {
        "Equip" => "Gear".into(),
        "Skill" => "Ability Matrix".into(),
        "Document" => "Profile".into(),
        "Relax" => "Preview".into(),
        "SP1" => "Special 1".into(),
        "SP2" => "Special 2".into(),
        "React" => "Reaction".into(),
        other => other.to_string(),
    }
}

/// The idle a state settles into: the `exit.to` chain's end (at most 8 hops).
fn settle<'a>(states: &'a Map<String, Value>, mut name: &'a str) -> &'a str {
    for _ in 0..8 {
        match states
            .get(name)
            .and_then(|s| s.pointer("/exit/to"))
            .and_then(Value::as_str)
        {
            Some(next) if next != name => name = next,
            _ => break,
        }
    }
    name
}

/// The character-screen "moments" a viewer offers (warfarin's Featured → MENU group), derived
/// from the flattened controller:
///
/// - pages: AnyState transitions `{EnableSwitch, FromIndex: 0, ToIndex: k}` (and the
///   overview's own `0 → 0` entry); labelled by the destination's state group (`Weapon`,
///   `Equip` → "Gear", `Skill` → "Ability Matrix", `Document` → "Profile", `Upgrade`,
///   `Relax` → "Preview");
/// - relax specials: the relax idle's conditional transitions (`SP1` → "Special 1", `SP2` →
///   "Special 2", `React` → "Reaction");
/// - team poses: AnyState transitions on `formation` (`formation: n` → "Team Pose n").
///
/// ```json
/// [{"id": "weapon", "label": "Weapon", "send": {"kind": "page", "index": 1},
///   "state": "Weapon.FromOverview", "idle": "Weapon.WeaponIdle"}]
/// ```
#[must_use]
pub fn moments(menu: &Value) -> Value {
    let empty = Map::new();
    let states = menu
        .get("states")
        .and_then(Value::as_object)
        .unwrap_or(&empty);
    let any = arr(menu, "any");
    let mut out: Vec<Value> = Vec::new();
    let mut seen_pages = std::collections::BTreeSet::new();
    let mut pages: Vec<(i64, &str)> = any
        .iter()
        .filter_map(|t| {
            let w = t.get("when")?;
            let to_index = w.get("ToIndex")?.as_i64()?;
            let from = w.get("FromIndex").and_then(Value::as_i64);
            let skip = w.get("SkipIn").and_then(Value::as_bool).unwrap_or(false);
            (from == Some(0) && !skip).then_some((to_index, t.get("to")?.as_str()?))
        })
        .collect();
    pages.sort_by_key(|(i, _)| *i);
    for (index, to) in pages {
        if !seen_pages.insert(index) {
            continue;
        }
        let group = to.split('.').next().unwrap_or(to);
        let idle = settle(states, to);
        let id = if index == 0 {
            "overview".to_string()
        } else {
            group.to_ascii_lowercase()
        };
        out.push(json!({
            "id": id,
            "label": group_label(group),
            "send": {"kind": "page", "index": index},
            "state": to,
            "idle": idle,
        }));
    }
    // Relax specials hang off the relax idle.
    let mut relax: Vec<(i64, &str, &str)> = Vec::new();
    for (name, s) in states {
        for t in arr(s, "on") {
            let Some(w) = t.get("when") else { continue };
            let (Some(from), Some(to_index), Some(to)) = (
                w.get("FromIndex").and_then(Value::as_i64),
                w.get("ToIndex").and_then(Value::as_i64),
                t.get("to").and_then(Value::as_str),
            ) else {
                continue;
            };
            if name.starts_with("Relax.") && from >= 0 {
                relax.push((to_index, to, name.as_str()));
            }
        }
    }
    relax.sort_by_key(|(i, _, _)| *i);
    relax.dedup_by_key(|(i, _, _)| *i);
    for (index, to, from) in relax {
        let leaf = to.rsplit('.').next().unwrap_or(to);
        let label = group_label(leaf);
        let id = label.to_ascii_lowercase().replace(' ', "");
        out.push(json!({
            "id": id,
            "label": label,
            "send": {"kind": "relax", "index": index},
            "from": from,
            "state": to,
            "idle": settle(states, to),
        }));
    }
    let mut team: Vec<(i64, &str)> = any
        .iter()
        .filter_map(|t| {
            // Slot 0 is the overview itself.
            let slot = t
                .get("when")?
                .get("formation")?
                .as_i64()
                .filter(|&s| s > 0)?;
            Some((slot, t.get("to")?.as_str()?))
        })
        .collect();
    team.sort_by_key(|(s, _)| *s);
    team.dedup_by_key(|(s, _)| *s);
    for (slot, to) in team {
        out.push(json!({
            "id": format!("team{slot}"),
            "label": format!("Team Pose {slot}"),
            "send": {"kind": "team", "slot": slot},
            "state": to,
            "idle": settle(states, to),
        }));
    }
    json!(out)
}

/// A scalar curve: `(time, value)` keys and whether values between keys interpolate linearly
/// (ACL samples) or hold (streamed keys of an editor-stepped curve).
#[derive(Debug, Clone, PartialEq)]
pub struct Curve {
    pub keys: Vec<(f32, f32)>,
    pub linear: bool,
}

/// Non-Transform bindings of a clip with their curves: ACL `FloatBufferData` (one track per
/// non-Transform binding, in binding order) or, for small plain Mecanim clips, the streamed
/// clip (curve `i` is binding `i`).
pub fn scalar_curves(clip: &Value) -> Result<Vec<(Value, Curve)>, String> {
    let bindings = clip
        .pointer("/m_ClipBindingConstant/genericBindings")
        .and_then(Value::as_array)
        .ok_or("clip has no bindings")?;
    let buffer = clip
        .pointer("/m_AclCompressedBuffer/FloatBufferData")
        .and_then(crate::extraction::unity::value_bytes)
        .filter(|b| !b.is_empty());
    if let Some(buffer) = buffer {
        let scalars: Vec<&Value> = bindings
            .iter()
            .filter(|b| int(b, "typeID") != Some(4))
            .collect();
        let (info, values) = super::acl::floats(&buffer)?;
        let tracks = info.num_tracks as usize;
        if tracks != scalars.len() {
            return Err(format!(
                "float buffer has {tracks} tracks, bindings name {}",
                scalars.len()
            ));
        }
        let samples = info.num_samples as usize;
        let rate = if info.sample_rate > 0.0 {
            info.sample_rate
        } else {
            30.0
        };
        return Ok(scalars
            .into_iter()
            .enumerate()
            .map(|(t, b)| {
                let keys = (0..samples)
                    .map(|s| (s as f32 / rate, values[s * tracks + t]))
                    .collect();
                (b.clone(), Curve { keys, linear: true })
            })
            .collect());
    }
    let words: Vec<u32> = clip
        .pointer("/m_MuscleClip/m_Clip/data/m_StreamedClip/data")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|w| w.as_u64().map(|w| w as u32))
                .collect()
        })
        .unwrap_or_default();
    let curves = streamed_keys(&words);
    Ok(bindings
        .iter()
        .enumerate()
        .filter_map(|(i, b)| {
            let keys = curves.get(&i)?.clone();
            Some((
                b.clone(),
                Curve {
                    keys,
                    linear: false,
                },
            ))
        })
        .collect())
}

/// Keys of a Mecanim streamed clip: frames of `time, keyCount, keyCount × (curveIndex,
/// 4 cubic coefficients)`; the value at a key is its last coefficient. Frames at
/// non-finite times (the `-FLT_MAX`/`+inf` sentinels) are skipped and negative times clamp
/// to 0.
#[must_use]
pub fn streamed_keys(words: &[u32]) -> HashMap<usize, Vec<(f32, f32)>> {
    let mut out: HashMap<usize, Vec<(f32, f32)>> = HashMap::new();
    let mut i = 0;
    while i + 2 <= words.len() {
        let time = f32::from_bits(words[i]);
        let count = words[i + 1] as usize;
        i += 2;
        for _ in 0..count {
            if i + 5 > words.len() {
                return out;
            }
            let index = words[i] as usize;
            let value = f32::from_bits(words[i + 4]);
            i += 5;
            if time.is_finite() && time > -1e30 {
                let keys = out.entry(index).or_default();
                let t = time.max(0.0);
                match keys.last_mut() {
                    Some(last) if last.0 == t => last.1 = value,
                    _ => keys.push((t, value)),
                }
            }
        }
    }
    out
}

/// Visibility steps `[[t, 0|1], …]` of a curve where `visible(value)` decides each key.
/// Linear curves place a change where the value crosses the threshold between two samples.
#[must_use]
pub fn steps(curve: &Curve, threshold: f32, visible_below: bool) -> Vec<[f64; 2]> {
    let vis = |v: f32| (v < threshold) == visible_below;
    let mut out: Vec<[f64; 2]> = Vec::new();
    let Some(&(_, first)) = curve.keys.first() else {
        return out;
    };
    out.push([0.0, f64::from(u8::from(vis(first)))]);
    for w in curve.keys.windows(2) {
        let ((t0, v0), (t1, v1)) = (w[0], w[1]);
        if vis(v0) == vis(v1) {
            continue;
        }
        let t = if curve.linear && (v1 - v0).abs() > f32::EPSILON {
            t0 + (threshold - v0) / (v1 - v0) * (t1 - t0)
        } else {
            t1
        };
        out.push([round4(f64::from(t)), f64::from(u8::from(vis(v1)))]);
    }
    out
}

/// The curve of Animator parameter `name` (bindings `typeID` 95, `customType` 0, attribute =
/// CRC32 of the name).
#[must_use]
pub fn animator_param<'a>(curves: &'a [(Value, Curve)], name: &str) -> Option<&'a Curve> {
    let hash = u64::from(crc32(name.as_bytes()));
    curves.iter().find_map(|(b, c)| {
        (int(b, "typeID") == Some(TYPE_ANIMATOR)
            && int(b, "customType") == Some(0)
            && b.get("attribute").and_then(Value::as_u64) == Some(hash))
        .then_some(c)
    })
}

/// `m_IsActive` curves by binding path hash.
#[must_use]
pub fn active_curves(curves: &[(Value, Curve)]) -> Vec<(u32, &Curve)> {
    let attr = u64::from(crc32(b"m_IsActive"));
    curves
        .iter()
        .filter(|(b, _)| {
            int(b, "typeID") == Some(TYPE_GAME_OBJECT)
                && b.get("attribute").and_then(Value::as_u64) == Some(attr)
        })
        .filter_map(|(b, c)| Some((b.get("path")?.as_u64()? as u32, c)))
        .collect()
}

/// A character's entry of `CharacterDisplayConfig` (`references.RefIds[].data` with this
/// `charId`).
#[must_use]
pub fn display_config<'a>(config: &'a Value, id: &str) -> Option<&'a Value> {
    arr(config.get("references")?, "RefIds")
        .iter()
        .filter_map(|r| r.get("data"))
        .find(|d| d.get("charId").and_then(Value::as_str) == Some(id))
}

/// The weapon a character holds by default: the signature weapon
/// (`GachaPoolCharPresetTable.perfectWeaponId`) or, for characters outside the gacha, the
/// starting one (`CharacterTable.defaultWeaponId`). `(weapon id, rule)`.
#[must_use]
pub fn default_weapon(
    id: &str,
    gacha_presets: &Value,
    characters: &Value,
) -> Option<(String, &'static str)> {
    if let Some(w) = gacha_presets
        .get(id)
        .and_then(|p| p.get("perfectWeaponId"))
        .and_then(Value::as_str)
        .filter(|w| !w.is_empty())
    {
        return Some((w.to_string(), "signature"));
    }
    characters
        .get(id)
        .and_then(|c| c.get("defaultWeaponId"))
        .and_then(Value::as_str)
        .filter(|w| !w.is_empty())
        .map(|w| (w.to_string(), "basic"))
}

/// Character-screen weapon mounts: `weaponConfig.weaponData[]` → `{node, scale}` (one copy per
/// entry; pistols and funnels carry several).
#[must_use]
pub fn weapon_mounts(display: &Value) -> Vec<Value> {
    display
        .pointer("/weaponConfig/weaponData")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|w| {
            let node = w.get("nodeUIIdle").and_then(Value::as_str)?;
            (!node.is_empty()).then(|| {
                json!({
                    "node": node,
                    "scale": round4(num(w, "weaponScale").unwrap_or(1.0)),
                })
            })
        })
        .collect()
}

/// Appear/disappear effect of the character-screen weapon: `{"effects": [names], "duration"}`.
#[must_use]
pub fn weapon_effect(display: &Value, kind: &str) -> Value {
    let cfg = display.get("weaponConfig");
    let names = cfg
        .and_then(|c| c.get(format!("weapon{kind}EffectName")))
        .cloned()
        .unwrap_or_else(|| json!([]));
    let duration = cfg
        .and_then(|c| num(c, &format!("weapon{kind}EffectDuration")))
        .map_or(0.0, round4);
    json!({"effects": names, "duration": duration})
}

/// `WeaponHide` below which the character-screen weapon counts as shown: at most 0.01 (plus
/// float slack; some clips rest at exactly 0.01). The parameter ramps between 0 and 1; the
/// weapon appears once the ramp has (almost) reached 0 and starts to dissolve as soon as it
/// leaves 0.
pub const WEAPON_SHOWN_BELOW: f32 = 0.0101;

/// Weapon visibility `[[t, 0|1]]` from `WeaponHide` changes (see [`steps`]): it shows at an
/// appear change and stays until its disappear dissolve (`disappear` seconds) has finished.
#[must_use]
pub fn weapon_track(changes: &[[f64; 2]], disappear: f64) -> Vec<[f64; 2]> {
    changes
        .iter()
        .enumerate()
        .map(|(i, &[t, v])| {
            if i > 0 && v < 0.5 {
                [round4(t + disappear), 0.0]
            } else {
                [t, v]
            }
        })
        .collect()
}

/// A photo-mode pose clip (`…_interact_camera_<nn>`), played with the weapon hidden.
#[must_use]
pub fn is_pose_clip(file: &str) -> bool {
    file.trim_end_matches(".glb")
        .rsplit_once("_interact_camera_")
        .is_some_and(|(_, n)| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()))
}

/// Container prefix of the character-screen UI models, their controllers and deco props.
pub const UIMODELS_DIR: &str = super::store::UIMODELS_PREFIX;
const DECO_DIR: &str = "assets/beyond/dynamicassets/gameplay/prefabs/uimodels/decoitems/";

const CLASS_GAME_OBJECT: i32 = 1;
const CLASS_TRANSFORM: i32 = 4;
const CLASS_ANIMATOR: i32 = 95;
const CLASS_CONTROLLER: i32 = 91;

/// Game tables and configs the character-screen data needs (`Value::Null` when missing).
#[derive(Debug, Default)]
pub struct Tables {
    pub gacha_presets: Value,
    pub characters: Value,
    pub weapons: Value,
    pub display: Value,
}

impl Tables {
    /// From an extraction root holding `decrypted/Table/TableCfg/*.json` and
    /// `gamedata/beyond/dynamicassets/gamedata/gameplayconfig/CharacterDisplayConfig.json`.
    #[must_use]
    pub fn load(data_root: &Path) -> Self {
        let read = |p: PathBuf| {
            std::fs::read(&p)
                .ok()
                .and_then(|b| serde_json::from_slice(&b).ok())
                .unwrap_or(Value::Null)
        };
        let tables = data_root.join("decrypted/Table/TableCfg");
        Self {
            gacha_presets: read(tables.join("GachaPoolCharPresetTable.json")),
            characters: read(tables.join("CharacterTable.json")),
            weapons: read(tables.join("WeaponBasicTable.json")),
            display: read(data_root.join(
                "gamedata/beyond/dynamicassets/gamedata/gameplayconfig/CharacterDisplayConfig.json",
            )),
        }
    }
}

/// Stored UI-model containers (lowercase path → `(cab, path_id)`).
#[must_use]
pub fn uimodel_containers(
    records: &[CabRecord],
    stored: &dyn Fn(&str) -> bool,
) -> HashMap<String, (String, i64)> {
    let mut out = HashMap::new();
    for rec in records {
        for (path, file_id, path_id) in &rec.containers {
            let lower = path.to_ascii_lowercase();
            if *file_id == 0 && lower.starts_with(UIMODELS_DIR) && stored(&rec.cab) {
                out.entry(lower)
                    .or_insert_with(|| (rec.cab.clone(), *path_id));
            }
        }
    }
    out
}

/// The UI-model id of an exported operator: its own id, else the longest id it starts with
/// (`chr_0003_endminfunmasked` → `chr_0003_endminf`).
#[must_use]
pub fn uimodel_id(containers: &HashMap<String, (String, i64)>, id: &str) -> Option<String> {
    let key = |i: &str| format!("{UIMODELS_DIR}{i}_controller.controller");
    if containers.contains_key(&key(id)) {
        return Some(id.to_string());
    }
    containers
        .keys()
        .filter_map(|k| {
            k.strip_prefix(UIMODELS_DIR)?
                .strip_suffix("_controller.controller")
        })
        .filter(|base| !base.contains('/') && base.len() > "chr_0000".len() && id.starts_with(base))
        .max_by_key(|base| base.len())
        .map(str::to_string)
}

/// A controller read from the store: its flattened form plus, per state, the clip object.
struct LoadedController {
    menu: Value,
    /// State name → clip object.
    state_clips: BTreeMap<String, ObjRef>,
    /// Every clip object the controller references, deserialized.
    clip_values: HashMap<ObjRef, Value>,
}

fn load_controller(
    ex: &mut Exporter<'_>,
    at: ObjRef,
    clips: &ClipIndex,
) -> Option<LoadedController> {
    let (class, ac) = ex.read(at)?;
    if class != CLASS_CONTROLLER {
        return None;
    }
    let clip_refs: Vec<Option<ObjRef>> = arr(&ac, "m_AnimationClips")
        .iter()
        .map(|p| ex.resolve(at.file, Some(p)))
        .collect();
    let mut clip_values: HashMap<ObjRef, Value> = HashMap::new();
    for r in clip_refs.iter().flatten() {
        if !clip_values.contains_key(r)
            && let Some((_, v)) = ex.read(*r)
        {
            clip_values.insert(*r, v);
        }
    }
    let clip_names: Vec<Option<String>> = clip_refs
        .iter()
        .map(|r| {
            let v = clip_values.get(&(*r)?)?;
            v.get("m_Name").and_then(Value::as_str).map(str::to_string)
        })
        .collect();
    let clip_lengths: Vec<Option<f64>> = clip_refs
        .iter()
        .map(|r| clip_length(clip_values.get(&(*r)?)?))
        .collect();
    let behaviours: Vec<Option<Value>> = arr(&ac, "m_StateMachineBehaviours")
        .iter()
        .map(|p| {
            let r = ex.resolve(at.file, Some(p))?;
            ex.read(r).map(|(_, v)| v)
        })
        .collect();
    let menu = flatten_controller(
        &ac,
        &ControllerRefs {
            clip_names: &clip_names,
            clip_lengths: &clip_lengths,
            behaviours: &behaviours,
            clips,
        },
    )?;
    // Clip object per state, by the same walk `flatten_controller` does.
    let tos = tos_map(&ac);
    let ctrl = data(ac.get("m_Controller")?);
    let layer = data(arr(ctrl, "m_LayerArray").first()?);
    let sm = data(
        arr(ctrl, "m_StateMachineArray")
            .get(usize::try_from(int(layer, "m_StateMachineIndex").unwrap_or(0)).ok()?)?,
    );
    let mut state_clips = BTreeMap::new();
    for s in arr(sm, "m_StateConstantArray") {
        let s = data(s);
        let Some(full) = s
            .get("m_FullPathID")
            .and_then(Value::as_u64)
            .and_then(|h| tos.get(&(h as u32)))
        else {
            continue;
        };
        let name = full.split_once('.').map_or(full.as_str(), |(_, r)| r);
        let clip = arr(s, "m_BlendTreeConstantArray")
            .iter()
            .flat_map(|bt| arr(data(bt), "m_NodeArray"))
            .filter_map(|n| data(n).get("m_ClipID").and_then(Value::as_u64))
            .find(|&id| id != NO_CLIP)
            .and_then(|id| clip_refs.get(usize::try_from(id).ok()?).copied().flatten());
        if let Some(clip) = clip {
            state_clips.insert(name.to_string(), clip);
        }
    }
    Some(LoadedController {
        menu,
        state_clips,
        clip_values,
    })
}

/// `m_MuscleClip.m_StopTime − m_StartTime` of an AnimationClip, in seconds.
#[must_use]
pub fn clip_length(clip: &Value) -> Option<f64> {
    let m = clip.get("m_MuscleClip")?;
    let stop = num(m, "m_StopTime")?;
    let start = num(m, "m_StartTime").unwrap_or(0.0);
    (stop >= start && stop.is_finite()).then_some(stop - start)
}

/// Scalar curves of a clip the controller references.
fn clip_curves(ctrl: &LoadedController, clip: ObjRef) -> Result<Vec<(Value, Curve)>, String> {
    scalar_curves(ctrl.clip_values.get(&clip).ok_or("clip not readable")?)
}

/// Path (below the root) of every Transform of a prefab, by CRC32 of the path.
fn node_paths(src: &CabSource, file: usize, root_go: i64) -> HashMap<u32, String> {
    let sf = &src.files.files[file].sf;
    let read = |class: i32| -> HashMap<i64, Value> {
        sf.objects
            .iter()
            .filter(|o| o.class_id == class)
            .filter_map(|o| Some((o.path_id, sf.read_object(o).ok()?)))
            .collect()
    };
    let gos = read(CLASS_GAME_OBJECT);
    let transforms = read(CLASS_TRANSFORM);
    let go_of = |t: &Value| t.pointer("/m_GameObject/m_PathID").and_then(Value::as_i64);
    let name_of = |t: &Value| {
        go_of(t)
            .and_then(|g| gos.get(&g))
            .and_then(|g| g.get("m_Name"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string()
    };
    let mut out = HashMap::new();
    let Some((root, _)) = transforms.iter().find(|(_, t)| go_of(t) == Some(root_go)) else {
        return out;
    };
    let mut stack = vec![(*root, String::new())];
    while let Some((id, path)) = stack.pop() {
        if out.len() > 4096 {
            break;
        }
        for child in arr(&transforms[&id], "m_Children") {
            let Some(cid) = child.get("m_PathID").and_then(Value::as_i64) else {
                continue;
            };
            let Some(t) = transforms.get(&cid) else {
                continue;
            };
            let name = name_of(t);
            let p = if path.is_empty() {
                name
            } else {
                format!("{path}/{name}")
            };
            out.insert(crc32(p.as_bytes()), p.clone());
            stack.push((cid, p));
        }
    }
    out
}

/// A prop GLB: file name (`<n>.glb`) and bytes.
pub type PropGlb = (String, Vec<u8>);

/// Everything written for one operator.
#[derive(Debug, Default)]
pub struct CharacterData {
    /// `clips/<kind>/<id>/menu.json`.
    pub menu: Option<Value>,
    /// `charinfo/weapons/<id>.json`.
    pub weapon: Option<Value>,
    /// `charinfo/deco/<id>/index.json` and its prop GLBs.
    pub deco: Option<(Value, Vec<PropGlb>)>,
    pub warnings: Vec<String>,
}

/// Inputs shared by every character of a run.
pub struct Context<'a> {
    pub records: &'a [CabRecord],
    pub containers: &'a HashMap<String, (String, i64)>,
    pub tables: &'a Tables,
    /// Export settings for deco prop GLBs.
    pub opts: &'a ExportOptions,
    /// Models root, to link existing weapon GLBs.
    pub models: &'a Path,
}

/// Character-screen data of operator `id` (see the module docs), reading the store through
/// `src`. `clips` is the operator's clip index; states and visibility tracks name its files.
/// `nodes` are the node names of the operator's exported GLB (empty: unknown, mounts are not
/// checked).
pub fn character(
    src: &mut CabSource,
    ctx: &Context<'_>,
    id: &str,
    clips: &ClipIndex,
    nodes: &HashSet<String>,
) -> CharacterData {
    let mut out = CharacterData::default();
    let Some(base) = uimodel_id(ctx.containers, id) else {
        out.warnings.push("no UI controller".into());
        return out;
    };
    let controller = ctx
        .containers
        .get(&format!("{UIMODELS_DIR}{base}_controller.controller"))
        .cloned();
    let mut loaded = None;
    if let Some((cab, path_id)) = controller
        && let Some(file) = src.get(&cab)
    {
        let mut ex = Exporter::new(src, ExportOptions::default());
        loaded = load_controller(&mut ex, ObjRef { file, path_id }, clips);
    }
    let Some(loaded) = loaded else {
        out.warnings
            .push(format!("UI controller of {base} not readable"));
        return out;
    };
    // One file per distinct clip, first state wins.
    let mut clip_files: BTreeMap<String, (ObjRef, String)> = BTreeMap::new();
    for (state, clip) in &loaded.state_clips {
        if let Some(file) = loaded
            .menu
            .pointer(&format!(
                "/states/{}/clip",
                state.replace('~', "~0").replace('/', "~1")
            ))
            .and_then(Value::as_str)
        {
            clip_files
                .entry(file.to_string())
                .or_insert((*clip, state.clone()));
        }
    }
    let mut menu = loaded.menu.clone();
    if let Some(m) = menu.as_object_mut() {
        m.insert("id".into(), json!(id));
        m.insert(
            "controller".into(),
            json!(format!("{UIMODELS_DIR}{base}_controller.controller")),
        );
    }
    let display = display_config(&ctx.tables.display, &base);
    // Weapon visibility per menu clip.
    let mut weapon_clips = Map::new();
    let mut wipe_clips = Map::new();
    let disappear = display
        .and_then(|d| d.pointer("/weaponConfig/weaponDisappearEffectDuration"))
        .and_then(Value::as_f64)
        .unwrap_or(0.0);
    for (file, (clip, state)) in &clip_files {
        let curves = match clip_curves(&loaded, *clip) {
            Ok(c) => c,
            Err(e) => {
                out.warnings.push(format!("{file}: {e}"));
                continue;
            }
        };
        let Some(hide) = animator_param(&curves, WEAPON_HIDE) else {
            // Not animated: the parameter keeps the page's value, shown on the weapon page.
            if state.starts_with("Weapon.") {
                weapon_clips.insert(file.clone(), json!([[0.0, 1.0]]));
            }
            continue;
        };
        let changes = steps(hide, WEAPON_SHOWN_BELOW, true);
        if changes.iter().any(|[_, v]| *v > 0.5) {
            let wipe: Vec<[f64; 2]> = changes.iter().skip(1).copied().collect();
            if !wipe.is_empty() {
                wipe_clips.insert(file.clone(), json!(wipe));
            }
            weapon_clips.insert(file.clone(), json!(weapon_track(&changes, disappear)));
        }
    }
    // The team screen shows the weapon although its clips keep `WeaponHide` at 1.
    for m in arr(&menu, "moments") {
        if m.pointer("/send/kind").and_then(Value::as_str) != Some("team") {
            continue;
        }
        let Some(file) = m
            .get("state")
            .and_then(Value::as_str)
            .and_then(|st| menu.get("states")?.get(st)?.get("clip")?.as_str())
        else {
            continue;
        };
        weapon_clips
            .entry(file.to_string())
            .or_insert_with(|| json!([[0.0, 1.0]]));
    }
    let hide_weapon: Vec<&str> = clips.files().filter(|f| is_pose_clip(f)).collect();
    if let Some((weapon, rule)) =
        default_weapon(&base, &ctx.tables.gacha_presets, &ctx.tables.characters)
    {
        let row = ctx.tables.weapons.get(&weapon);
        // Mounts on nodes the exported model lacks (props of other parts) are listed apart.
        let (mounts, unresolved): (Vec<Value>, Vec<Value>) = display
            .map(weapon_mounts)
            .unwrap_or_default()
            .into_iter()
            .partition(|m| {
                nodes.is_empty()
                    || m.get("node")
                        .and_then(Value::as_str)
                        .is_some_and(|n| nodes.contains(n))
            });
        let glb = format!("weapon/{weapon}.glb");
        out.weapon = Some(json!({
            "format": 1,
            "id": id,
            "weapon": weapon,
            "rule": rule,
            "type": row.and_then(|r| r.get("weaponType")).cloned().unwrap_or(Value::Null),
            "rarity": row.and_then(|r| r.get("rarity")).cloned().unwrap_or(Value::Null),
            "glb": ctx.models.join(&glb).is_file().then_some(glb),
            "mounts": mounts,
            "unresolvedMounts": unresolved,
            "clips": weapon_clips,
            "hideWeapon": hide_weapon,
            "wipe": {
                "clips": wipe_clips,
                "appear": display.map_or(Value::Null, |d| weapon_effect(d, "Appear")),
                "disappear": display.map_or(Value::Null, |d| weapon_effect(d, "Disappear")),
            },
        }));
    } else {
        out.warnings.push(format!("no default weapon for {base}"));
    }
    // Deco props.
    let deco_items: Vec<(String, String)> = display
        .and_then(|d| d.pointer("/decoItemConfig/decoItemData"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|d| {
            Some((
                d.get("prefabPath")?.as_str()?.to_string(),
                d.get("mountPoint")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
            ))
        })
        .collect();
    if !deco_items.is_empty() {
        out.deco = Some(deco(
            src,
            ctx,
            id,
            &deco_items,
            &clip_files,
            clips,
            &mut out.warnings,
        ));
    }
    out.menu = Some(menu);
    out
}

/// Deco props: `{"format": 1, "id", "items": [{"n", "prefab", "file", "mount", "kind": "deco"}],
/// "clips": {clipFile: {"<n>": {"visible": {"<node path>": [[t, 0|1]]}}}}}`.
fn deco(
    src: &mut CabSource,
    ctx: &Context<'_>,
    id: &str,
    items: &[(String, String)],
    menu_clips: &BTreeMap<String, (ObjRef, String)>,
    clips: &ClipIndex,
    warnings: &mut Vec<String>,
) -> (Value, Vec<(String, Vec<u8>)>) {
    let mut out_items = Vec::new();
    let mut glbs = Vec::new();
    let mut per_clip: BTreeMap<String, Map<String, Value>> = BTreeMap::new();
    for (i, (prefab, mount)) in items.iter().enumerate() {
        let n = i + 1;
        let container = format!("{DECO_DIR}{}.prefab", prefab.to_ascii_lowercase());
        let mut item = Map::new();
        item.insert("n".into(), json!(n));
        item.insert("prefab".into(), json!(prefab));
        item.insert("mount".into(), json!(mount));
        item.insert("kind".into(), json!("deco"));
        let Some((cab, root_go)) = ctx.containers.get(&container).cloned() else {
            warnings.push(format!("deco {prefab} not stored"));
            item.insert("file".into(), Value::Null);
            out_items.push(Value::Object(item));
            continue;
        };
        let Some(file) = src.get(&cab) else {
            item.insert("file".into(), Value::Null);
            out_items.push(Value::Object(item));
            continue;
        };
        let root = ObjRef {
            file,
            path_id: root_go,
        };
        // The prop as its own GLB.
        let extras = json!({
            "source": {"container": container, "cab": cab, "pathId": root_go.to_string()},
            "entity": {"id": prefab, "kind": "deco", "owner": id, "mount": mount},
        });
        let exported = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            Exporter::new(src, ctx.opts.clone()).export(root, extras)
        }))
        .unwrap_or_else(|_| Err("exporter panicked".into()));
        match exported {
            Ok(r) if r.stats.meshes > 0 => {
                let name = format!("{n}.glb");
                item.insert("file".into(), json!(name));
                item.insert("meshes".into(), json!(r.stats.meshes));
                glbs.push((name, r.glb));
            }
            Ok(_) => {
                item.insert("file".into(), Value::Null);
            }
            Err(e) => {
                warnings.push(format!("deco {prefab}: {e}"));
                item.insert("file".into(), Value::Null);
            }
        }
        // Its controller: the Animator on the prop root.
        let file = src.get(&cab).unwrap_or(file);
        let paths = node_paths(src, file, root_go);
        let animator = {
            let sf = &src.files.files[file].sf;
            sf.objects
                .iter()
                .filter(|o| o.class_id == CLASS_ANIMATOR)
                .find_map(|o| sf.read_object(o).ok())
        };
        let mut ex = Exporter::new(src, ExportOptions::default());
        let deco_ctrl = animator
            .as_ref()
            .and_then(|a| ex.resolve(file, a.get("m_Controller")))
            .and_then(|r| load_controller(&mut ex, r, clips));
        let Some(deco_ctrl) = deco_ctrl else {
            out_items.push(Value::Object(item));
            continue;
        };
        for (clip_file, (_, state)) in menu_clips {
            let Some(deco_clip) = deco_ctrl.state_clips.get(state) else {
                continue;
            };
            let curves = match clip_curves(&deco_ctrl, *deco_clip) {
                Ok(c) => c,
                Err(e) => {
                    warnings.push(format!("deco {prefab} {state}: {e}"));
                    continue;
                }
            };
            let mut visible = Map::new();
            for (hash, curve) in active_curves(&curves) {
                let node = paths
                    .get(&hash)
                    .cloned()
                    .unwrap_or_else(|| format!("#{hash}"));
                visible.insert(node, json!(steps(curve, 0.5, false)));
            }
            if visible.is_empty() {
                continue;
            }
            per_clip
                .entry(clip_file.clone())
                .or_default()
                .insert(n.to_string(), json!({"visible": visible}));
        }
        out_items.push(Value::Object(item));
    }
    let index = json!({
        "format": 1,
        "id": id,
        "items": out_items,
        "clips": per_clip,
    });
    (index, glbs)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pptr_list(n: usize) -> Vec<Option<String>> {
        (0..n)
            .map(|i| Some(format!("A_actor_x_clip_{i}")))
            .collect()
    }

    fn controller() -> Value {
        let tos = [
            "Base Layer.Overview.OverviewIdle",
            "Base Layer.Weapon.WeaponIdle",
            "Base Layer.Weapon.FromOverview",
            "Base Layer.Relax.RelaxIdle",
            "Base Layer.Relax.SP1",
            "ToIndex",
            "FromIndex",
            "EnableSwitch",
            "formation",
            "Base Layer.Formation.Slot2",
        ];
        let h = |s: &str| u64::from(crc32(s.as_bytes()));
        let state = |name: &str, clip: u64, transitions: Value| {
            json!({"data": {
                "m_FullPathID": h(name), "m_Loop": false,
                "m_BlendTreeConstantArray": [{"data": {"m_NodeArray": [{"data": {"m_ClipID": clip}}]}}],
                "m_TransitionConstantArray": transitions,
            }})
        };
        let cond = |name: &str, mode: i64, thr: f64| json!({"data": {"m_ConditionMode": mode, "m_EventID": h(name), "m_EventThreshold": thr}});
        json!({
            "m_TOSData": tos,
            "m_Controller": {
                "m_LayerArray": [{"data": {"m_StateMachineIndex": 0}}],
                "m_Values": {"data": {"m_ValueArray": [
                    {"m_ID": h("ToIndex"), "m_Type": 3},
                    {"m_ID": h("FromIndex"), "m_Type": 3},
                    {"m_ID": h("EnableSwitch"), "m_Type": 9},
                    {"m_ID": h("formation"), "m_Type": 3},
                ]}},
                "m_StateMachineArray": [{"data": {
                    "m_DefaultState": 30000,
                    "m_SelectorStateConstantArray": [{"data": {"m_TransitionConstantArray": [
                        {"data": {"m_ConditionConstantArray": [], "m_Destination": 0}}]}}],
                    "m_StateConstantArray": [
                        state("Base Layer.Overview.OverviewIdle", 0, json!([])),
                        state("Base Layer.Weapon.WeaponIdle", 1, json!([])),
                        state("Base Layer.Weapon.FromOverview", 2, json!([{"data": {
                            "m_DestinationState": 1, "m_ConditionConstantArray": [],
                            "m_HasExitTime": true, "m_ExitTime": 0.5,
                            "m_HasFixedDuration": false, "m_TransitionDuration": 0.25,
                            "m_TransitionOffset": 0.0}}])),
                        state("Base Layer.Relax.RelaxIdle", 3, json!([{"data": {
                            "m_DestinationState": 4,
                            "m_ConditionConstantArray": [cond("EnableSwitch", 1, 0.0),
                                cond("FromIndex", 6, 6.0), cond("ToIndex", 6, 20.0)],
                            "m_HasExitTime": true, "m_ExitTime": 0.9,
                            "m_HasFixedDuration": true, "m_TransitionDuration": 0.2,
                            "m_TransitionOffset": 0.0}}])),
                        state("Base Layer.Relax.SP1", 4, json!([])),
                        state("Base Layer.Formation.Slot2", 0, json!([])),
                    ],
                    "m_AnyStateTransitionConstantArray": [
                        {"data": {"m_DestinationState": 2,
                            "m_ConditionConstantArray": [cond("FromIndex", 6, 0.0),
                                cond("ToIndex", 6, 1.0), cond("EnableSwitch", 1, 0.0)],
                            "m_HasFixedDuration": false, "m_TransitionDuration": 0.1,
                            "m_TransitionOffset": 0.0}},
                        {"data": {"m_DestinationState": 0,
                            "m_ConditionConstantArray": [cond("FromIndex", 6, 0.0),
                                cond("ToIndex", 6, 0.0), cond("EnableSwitch", 1, 0.0)],
                            "m_HasFixedDuration": true, "m_TransitionDuration": 0.0,
                            "m_TransitionOffset": 0.0}},
                        {"data": {"m_DestinationState": 5,
                            "m_ConditionConstantArray": [cond("formation", 6, 1.0)],
                            "m_HasFixedDuration": true, "m_TransitionDuration": 0.0,
                            "m_TransitionOffset": 0.0}},
                    ],
                }}],
            },
            "m_StateMachineBehaviourVectorDescription": {
                "m_StateMachineBehaviourIndices": [0],
                "m_StateMachineBehaviourRanges": {
                    format!("{{\"m_LayerIndex\":0,\"m_StateID\":{}}}", h("Base Layer.Weapon.FromOverview")):
                        {"m_StartIndex": 0, "m_Count": 1}
                },
            },
        })
    }

    #[test]
    fn flattens_states_transitions_and_moments() {
        let names = pptr_list(5);
        let entries: Vec<Value> = (0..5)
            .map(|i| {
                json!({"clip": format!("A_actor_x_clip_{i}"), "file": format!("a_actor_x_clip_{i}.glb"),
                       "duration": 2.0, "loop": i < 2})
            })
            .collect();
        let clips = ClipIndex::from_entries(&entries);
        let behaviours = vec![Some(json!({"_effects": [
            {"effectName": "P_fx_start", "mountPoint": "wep_R"}]}))];
        let menu = flatten_controller(
            &controller(),
            &ControllerRefs {
                clip_names: &names,
                clip_lengths: &[],
                behaviours: &behaviours,
                clips: &clips,
            },
        )
        .unwrap();
        assert_eq!(menu["default"], "Overview.OverviewIdle");
        let from = &menu["states"]["Weapon.FromOverview"];
        assert_eq!(from["clip"], "a_actor_x_clip_2.glb");
        assert_eq!(from["length"], 2.0);
        assert_eq!(from["loop"], false);
        assert_eq!(
            from["exit"],
            json!({"to": "Weapon.WeaponIdle", "at": 1.0, "fade": 0.5, "offset": 0.0})
        );
        assert_eq!(from["effects"], json!([["P_fx_start", "wep_R"]]));
        assert_eq!(menu["states"]["Overview.OverviewIdle"]["loop"], true);
        let relax = &menu["states"]["Relax.RelaxIdle"];
        assert_eq!(relax["exit"], Value::Null);
        assert_eq!(
            relax["on"][0],
            json!({"when": {"EnableSwitch": true, "FromIndex": 6, "ToIndex": 20},
                   "to": "Relax.SP1", "at": 1.8, "fade": 0.2, "offset": 0.0})
        );
        assert_eq!(
            menu["any"][0],
            json!({"when": {"FromIndex": 0, "ToIndex": 1, "EnableSwitch": true},
                   "to": "Weapon.FromOverview", "fadeOfSource": 0.1, "offset": 0.0})
        );
        assert_eq!(menu["params"]["EnableSwitch"], "trigger");
        let ids: Vec<&str> = menu["moments"]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| m["id"].as_str().unwrap())
            .collect();
        assert_eq!(ids, ["overview", "weapon", "special1", "team1"]);
        assert_eq!(menu["moments"][1]["idle"], "Weapon.WeaponIdle");
        assert_eq!(menu["moments"][2]["label"], "Special 1");
        assert_eq!(menu["moments"][3]["label"], "Team Pose 1");
    }

    #[test]
    fn missing_clips_keep_the_state() {
        let names = vec![None, None, Some("A_unknown".to_string()), None, None];
        let menu = flatten_controller(
            &controller(),
            &ControllerRefs {
                clip_names: &names,
                clip_lengths: &[],
                behaviours: &[],
                clips: &ClipIndex::default(),
            },
        )
        .unwrap();
        let from = &menu["states"]["Weapon.FromOverview"];
        assert_eq!(from["clip"], Value::Null);
        assert_eq!(from["missingClip"], "A_unknown");
        // Without a length the exit time and fade stay normalized.
        assert_eq!(from["exit"]["atNormalized"], 0.5);
        assert_eq!(from["exit"]["fadeOfSource"], 0.25);
    }

    #[test]
    fn streamed_keys_read_constant_frames() {
        let one = 1.0f32.to_bits();
        let words = [
            0xff7f_ffff,
            1,
            0,
            0,
            0,
            0,
            one,
            0,
            1,
            0,
            0,
            0,
            0,
            0,
            0.5f32.to_bits(),
            1,
            0,
            0,
            0,
            0,
            one,
            f32::INFINITY.to_bits(),
            0,
        ];
        let keys = streamed_keys(&words);
        assert_eq!(keys[&0], vec![(0.0, 0.0), (0.5, 1.0)]);
    }

    #[test]
    fn steps_interpolate_linear_crossings() {
        let curve = Curve {
            keys: vec![(0.0, 1.0), (0.5, 1.0), (0.75, 0.0), (1.0, 0.0)],
            linear: true,
        };
        // WeaponHide: visible below 0.5.
        assert_eq!(steps(&curve, 0.5, true), vec![[0.0, 0.0], [0.625, 1.0]]);
        let held = Curve {
            linear: false,
            ..curve
        };
        assert_eq!(steps(&held, 0.5, true), vec![[0.0, 0.0], [0.75, 1.0]]);
        assert_eq!(
            weapon_track(&[[0.0, 0.0], [0.375, 1.0], [1.0, 0.0]], 0.5),
            vec![[0.0, 0.0], [0.375, 1.0], [1.5, 0.0]]
        );
        assert!(is_pose_clip("a_actor_chen_interact_camera_01.glb"));
        assert!(!is_pose_clip("a_actor_chen_interact_receive_gift.glb"));
    }

    #[test]
    fn default_weapon_prefers_the_signature() {
        let gacha = json!({"chr_0005_chen": {"perfectWeaponId": "wpn_sword_0015"}});
        let chars = json!({"chr_0005_chen": {"defaultWeaponId": "wpn_sword_0003"},
                           "chr_0003_endminf": {"defaultWeaponId": "wpn_sword_0003"}});
        assert_eq!(
            default_weapon("chr_0005_chen", &gacha, &chars),
            Some(("wpn_sword_0015".into(), "signature"))
        );
        assert_eq!(
            default_weapon("chr_0003_endminf", &gacha, &chars),
            Some(("wpn_sword_0003".into(), "basic"))
        );
        assert_eq!(default_weapon("chr_9999_x", &gacha, &chars), None);
    }

    #[test]
    fn weapon_mounts_from_display_config() {
        let display = json!({"weaponConfig": {"weaponData": [
            {"nodeUIIdle": "Funnel_Point_001", "weaponScale": 1.0},
            {"nodeUIIdle": "", "weaponScale": 1.0},
            {"nodeUIIdle": "Funnel_Point_002", "weaponScale": 0.8}],
            "weaponDisappearEffectName": ["P_out"], "weaponDisappearEffectDuration": 0.5}});
        assert_eq!(
            weapon_mounts(&display),
            vec![
                json!({"node": "Funnel_Point_001", "scale": 1.0}),
                json!({"node": "Funnel_Point_002", "scale": 0.8})
            ]
        );
        assert_eq!(
            weapon_effect(&display, "Disappear"),
            json!({"effects": ["P_out"], "duration": 0.5})
        );
    }
}
