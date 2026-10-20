//! Items: `ItemTable` (~3k entries) with type names from `ItemTypeTable` and obtain-way
//! descriptions from `SystemJumpTable`.

use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::sync::Arc;

use serde::Serialize;
use serde_json::Value;

use super::operators::Coded;
use super::tables::*;
use super::text::{Rich, TextRef, Texts};
use super::{BaseItem, BaseTag, Env, dep, finish_detail, sort_id};
use crate::api::error::ApiResult;

pub struct Tables {
    pub items: Arc<ItemTable>,
    pub types: Arc<ItemTypeTable>,
    pub jumps: Arc<SystemJumpTable>,
    pub weapons: Arc<WeaponBasicTable>,
}

impl Tables {
    pub async fn load(c: &TableCache, dir: &Path) -> ApiResult<Self> {
        Ok(Self {
            items: c.get(dir).await?,
            types: c.get_or_default(dir).await?,
            jumps: c.get_or_default(dir).await?,
            weapons: c.get_or_default(dir).await?,
        })
    }

    pub fn deps(&self) -> Vec<AnyArc> {
        vec![dep(&self.items), dep(&self.types)]
    }

    fn type_names(&self) -> HashMap<&str, TextRef> {
        self.types
            .values()
            .map(|t| (t.item_type.as_str(), t.name))
            .collect()
    }
}

pub fn base(t: &Tables, env: &Env) -> Vec<BaseItem> {
    let names = t.type_names();
    let mut items: Vec<BaseItem> = t
        .items
        .iter()
        .map(|(id, it)| BaseItem {
            id: id.clone(),
            name: it.name,
            fallback_name: id.clone(),
            rarity: Some(it.rarity).filter(|r| *r > 0),
            tags: vec![BaseTag::text(
                "itemType",
                &it.kind,
                names.get(it.kind.as_str()).copied().unwrap_or_default(),
            )],
            icon: env.ui("itemicon", &it.icon_id),
            has_model: false,
            type_code: Some(it.kind.clone()).filter(|s| !s.is_empty()),
        })
        .collect();
    sort_id(&mut items);
    items
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ObtainOut {
    pub id: String,
    pub description: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub system: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Related {
    pub kind: &'static str,
    pub id: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ItemDetail {
    pub kind: &'static str,
    pub id: String,
    pub name: String,
    pub description: Rich,
    pub flavor: Rich,
    pub rarity: u8,
    pub item_type: Coded,
    pub obtain_ways: Vec<ObtainOut>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub no_obtain_hint: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub related: Option<Related>,
    pub art: BTreeMap<&'static str, String>,
}

pub fn detail(t: &Tables, env: &Env, texts: &Texts, id: &str) -> Option<Value> {
    let it = t.items.get(id)?;
    let names = t.type_names();
    let obtain_ways = it
        .obtain_way_ids
        .iter()
        .map(|w| {
            let j = t.jumps.get(w);
            ObtainOut {
                id: w.clone(),
                description: j.map(|j| texts.plain(j.desc)).unwrap_or_default(),
                system: j
                    .map(|j| j.bind_system.clone())
                    .filter(|s| s != "None")
                    .unwrap_or_default(),
            }
        })
        .collect();
    let related = if t.weapons.contains_key(id) {
        Some(Related {
            kind: "weapons",
            id: id.to_string(),
        })
    } else if it.kind == "Char" && id.starts_with("chr_") {
        Some(Related {
            kind: "operators",
            id: id.to_string(),
        })
    } else {
        None
    };
    let mut art = BTreeMap::new();
    for (key, dir) in [("icon", "itemicon"), ("iconBig", "itemiconbig")] {
        if let Some(p) = env.ui(dir, &it.icon_id) {
            art.insert(key, p);
        }
    }
    let d = ItemDetail {
        kind: "items",
        id: id.to_string(),
        name: Some(texts.plain(it.name))
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| id.to_string()),
        description: texts.rich(it.desc, None),
        flavor: texts.rich(it.deco_desc, None),
        rarity: it.rarity,
        item_type: Coded {
            code: it.kind.clone(),
            name: names
                .get(it.kind.as_str())
                .map(|r| texts.plain(*r))
                .unwrap_or_default(),
            icon: None,
            color: None,
        },
        obtain_ways,
        no_obtain_hint: texts.plain_opt(it.no_obtain_way_hint),
        related,
        art,
    };
    Some(finish_detail(&d, texts))
}
