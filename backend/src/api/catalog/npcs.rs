//! NPCs: `NpcTable` (names, titles, factions) with mesh folders from `dataKey`.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use serde::Serialize;
use serde_json::Value;

use super::assets::{self, Files, ModelInfo, UI, model_info};
use super::tables::*;
use super::text::Texts;
use super::{BaseItem, BaseTag, Env, TagName, dep, finish_detail, sort_id};
use crate::api::error::ApiResult;

pub struct Tables {
    pub npcs: Arc<NpcTable>,
    pub prefab_chars: Arc<NpcPrefabCorrespondingCharTable>,
    pub sns_chats: Arc<SnsChatTable>,
}

impl Tables {
    pub async fn load(c: &TableCache, dir: &Path) -> ApiResult<Self> {
        Ok(Self {
            npcs: c.get(dir).await?,
            prefab_chars: c.get_or_default(dir).await?,
            sns_chats: c.get_or_default(dir).await?,
        })
    }

    pub fn deps(&self) -> Vec<AnyArc> {
        vec![
            dep(&self.npcs),
            dep(&self.prefab_chars),
            dep(&self.sns_chats),
        ]
    }
}

/// NPC portrait sprites keyed by lowercased name token, built once per listing.
struct NpcIcons {
    /// `charicon/icon_npc_<n>_<name>` → its sprite path.
    charicon: HashMap<String, String>,
    /// `charremoteicon/` then `charroundicon/` sprites, the first dir winning per name.
    others: HashMap<String, String>,
}

impl NpcIcons {
    fn new(env: &Env) -> Self {
        let mut others = HashMap::new();
        for dir in ["charremoteicon", "charroundicon"] {
            for (name, path) in npc_icon_names(env, dir) {
                others.entry(name).or_insert(path);
            }
        }
        Self {
            charicon: npc_icon_names(env, "charicon").into_iter().collect(),
            others,
        }
    }

    /// The sprite for a name token, preferring `charicon/`.
    fn get(&self, name: &str) -> Option<String> {
        self.charicon
            .get(name)
            .or_else(|| self.others.get(name))
            .cloned()
    }
}

/// Portrait for an NPC, first match wins:
/// 1. `charicon/icon_npc_<id>` or `icon_npc_<dataKey>`, an `icon_npc_<n>_<name>` sprite
///    matching the id's name token, or the operator icon for operator stand-ins
///    (`npc_0005_chen_spaceship_i001` → `icon_chr_0005_chen`);
/// 2. the operator icon from `NpcPrefabCorrespondingCharTable` for the `dataKey`
///    (`npc_chr_0004_pelica` → `icon_chr_0004_pelica`);
/// 3. the `SNSChatTable` avatar in `charroundicon/` for the `npcGroupId`;
/// 4. a sprite in `charicon/`, `charremoteicon/` or `charroundicon/` named after the id's
///    name token, the whole id (`kelao`), or the name in an `npc_spl_<name>_<nn>` dataKey.
fn npc_icon(t: &Tables, env: &Env, icons: &NpcIcons, id: &str, n: &Npc) -> Option<String> {
    let data_key = n.data_key.as_str();
    let charicon = |name: &str| env.ui("charicon", name);
    let parts: Vec<&str> = id.split('_').collect();
    // `npc_<n>_<name>_…` → (`<n>`, `<name>`).
    let numbered = match parts.as_slice() {
        [prefix, num, name, ..] if *prefix == "npc" && num.chars().all(|c| c.is_ascii_digit()) => {
            Some((*num, *name))
        }
        _ => None,
    };
    let spl_name = data_key
        .strip_prefix("npc_spl_")
        .and_then(|r| r.rsplit_once('_'))
        .filter(|(_, nn)| nn.chars().all(|c| c.is_ascii_digit()))
        .map(|(name, _)| name);
    charicon(&format!("icon_npc_{id}"))
        .or_else(|| {
            Some(data_key)
                .filter(|k| !k.is_empty())
                .and_then(|k| charicon(&format!("icon_npc_{k}")))
        })
        .or_else(|| {
            let (num, name) = numbered?;
            icons
                .charicon
                .get(&name.to_ascii_lowercase())
                .cloned()
                .or_else(|| charicon(&format!("icon_chr_{num}_{name}")))
        })
        .or_else(|| {
            let c = t.prefab_chars.get(data_key)?;
            charicon(&format!("icon_{}", c.corresponding_char_id))
        })
        .or_else(|| {
            let chat = t.sns_chats.get(&n.npc_group_id)?;
            env.ui("charroundicon", &chat.icon)
        })
        .or_else(|| {
            [numbered.map(|(_, name)| name), Some(id), spl_name]
                .into_iter()
                .flatten()
                .find_map(|tok| icons.get(&tok.to_ascii_lowercase()))
        })
}

/// NPC sprites in `<dir>/` by name token: `icon_npc_1001_andrew` → `andrew` and
/// `icon_sns_npc_andrew_01` → `andrew`, each with its sprite path.
fn npc_icon_names(env: &Env, dir: &str) -> Vec<(String, String)> {
    env.textures
        .entries(&format!("{UI}/{dir}"))
        .into_iter()
        .filter_map(|(f, _)| {
            let stem = f.strip_suffix(".png")?;
            let name = if let Some(rest) = stem.strip_prefix("icon_sns_npc_") {
                let (name, nn) = rest.rsplit_once('_')?;
                nn.chars().all(|c| c.is_ascii_digit()).then_some(name)?
            } else {
                stem.strip_prefix("icon_npc_")?.rsplit('_').next()?
            };
            Some((name.to_ascii_lowercase(), format!("{UI}/{dir}/{f}")))
        })
        .collect()
}

pub fn base(t: &Tables, env: &Env) -> Vec<BaseItem> {
    let icons = NpcIcons::new(env);
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
                icon: npc_icon(t, env, &icons, id, n),
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
        icon: npc_icon(t, env, &NpcIcons::new(env), id, n),
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
