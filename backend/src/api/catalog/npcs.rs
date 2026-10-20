//! NPCs: `NpcTable` (names, titles, factions) with mesh folders from `dataKey`.

use std::path::Path;
use std::sync::Arc;

use serde::Serialize;
use serde_json::Value;

use super::assets::{ModelInfo, model_info};
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

pub fn base(t: &Tables, env: &Env) -> Vec<BaseItem> {
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
                icon: None,
                has_model: env.meshes.npc(&n.data_key).is_some(),
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
        model: model_info(
            &env.output,
            &env.rig,
            "npcs",
            id,
            env.meshes.npc(&n.data_key),
        ),
    };
    Some(finish_detail(&d, texts))
}
