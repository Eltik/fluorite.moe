//! Enemies: `EnemyTemplateDisplayInfoTable` (the catalog) + spawnable variants
//! (`EnemyTable`) + attribute templates.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use indexmap::IndexMap;
use serde::Serialize;
use serde_json::Value;

use super::assets::{ModelInfo, model_info};
use super::operators::{AttrSeries, Coded, attr_series};
use super::tables::*;
use super::text::{Rich, TextRef, Texts};
use super::{BaseItem, BaseTag, Env, dep, finish_detail, num, sort_id};
use crate::api::error::ApiResult;

/// `displayType` codes in enum order (the index keys `DisplayEnemyTypeTable`).
const DISPLAY_TYPES: [&str; 5] = ["Normal", "Elite", "Boss", "HighLevel", "MiniBoss"];

pub struct Tables {
    pub templates: Arc<EnemyTemplateDisplayInfoTable>,
    pub enemies: Arc<EnemyTable>,
    pub displays: Arc<EnemyDisplayInfoTable>,
    pub attrs: Arc<EnemyAttributeTemplateTable>,
    pub abilities: Arc<EnemyAbilityDescTable>,
    pub tags: Arc<EnemyTagTable>,
    pub display_types: Arc<DisplayEnemyTypeTable>,
}

impl Tables {
    pub async fn load(c: &TableCache, dir: &Path) -> ApiResult<Self> {
        Ok(Self {
            templates: c.get(dir).await?,
            enemies: c.get_or_default(dir).await?,
            displays: c.get_or_default(dir).await?,
            attrs: c.get_or_default(dir).await?,
            abilities: c.get_or_default(dir).await?,
            tags: c.get_or_default(dir).await?,
            display_types: c.get_or_default(dir).await?,
        })
    }

    pub fn deps(&self) -> Vec<AnyArc> {
        vec![
            dep(&self.templates),
            dep(&self.tags),
            dep(&self.display_types),
        ]
    }

    fn display_type_name(&self, code: &str) -> TextRef {
        DISPLAY_TYPES
            .iter()
            .position(|c| *c == code)
            .and_then(|i| self.display_types.get(&i.to_string()))
            .map(|n| n.name)
            .unwrap_or_default()
    }
}

pub fn base(t: &Tables, env: &Env) -> Vec<BaseItem> {
    let mut items: Vec<BaseItem> = t
        .templates
        .iter()
        .map(|(id, e)| {
            let mut tags = vec![BaseTag::text(
                "displayType",
                &e.display_type,
                t.display_type_name(&e.display_type),
            )];
            for tag in &e.tags {
                tags.push(BaseTag::text(
                    "enemyTag",
                    tag,
                    t.tags.get(tag).map(|x| x.tag_text).unwrap_or_default(),
                ));
            }
            BaseItem {
                id: id.clone(),
                name: e.name,
                fallback_name: id.clone(),
                rarity: None,
                tags,
                icon: env.ui("monstericon", id),
                has_model: env.meshes.enemy(id).is_some()
                    || env.rig.find(&env.output, "enemies", id).is_some(),
                type_code: Some(e.display_type.clone()).filter(|s| !s.is_empty()),
            }
        })
        .collect();
    sort_id(&mut items);
    items
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AbilityOut {
    pub id: String,
    pub name: String,
    pub description: Rich,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VariantOut {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub attr_template_id: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AttrTemplateOut {
    /// Per level: `series.MaxHp[i]` at `level[i]`.
    pub levels: AttrSeries,
    pub fixed: IndexMap<String, Value>,
    pub resistances: IndexMap<&'static str, Value>,
    pub max_resilience: Value,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EnemyDetail {
    pub kind: &'static str,
    pub id: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nickname: Option<String>,
    pub description: Rich,
    pub display_type: Coded,
    pub tags: Vec<Coded>,
    pub abilities: Vec<AbilityOut>,
    pub variants: Vec<VariantOut>,
    /// Attribute templates referenced by `variants[].attrTemplateId`.
    pub attribute_templates: BTreeMap<String, AttrTemplateOut>,
    pub art: BTreeMap<&'static str, String>,
    pub model: ModelInfo,
}

fn attr_template(a: &EnemyAttrTemplate) -> AttrTemplateOut {
    let mut levels = attr_series(
        a.level_dependent_attributes
            .iter()
            .map(|l| (0u32, l.attrs.as_slice())),
    );
    levels.break_stage.clear();
    let fixed = a
        .level_independent_attributes
        .attrs
        .iter()
        .map(|x| (x.attr_type.clone(), num(x.attr_value)))
        .collect();
    let resistances = [
        ("physical", a.physical_resistance),
        ("fire", a.fire_resistance),
        ("cryst", a.cryst_resistance),
        ("pulse", a.pulse_resistance),
        ("natural", a.natural_resistance),
    ]
    .into_iter()
    .map(|(k, v)| (k, num(v)))
    .collect();
    AttrTemplateOut {
        levels,
        fixed,
        resistances,
        max_resilience: num(a.max_resilience),
    }
}

pub fn detail(t: &Tables, env: &Env, texts: &Texts, id: &str) -> Option<Value> {
    let e = t.templates.get(id)?;
    let abilities = e
        .ability_desc_ids
        .iter()
        .map(|aid| {
            let a = t.abilities.get(aid);
            AbilityOut {
                id: aid.clone(),
                name: a.map(|a| texts.plain(a.name)).unwrap_or_default(),
                description: a
                    .map(|a| texts.rich(a.description, None))
                    .unwrap_or_default(),
            }
        })
        .collect();
    let mut variants: Vec<VariantOut> = t
        .enemies
        .values()
        .filter(|v| v.template_id == id)
        .map(|v| VariantOut {
            id: v.enemy_id.clone(),
            name: t
                .displays
                .get(&v.enemy_id)
                .and_then(|d| texts.plain_opt(d.name)),
            attr_template_id: v.attr_template_id.clone(),
        })
        .collect();
    variants.sort_by(|a, b| (a.id.len() != id.len(), &a.id).cmp(&(b.id.len() != id.len(), &b.id)));
    let attribute_templates = variants
        .iter()
        .filter_map(|v| {
            t.attrs
                .get(&v.attr_template_id)
                .map(|a| (v.attr_template_id.clone(), attr_template(a)))
        })
        .collect();
    let tags = e
        .tags
        .iter()
        .map(|tag| Coded {
            code: tag.clone(),
            name: t
                .tags
                .get(tag)
                .map(|x| texts.plain(x.tag_text))
                .unwrap_or_default(),
            icon: None,
            color: None,
        })
        .collect();
    let mut art = BTreeMap::new();
    for (key, dir, name) in [
        ("icon", "monstericon", id.to_string()),
        ("iconBig", "monstericonbig", id.to_string()),
        ("adventureIcon", "monstericon", format!("adventure_{id}")),
    ] {
        if let Some(p) = env.ui(dir, &name) {
            art.insert(key, p);
        }
    }
    let d = EnemyDetail {
        kind: "enemies",
        id: id.to_string(),
        name: Some(texts.plain(e.name))
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| id.to_string()),
        nickname: texts.plain_opt(e.nickname),
        description: texts.rich(e.description, None),
        display_type: Coded {
            code: e.display_type.clone(),
            name: texts.plain(t.display_type_name(&e.display_type)),
            icon: None,
            color: None,
        },
        tags,
        abilities,
        variants,
        attribute_templates,
        art,
        model: model_info(&env.output, &env.rig, "enemies", id, env.meshes.enemy(id)),
    };
    Some(finish_detail(&d, texts))
}
