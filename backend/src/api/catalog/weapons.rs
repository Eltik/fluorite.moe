//! Weapons: `WeaponBasicTable` + level/breakthrough templates + `SkillPatchTable`.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use serde::Serialize;
use serde_json::Value;

use super::assets::{ModelInfo, model_info};
use super::operators::{Coded, ItemCountOut, SkillLevelOut, item_counts, skill_levels};
use super::tables::*;
use super::text::{Rich, TextRef, Texts};
use super::{BaseItem, BaseTag, Env, dep, finish_detail, num, sort_rarity_desc};
use crate::api::error::ApiResult;

/// Display name of a weapon type code via `TextTable["LUA_WEAPON_TYPE_<n>"]`.
pub fn weapon_type_name(text: &TextTable, code: &str) -> TextRef {
    let n = match code {
        "Sword" => 1,
        "Wand" => 2,
        "Claymores" => 3,
        "Lance" => 5,
        "Pistol" => 6,
        _ => return TextRef::default(),
    };
    text.get(&format!("LUA_WEAPON_TYPE_{n}"))
        .copied()
        .unwrap_or_default()
}

pub struct Tables {
    pub weapons: Arc<WeaponBasicTable>,
    pub upgrades: Arc<WeaponUpgradeTemplateTable>,
    pub breaks: Arc<WeaponBreakThroughTemplateTable>,
    pub skills: Arc<SkillPatchTable>,
    pub text: Arc<TextTable>,
}

impl Tables {
    pub async fn load(c: &TableCache, dir: &Path) -> ApiResult<Self> {
        Ok(Self {
            weapons: c.get(dir).await?,
            upgrades: c.get_or_default(dir).await?,
            breaks: c.get_or_default(dir).await?,
            skills: c.get_or_default(dir).await?,
            text: c.get_or_default(dir).await?,
        })
    }

    pub fn deps(&self) -> Vec<AnyArc> {
        vec![dep(&self.weapons), dep(&self.text)]
    }
}

pub fn base(t: &Tables, env: &Env) -> Vec<BaseItem> {
    let mut items: Vec<BaseItem> = t
        .weapons
        .iter()
        .map(|(id, w)| BaseItem {
            id: id.clone(),
            name: w.eng_name,
            fallback_name: id.clone(),
            rarity: Some(w.rarity),
            tags: vec![BaseTag::text(
                "weaponType",
                &w.weapon_type,
                weapon_type_name(&t.text, &w.weapon_type),
            )],
            icon: env.ui("itemicon", id),
            has_model: env.meshes.weapon(id).is_some()
                || env.rig.find(&env.output, "weapons", id).is_some(),
            type_code: Some(w.weapon_type.clone()).filter(|s| !s.is_empty()),
        })
        .collect();
    sort_rarity_desc(&mut items);
    items
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AtkSeries {
    pub level: Vec<u32>,
    pub base_atk: Vec<Value>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WeaponSkillOut {
    pub id: String,
    pub name: String,
    pub levels: Vec<SkillLevelOut>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BreakthroughOut {
    pub level: u32,
    pub stage: u32,
    pub gold: i64,
    pub cost: Vec<ItemCountOut>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WeaponDetail {
    pub kind: &'static str,
    pub id: String,
    pub name: String,
    pub rarity: u8,
    pub weapon_type: Coded,
    pub description: Rich,
    pub max_level: u32,
    pub atk: AtkSeries,
    pub breakthroughs: Vec<BreakthroughOut>,
    pub skills: Vec<WeaponSkillOut>,
    pub art: BTreeMap<&'static str, String>,
    pub model: ModelInfo,
}

pub fn detail(t: &Tables, env: &Env, texts: &Texts, id: &str) -> Option<Value> {
    let w = t.weapons.get(id)?;
    let (level, base_atk) = t
        .upgrades
        .get(&w.level_template_id)
        .map(|u| {
            u.list
                .iter()
                .filter(|l| w.max_lv == 0 || l.weapon_lv <= w.max_lv)
                .map(|l| (l.weapon_lv, num(l.base_atk)))
                .unzip()
        })
        .unwrap_or_default();
    let breakthroughs = t
        .breaks
        .get(&w.breakthrough_template_id)
        .map(|b| {
            b.list
                .iter()
                .map(|s| BreakthroughOut {
                    level: s.breakthrough_lv,
                    stage: s.breakthrough_show_lv,
                    gold: s.breakthrough_gold,
                    cost: item_counts(&s.break_item_list),
                })
                .collect()
        })
        .unwrap_or_default();
    let skills = w
        .weapon_skill_list
        .iter()
        .map(|sid| {
            let name = t
                .skills
                .get(sid)
                .and_then(|s| s.levels.first())
                .map(|l| texts.plain(l.skill_name))
                .unwrap_or_default();
            WeaponSkillOut {
                id: sid.clone(),
                name,
                levels: skill_levels(&t.skills, std::slice::from_ref(sid), None, texts),
            }
        })
        .collect();
    let mut art = BTreeMap::new();
    for (key, dir) in [
        ("icon", "itemicon"),
        ("iconBig", "itemiconbig"),
        ("gacha", "gachaweapon"),
    ] {
        if let Some(p) = env.ui(dir, id) {
            art.insert(key, p);
        }
    }
    let d = WeaponDetail {
        kind: "weapons",
        id: id.to_string(),
        name: Some(texts.plain(w.eng_name))
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| id.to_string()),
        rarity: w.rarity,
        weapon_type: Coded {
            code: w.weapon_type.clone(),
            name: texts.plain(weapon_type_name(&t.text, &w.weapon_type)),
            icon: None,
            color: None,
        },
        description: texts.rich(w.weapon_desc, None),
        max_level: w.max_lv,
        atk: AtkSeries { level, base_atk },
        breakthroughs,
        skills,
        art,
        model: model_info(&env.output, &env.rig, "weapons", id, env.meshes.weapon(id)),
    };
    Some(finish_detail(&d, texts))
}
