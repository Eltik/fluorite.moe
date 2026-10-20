//! Facilities: `FactoryBuildingTable` (AIC buildings).

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use serde::Serialize;
use serde_json::Value;

use super::tables::*;
use super::text::{Rich, Texts};
use super::{BaseItem, BaseTag, Env, TagName, dep, finish_detail, num, sort_id};
use crate::api::error::ApiResult;

pub struct Tables {
    pub buildings: Arc<FactoryBuildingTable>,
}

impl Tables {
    pub async fn load(c: &TableCache, dir: &Path) -> ApiResult<Self> {
        Ok(Self {
            buildings: c.get(dir).await?,
        })
    }

    pub fn deps(&self) -> Vec<AnyArc> {
        vec![dep(&self.buildings)]
    }
}

pub fn base(t: &Tables, env: &Env) -> Vec<BaseItem> {
    let mut items: Vec<BaseItem> = t
        .buildings
        .iter()
        .map(|(id, b)| BaseItem {
            id: id.clone(),
            name: b.name,
            fallback_name: id.clone(),
            rarity: None,
            tags: vec![BaseTag {
                kind: "buildingType",
                code: b.kind.clone(),
                name: TagName::Literal(b.kind.clone()),
            }],
            icon: env.ui("factory/buildingpanelicon", &b.icon_on_panel),
            has_model: false,
            type_code: Some(b.kind.clone()).filter(|s| !s.is_empty()),
        })
        .collect();
    sort_id(&mut items);
    items
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Footprint {
    pub width: Value,
    pub depth: Value,
    pub height: Value,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FacilityDetail {
    pub kind: &'static str,
    pub id: String,
    pub name: String,
    pub description: Rich,
    #[serde(rename = "type")]
    pub building_type: String,
    pub power_consume: Value,
    pub need_power: bool,
    pub bandwidth: Value,
    pub range: Footprint,
    pub input_ports: usize,
    pub output_ports: usize,
    pub art: BTreeMap<&'static str, String>,
}

pub fn detail(t: &Tables, env: &Env, texts: &Texts, id: &str) -> Option<Value> {
    let b = t.buildings.get(id)?;
    let mut art = BTreeMap::new();
    for (key, dir, name) in [
        ("icon", "factory/buildingpanelicon", b.icon_on_panel.clone()),
        (
            "iconBig",
            "factory/buildingpaneliconbig",
            b.icon_on_panel.clone(),
        ),
        ("image", "factory/buildingimage", format!("image_{id}")),
    ] {
        if let Some(p) = env.ui(dir, &name) {
            art.insert(key, p);
        }
    }
    let d = FacilityDetail {
        kind: "facilities",
        id: id.to_string(),
        name: Some(texts.plain(b.name))
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| id.to_string()),
        description: texts.rich(b.desc, None),
        building_type: b.kind.clone(),
        power_consume: num(b.power_consume),
        need_power: b.need_power,
        bandwidth: num(b.bandwidth),
        range: Footprint {
            width: num(b.range.width),
            depth: num(b.range.depth),
            height: num(b.range.height),
        },
        input_ports: b.input_ports.len(),
        output_ports: b.output_ports.len(),
        art,
    };
    Some(finish_detail(&d, texts))
}
