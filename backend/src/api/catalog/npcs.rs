//! NPCs: `NpcTable` (names, titles, factions) with mesh folders from `dataKey`.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use serde::Serialize;
use serde_json::Value;

use super::assets::{self, ModelInfo, UI, model_info};
use super::tables::*;
use super::text::Texts;
use super::{BaseItem, BaseTag, Env, TagName, dep, finish_detail, sort_id};
use crate::api::error::ApiResult;

pub struct Tables {
    pub npcs: Arc<NpcTable>,
}

impl Tables {
    pub async fn load(c: &TableCache, dir: &Path) -> ApiResult<Self> {
        Ok(Self {
            npcs: c.get(dir).await?,
        })
    }

    pub fn deps(&self) -> Vec<AnyArc> {
        vec![dep(&self.npcs)]
    }
}

/// Portrait for an NPC from `charicon/`: `icon_npc_<id>` or `icon_npc_<dataKey>`, an
/// `icon_npc_<n>_<name>` sprite matching the id's name token, or the operator icon
/// for operator stand-ins (`npc_0005_chen_spaceship_i001` → `icon_chr_0005_chen`).
fn npc_icon(
    env: &Env,
    npc_names: &HashMap<String, String>,
    id: &str,
    data_key: &str,
) -> Option<String> {
    let mut names = vec![format!("icon_npc_{id}")];
    if !data_key.is_empty() {
        names.push(format!("icon_npc_{data_key}"));
    }
    let parts: Vec<&str> = id.split('_').collect();
    if let [prefix, num, name, ..] = parts.as_slice()
        && *prefix == "npc"
        && num.chars().all(|c| c.is_ascii_digit())
    {
        if let Some(file) = npc_names.get(&name.to_ascii_lowercase()) {
            names.push(file.clone());
        }
        names.push(format!("icon_chr_{num}_{name}"));
    }
    names.iter().find_map(|n| env.ui("charicon", n))
}

/// `icon_npc_1001_andrew` → `andrew`, for every NPC sprite in `charicon/`.
fn npc_icon_names(env: &Env) -> HashMap<String, String> {
    let dir = env.textures.join(UI).join("charicon");
    let Ok(rd) = std::fs::read_dir(dir) else {
        return HashMap::new();
    };
    rd.flatten()
        .filter_map(|e| e.file_name().into_string().ok())
        .filter_map(|f| {
            let stem = f.strip_suffix(".png")?.strip_prefix("icon_npc_")?;
            let name = stem.rsplit('_').next()?;
            Some((name.to_ascii_lowercase(), format!("icon_npc_{stem}")))
        })
        .collect()
}

pub fn base(t: &Tables, env: &Env) -> Vec<BaseItem> {
    let npc_names = npc_icon_names(env);
    let mut items: Vec<BaseItem> = t
        .npcs
        .iter()
        .map(|(id, n)| {
            let mut tags = Vec::new();
            if !n.title.is_empty() {
                tags.push(BaseTag {
                    kind: "title",
                    code: n.title.key(),
                    name: TagName::Text(n.title),
                });
            }
            if !n.faction.is_empty() {
                tags.push(BaseTag {
                    kind: "faction",
                    code: n.faction.key(),
                    name: TagName::Text(n.faction),
                });
            }
            BaseItem {
                id: id.clone(),
                name: n.name,
                fallback_name: id.clone(),
                rarity: None,
                tags,
                icon: npc_icon(env, &npc_names, id, &n.data_key),
                has_model: env.meshes.npc(&n.data_key).is_some()
                    || env.rig.find_npc(&env.output, id, &n.data_key).is_some(),
                type_code: None,
            }
        })
        .collect();
    sort_id(&mut items);
    items
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NpcDetail {
    pub kind: &'static str,
    pub id: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub faction: Option<String>,
    pub data_key: String,
    pub group_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    pub model: ModelInfo,
}

pub fn detail(t: &Tables, env: &Env, texts: &Texts, id: &str) -> Option<Value> {
    let n = t.npcs.get(id)?;
    let d = NpcDetail {
        kind: "npcs",
        id: id.to_string(),
        name: Some(texts.plain(n.name))
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| id.to_string()),
        title: texts.plain_opt(n.title),
        faction: texts.plain_opt(n.faction),
        data_key: n.data_key.clone(),
        group_id: n.npc_group_id.clone(),
        icon: npc_icon(env, &npc_icon_names(env), id, &n.data_key),
        model: {
            let rigged = env.rig.find_npc(&env.output, id, &n.data_key);
            ModelInfo {
                clips: assets::clips_index(&env.output, rigged.as_deref()),
                rigged,
                ..model_info(
                    &env.output,
                    &env.rig,
                    "npcs",
                    id,
                    env.meshes.npc(&n.data_key),
                )
            }
        },
    };
    Some(finish_detail(&d, texts))
}
