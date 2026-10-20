//! Operators: `CharacterTable` + `CharGrowthTable` (skills, talents, promotions) +
//! `SkillPatchTable` (per-level numbers) + potentials.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use indexmap::IndexMap;
use serde::Serialize;
use serde_json::Value;

use super::assets::{ModelInfo, model_info};
use super::tables::*;
use super::text::{BbEntry, Blackboard, Rich, TextRef, Texts};
use super::weapons::weapon_type_name;
use super::{BaseItem, BaseTag, Env, bb_json, dep, finish_detail, num, sort_rarity_desc};
use crate::api::error::ApiResult;

pub struct Tables {
    pub chars: Arc<CharacterTable>,
    pub growth: Arc<CharGrowthTable>,
    pub skills: Arc<SkillPatchTable>,
    pub effects: Arc<PotentialTalentEffectTable>,
    pub potentials: Arc<CharacterPotentialTable>,
    pub professions: Arc<CharProfessionTable>,
    pub types: Arc<CharTypeTable>,
    pub battle_tags: Arc<CharBattleTagTable>,
    pub text: Arc<TextTable>,
    pub weapons: Arc<WeaponBasicTable>,
}

impl Tables {
    pub async fn load(c: &TableCache, dir: &Path) -> ApiResult<Self> {
        Ok(Self {
            chars: c.get(dir).await?,
            growth: c.get_or_default(dir).await?,
            skills: c.get_or_default(dir).await?,
            effects: c.get_or_default(dir).await?,
            potentials: c.get_or_default(dir).await?,
            professions: c.get_or_default(dir).await?,
            types: c.get_or_default(dir).await?,
            battle_tags: c.get_or_default(dir).await?,
            text: c.get_or_default(dir).await?,
            weapons: c.get_or_default(dir).await?,
        })
    }

    /// Tables the list depends on.
    pub fn deps(&self) -> Vec<AnyArc> {
        vec![
            dep(&self.chars),
            dep(&self.professions),
            dep(&self.types),
            dep(&self.text),
        ]
    }

    fn profession(&self, code: &str) -> Option<&Profession> {
        self.professions.values().find(|p| p.profession == code)
    }
}

/// Playable operators (the generic `chr_9000_*` Endministrator alias is skipped).
fn is_listed(id: &str) -> bool {
    !id.starts_with("chr_9000")
}

pub fn base(t: &Tables, env: &Env) -> Vec<BaseItem> {
    let mut items: Vec<BaseItem> = t
        .chars
        .iter()
        .filter(|(id, _)| is_listed(id))
        .map(|(id, c)| {
            let mut tags = Vec::new();
            if let Some(p) = t.profession(&c.profession) {
                tags.push(BaseTag::text("profession", &c.profession, p.name));
            }
            if let Some(e) = t.types.get(&c.char_type_id) {
                tags.push(BaseTag::text("element", &c.char_type_id, e.name));
            }
            if !c.weapon_type.is_empty() {
                tags.push(BaseTag::text(
                    "weaponType",
                    &c.weapon_type,
                    weapon_type_name(&t.text, &c.weapon_type),
                ));
            }
            BaseItem {
                id: id.clone(),
                name: c.name,
                fallback_name: c.eng_name.clone(),
                rarity: Some(c.rarity),
                tags,
                icon: env.ui("charicon", &format!("icon_{id}")),
                has_model: env.meshes.operator(id).is_some()
                    || env.rig.find(&env.output, "operators", id).is_some(),
                type_code: Some(c.profession.clone()).filter(|s| !s.is_empty()),
            }
        })
        .collect();
    sort_rarity_desc(&mut items);
    items
}

// ── Detail shapes ────────────────────────────────────────────────────────

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Coded {
    pub code: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VoiceActors {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cn: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub en: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub jp: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kr: Option<String>,
}

/// Column-oriented stat table: `series[attr][i]` is the value at `level[i]` /
/// `breakStage[i]`; attributes that never change are in `constants`.
#[derive(Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct AttrSeries {
    pub level: Vec<Value>,
    pub break_stage: Vec<u32>,
    pub series: IndexMap<String, Vec<Value>>,
    pub constants: IndexMap<String, Value>,
}

/// Build an [`AttrSeries`] from rows of `(breakStage, attrs)`; `Level` becomes the index.
pub fn attr_series<'a>(rows: impl IntoIterator<Item = (u32, &'a [Attr])>) -> AttrSeries {
    let mut out = AttrSeries::default();
    let mut cols: IndexMap<String, Vec<Value>> = IndexMap::new();
    let mut n = 0usize;
    for (stage, attrs) in rows {
        out.break_stage.push(stage);
        let mut level = Value::Null;
        for a in attrs {
            if a.attr_type == "Level" {
                level = num(a.attr_value);
                continue;
            }
            let col = cols.entry(a.attr_type.clone()).or_default();
            col.resize(n, Value::Null);
            col.push(num(a.attr_value));
        }
        out.level.push(level);
        n += 1;
    }
    for (k, mut col) in cols {
        col.resize(n, Value::Null);
        if col.iter().all(|v| *v == col[0]) {
            out.constants.insert(k, col[0].clone());
        } else {
            out.series.insert(k, col);
        }
    }
    out
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Stat {
    pub name: String,
    pub value: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Cost {
    #[serde(rename = "type")]
    pub kind: String,
    pub value: Value,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillLevelOut {
    pub level: u32,
    pub description: Rich,
    pub blackboard: serde_json::Map<String, Value>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub stats: Vec<Stat>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cooldown: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cost: Option<Cost>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillGroupOut {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    pub skill_ids: Vec<String>,
    pub levels: Vec<SkillLevelOut>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TalentOut {
    pub id: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    pub index: u32,
    pub level: u32,
    pub break_stage: u32,
    pub effect_id: String,
    pub description: Rich,
    pub cost: Vec<ItemCountOut>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AttrNodeOut {
    pub id: String,
    pub title: String,
    pub description: Rich,
    pub break_stage: u32,
    pub modifiers: Vec<ModifierOut>,
    pub cost: Vec<ItemCountOut>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModifierOut {
    pub attr_type: String,
    pub value: Value,
    pub modifier_type: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ItemCountOut {
    pub id: String,
    pub count: i64,
}

pub fn item_counts(items: &[ItemCount]) -> Vec<ItemCountOut> {
    items
        .iter()
        .map(|i| ItemCountOut {
            id: i.id.clone(),
            count: i.count,
        })
        .collect()
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PromotionOut {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub break_stage: u32,
    pub name: String,
    pub description: Rich,
    pub cost: Vec<ItemCountOut>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PotentialOut {
    pub level: u32,
    pub name: String,
    pub effect_id: String,
    pub description: Rich,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Unlock {
    #[serde(rename = "type")]
    pub kind: String,
    pub value: i64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileOut {
    pub id: String,
    pub title: String,
    pub text: Rich,
    pub unlock: Unlock,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VoiceOut {
    pub id: String,
    pub title: String,
    pub text: Rich,
    pub vo_id: String,
}

/// Add `audio` (`{CN: {ogg, m4a}, ...}`) to each serialized voice line whose `voId`
/// `audio_for` resolves; lines without audio keep their shape.
pub fn attach_voice_audio(detail: &mut Value, audio_for: impl Fn(&str) -> Option<Value>) {
    let Some(lines) = detail.get_mut("voiceLines").and_then(Value::as_array_mut) else {
        return;
    };
    for line in lines {
        let Some(audio) = line
            .get("voId")
            .and_then(Value::as_str)
            .and_then(&audio_for)
        else {
            continue;
        };
        if let Value::Object(o) = line {
            o.insert("audio".into(), audio);
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NamedRef {
    pub id: String,
    pub name: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OperatorDetail {
    pub kind: &'static str,
    pub id: String,
    pub name: String,
    pub eng_name: String,
    pub rarity: u8,
    pub profession: Coded,
    pub element: Coded,
    pub weapon_type: Coded,
    pub department: String,
    pub main_attribute: String,
    pub sub_attribute: String,
    pub battle_tags: Vec<Coded>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_weapon: Option<NamedRef>,
    pub voice_actors: VoiceActors,
    pub attributes: AttrSeries,
    pub promotions: Vec<PromotionOut>,
    pub skills: Vec<SkillGroupOut>,
    pub talents: Vec<TalentOut>,
    pub attribute_nodes: Vec<AttrNodeOut>,
    pub potentials: Vec<PotentialOut>,
    pub profile: Vec<ProfileOut>,
    pub voice_lines: Vec<VoiceOut>,
    pub art: BTreeMap<&'static str, String>,
    pub model: ModelInfo,
}

const GROUP_ORDER: [&str; 4] = ["NormalAttack", "NormalSkill", "ComboSkill", "UltimateSkill"];

/// Level entry `level` of a skill (or the highest one below it).
fn skill_at(s: &SkillPatch, level: u32) -> Option<&SkillLevel> {
    s.levels
        .iter()
        .filter(|l| l.level <= level)
        .max_by_key(|l| l.level)
        .or_else(|| s.levels.first())
}

/// Per-level rows for a set of skills that share one description (a skill group, or a
/// single weapon skill). Blackboards are merged in skill order.
pub fn skill_levels(
    skills: &SkillPatchTable,
    ids: &[String],
    group_desc: Option<TextRef>,
    texts: &Texts,
) -> Vec<SkillLevelOut> {
    let patches: Vec<&SkillPatch> = ids.iter().filter_map(|id| skills.get(id)).collect();
    let max = patches
        .iter()
        .flat_map(|p| p.levels.iter().map(|l| l.level))
        .max()
        .unwrap_or(0);
    (1..=max)
        .map(|level| {
            let at: Vec<&SkillLevel> = patches.iter().filter_map(|p| skill_at(p, level)).collect();
            let mut bb = Blackboard::default();
            let mut rows: Vec<BbEntry> = Vec::new();
            for l in &at {
                bb.extend(&l.blackboard);
                for r in &l.blackboard {
                    if !rows.iter().any(|x| x.key == r.key) {
                        rows.push(r.clone());
                    }
                }
            }
            let desc = group_desc
                .filter(|d| !d.is_empty())
                .or_else(|| at.iter().map(|l| l.description).find(|d| !d.is_empty()));
            let stats = at
                .iter()
                .flat_map(|l| l.sub_desc_data_list.iter())
                .map(|s| Stat {
                    name: texts.plain(s.name),
                    value: s.desc.clone(),
                })
                .filter(|s| !s.name.is_empty() || !s.value.is_empty())
                .collect();
            let main = at
                .iter()
                .find(|l| l.cost_value > 0.0 || l.cool_down > 0.0)
                .copied();
            SkillLevelOut {
                level,
                description: desc.map(|d| texts.rich(d, Some(&bb))).unwrap_or_default(),
                blackboard: bb_json(&rows),
                stats,
                cooldown: main.filter(|l| l.cool_down > 0.0).map(|l| num(l.cool_down)),
                cost: main.filter(|l| l.cost_value > 0.0).map(|l| Cost {
                    kind: l.cost_type.clone(),
                    value: num(l.cost_value),
                }),
            }
        })
        .collect()
}

/// Blackboard of a talent/potential effect: attached buff/skill blackboards plus the
/// values its modifiers apply (`ModifyAttr` → attribute name, `ChangeSkillBlackboard` →
/// `bbKey`, `ChangeSkillParam` → `costvalue` / `coolDown` …).
pub fn effect_bb(e: &PotentialTalentEffect) -> Blackboard {
    let mut bb = Blackboard::default();
    for d in &e.data_list {
        bb.extend(&d.attach_buff.blackboard);
        bb.extend(&d.attach_skill.blackboard);
        let mut put = |k: &str, v: f64| {
            if !k.is_empty() {
                bb.nums.entry(k.to_string()).or_insert(v);
            }
        };
        match d.modify_type.as_str() {
            "ModifyAttr" => put(&d.attr_modifier.attr_type, d.attr_modifier.attr_value),
            "ChangeSkillBlackboard" => {
                let m = &d.skill_bb_modifier;
                put(&m.bb_key, m.float_value);
                if !m.string_value.is_empty() {
                    bb.strs
                        .entry(m.bb_key.clone())
                        .or_insert_with(|| m.string_value.clone());
                }
            }
            "ChangeSkillParam" => {
                let m = &d.skill_param_modifier;
                let lower_camel = {
                    let mut c = m.param_type.chars();
                    c.next()
                        .map(|f| f.to_lowercase().chain(c).collect::<String>())
                        .unwrap_or_default()
                };
                put(&m.param_type, m.param_value);
                put(&m.param_type.to_ascii_lowercase(), m.param_value);
                put(&lower_camel, m.param_value);
            }
            _ => {}
        }
    }
    bb
}

fn effect_desc(t: &Tables, texts: &Texts, effect_id: &str) -> Rich {
    t.effects
        .get(effect_id)
        .map(|e| texts.rich(e.desc, Some(&effect_bb(e))))
        .unwrap_or_default()
}

pub fn detail(t: &Tables, env: &Env, texts: &Texts, id: &str) -> Option<Value> {
    let c = t.chars.get(id)?;
    let growth = t.growth.get(id);

    let profession = {
        let p = t.profession(&c.profession);
        Coded {
            code: c.profession.clone(),
            name: p.map(|p| texts.plain(p.name)).unwrap_or_default(),
            icon: p.and_then(|p| env.ui("charprofessionicon", &p.icon_id)),
            color: None,
        }
    };
    let element = {
        let e = t.types.get(&c.char_type_id);
        Coded {
            code: c.char_type_id.clone(),
            name: e.map(|e| texts.plain(e.name)).unwrap_or_default(),
            icon: e.and_then(|e| env.ui("elementicon", &e.icon)),
            color: e
                .map(|e| e.color.clone())
                .filter(|c| !c.is_empty())
                .map(|c| format!("#{}", c.trim_start_matches('#'))),
        }
    };
    let weapon_type = Coded {
        code: c.weapon_type.clone(),
        name: texts.plain(weapon_type_name(&t.text, &c.weapon_type)),
        icon: None,
        color: None,
    };
    let battle_tags = c
        .char_battle_tag_ids
        .iter()
        .map(|tag| Coded {
            code: tag.clone(),
            name: t
                .battle_tags
                .get(tag)
                .map(|r| texts.plain(*r))
                .unwrap_or_default(),
            icon: None,
            color: None,
        })
        .collect();
    let default_weapon = t.weapons.get(&c.default_weapon_id).map(|w| NamedRef {
        id: c.default_weapon_id.clone(),
        name: texts.plain(w.eng_name),
    });

    let attributes = attr_series(
        c.attributes
            .iter()
            .map(|r| (r.break_stage, r.attribute.attrs.as_slice())),
    );

    let mut promotions: Vec<PromotionOut> = growth
        .map(|g| {
            g.char_break_cost_map
                .values()
                .map(|b| PromotionOut {
                    id: b.node_id.clone(),
                    kind: b.node_type.clone(),
                    break_stage: b.break_stage,
                    name: texts.plain(b.name),
                    description: texts.rich(b.description, None),
                    cost: item_counts(&b.required_item),
                })
                .collect()
        })
        .unwrap_or_default();
    promotions
        .sort_by(|a, b| (a.break_stage, &a.kind, &a.id).cmp(&(b.break_stage, &b.kind, &b.id)));

    let mut skills: Vec<SkillGroupOut> = growth
        .map(|g| {
            g.skill_group_map
                .values()
                .map(|sg| SkillGroupOut {
                    id: sg.skill_group_id.clone(),
                    kind: sg.skill_group_type.clone(),
                    name: texts.plain(sg.name),
                    icon: env.ui("skillicon", &sg.icon),
                    skill_ids: sg.skill_id_list.clone(),
                    levels: skill_levels(&t.skills, &sg.skill_id_list, Some(sg.desc), texts),
                })
                .collect()
        })
        .unwrap_or_default();
    skills.sort_by_key(|s| {
        (
            GROUP_ORDER
                .iter()
                .position(|g| *g == s.kind)
                .unwrap_or(GROUP_ORDER.len()),
            s.id.clone(),
        )
    });

    let mut talents = Vec::new();
    let mut attribute_nodes = Vec::new();
    if let Some(g) = growth {
        for n in g.talent_node_map.values() {
            match n.node_type.as_str() {
                "PassiveSkill" => {
                    let p = &n.passive_skill_node_info;
                    talents.push(TalentOut {
                        id: n.node_id.clone(),
                        name: texts.plain(p.name),
                        icon: env.ui("skillicon", &p.icon_id),
                        index: p.index,
                        level: p.level,
                        break_stage: p.break_stage,
                        effect_id: p.talent_effect_id.clone(),
                        description: effect_desc(t, texts, &p.talent_effect_id),
                        cost: item_counts(&n.required_item),
                    });
                }
                "Attr" => {
                    let a = &n.attribute_node_info;
                    attribute_nodes.push(AttrNodeOut {
                        id: n.node_id.clone(),
                        title: texts.plain(a.title),
                        description: texts.rich(a.desc, None),
                        break_stage: a.break_stage,
                        modifiers: a
                            .attribute_modifiers
                            .iter()
                            .map(|m| ModifierOut {
                                attr_type: m.attr_type.clone(),
                                value: num(m.attr_value),
                                modifier_type: m.modifier_type.clone(),
                            })
                            .collect(),
                        cost: item_counts(&n.required_item),
                    });
                }
                _ => {}
            }
        }
    }
    talents.sort_by(|a, b| (a.index, a.level, &a.id).cmp(&(b.index, b.level, &b.id)));
    attribute_nodes.sort_by(|a, b| (a.break_stage, &a.id).cmp(&(b.break_stage, &b.id)));

    let potentials = t
        .potentials
        .get(id)
        .map(|p| {
            p.potential_unlock_bundle
                .iter()
                .map(|u| PotentialOut {
                    level: u.level,
                    name: texts.plain(u.name),
                    effect_id: u.potential_effect_id.clone(),
                    description: effect_desc(t, texts, &u.potential_effect_id),
                })
                .collect()
        })
        .unwrap_or_default();

    let profile = c
        .profile_record
        .iter()
        .map(|r| ProfileOut {
            id: r.id.clone(),
            title: texts.plain(r.record_title),
            text: texts.rich(r.record_desc, None),
            unlock: Unlock {
                kind: r.unlock_type.clone(),
                value: r.unlock_value,
            },
        })
        .collect();
    let voice_lines = c
        .profile_voice
        .iter()
        .map(|v| VoiceOut {
            id: v.id.clone(),
            title: texts.plain(v.voice_title),
            text: texts.rich(v.voice_desc, None),
            vo_id: v.vo_id.clone(),
        })
        .collect();

    let mut art = BTreeMap::new();
    for (key, dir, name) in [
        ("portrait", "characterportrait", id.to_string()),
        ("gacha", "chargacha", id.to_string()),
        ("gachaShadow", "gachashadow", id.to_string()),
        ("icon", "charicon", format!("icon_{id}")),
        ("roundIcon", "charroundicon", format!("icon_round_{id}")),
        ("horizontalHeadIcon", "charhorheadicon", id.to_string()),
        ("remoteIcon", "charremoteicon", format!("icon_{id}")),
        ("battleIcon", "charbattleicon", format!("icon_{id}")),
        ("infoBackground", "charinfo", format!("bg_charinfo_{id}")),
    ] {
        if let Some(p) = env.ui(dir, &name) {
            art.insert(key, p);
        }
    }

    let d = OperatorDetail {
        kind: "operators",
        id: id.to_string(),
        name: {
            let n = texts.plain(c.name);
            if n.is_empty() { c.eng_name.clone() } else { n }
        },
        eng_name: c.eng_name.clone(),
        rarity: c.rarity,
        profession,
        element,
        weapon_type,
        department: c.department.clone(),
        main_attribute: c.main_attr_type.clone(),
        sub_attribute: c.sub_attr_type.clone(),
        battle_tags,
        default_weapon,
        voice_actors: VoiceActors {
            cn: texts.plain_opt(c.cv_name.chi_c_v_name),
            en: texts.plain_opt(c.cv_name.eng_c_v_name),
            jp: texts.plain_opt(c.cv_name.jap_c_v_name),
            kr: texts.plain_opt(c.cv_name.kor_c_v_name),
        },
        attributes,
        promotions,
        skills,
        talents,
        attribute_nodes,
        potentials,
        profile,
        voice_lines,
        art,
        model: model_info(
            &env.output,
            &env.rig,
            "operators",
            id,
            env.meshes.operator(id),
        ),
    };
    Some(finish_detail(&d, texts))
}
