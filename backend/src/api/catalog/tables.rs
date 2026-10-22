//! Typed views of the SparkBuffer JSON tables the catalogs read, and a cache that parses
//! each table once (on a blocking thread) and re-parses it when its size or mtime changes.
//!
//! Only the fields the catalogs use are declared; serde skips the rest, which keeps the
//! resident size far below the raw JSON (CharacterTable is 12.5 MB on disk).

use std::any::Any;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Instant, SystemTime};

use serde::Deserialize;
use serde::de::DeserializeOwned;

use super::text::{BbEntry, TextRef};
use crate::api::error::{ApiError, ApiResult};

/// A table file `<tables>/<NAME>.json` deserializable as `Self`.
pub trait Table: DeserializeOwned + Send + Sync + 'static {
    const NAME: &'static str;
}

macro_rules! map_table {
    ($ty:ident, $name:literal, $entry:ty) => {
        #[derive(Debug, Default, Deserialize)]
        #[serde(transparent)]
        pub struct $ty(pub HashMap<String, $entry>);
        impl Table for $ty {
            const NAME: &'static str = $name;
        }
        impl std::ops::Deref for $ty {
            type Target = HashMap<String, $entry>;
            fn deref(&self) -> &Self::Target {
                &self.0
            }
        }
    };
}

pub type AnyArc = Arc<dyn Any + Send + Sync>;

type Stamp = (u64, SystemTime);

struct Slot {
    stamp: Stamp,
    value: AnyArc,
}

/// Parsed tables keyed by name. Each table has its own async lock so a slow parse of one
/// table never blocks reads of another, and concurrent requests parse a table once.
#[derive(Default)]
pub struct TableCache {
    slots: std::sync::Mutex<HashMap<&'static str, Arc<tokio::sync::Mutex<Option<Slot>>>>>,
}

fn stamp(path: &Path) -> std::io::Result<Stamp> {
    let meta = std::fs::metadata(path)?;
    Ok((
        meta.len(),
        meta.modified().unwrap_or(SystemTime::UNIX_EPOCH),
    ))
}

impl TableCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// The parsed table, loading or reloading it if needed. Returns it type-erased too so
    /// derived caches can depend on its identity.
    pub async fn get<T: Table>(&self, dir: &Path) -> ApiResult<Arc<T>> {
        let any = self.get_any::<T>(dir).await?;
        any.downcast::<T>()
            .map_err(|_| ApiError::internal(format!("table {} type mismatch", T::NAME)))
    }

    /// Like [`get`](Self::get) but a missing table yields an empty default.
    pub async fn get_or_default<T: Table + Default>(&self, dir: &Path) -> ApiResult<Arc<T>> {
        match self.get::<T>(dir).await {
            Ok(t) => Ok(t),
            Err(e) if e.status == axum::http::StatusCode::NOT_FOUND => Ok(Arc::new(T::default())),
            Err(e) => Err(e),
        }
    }

    async fn get_any<T: Table>(&self, dir: &Path) -> ApiResult<AnyArc> {
        let path: PathBuf = dir.join(format!("{}.json", T::NAME));
        let slot = {
            let mut slots = self.slots.lock().unwrap_or_else(|e| e.into_inner());
            slots.entry(T::NAME).or_default().clone()
        };
        let mut slot = slot.lock().await;
        let p = path.clone();
        let current = tokio::task::spawn_blocking(move || stamp(&p))
            .await
            .map_err(|e| ApiError::internal(e.to_string()))?
            .map_err(|e| ApiError::io(e, &format!("table {}", T::NAME)))?;
        if let Some(s) = slot.as_ref()
            && s.stamp == current
        {
            return Ok(s.value.clone());
        }
        let started = Instant::now();
        let value: AnyArc = tokio::task::spawn_blocking(move || -> ApiResult<AnyArc> {
            let bytes =
                std::fs::read(&path).map_err(|e| ApiError::io(e, &format!("table {}", T::NAME)))?;
            let parsed: T = serde_json::from_slice(&bytes)
                .map_err(|e| ApiError::internal(format!("table {}: {e}", T::NAME)))?;
            Ok(Arc::new(parsed))
        })
        .await
        .map_err(|e| ApiError::internal(e.to_string()))??;
        tracing::info!(
            "loaded table {} in {} ms",
            T::NAME,
            started.elapsed().as_millis()
        );
        *slot = Some(Slot {
            stamp: current,
            value: value.clone(),
        });
        Ok(value)
    }
}

// ── Shared shapes ────────────────────────────────────────────────────────

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Attr {
    #[serde(default)]
    pub attr_type: String,
    #[serde(default)]
    pub attr_value: f64,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct AttrList {
    #[serde(default)]
    pub attrs: Vec<Attr>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct ItemCount {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub count: i64,
}

// ── Operators ────────────────────────────────────────────────────────────

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Character {
    pub char_id: String,
    pub name: TextRef,
    pub eng_name: String,
    pub rarity: u8,
    pub profession: String,
    pub char_type_id: String,
    pub weapon_type: String,
    pub department: String,
    pub main_attr_type: String,
    pub sub_attr_type: String,
    pub default_weapon_id: String,
    pub cv_name: CvName,
    pub char_battle_tag_ids: Vec<String>,
    pub attributes: Vec<CharAttrRow>,
    pub profile_record: Vec<ProfileRecord>,
    pub profile_voice: Vec<ProfileVoice>,
    pub sort_order: i64,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct CvName {
    pub chi_c_v_name: TextRef,
    pub eng_c_v_name: TextRef,
    pub jap_c_v_name: TextRef,
    pub kor_c_v_name: TextRef,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct CharAttrRow {
    #[serde(rename = "breakStage")]
    pub break_stage: u32,
    #[serde(rename = "Attribute")]
    pub attribute: AttrList,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ProfileRecord {
    pub id: String,
    pub record_title: TextRef,
    pub record_desc: TextRef,
    pub unlock_type: String,
    pub unlock_value: i64,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ProfileVoice {
    pub id: String,
    pub vo_id: String,
    pub voice_title: TextRef,
    pub voice_desc: TextRef,
    pub unlock_type: String,
    pub unlock_value: i64,
}

map_table!(CharacterTable, "CharacterTable", Character);

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CharGrowth {
    pub char_break_cost_map: HashMap<String, BreakNode>,
    pub skill_group_map: HashMap<String, SkillGroup>,
    pub talent_node_map: HashMap<String, TalentNode>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct BreakNode {
    pub node_id: String,
    pub node_type: String,
    pub break_stage: u32,
    pub name: TextRef,
    pub description: TextRef,
    pub required_item: Vec<ItemCount>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SkillGroup {
    pub skill_group_id: String,
    pub skill_group_type: String,
    pub name: TextRef,
    pub desc: TextRef,
    pub icon: String,
    pub skill_id_list: Vec<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct TalentNode {
    pub node_id: String,
    pub node_type: String,
    pub attribute_node_info: AttributeNodeInfo,
    pub passive_skill_node_info: PassiveSkillNodeInfo,
    pub factory_skill_node_info: FactorySkillNodeInfo,
    pub required_item: Vec<ItemCount>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AttributeNodeInfo {
    pub attribute_modifiers: Vec<AttrModifier>,
    pub break_stage: u32,
    pub desc: TextRef,
    pub title: TextRef,
    pub favorability: i64,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AttrModifier {
    pub attr_type: String,
    pub attr_value: f64,
    pub modifier_type: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct PassiveSkillNodeInfo {
    pub break_stage: u32,
    pub icon_id: String,
    pub index: u32,
    pub level: u32,
    pub name: TextRef,
    pub talent_effect_id: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct FactorySkillNodeInfo {
    pub break_stage: u32,
    pub index: u32,
    pub level: u32,
}

map_table!(CharGrowthTable, "CharGrowthTable", CharGrowth);

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct SkillPatch {
    #[serde(rename = "SkillPatchDataBundle")]
    pub levels: Vec<SkillLevel>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SkillLevel {
    pub level: u32,
    pub blackboard: Vec<BbEntry>,
    pub cool_down: f64,
    pub cost_type: String,
    pub cost_value: f64,
    pub description: TextRef,
    pub skill_name: TextRef,
    pub icon_id: String,
    pub sub_desc_data_list: Vec<SubDesc>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct SubDesc {
    pub name: TextRef,
    pub desc: String,
}

map_table!(SkillPatchTable, "SkillPatchTable", SkillPatch);

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct PotentialTalentEffect {
    pub desc: TextRef,
    pub data_list: Vec<EffectData>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct EffectData {
    pub modify_type: String,
    pub attach_buff: BbHolder,
    pub attach_skill: BbHolder,
    pub attr_modifier: AttrModifier,
    pub skill_bb_modifier: SkillBbModifier,
    pub skill_param_modifier: SkillParamModifier,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SkillBbModifier {
    pub bb_key: String,
    pub float_value: f64,
    pub string_value: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SkillParamModifier {
    pub param_type: String,
    pub param_value: f64,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct BbHolder {
    pub blackboard: Vec<BbEntry>,
}

map_table!(
    PotentialTalentEffectTable,
    "PotentialTalentEffectTable",
    PotentialTalentEffect
);

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CharPotentials {
    pub potential_unlock_bundle: Vec<PotentialUnlock>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct PotentialUnlock {
    pub level: u32,
    pub name: TextRef,
    pub potential_effect_id: String,
    pub item_ids: Vec<String>,
    pub item_cnts: Vec<i64>,
}

map_table!(
    CharacterPotentialTable,
    "CharacterPotentialTable",
    CharPotentials
);

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Profession {
    pub profession: String,
    pub name: TextRef,
    pub desc: TextRef,
    pub icon_id: String,
}

map_table!(CharProfessionTable, "CharProfessionTable", Profession);

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CharType {
    pub char_type_id: String,
    pub name: TextRef,
    pub color: String,
    pub icon: String,
}

map_table!(CharTypeTable, "CharTypeTable", CharType);

map_table!(CharBattleTagTable, "CharBattleTagTable", TextRef);

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ActorImage {
    pub avatar_path: String,
    pub bust_path: String,
}

map_table!(ActorImageTable, "ActorImageTable", ActorImage);

// ── Text helpers ─────────────────────────────────────────────────────────

// `TextTable`: string key → i18n id.
map_table!(TextTable, "TextTable", TextRef);

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct RichTextStyle {
    pub pre_def: Vec<String>,
}

map_table!(RichTextStyleTable, "RichTextStyleTable", RichTextStyle);

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Hyperlink {
    pub name: TextRef,
    pub desc: TextRef,
    pub icon_path: String,
    pub rich_text_id: String,
}

map_table!(HyperlinkTextTable, "HyperlinkTextTable", Hyperlink);

// ── Weapons ──────────────────────────────────────────────────────────────

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Weapon {
    pub weapon_id: String,
    pub eng_name: TextRef,
    pub weapon_desc: TextRef,
    pub rarity: u8,
    pub weapon_type: String,
    pub max_lv: u32,
    pub level_template_id: String,
    pub breakthrough_template_id: String,
    pub weapon_skill_list: Vec<String>,
    pub weapon_potential_skill: String,
}

map_table!(WeaponBasicTable, "WeaponBasicTable", Weapon);

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct WeaponUpgrade {
    pub list: Vec<WeaponLevel>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct WeaponLevel {
    pub weapon_lv: u32,
    pub base_atk: f64,
}

map_table!(
    WeaponUpgradeTemplateTable,
    "WeaponUpgradeTemplateTable",
    WeaponUpgrade
);

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct WeaponBreakthrough {
    pub list: Vec<WeaponBreakStage>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct WeaponBreakStage {
    pub breakthrough_lv: u32,
    pub breakthrough_show_lv: u32,
    pub break_item_list: Vec<ItemCount>,
    pub breakthrough_gold: i64,
}

map_table!(
    WeaponBreakThroughTemplateTable,
    "WeaponBreakThroughTemplateTable",
    WeaponBreakthrough
);

// ── Enemies ──────────────────────────────────────────────────────────────

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct EnemyTemplate {
    pub template_id: String,
    pub name: TextRef,
    pub nickname: TextRef,
    pub description: TextRef,
    pub display_type: String,
    pub ability_desc_ids: Vec<String>,
    pub tags: Vec<String>,
}

map_table!(
    EnemyTemplateDisplayInfoTable,
    "EnemyTemplateDisplayInfoTable",
    EnemyTemplate
);

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Enemy {
    pub enemy_id: String,
    pub template_id: String,
    pub attr_template_id: String,
}

map_table!(EnemyTable, "EnemyTable", Enemy);

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct EnemyDisplay {
    pub name: TextRef,
    pub description: TextRef,
}

map_table!(EnemyDisplayInfoTable, "EnemyDisplayInfoTable", EnemyDisplay);

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct EnemyAttrTemplate {
    pub level_dependent_attributes: Vec<AttrList>,
    pub level_independent_attributes: AttrList,
    pub physical_resistance: f64,
    pub fire_resistance: f64,
    pub cryst_resistance: f64,
    pub pulse_resistance: f64,
    pub natural_resistance: f64,
    pub max_resilience: f64,
}

map_table!(
    EnemyAttributeTemplateTable,
    "EnemyAttributeTemplateTable",
    EnemyAttrTemplate
);

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct EnemyAbility {
    pub name: TextRef,
    pub description: TextRef,
}

map_table!(EnemyAbilityDescTable, "EnemyAbilityDescTable", EnemyAbility);

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct EnemyTag {
    pub tag_text: TextRef,
}

map_table!(EnemyTagTable, "EnemyTagTable", EnemyTag);

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct NameOnly {
    pub name: TextRef,
}

// Keyed `"0".."4"` in `displayType` enum order.
map_table!(DisplayEnemyTypeTable, "DisplayEnemyTypeTable", NameOnly);

// ── Items ────────────────────────────────────────────────────────────────

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Item {
    pub id: String,
    pub name: TextRef,
    pub desc: TextRef,
    pub deco_desc: TextRef,
    pub no_obtain_way_hint: TextRef,
    pub rarity: u8,
    #[serde(rename = "type")]
    pub kind: String,
    pub icon_id: String,
    pub obtain_way_ids: Vec<String>,
    pub sort_id1: i64,
    pub sort_id2: i64,
    pub max_stack_count: i64,
}

map_table!(ItemTable, "ItemTable", Item);

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ItemType {
    pub item_type: String,
    pub name: TextRef,
}

// Keyed by an index; `itemType` is the code used by `ItemTable.type`.
map_table!(ItemTypeTable, "ItemTypeTable", ItemType);

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SystemJump {
    pub desc: TextRef,
    pub icon_id: String,
    pub bind_system: String,
}

map_table!(SystemJumpTable, "SystemJumpTable", SystemJump);

// ── NPCs ─────────────────────────────────────────────────────────────────

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Npc {
    pub npc_id: String,
    pub name: TextRef,
    pub title: TextRef,
    pub faction: TextRef,
    pub data_key: String,
    pub npc_group_id: String,
}

map_table!(NpcTable, "NpcTable", Npc);

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct NpcPrefabChar {
    pub template_id: String,
    pub corresponding_char_id: String,
}

// Keyed by `templateId` (an NPC's `dataKey`, e.g. `npc_chr_0004_pelica`).
map_table!(
    NpcPrefabCorrespondingCharTable,
    "NpcPrefabCorrespondingCharTable",
    NpcPrefabChar
);

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SnsChat {
    pub chat_id: String,
    pub icon: String,
}

// Keyed by `chatId`, which matches an NPC's `npcGroupId` for one-on-one chats.
map_table!(SnsChatTable, "SNSChatTable", SnsChat);

// ── Facilities ───────────────────────────────────────────────────────────

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Building {
    pub id: String,
    pub name: TextRef,
    pub desc: TextRef,
    #[serde(rename = "type")]
    pub kind: String,
    pub icon_on_panel: String,
    pub power_consume: f64,
    pub need_power: bool,
    pub bandwidth: f64,
    pub range: BuildingRange,
    pub model_height: f64,
    pub input_ports: Vec<serde::de::IgnoredAny>,
    pub output_ports: Vec<serde::de::IgnoredAny>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct BuildingRange {
    pub width: f64,
    pub depth: f64,
    pub height: f64,
}

map_table!(FactoryBuildingTable, "FactoryBuildingTable", Building);

// ── Dialog ───────────────────────────────────────────────────────────────

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct DialogLine {
    pub actor_name: TextRef,
    pub actor_name_id: String,
    pub dialog_text: TextRef,
    pub emotion_type: String,
    pub audio_override: String,
}

map_table!(DialogTextTable, "DialogTextTable", DialogLine);

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct DialogOption {
    pub option_text: TextRef,
    pub icon_type: String,
}

map_table!(DialogOptionTable, "DialogOptionTable", DialogOption);

map_table!(DialogSummaryTable, "DialogSummaryTable", TextRef);
