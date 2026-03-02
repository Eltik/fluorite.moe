//! IL2CPP Metadata + Character Data Companion
//!
//! Pass 1: Parse IL2CPP dump JSON (from frida_il2cpp_scan.js)
//! Pass 2: Rank classes by "stat score" — identify hook candidates
//! Pass 3: Parse capture log (from frida_char_data.js)
//! Pass 4: Correlate captured method calls with class metadata
//! Pass 5: Extract & group character stats

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;

// ══════════════════════════════════════════════════════════════
// Data structures — IL2CPP dump
// ══════════════════════════════════════════════════════════════

#[derive(Debug, Deserialize)]
struct Il2cppDump {
    meta: DumpMeta,
    matched: Vec<ClassEntry>,
    #[serde(rename = "signatureHits")]
    signature_hits: Vec<ClassEntry>,
    #[serde(rename = "classIndex")]
    class_index: Vec<ClassIndexEntry>,
}

#[derive(Debug, Deserialize)]
#[expect(dead_code)]
struct DumpMeta {
    timestamp: String,
    #[serde(rename = "processId")]
    process_id: u64,
    arch: String,
    #[serde(rename = "gameAssemblyBase")]
    game_assembly_base: String,
    #[serde(rename = "gameAssemblySize")]
    game_assembly_size: u64,
    #[serde(rename = "totalAssemblies")]
    total_assemblies: u32,
    #[serde(rename = "totalClasses")]
    total_classes: u32,
    #[serde(rename = "totalMethods")]
    total_methods: u32,
    #[serde(rename = "totalFields")]
    total_fields: u32,
    #[serde(rename = "matchedClasses")]
    matched_classes: u32,
    #[serde(rename = "signatureHits")]
    signature_hits_count: u32,
}

#[derive(Debug, Deserialize, Clone)]
#[expect(dead_code)]
struct ClassEntry {
    name: String,
    namespace: String,
    image: String,
    #[serde(default)]
    parent: String,
    #[serde(default)]
    fields: Vec<FieldEntry>,
    #[serde(default)]
    methods: Vec<MethodEntry>,
    #[serde(rename = "numericFieldCount", default)]
    numeric_field_count: u32,
    #[serde(rename = "matchTier", default)]
    match_tier: u32,
    #[serde(rename = "matchReason", default)]
    match_reason: String,
    #[serde(rename = "isSignatureHit", default)]
    is_signature_hit: bool,
}

#[derive(Debug, Deserialize, Clone)]
#[expect(dead_code)]
struct FieldEntry {
    name: String,
    offset: i32,
    #[serde(rename = "typeEnum")]
    type_enum: i32,
    #[serde(rename = "typeName")]
    type_name: String,
    #[serde(rename = "isStatic")]
    is_static: bool,
    #[serde(rename = "isNumeric")]
    is_numeric: bool,
}

#[derive(Debug, Deserialize, Clone)]
struct MethodEntry {
    name: String,
    #[serde(rename = "paramCount")]
    param_count: i32,
    rva: u64,
    #[serde(rename = "rvaHex")]
    rva_hex: String,
    #[serde(rename = "returnType", default)]
    return_type: String,
}

#[derive(Debug, Deserialize)]
#[expect(dead_code)]
struct ClassIndexEntry {
    name: String,
    namespace: String,
    image: String,
}

// ══════════════════════════════════════════════════════════════
// Data structures — Capture events (flexible: handles both formats)
// ══════════════════════════════════════════════════════════════

// Old format (frida_char_data.js METHOD_CALL events)
#[derive(Debug, Deserialize, Clone)]
#[expect(dead_code)]
struct CaptureEvent {
    #[serde(rename = "type")]
    event_type: String,
    #[serde(default)]
    hook: String,
    #[serde(rename = "hookIdx", default)]
    hook_idx: u32,
    #[serde(rename = "callNum", default)]
    call_num: u32,
    #[serde(default)]
    rva: String,
    #[serde(rename = "thisPtr", default)]
    this_ptr: String,
    #[serde(default)]
    args: Vec<String>,
    #[serde(default)]
    retval: String,
    #[serde(default)]
    timestamp: u64,
    #[serde(default)]
    fields: Option<serde_json::Value>,
    #[serde(rename = "retvalInt", default)]
    retval_int: Option<i64>,
    #[serde(rename = "retvalClass", default)]
    retval_class: Option<String>,
    #[serde(rename = "retvalFields", default)]
    retval_fields: Option<serde_json::Value>,
    #[serde(rename = "retvalString", default)]
    retval_string: Option<String>,
}

// ══════════════════════════════════════════════════════════════
// Gadget event extraction (frida_char_data_gadget.js)
// ══════════════════════════════════════════════════════════════

/// Attribute names indexed by position in Attributes.m_rawData
const ATTR_NAMES: &[&str] = &[
    "hp",
    "str",
    "agi",
    "wisd",
    "will",
    "atk",
    "def",
    "maxHp",
    "maxUltimateSp",
    "maxPoise",
    "poiseRecTime",
    "poiseRecTimeScalar",
    "physicalDamageTakenScalar",
    "fireDamageTakenScalar",
    "pulseDamageTakenScalar",
    "crystDamageTakenScalar",
    "naturalDamageTakenScalar",
    "etherDamageTakenScalar",
    "weight",
    "criticalRate",
    "criticalDamageIncrease",
    "moveSpeedMultiplier",
    "turnRateMultiplier",
    "attackRate",
    "skillCooldownScalar",
    "normalAttackRange",
    "hpRecoveryPerSec",
    "hpRecoveryPerSecByHpRatio",
    "poiseTakenScalar",
    "poiseOutputScalar",
    "breakingAttackDamageTakenScalar",
    "knockDownTimeAddition",
    "healOutputIncrease",
    "healTakenIncrease",
    "lifeSteal",
    "ultimateSpGainScalar",
    "atbCostAddition",
    "skillCooldownAddition",
    "comboSkillCooldownScalar",
    "igniteDamageScalar",
    "physicalInflictionDamageScalar",
    "physicalDamageIncrease",
    "fireDamageIncrease",
    "pulseDamageIncrease",
    "crystDamageIncrease",
    "naturalDamageIncrease",
    "etherDamageIncrease",
];

#[derive(Debug, Default, Serialize, Clone)]
struct ScalarValues {
    atk_from_main: Option<f64>,
    atk_from_str: Option<f64>,
    atk_from_agi: Option<f64>,
    atk_from_wisd: Option<f64>,
    atk_from_will: Option<f64>,
    hp_from_str: Option<f64>,
    heal_from_will: Option<f64>,
    phys_resist_from_agi: Option<f64>,
    energy_resist_from_wisd: Option<f64>,
}

#[derive(Debug, Default, Serialize, Clone)]
#[allow(dead_code)]
struct FinalCacheData {
    hex: String,
    data: Option<Vec<f64>>,
    named_attrs: HashMap<String, f64>,
    field8: i32,
    field12: i32,
    field16: Option<f64>,
}

#[derive(Debug, Default, Serialize, Clone)]
struct ComputedStats {
    /// Final values from m_finalCachedData (post-modifier)
    final_hp: Option<f64>,
    final_atk: Option<f64>,
    final_def: Option<f64>,
    final_max_hp: Option<f64>,
    /// All final cache attributes
    final_attrs: HashMap<String, f64>,
    /// Cross-attribute scalars
    scalars: ScalarValues,
    /// Main/sub attribute types
    main_attr_type: Option<i32>,
    sub_attr_type: Option<i32>,
}

#[derive(Debug, Default, Serialize, Clone)]
struct GadgetCharacterStats {
    template_id: String,
    level: i64,
    break_stage: i64,
    friendship: i64,
    potential_level: i64,
    inst_id: String,
    attributes: HashMap<String, f64>,
    raw_data: Vec<f64>,
    data_min: Option<Vec<f64>>,
    data_max: Option<Vec<f64>>,
    instance_uid: u64,
    capture_count: u32,
    sources: Vec<String>,
    computed_stats: Option<ComputedStats>,
}

/// Collects attrPtr→templateId mappings from multiple event sources.
fn build_attr_ptr_map(events_raw: &[serde_json::Value]) -> HashMap<String, String> {
    let mut map: HashMap<String, String> = HashMap::new();

    for ev in events_raw {
        let event_type = ev.get("type").and_then(|v| v.as_str()).unwrap_or("");

        match event_type {
            // ABILITY_INIT_DEEP: primary source — attrPtrs (plural) from re-entrancy tracking
            "ABILITY_INIT_DEEP" => {
                if let Some(as_data) = ev.get("abilitySystem") {
                    let template_id = as_data
                        .get("templateId")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    if template_id.is_empty() {
                        continue;
                    }
                    // Single attrPtr field
                    if let Some(ap) = as_data.get("attrPtr").and_then(|v| v.as_str())
                        && !ap.is_empty()
                    {
                        map.insert(ap.to_string(), template_id.clone());
                    }
                    // Array of attrPtrs (from re-entrancy tracking)
                    if let Some(aps) = as_data.get("attrPtrs").and_then(|v| v.as_array()) {
                        for ap in aps {
                            if let Some(s) = ap.as_str() {
                                map.insert(s.to_string(), template_id.clone());
                            }
                        }
                    }
                }
            }
            // CHAR_GET_ATTRIBUTES: attrPtr is the return value
            "CHAR_GET_ATTRIBUTES" => {
                let template_id = ev
                    .get("charInfo")
                    .and_then(|ci| ci.get("templateId"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let attr_ptr = ev.get("attrPtr").and_then(|v| v.as_str()).unwrap_or("");
                if !template_id.is_empty() && template_id != "(null)" && !attr_ptr.is_empty() {
                    map.insert(attr_ptr.to_string(), template_id.to_string());
                }
            }
            // ATTR_PTR_FIRST_SEEN: may include templateId from charMap
            "ATTR_PTR_FIRST_SEEN" => {
                let template_id = ev.get("templateId").and_then(|v| v.as_str()).unwrap_or("");
                let attr_ptr = ev.get("attrPtr").and_then(|v| v.as_str()).unwrap_or("");
                if !template_id.is_empty() && !attr_ptr.is_empty() {
                    map.insert(attr_ptr.to_string(), template_id.to_string());
                }
            }
            _ => {}
        }
    }

    // Temporal correlation: CONFIG_TABLE_LOOKUP → next ATTR_PTR_FIRST_SEEN
    // Each CONFIG_TABLE_LOOKUP with stackCharId is immediately followed by the
    // ATTR_PTR_FIRST_SEEN for that character's attrPtr.
    let mut pending_config_char: Option<String> = None;
    for ev in events_raw {
        match ev.get("type").and_then(|v| v.as_str()).unwrap_or("") {
            "CONFIG_TABLE_LOOKUP" => {
                if let Some(cid) = ev.get("stackCharId").and_then(|v| v.as_str())
                    && !cid.is_empty()
                {
                    pending_config_char = Some(cid.to_string());
                }
            }
            "ATTR_PTR_FIRST_SEEN" => {
                if let Some(char_id) = pending_config_char.take()
                    && let Some(ap) = ev.get("attrPtr").and_then(|v| v.as_str())
                {
                    map.entry(ap.to_string()).or_insert(char_id);
                }
                // Also check duringInitAttributes → ABILITY_INIT_DEEP link
                if let Some(ap) = ev.get("attrPtr").and_then(|v| v.as_str()) {
                    let during = ev
                        .get("duringInitAttributes")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false);
                    if during
                        && let Some(init_ptr) = ev.get("initSystemPtr").and_then(|v| v.as_str())
                    {
                        for ev2 in events_raw {
                            if ev2.get("type").and_then(|v| v.as_str()) == Some("ABILITY_INIT_DEEP")
                                && let Some(as2) = ev2.get("abilitySystem")
                                && as2.get("ptr").and_then(|v| v.as_str()) == Some(init_ptr)
                            {
                                if let Some(tid) = as2.get("templateId").and_then(|v| v.as_str())
                                    && !tid.is_empty()
                                {
                                    map.entry(ap.to_string()).or_insert_with(|| tid.to_string());
                                }
                                break;
                            }
                        }
                    }
                }
            }
            _ => {}
        }
    }

    map
}

fn extract_gadget_stats(events_raw: &[serde_json::Value]) -> HashMap<String, GadgetCharacterStats> {
    let mut characters: HashMap<String, GadgetCharacterStats> = HashMap::new();

    // Pre-build attrPtr → templateId map from all sources
    let attr_ptr_map = build_attr_ptr_map(events_raw);

    for ev in events_raw {
        let event_type = ev.get("type").and_then(|v| v.as_str()).unwrap_or("");

        match event_type {
            "CHAR_ATTRIBUTE_REFRESH" | "CHAR_GET_ATTRIBUTES" => {
                let char_info = match ev.get("charInfo") {
                    Some(ci) => ci,
                    None => continue,
                };

                let template_id = char_info
                    .get("templateId")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown")
                    .to_string();

                if template_id == "(null)" || template_id == "unknown" {
                    continue;
                }

                let entry =
                    characters
                        .entry(template_id.clone())
                        .or_insert_with(|| GadgetCharacterStats {
                            template_id: template_id.clone(),
                            ..Default::default()
                        });

                // Update char info
                if let Some(v) = char_info.get("level").and_then(|v| v.as_i64()) {
                    entry.level = v;
                }
                if let Some(v) = char_info.get("breakStage").and_then(|v| v.as_i64()) {
                    entry.break_stage = v;
                }
                if let Some(v) = char_info.get("friendship").and_then(|v| v.as_i64()) {
                    entry.friendship = v;
                }
                if let Some(v) = char_info.get("potentialLevel").and_then(|v| v.as_i64()) {
                    entry.potential_level = v;
                }
                if let Some(v) = char_info.get("instId").and_then(|v| v.as_str()) {
                    entry.inst_id = v.to_string();
                }

                // Extract attributes
                if let Some(attrs) = ev.get("attributes") {
                    extract_attributes_into(attrs, entry);
                }

                entry.capture_count += 1;
                entry.sources.push(event_type.to_string());
            }

            "ATTR_REFRESH_ALL" => {
                // Use the "after" state (post-refresh)
                if let Some(after) = ev.get("after") {
                    let template_id = after
                        .get("templateId")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();

                    if !template_id.is_empty() {
                        let entry = characters.entry(template_id.clone()).or_insert_with(|| {
                            GadgetCharacterStats {
                                template_id: template_id.clone(),
                                ..Default::default()
                            }
                        });
                        extract_attributes_into(after, entry);
                        entry.capture_count += 1;
                        entry.sources.push("ATTR_REFRESH_ALL".to_string());
                    }
                }
            }

            // Phase S2: live stat update from stat_capture_gadget.js
            "ATTR_REFRESH_UPDATE" => {
                let template_id = ev
                    .get("templateId")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();

                if !template_id.is_empty() {
                    let entry = characters.entry(template_id.clone()).or_insert_with(|| {
                        GadgetCharacterStats {
                            template_id: template_id.clone(),
                            ..Default::default()
                        }
                    });

                    if let Some(named) = ev.get("namedAttrs").and_then(|v| v.as_object()) {
                        for (key, val) in named {
                            if let Some(n) = val.as_f64() {
                                entry.attributes.insert(key.clone(), n);
                            }
                        }
                    }
                    if let Some(raw) = ev.get("rawData").and_then(|v| v.as_array()) {
                        let data: Vec<f64> = raw.iter().filter_map(|v| v.as_f64()).collect();
                        if !data.is_empty() {
                            for (i, &val) in data.iter().enumerate() {
                                if val != 0.0 {
                                    let name = if i < ATTR_NAMES.len() {
                                        ATTR_NAMES[i].to_string()
                                    } else {
                                        format!("attr_{}", i)
                                    };
                                    entry.attributes.insert(name, val);
                                }
                            }
                            entry.raw_data = data;
                        }
                    }

                    entry.capture_count += 1;
                    entry.sources.push("ATTR_REFRESH_UPDATE".to_string());
                }
            }

            "ATTR_DUMP" => {
                if let Some(src) = ev.get("sourceAttributes") {
                    let template_id = src
                        .get("templateId")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();

                    if !template_id.is_empty() {
                        let entry = characters.entry(template_id.clone()).or_insert_with(|| {
                            GadgetCharacterStats {
                                template_id: template_id.clone(),
                                ..Default::default()
                            }
                        });
                        extract_attributes_into(src, entry);

                        // Also check dumpResult for the AttributesData.m_attrs
                        if let Some(dump_result) = ev.get("dumpResult")
                            && let Some(m_attrs) =
                                dump_result.get("m_attrs").and_then(|v| v.as_array())
                        {
                            let raw: Vec<f64> = m_attrs.iter().filter_map(|v| v.as_f64()).collect();
                            if !raw.is_empty() {
                                entry.raw_data = raw;
                            }
                        }

                        entry.capture_count += 1;
                        entry.sources.push("ATTR_DUMP".to_string());
                    }
                }
            }

            "ABILITY_INIT_ATTRIBUTES" | "ABILITY_INIT_DEEP" => {
                if let Some(as_data) = ev.get("abilitySystem") {
                    // Try to get templateId from multiple locations
                    let template_id = as_data
                        .get("templateId")
                        .and_then(|v| v.as_str())
                        .filter(|s| !s.is_empty())
                        .or_else(|| {
                            // Fallback: resolve via attrPtr map
                            as_data
                                .get("attrPtr")
                                .and_then(|v| v.as_str())
                                .and_then(|ptr| attr_ptr_map.get(ptr))
                                .map(|s| s.as_str())
                        })
                        .unwrap_or("")
                        .to_string();

                    if !template_id.is_empty() {
                        let entry = characters.entry(template_id.clone()).or_insert_with(|| {
                            GadgetCharacterStats {
                                template_id: template_id.clone(),
                                ..Default::default()
                            }
                        });

                        // Extract attributes from the abilitySystem data
                        if let Some(attrs) = as_data.get("attributes") {
                            extract_attributes_into(attrs, entry);
                        }
                        // Also check top-level namedAttrs/rawData (from ABILITY_INIT_DEEP)
                        if let Some(named) = as_data.get("namedAttrs").and_then(|v| v.as_object()) {
                            for (key, val) in named {
                                if let Some(n) = val.as_f64() {
                                    entry.attributes.insert(key.clone(), n);
                                }
                            }
                        }
                        if let Some(raw) = as_data.get("rawData").and_then(|v| v.as_array()) {
                            let data: Vec<f64> = raw.iter().filter_map(|v| v.as_f64()).collect();
                            if !data.is_empty() {
                                for (i, &val) in data.iter().enumerate() {
                                    if val != 0.0 {
                                        let name = if i < ATTR_NAMES.len() {
                                            ATTR_NAMES[i].to_string()
                                        } else {
                                            format!("attr_{}", i)
                                        };
                                        entry.attributes.insert(name, val);
                                    }
                                }
                                entry.raw_data = data;
                            }
                        }

                        entry.capture_count += 1;
                        entry.sources.push(event_type.to_string());
                    }
                }
            }

            "ATTR_GET_VALUE" => {
                // Resolve templateId: direct field first, then attrPtr map
                let direct_tid = ev
                    .get("templateId")
                    .and_then(|v| v.as_str())
                    .filter(|s| !s.is_empty())
                    .map(|s| s.to_string());

                let template_id = direct_tid.or_else(|| {
                    ev.get("attrPtr")
                        .and_then(|v| v.as_str())
                        .and_then(|ptr| attr_ptr_map.get(ptr))
                        .cloned()
                });

                let attr_name = ev
                    .get("attributeName")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown");
                let value = ev.get("value").and_then(|v| v.as_f64());

                if let (Some(tid), Some(val)) = (&template_id, value) {
                    let entry =
                        characters
                            .entry(tid.clone())
                            .or_insert_with(|| GadgetCharacterStats {
                                template_id: tid.clone(),
                                ..Default::default()
                            });
                    entry.attributes.insert(attr_name.to_string(), val);
                }
            }

            // v7: Batched GetValue snapshot — attrs map per attrPtr
            "ATTR_VALUE_SNAPSHOT" => {
                let direct_tid = ev
                    .get("templateId")
                    .and_then(|v| v.as_str())
                    .filter(|s| !s.is_empty())
                    .map(|s| s.to_string());

                let template_id = direct_tid.or_else(|| {
                    ev.get("attrPtr")
                        .and_then(|v| v.as_str())
                        .and_then(|ptr| attr_ptr_map.get(ptr))
                        .cloned()
                });

                if let Some(tid) = &template_id
                    && let Some(attrs) = ev.get("attrs").and_then(|v| v.as_object())
                {
                    let entry =
                        characters
                            .entry(tid.clone())
                            .or_insert_with(|| GadgetCharacterStats {
                                template_id: tid.clone(),
                                ..Default::default()
                            });
                    for (key, val) in attrs {
                        if let Some(n) = val.as_f64() {
                            entry.attributes.insert(key.clone(), n);
                        }
                    }
                }
            }

            // v10: Table dictionary walk entries — character data from static config table
            "TABLE_CHARACTER_ENTRY" => {
                let char_id = ev
                    .get("charId")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                if char_id.is_empty() {
                    continue;
                }

                let entry =
                    characters
                        .entry(char_id.clone())
                        .or_insert_with(|| GadgetCharacterStats {
                            template_id: char_id.clone(),
                            ..Default::default()
                        });

                // Extract stats from the probed value object
                if let Some(stats) = ev.get("stats").and_then(|v| v.as_object()) {
                    for (key, val) in stats {
                        if let Some(n) = val.as_f64()
                            && n != 0.0
                        {
                            entry.attributes.entry(key.clone()).or_insert(n);
                        }
                    }
                }

                // Extract rawData array
                if let Some(raw) = ev.get("rawData").and_then(|v| v.as_array()) {
                    let data: Vec<f64> = raw.iter().filter_map(|v| v.as_f64()).collect();
                    if !data.is_empty() && entry.raw_data.is_empty() {
                        for (i, &val) in data.iter().enumerate() {
                            if val != 0.0 {
                                let name = if i < ATTR_NAMES.len() {
                                    ATTR_NAMES[i].to_string()
                                } else {
                                    format!("attr_{}", i)
                                };
                                entry.attributes.entry(name).or_insert(val);
                            }
                        }
                        entry.raw_data = data;
                    }
                }

                entry.capture_count += 1;
                entry.sources.push("TABLE_CHARACTER_ENTRY".to_string());
            }

            // Phase S2: unified full character capture from stat_capture_gadget.js
            "FULL_CHARACTER_CAPTURE" => {
                let template_id = ev
                    .get("templateId")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();

                if template_id.is_empty() || template_id == "(null)" {
                    continue;
                }

                let entry =
                    characters
                        .entry(template_id.clone())
                        .or_insert_with(|| GadgetCharacterStats {
                            template_id: template_id.clone(),
                            ..Default::default()
                        });

                // Level
                if let Some(v) = ev.get("level").and_then(|v| v.as_i64()) {
                    entry.level = v;
                }

                // Raw data + named attributes
                if let Some(named) = ev.get("namedAttrs").and_then(|v| v.as_object()) {
                    for (key, val) in named {
                        if let Some(n) = val.as_f64() {
                            entry.attributes.insert(key.clone(), n);
                        }
                    }
                }
                if let Some(raw) = ev.get("rawData").and_then(|v| v.as_array()) {
                    let data: Vec<f64> = raw.iter().filter_map(|v| v.as_f64()).collect();
                    if !data.is_empty() {
                        for (i, &val) in data.iter().enumerate() {
                            if val != 0.0 {
                                let name = if i < ATTR_NAMES.len() {
                                    ATTR_NAMES[i].to_string()
                                } else {
                                    format!("attr_{}", i)
                                };
                                entry.attributes.insert(name, val);
                            }
                        }
                        entry.raw_data = data;
                    }
                }

                // Data min/max
                if let Some(min) = ev.get("dataMin").and_then(|v| v.as_array()) {
                    entry.data_min = Some(min.iter().filter_map(|v| v.as_f64()).collect());
                }
                if let Some(max) = ev.get("dataMax").and_then(|v| v.as_array()) {
                    entry.data_max = Some(max.iter().filter_map(|v| v.as_f64()).collect());
                }

                // Computed stats: final cached data, scalars, attr types
                let mut computed = ComputedStats::default();

                if let Some(fcd) = ev.get("finalCachedData") {
                    if let Some(data) = fcd.get("data").and_then(|v| v.as_array()) {
                        let vals: Vec<f64> = data.iter().filter_map(|v| v.as_f64()).collect();
                        for (i, &val) in vals.iter().enumerate() {
                            if val != 0.0 {
                                let name = if i < ATTR_NAMES.len() {
                                    ATTR_NAMES[i].to_string()
                                } else {
                                    format!("attr_{}", i)
                                };
                                computed.final_attrs.insert(name.clone(), val);
                                match name.as_str() {
                                    "hp" => computed.final_hp = Some(val),
                                    "atk" => computed.final_atk = Some(val),
                                    "def" => computed.final_def = Some(val),
                                    "maxHp" => computed.final_max_hp = Some(val),
                                    _ => {}
                                }
                            }
                        }
                    }
                    if let Some(named) = fcd.get("namedAttrs").and_then(|v| v.as_object()) {
                        for (k, v) in named {
                            if let Some(n) = v.as_f64() {
                                computed.final_attrs.entry(k.clone()).or_insert(n);
                            }
                        }
                    }
                }

                // Scalars
                if let Some(scalars) = ev.get("scalars").and_then(|v| v.as_object()) {
                    computed.scalars.atk_from_main =
                        scalars.get("atkFromMain").and_then(|v| v.as_f64());
                    computed.scalars.atk_from_str =
                        scalars.get("atkFromStr").and_then(|v| v.as_f64());
                    computed.scalars.atk_from_agi =
                        scalars.get("atkFromAgi").and_then(|v| v.as_f64());
                    computed.scalars.atk_from_wisd =
                        scalars.get("atkFromWisd").and_then(|v| v.as_f64());
                    computed.scalars.atk_from_will =
                        scalars.get("atkFromWill").and_then(|v| v.as_f64());
                    computed.scalars.hp_from_str =
                        scalars.get("hpFromStr").and_then(|v| v.as_f64());
                    computed.scalars.heal_from_will =
                        scalars.get("healFromWill").and_then(|v| v.as_f64());
                    computed.scalars.phys_resist_from_agi =
                        scalars.get("physResistFromAgi").and_then(|v| v.as_f64());
                    computed.scalars.energy_resist_from_wisd =
                        scalars.get("energyResistFromWisd").and_then(|v| v.as_f64());
                }

                // Main/sub attr types
                if let Some(mat) = ev.get("mainAttrType")
                    && let Some(has) = mat.get("hasValue").and_then(|v| v.as_bool())
                    && has
                {
                    computed.main_attr_type =
                        mat.get("value").and_then(|v| v.as_i64()).map(|v| v as i32);
                }
                if let Some(sat) = ev.get("subAttrType")
                    && let Some(has) = sat.get("hasValue").and_then(|v| v.as_bool())
                    && has
                {
                    computed.sub_attr_type =
                        sat.get("value").and_then(|v| v.as_i64()).map(|v| v as i32);
                }

                entry.computed_stats = Some(computed);
                entry.capture_count += 1;
                entry.sources.push("FULL_CHARACTER_CAPTURE".to_string());
            }

            "FINAL_ATTRIBUTES" | "SCAN_CHAR_DATA" => {
                // Extract character identity from charInfo
                let template_id = ev
                    .get("charInfo")
                    .and_then(|ci| ci.get("templateId"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();

                if template_id.is_empty() || template_id == "(null)" {
                    continue;
                }

                let entry =
                    characters
                        .entry(template_id.clone())
                        .or_insert_with(|| GadgetCharacterStats {
                            template_id: template_id.clone(),
                            ..Default::default()
                        });

                // Update char info from charInfo block
                if let Some(ci) = ev.get("charInfo") {
                    if let Some(v) = ci.get("level").and_then(|v| v.as_i64()) {
                        entry.level = v;
                    }
                    if let Some(v) = ci.get("breakStage").and_then(|v| v.as_i64()) {
                        entry.break_stage = v;
                    }
                    if let Some(v) = ci.get("friendship").and_then(|v| v.as_i64()) {
                        entry.friendship = v;
                    }
                    if let Some(v) = ci.get("potentialLevel").and_then(|v| v.as_i64()) {
                        entry.potential_level = v;
                    }
                    if let Some(v) = ci.get("instId").and_then(|v| v.as_str()) {
                        entry.inst_id = v.to_string();
                    }
                }

                // Extract raw attributes
                if let Some(named) = ev.get("namedAttrs").and_then(|v| v.as_object()) {
                    for (key, val) in named {
                        if let Some(n) = val.as_f64() {
                            entry.attributes.insert(key.clone(), n);
                        }
                    }
                }
                if let Some(raw) = ev.get("rawData").and_then(|v| v.as_array()) {
                    let data: Vec<f64> = raw.iter().filter_map(|v| v.as_f64()).collect();
                    if !data.is_empty() {
                        for (i, &val) in data.iter().enumerate() {
                            if val != 0.0 {
                                let name = if i < ATTR_NAMES.len() {
                                    ATTR_NAMES[i].to_string()
                                } else {
                                    format!("attr_{}", i)
                                };
                                entry.attributes.insert(name, val);
                            }
                        }
                        entry.raw_data = data;
                    }
                }

                // Extract final cached data
                let mut computed = ComputedStats::default();

                if let Some(fcd) = ev.get("finalCachedData") {
                    let mut cache = FinalCacheData::default();
                    if let Some(hex) = fcd.get("hex").and_then(|v| v.as_str()) {
                        cache.hex = hex.to_string();
                    }
                    if let Some(data) = fcd.get("data").and_then(|v| v.as_array()) {
                        let vals: Vec<f64> = data.iter().filter_map(|v| v.as_f64()).collect();
                        // Map to named final attributes
                        for (i, &val) in vals.iter().enumerate() {
                            if val != 0.0 {
                                let name = if i < ATTR_NAMES.len() {
                                    ATTR_NAMES[i].to_string()
                                } else {
                                    format!("attr_{}", i)
                                };
                                computed.final_attrs.insert(name.clone(), val);
                                // Extract key final stats
                                match name.as_str() {
                                    "hp" => computed.final_hp = Some(val),
                                    "atk" => computed.final_atk = Some(val),
                                    "def" => computed.final_def = Some(val),
                                    "maxHp" => computed.final_max_hp = Some(val),
                                    _ => {}
                                }
                            }
                        }
                        cache.data = Some(vals);
                    }
                    if let Some(named) = fcd.get("namedAttrs").and_then(|v| v.as_object()) {
                        for (k, v) in named {
                            if let Some(n) = v.as_f64() {
                                cache.named_attrs.insert(k.clone(), n);
                                computed.final_attrs.entry(k.clone()).or_insert(n);
                            }
                        }
                    }
                    cache.field8 = fcd.get("field8").and_then(|v| v.as_i64()).unwrap_or(0) as i32;
                    cache.field12 = fcd.get("field12").and_then(|v| v.as_i64()).unwrap_or(0) as i32;
                    cache.field16 = fcd.get("field16").and_then(|v| v.as_f64());
                    let _ = cache; // consumed into computed
                }

                // Extract scalars
                if let Some(scalars) = ev.get("scalars").and_then(|v| v.as_object()) {
                    computed.scalars.atk_from_main =
                        scalars.get("atkFromMain").and_then(|v| v.as_f64());
                    computed.scalars.atk_from_str =
                        scalars.get("atkFromStr").and_then(|v| v.as_f64());
                    computed.scalars.atk_from_agi =
                        scalars.get("atkFromAgi").and_then(|v| v.as_f64());
                    computed.scalars.atk_from_wisd =
                        scalars.get("atkFromWisd").and_then(|v| v.as_f64());
                    computed.scalars.atk_from_will =
                        scalars.get("atkFromWill").and_then(|v| v.as_f64());
                    computed.scalars.hp_from_str =
                        scalars.get("hpFromStr").and_then(|v| v.as_f64());
                    computed.scalars.heal_from_will =
                        scalars.get("healFromWill").and_then(|v| v.as_f64());
                    computed.scalars.phys_resist_from_agi =
                        scalars.get("physResistFromAgi").and_then(|v| v.as_f64());
                    computed.scalars.energy_resist_from_wisd =
                        scalars.get("energyResistFromWisd").and_then(|v| v.as_f64());
                }

                // Extract main/sub attr types
                if let Some(mat) = ev.get("mainAttrType")
                    && let Some(has) = mat.get("hasValue").and_then(|v| v.as_bool())
                    && has
                {
                    computed.main_attr_type =
                        mat.get("value").and_then(|v| v.as_i64()).map(|v| v as i32);
                }
                if let Some(sat) = ev.get("subAttrType")
                    && let Some(has) = sat.get("hasValue").and_then(|v| v.as_bool())
                    && has
                {
                    computed.sub_attr_type =
                        sat.get("value").and_then(|v| v.as_i64()).map(|v| v as i32);
                }

                entry.computed_stats = Some(computed);
                entry.capture_count += 1;
                entry.sources.push("FINAL_ATTRIBUTES".to_string());
            }

            _ => {}
        }
    }

    characters
}

fn extract_attributes_into(attrs_json: &serde_json::Value, target: &mut GadgetCharacterStats) {
    if let Some(uid) = attrs_json.get("instanceUid").and_then(|v| v.as_u64()) {
        target.instance_uid = uid;
    }

    // Named attributes (non-zero values pre-mapped)
    if let Some(named) = attrs_json.get("namedAttrs").and_then(|v| v.as_object()) {
        for (key, val) in named {
            if let Some(n) = val.as_f64() {
                target.attributes.insert(key.clone(), n);
            }
        }
    }

    // Raw data array
    if let Some(raw) = attrs_json.get("rawData").and_then(|v| v.as_array()) {
        let data: Vec<f64> = raw.iter().filter_map(|v| v.as_f64()).collect();
        if !data.is_empty() {
            // Map to named attributes
            for (i, &val) in data.iter().enumerate() {
                if val != 0.0 {
                    let name = if i < ATTR_NAMES.len() {
                        ATTR_NAMES[i].to_string()
                    } else {
                        format!("attr_{}", i)
                    };
                    target.attributes.insert(name, val);
                }
            }
            target.raw_data = data;
        }
    }

    // Data min/max
    if let Some(min) = attrs_json.get("dataMin").and_then(|v| v.as_array()) {
        target.data_min = Some(min.iter().filter_map(|v| v.as_f64()).collect());
    }
    if let Some(max) = attrs_json.get("dataMax").and_then(|v| v.as_array()) {
        target.data_max = Some(max.iter().filter_map(|v| v.as_f64()).collect());
    }
}

// ══════════════════════════════════════════════════════════════
// Scoring + ranking
// ══════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Serialize)]
struct ScoredClass {
    name: String,
    namespace: String,
    image: String,
    score: u32,
    reasons: Vec<String>,
    numeric_fields: u32,
    total_fields: u32,
    total_methods: u32,
    stat_fields: Vec<String>,
    hook_candidates: Vec<HookCandidate>,
}

#[derive(Debug, Clone, Serialize)]
struct HookCandidate {
    method: String,
    rva: String,
    rva_int: u64,
    param_count: i32,
    return_type: String,
    reason: String,
}

const STAT_FIELD_KEYWORDS: &[&str] = &[
    "hp",
    "atk",
    "def",
    "attack",
    "defense",
    "health",
    "crit",
    "damage",
    "speed",
    "resist",
    "maxhp",
    "baseatk",
    "basedef",
    "level",
    "exp",
    "growth",
    "stat",
    "power",
    "strength",
    "agility",
    "stamina",
    "energy",
    "shield",
    "armor",
    "penetration",
    "lifesteal",
    "regen",
];

const STAT_METHOD_KEYWORDS: &[&str] = &[
    "get",
    "set",
    "calc",
    "compute",
    "load",
    "init",
    "parse",
    "merge",
    "deserialize",
    "read",
    "decode",
    "apply",
    "update",
    "refresh",
    "setup",
];

fn score_class(entry: &ClassEntry) -> ScoredClass {
    let mut score = 0u32;
    let mut reasons = Vec::new();
    let mut stat_fields = Vec::new();
    let mut hook_candidates = Vec::new();

    // Tier 1 match = high base score
    match entry.match_tier {
        1 => {
            score += 50;
            reasons.push(format!("tier1: {}", entry.match_reason));
        }
        2 => {
            score += 20;
            reasons.push(format!("tier2: {}", entry.match_reason));
        }
        3 => {
            score += 5;
            reasons.push(format!("tier3: {}", entry.match_reason));
        }
        _ => {}
    }

    // Signature hit bonus
    if entry.is_signature_hit {
        score += 15;
        reasons.push(format!("{} numeric fields", entry.numeric_field_count));
    }

    // Score individual fields
    for field in &entry.fields {
        if field.is_static {
            continue;
        }

        let lower = field.name.to_lowercase();
        for &kw in STAT_FIELD_KEYWORDS {
            if lower.contains(kw) {
                score += 10;
                stat_fields.push(format!(
                    "{} ({}, offset={})",
                    field.name, field.type_name, field.offset
                ));
                reasons.push(format!("field '{}' matches '{}'", field.name, kw));
                break;
            }
        }

        // Numeric field bonus
        if field.is_numeric {
            score += 2;
        }
    }

    // Score methods + identify hook candidates
    for method in &entry.methods {
        let lower = method.name.to_lowercase();
        let mut method_score = 0u32;
        let mut method_reason = String::new();

        // Check for stat-related method names
        for &mkw in STAT_METHOD_KEYWORDS {
            if lower.contains(mkw) {
                method_score += 5;
                method_reason = format!("name contains '{}'", mkw);
                break;
            }
        }

        // Check for protobuf methods
        if lower.contains("mergefrom") || lower.contains("parsefrom") {
            method_score += 15;
            method_reason = "protobuf deserialization".into();
        }

        // Check return type for stat-like types
        let ret_lower = method.return_type.to_lowercase();
        if (ret_lower.contains("int")
            || ret_lower.contains("float")
            || ret_lower.contains("double"))
            && (lower.contains("stat")
                || lower.contains("hp")
                || lower.contains("atk")
                || lower.contains("def")
                || lower.contains("get"))
        {
            method_score += 10;
            method_reason = format!("returns {} with stat-like name", method.return_type);
        }

        if method_score > 0 && method.rva > 0 {
            score += method_score;
            hook_candidates.push(HookCandidate {
                method: method.name.clone(),
                rva: method.rva_hex.clone(),
                rva_int: method.rva,
                param_count: method.param_count,
                return_type: method.return_type.clone(),
                reason: method_reason,
            });
        }
    }

    // Sort hook candidates by priority
    hook_candidates.sort_by(|a, b| b.reason.len().cmp(&a.reason.len()));

    ScoredClass {
        name: entry.name.clone(),
        namespace: entry.namespace.clone(),
        image: entry.image.clone(),
        score,
        reasons,
        numeric_fields: entry.numeric_field_count,
        total_fields: entry.fields.len() as u32,
        total_methods: entry.methods.len() as u32,
        stat_fields,
        hook_candidates,
    }
}

// ══════════════════════════════════════════════════════════════
// Character stat extraction
// ══════════════════════════════════════════════════════════════

#[derive(Debug, Default, Serialize)]
struct CharacterStats {
    char_id: String,
    captures: Vec<StatCapture>,
}

#[derive(Debug, Clone, Serialize)]
struct StatCapture {
    source_hook: String,
    timestamp: u64,
    fields: HashMap<String, serde_json::Value>,
    stat_values: HashMap<String, f64>,
}

fn extract_stats_from_fields(
    fields: &serde_json::Value,
) -> (HashMap<String, serde_json::Value>, HashMap<String, f64>) {
    let mut all_fields = HashMap::new();
    let mut stat_values = HashMap::new();

    if let Some(obj) = fields.as_object() {
        for (key, val) in obj {
            if key.starts_with('_') {
                continue; // Skip metadata keys
            }

            all_fields.insert(key.clone(), val.clone());

            let lower = key.to_lowercase();
            let is_stat = STAT_FIELD_KEYWORDS.iter().any(|kw| lower.contains(kw));

            if is_stat {
                if let Some(n) = val.as_f64() {
                    stat_values.insert(key.clone(), n);
                } else if let Some(n) = val.as_i64() {
                    stat_values.insert(key.clone(), n as f64);
                }
            }
        }
    }

    (all_fields, stat_values)
}

fn identify_character(fields: &serde_json::Value) -> Option<String> {
    if let Some(obj) = fields.as_object() {
        // Look for character ID fields
        let id_keys = [
            "charId",
            "characterId",
            "id",
            "Id",
            "CharId",
            "char_id",
            "character_id",
            "unitId",
            "unit_id",
            "name",
            "Name",
        ];

        for key in &id_keys {
            if let Some(val) = obj.get(*key) {
                if let Some(s) = val.as_str()
                    && !s.is_empty()
                {
                    return Some(s.to_string());
                }
                if let Some(n) = val.as_i64() {
                    return Some(n.to_string());
                }
            }
        }

        // Check nested _class for character type identification
        if let Some(class_val) = obj.get("_class")
            && let Some(class_str) = class_val.as_str()
            && class_str.to_lowercase().contains("character")
            && let Some(ptr_val) = obj.get("_ptr")
            && let Some(ptr_str) = ptr_val.as_str()
        {
            return Some(format!("{}@{}", class_str, ptr_str));
        }
    }
    None
}

// ══════════════════════════════════════════════════════════════
// Pass implementations
// ══════════════════════════════════════════════════════════════

fn pass_1_parse_dump(dump_path: &str) -> Option<Il2cppDump> {
    println!("\n{}", "═".repeat(62));
    println!("  Pass 1: Parse IL2CPP Dump");
    println!("{}\n", "═".repeat(62));

    let content = match fs::read_to_string(dump_path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("  ERROR: Cannot read {}: {}", dump_path, e);
            return None;
        }
    };

    println!("  Loaded: {} ({} bytes)", dump_path, content.len());

    let dump: Il2cppDump = match serde_json::from_str(&content) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("  ERROR: JSON parse failed: {}", e);
            return None;
        }
    };

    println!("  Scan metadata:");
    println!("    Timestamp:        {}", dump.meta.timestamp);
    println!("    Architecture:     {}", dump.meta.arch);
    println!("    GA base:          {}", dump.meta.game_assembly_base);
    println!(
        "    GA size:          {} bytes",
        dump.meta.game_assembly_size
    );
    println!("    Total assemblies: {}", dump.meta.total_assemblies);
    println!("    Total classes:    {}", dump.meta.total_classes);
    println!("    Total methods:    {}", dump.meta.total_methods);
    println!("    Total fields:     {}", dump.meta.total_fields);
    println!("    Keyword matches:  {}", dump.meta.matched_classes);
    println!("    Signature hits:   {}", dump.meta.signature_hits_count);
    println!("    Class index size: {}", dump.class_index.len());

    // Namespace distribution
    let mut ns_counts: HashMap<String, u32> = HashMap::new();
    for entry in &dump.class_index {
        let ns = if entry.namespace.is_empty() {
            "(global)"
        } else {
            &entry.namespace
        };
        *ns_counts.entry(ns.to_string()).or_insert(0) += 1;
    }

    let mut ns_sorted: Vec<_> = ns_counts.into_iter().collect();
    ns_sorted.sort_by(|a, b| b.1.cmp(&a.1));

    println!("\n  Top namespaces:");
    for (ns, count) in ns_sorted.iter().take(15) {
        println!("    {:40} : {} classes", ns, count);
    }

    Some(dump)
}

fn pass_2_rank(dump: &Il2cppDump) -> Vec<ScoredClass> {
    println!("\n{}", "═".repeat(62));
    println!("  Pass 2: Rank Classes by Stat Score");
    println!("{}\n", "═".repeat(62));

    // Combine matched + signature hits, dedup by name+namespace
    let mut seen = std::collections::HashSet::new();
    let mut all_entries = Vec::new();

    for entry in dump.matched.iter().chain(dump.signature_hits.iter()) {
        let key = format!("{}.{}", entry.namespace, entry.name);
        if seen.insert(key) {
            all_entries.push(entry.clone());
        }
    }

    println!("  Scoring {} unique classes...", all_entries.len());

    let mut scored: Vec<ScoredClass> = all_entries.iter().map(score_class).collect();
    scored.sort_by(|a, b| b.score.cmp(&a.score));

    // Print top results
    println!("\n  Top 30 stat candidates:\n");
    println!(
        "  {:>4}  {:>5}  {:40}  {:>3}F {:>3}M  {:>3}N  Hook candidates",
        "Rank", "Score", "Class", "", "", ""
    );
    println!("  {}", "-".repeat(100));

    for (i, sc) in scored.iter().take(30).enumerate() {
        let full_name = if sc.namespace.is_empty() {
            sc.name.clone()
        } else {
            format!("{}.{}", sc.namespace, sc.name)
        };

        let hooks_str = if sc.hook_candidates.is_empty() {
            "none".to_string()
        } else {
            sc.hook_candidates
                .iter()
                .take(3)
                .map(|h| format!("{}@{}", h.method, h.rva))
                .collect::<Vec<_>>()
                .join(", ")
        };

        println!(
            "  {:>4}  {:>5}  {:40}  {:>3}  {:>3}  {:>3}  {}",
            i + 1,
            sc.score,
            if full_name.len() > 40 {
                format!("{}...", &full_name[..37])
            } else {
                full_name
            },
            sc.total_fields,
            sc.total_methods,
            sc.numeric_fields,
            hooks_str
        );
    }

    // Print detailed view of top 10
    println!("\n  Detailed view of top 10:\n");
    for (i, sc) in scored.iter().take(10).enumerate() {
        println!(
            "  === #{} {} (score={}) ===",
            i + 1,
            if sc.namespace.is_empty() {
                sc.name.clone()
            } else {
                format!("{}.{}", sc.namespace, sc.name)
            },
            sc.score
        );
        println!("    Image: {}", sc.image);
        println!("    Reasons: {}", sc.reasons.join("; "));

        if !sc.stat_fields.is_empty() {
            println!("    Stat fields:");
            for sf in &sc.stat_fields {
                println!("      - {}", sf);
            }
        }

        if !sc.hook_candidates.is_empty() {
            println!("    Hook candidates:");
            for hc in sc.hook_candidates.iter().take(5) {
                println!(
                    "      - {} @ {} (params={}, ret={}) — {}",
                    hc.method, hc.rva, hc.param_count, hc.return_type, hc.reason
                );
            }
        }
        println!();
    }

    scored
}

fn pass_3_parse_capture(capture_path: &str) -> (Vec<CaptureEvent>, Vec<serde_json::Value>) {
    println!("\n{}", "═".repeat(62));
    println!("  Pass 3: Parse Capture Log");
    println!("{}\n", "═".repeat(62));

    let content = match fs::read_to_string(capture_path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("  ERROR: Cannot read {}: {}", capture_path, e);
            return (Vec::new(), Vec::new());
        }
    };

    println!("  Loaded: {} ({} bytes)", capture_path, content.len());

    let mut old_events = Vec::new();
    let mut raw_events = Vec::new();
    let mut parse_errors = 0u32;
    let mut is_gadget_format = false;

    for line in content.lines() {
        let trimmed = line.trim();
        // Strip leading null bytes (Frida File BOM artifact)
        let trimmed = trimmed.trim_start_matches('\0').trim();
        if trimmed.is_empty() {
            continue;
        }

        // Try IL2CPP_EVENT| prefix (console capture log format)
        let json_str = if let Some(s) = trimmed.strip_prefix("IL2CPP_EVENT|") {
            s
        } else if trimmed.starts_with('{') {
            // Bare JSON line (JSONL file format)
            trimmed
        } else {
            continue;
        };

        match serde_json::from_str::<serde_json::Value>(json_str) {
            Ok(val) => {
                let event_type = val.get("type").and_then(|v| v.as_str()).unwrap_or("");

                // Detect format: gadget events have types like CHAR_ATTRIBUTE_REFRESH
                if matches!(
                    event_type,
                    "CHAR_ATTRIBUTE_REFRESH"
                        | "CHAR_GET_ATTRIBUTES"
                        | "ATTR_REFRESH_ALL"
                        | "ATTR_DUMP"
                        | "ABILITY_INIT_ATTRIBUTES"
                        | "ATTR_GET_VALUE"
                        | "ATTR_VALUE_SNAPSHOT"
                        | "ATTR_PTR_FIRST_SEEN"
                        | "ABILITY_INIT_DEEP"
                        | "FINAL_ATTRIBUTES"
                        | "SCAN_CHAR_DATA"
                        | "CONFIG_TABLE_LOOKUP"
                        | "CALC_BASE_ATTRIBUTE"
                        | "CALC_FINAL_ATTRIBUTE"
                        | "CHARACTER_ATTRIBUTE_TABLE_DUMP"
                        | "TABLE_DEEP_PROBE"
                        | "TABLE_CHARACTER_ENTRY"
                        | "TABLE_DICT_SUMMARY"
                ) {
                    is_gadget_format = true;
                }

                raw_events.push(val.clone());

                // Also try parsing as old CaptureEvent format
                if let Ok(ev) = serde_json::from_value::<CaptureEvent>(val) {
                    old_events.push(ev);
                }
            }
            Err(_) => parse_errors += 1,
        }
    }

    println!(
        "  Parsed {} events ({} parse errors)",
        raw_events.len(),
        parse_errors
    );
    println!(
        "  Format: {}",
        if is_gadget_format { "gadget" } else { "legacy" }
    );

    // Event type distribution
    let mut type_counts: HashMap<String, u32> = HashMap::new();
    for ev in &raw_events {
        let t = ev
            .get("type")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .to_string();
        *type_counts.entry(t).or_insert(0) += 1;
    }

    println!("\n  Event types:");
    let mut sorted_types: Vec<_> = type_counts.iter().collect();
    sorted_types.sort_by(|a, b| b.1.cmp(a.1));
    for (t, c) in &sorted_types {
        println!("    {:40} : {}", t, c);
    }

    (old_events, raw_events)
}

fn pass_4_correlate(events: &[CaptureEvent], scored: &[ScoredClass], dump: &Il2cppDump) {
    println!("\n{}", "═".repeat(62));
    println!("  Pass 4: Correlate Captures with Metadata");
    println!("{}\n", "═".repeat(62));

    if events.is_empty() {
        println!("  No capture events to correlate.");
        return;
    }

    // Build RVA → class+method lookup
    let mut rva_lookup: HashMap<String, (String, String)> = HashMap::new();
    for entry in dump.matched.iter().chain(dump.signature_hits.iter()) {
        for method in &entry.methods {
            if method.rva > 0 {
                let full_class = if entry.namespace.is_empty() {
                    entry.name.clone()
                } else {
                    format!("{}.{}", entry.namespace, entry.name)
                };
                rva_lookup.insert(
                    method.rva_hex.clone(),
                    (full_class.clone(), method.name.clone()),
                );
            }
        }
    }

    // Correlate each event
    let mut correlated = 0u32;
    let mut unique_classes_seen = std::collections::HashSet::new();

    for ev in events {
        if let Some((class_name, method_name)) = rva_lookup.get(&ev.rva) {
            correlated += 1;
            unique_classes_seen.insert(class_name.clone());

            if correlated <= 20 {
                println!(
                    "  Event #{}: {}.{} (hook: {})",
                    ev.call_num, class_name, method_name, ev.hook
                );

                // Check if this class is in our scored list
                if let Some(sc) = scored.iter().find(|s| {
                    let full = if s.namespace.is_empty() {
                        s.name.clone()
                    } else {
                        format!("{}.{}", s.namespace, s.name)
                    };
                    full == *class_name
                }) {
                    println!(
                        "    Score: {} | Reasons: {}",
                        sc.score,
                        sc.reasons.join("; ")
                    );
                }

                // Print captured field values
                if let Some(fields) = &ev.fields
                    && let Some(obj) = fields.as_object()
                {
                    let stat_fields: Vec<_> = obj
                        .iter()
                        .filter(|(k, _)| {
                            !k.starts_with('_')
                                && STAT_FIELD_KEYWORDS
                                    .iter()
                                    .any(|kw| k.to_lowercase().contains(kw))
                        })
                        .collect();

                    if !stat_fields.is_empty() {
                        println!("    STAT FIELDS:");
                        for (k, v) in &stat_fields {
                            println!("      {} = {}", k, v);
                        }
                    }
                }
            }
        }
    }

    println!("\n  Correlated: {}/{} events", correlated, events.len());
    println!("  Unique classes seen: {}", unique_classes_seen.len());
    for cls in &unique_classes_seen {
        println!("    - {}", cls);
    }
}

fn pass_5_extract_stats(events: &[CaptureEvent], output_dir: &str) {
    println!("\n{}", "═".repeat(62));
    println!("  Pass 5: Extract Character Stats");
    println!("{}\n", "═".repeat(62));

    if events.is_empty() {
        println!("  No events to extract stats from.");
        return;
    }

    let _ = fs::create_dir_all(output_dir);

    let mut characters: HashMap<String, CharacterStats> = HashMap::new();
    let mut unidentified = Vec::new();

    for ev in events {
        // Process both 'this' fields and retval fields
        let field_sources: Vec<(&str, &serde_json::Value)> = [
            ev.fields.as_ref().map(|f| ("this", f)),
            ev.retval_fields.as_ref().map(|f| ("retval", f)),
        ]
        .into_iter()
        .flatten()
        .collect();

        for (source, fields) in &field_sources {
            let (all_fields, stat_values) = extract_stats_from_fields(fields);

            if stat_values.is_empty() {
                continue;
            }

            let capture = StatCapture {
                source_hook: format!("{} ({})", ev.hook, source),
                timestamp: ev.timestamp,
                fields: all_fields,
                stat_values: stat_values.clone(),
            };

            if let Some(char_id) = identify_character(fields) {
                let entry = characters
                    .entry(char_id.clone())
                    .or_insert_with(|| CharacterStats {
                        char_id: char_id.clone(),
                        captures: Vec::new(),
                    });
                entry.captures.push(capture);
            } else {
                unidentified.push(capture);
            }
        }
    }

    // Output results
    println!("  Identified {} characters with stats:", characters.len());
    for (id, stats) in &characters {
        println!(
            "\n  === Character: {} ({} captures) ===",
            id,
            stats.captures.len()
        );

        // Aggregate stat values across captures
        let mut stat_agg: HashMap<String, Vec<f64>> = HashMap::new();
        for cap in &stats.captures {
            for (k, v) in &cap.stat_values {
                stat_agg.entry(k.clone()).or_default().push(*v);
            }
        }

        for (stat_name, values) in &stat_agg {
            let unique: std::collections::HashSet<String> =
                values.iter().map(|v| format!("{:.2}", v)).collect();
            if unique.len() == 1 {
                println!("    {:20} = {:.2}", stat_name, values[0]);
            } else {
                println!(
                    "    {:20} = {} values: {:?}",
                    stat_name,
                    values.len(),
                    values
                );
            }
        }
    }

    if !unidentified.is_empty() {
        println!(
            "\n  {} stat captures without character ID:",
            unidentified.len()
        );
        for (i, cap) in unidentified.iter().take(10).enumerate() {
            println!("    #{}: {} — {:?}", i, cap.source_hook, cap.stat_values);
        }
    }

    // Save JSON output
    let json_output = serde_json::json!({
        "characters": characters,
        "unidentified": unidentified.len(),
        "total_captures": events.len(),
    });

    let json_path = format!("{}/character_stats.json", output_dir);
    match fs::write(
        &json_path,
        serde_json::to_string_pretty(&json_output).unwrap_or_default(),
    ) {
        Ok(()) => println!("\n  Saved: {}", json_path),
        Err(e) => println!("\n  ERROR saving {}: {}", json_path, e),
    }

    if characters.is_empty() && unidentified.is_empty() {
        println!("\n  No stat values found in captures.");
        println!("  Tips:");
        println!("    - Ensure hooks target stat-related classes");
        println!("    - Navigate character/equipment screens in-game");
        println!("    - Enter combat to trigger stat calculations");
        println!("    - Check the IL2CPP dump for better hook targets");
    }
}

// ══════════════════════════════════════════════════════════════
// Suggest mode — generate frida_char_data.js targets from dump
// ══════════════════════════════════════════════════════════════

fn suggest_targets(scored: &[ScoredClass], output_dir: &str) {
    println!("\n{}", "═".repeat(62));
    println!("  Hook Target Suggestions");
    println!("{}\n", "═".repeat(62));

    let mut all_hooks: Vec<(u32, String, HookCandidate)> = Vec::new();

    for sc in scored.iter().take(20) {
        let class_name = if sc.namespace.is_empty() {
            sc.name.clone()
        } else {
            format!("{}.{}", sc.namespace, sc.name)
        };

        for hc in &sc.hook_candidates {
            all_hooks.push((sc.score, class_name.clone(), hc.clone()));
        }
    }

    // Sort by parent class score
    all_hooks.sort_by(|a, b| b.0.cmp(&a.0));

    // Generate JS snippet
    let mut js_targets = Vec::new();

    println!("  Recommended hook targets (top 10):\n");
    for (i, (score, class_name, hc)) in all_hooks.iter().take(10).enumerate() {
        let target_name = format!("{}.{}", class_name, hc.method);
        println!(
            "  {}. {} @ {} (params={}, score={})",
            i + 1,
            target_name,
            hc.rva,
            hc.param_count,
            score
        );
        println!("     Reason: {}", hc.reason);

        js_targets.push(format!(
            "    {{ rva: {}, name: \"{}\", paramCount: {}, readFields: true }},",
            hc.rva, target_name, hc.param_count
        ));
    }

    // Output JS snippet
    let js_snippet = format!(
        "// Auto-generated hook targets from IL2CPP scan\n\
         // Paste into HOOK_TARGETS array in frida_char_data.js\n\
         var HOOK_TARGETS = [\n{}\n];",
        js_targets.join("\n")
    );

    println!("\n  Generated JS snippet:\n");
    println!("{}", js_snippet);

    // Save to file
    let snippet_path = format!("{}/hook_targets.js", output_dir);
    match fs::write(&snippet_path, &js_snippet) {
        Ok(()) => println!("\n  Saved: {}", snippet_path),
        Err(e) => println!("\n  ERROR saving {}: {}", snippet_path, e),
    }

    // Also save full scored list as JSON
    let scored_path = format!("{}/scored_classes.json", output_dir);
    let top_scored: Vec<_> = scored.iter().take(50).collect();
    match fs::write(
        &scored_path,
        serde_json::to_string_pretty(&top_scored).unwrap_or_default(),
    ) {
        Ok(()) => println!("  Saved: {}", scored_path),
        Err(e) => println!("  ERROR saving {}: {}", scored_path, e),
    }
}

// ══════════════════════════════════════════════════════════════
// Gadget format extraction (Pass 4+5 combined)
// ══════════════════════════════════════════════════════════════

fn pass_gadget_extract(raw_events: &[serde_json::Value], output_dir: &str) {
    println!("\n{}", "═".repeat(62));
    println!("  Pass 4+5: Gadget Character Stat Extraction");
    println!("{}\n", "═".repeat(62));

    let characters = extract_gadget_stats(raw_events);

    if characters.is_empty() {
        println!("  No character data found in gadget events.");
        println!("  Tips:");
        println!("    - Navigate character roster / equipment screens in-game");
        println!("    - Enter combat to trigger InitAttributes + stat getters");
        println!("    - Check il2cpp_char_data.log for hook fire confirmations");
        return;
    }

    println!("  Found {} characters with stats:\n", characters.len());

    // Sort by template_id for consistent output
    let mut chars: Vec<_> = characters.values().collect();
    chars.sort_by(|a, b| a.template_id.cmp(&b.template_id));

    // Classify characters by data source
    let table_only_count = chars
        .iter()
        .filter(|c| c.sources.iter().all(|s| s == "TABLE_CHARACTER_ENTRY"))
        .count();
    let table_plus_runtime = chars
        .iter()
        .filter(|c| {
            c.sources.iter().any(|s| s == "TABLE_CHARACTER_ENTRY")
                && c.sources.iter().any(|s| s != "TABLE_CHARACTER_ENTRY")
        })
        .count();
    let runtime_only = chars.len() - table_only_count - table_plus_runtime;

    if table_only_count > 0 || table_plus_runtime > 0 {
        println!(
            "  Sources: {} runtime-only, {} table+runtime, {} table-only\n",
            runtime_only, table_plus_runtime, table_only_count
        );
    }

    for ch in &chars {
        let source_tag = if ch.sources.iter().all(|s| s == "TABLE_CHARACTER_ENTRY") {
            " [TABLE ONLY]"
        } else if ch.sources.iter().any(|s| s == "TABLE_CHARACTER_ENTRY") {
            " [TABLE + RUNTIME]"
        } else {
            ""
        };
        println!(
            "  ╔══ {} (lv{}, break{}, potential{}){} ══",
            ch.template_id, ch.level, ch.break_stage, ch.potential_level, source_tag
        );
        println!(
            "  ║ instId={}, uid={}, captures={}",
            ch.inst_id, ch.instance_uid, ch.capture_count
        );

        // Core combat stats
        let core_stats = ["hp", "maxHp", "atk", "def", "str", "agi", "wisd", "will"];
        println!("  ║ Core stats:");
        for &stat in &core_stats {
            if let Some(&val) = ch.attributes.get(stat) {
                println!("  ║   {:20} = {:.2}", stat, val);
            }
        }

        // Secondary stats
        let secondary = [
            "criticalRate",
            "criticalDamageIncrease",
            "attackRate",
            "moveSpeedMultiplier",
            "hpRecoveryPerSec",
            "maxPoise",
            "weight",
            "healOutputIncrease",
            "lifeSteal",
        ];
        let has_secondary = secondary.iter().any(|s| ch.attributes.contains_key(*s));
        if has_secondary {
            println!("  ║ Secondary stats:");
            for &stat in &secondary {
                if let Some(&val) = ch.attributes.get(stat) {
                    println!("  ║   {:20} = {:.4}", stat, val);
                }
            }
        }

        // Damage/resistance stats
        let damage_stats: Vec<_> = ch
            .attributes
            .iter()
            .filter(|(k, _)| {
                k.contains("Damage") || k.contains("Resist") || k.contains("Infliction")
            })
            .collect();
        if !damage_stats.is_empty() {
            println!("  ║ Damage/Resistance:");
            for (k, v) in &damage_stats {
                println!("  ║   {:40} = {:.4}", k, v);
            }
        }

        // Any remaining non-zero attrs
        let shown: std::collections::HashSet<&str> =
            core_stats.iter().chain(secondary.iter()).copied().collect();
        let remaining: Vec<_> = ch
            .attributes
            .iter()
            .filter(|(k, v)| {
                !shown.contains(k.as_str())
                    && !k.contains("Damage")
                    && !k.contains("Resist")
                    && !k.contains("Infliction")
                    && **v != 0.0
            })
            .collect();
        if !remaining.is_empty() {
            println!("  ║ Other:");
            for (k, v) in &remaining {
                println!("  ║   {:40} = {:.4}", k, v);
            }
        }

        // Computed / final stats (from FINAL_ATTRIBUTES)
        if let Some(cs) = &ch.computed_stats {
            if !cs.final_attrs.is_empty() {
                println!("  ║ *** FINAL COMPUTED STATS (post-modifier) ***:");
                let final_core = ["hp", "maxHp", "atk", "def", "str", "agi", "wisd", "will"];
                for &stat in &final_core {
                    if let Some(&val) = cs.final_attrs.get(stat) {
                        let raw_val = ch.attributes.get(stat).copied().unwrap_or(0.0);
                        let delta = if raw_val != 0.0 {
                            format!(" (raw={:.2}, x{:.2})", raw_val, val / raw_val)
                        } else {
                            String::new()
                        };
                        println!("  ║   {:20} = {:.2}{}", stat, val, delta);
                    }
                }

                // Non-core final attrs
                let shown_final: std::collections::HashSet<&str> =
                    final_core.iter().copied().collect();
                let mut other_final: Vec<_> = cs
                    .final_attrs
                    .iter()
                    .filter(|(k, v)| !shown_final.contains(k.as_str()) && **v != 0.0)
                    .collect();
                other_final.sort_by(|a, b| a.0.cmp(b.0));
                if !other_final.is_empty() {
                    println!("  ║   Other final:");
                    for (k, v) in &other_final {
                        println!("  ║     {:36} = {:.4}", k, v);
                    }
                }
            }

            // Scalars
            let s = &cs.scalars;
            let has_scalars =
                s.atk_from_main.is_some() || s.atk_from_str.is_some() || s.hp_from_str.is_some();
            if has_scalars {
                println!("  ║ Cross-attribute scalars:");
                if let Some(v) = s.atk_from_main {
                    println!("  ║   atkFromMain       = {:.4}", v);
                }
                if let Some(v) = s.atk_from_str {
                    println!("  ║   atkFromStr        = {:.4}", v);
                }
                if let Some(v) = s.atk_from_agi {
                    println!("  ║   atkFromAgi        = {:.4}", v);
                }
                if let Some(v) = s.atk_from_wisd {
                    println!("  ║   atkFromWisd       = {:.4}", v);
                }
                if let Some(v) = s.atk_from_will {
                    println!("  ║   atkFromWill       = {:.4}", v);
                }
                if let Some(v) = s.hp_from_str {
                    println!("  ║   hpFromStr         = {:.4}", v);
                }
                if let Some(v) = s.heal_from_will {
                    println!("  ║   healFromWill      = {:.4}", v);
                }
                if let Some(v) = s.phys_resist_from_agi {
                    println!("  ║   physResistFromAgi = {:.4}", v);
                }
                if let Some(v) = s.energy_resist_from_wisd {
                    println!("  ║   energyResistWisd  = {:.4}", v);
                }
            }

            // Main/sub attr types
            if let Some(mat) = cs.main_attr_type {
                let name = if (mat as usize) < ATTR_NAMES.len() {
                    ATTR_NAMES[mat as usize]
                } else {
                    "?"
                };
                println!("  ║ mainAttrType = {} ({})", mat, name);
            }
            if let Some(sat) = cs.sub_attr_type {
                let name = if (sat as usize) < ATTR_NAMES.len() {
                    ATTR_NAMES[sat as usize]
                } else {
                    "?"
                };
                println!("  ║ subAttrType  = {} ({})", sat, name);
            }
        }

        println!(
            "  ╚══ rawData[{}] sources: {:?}",
            ch.raw_data.len(),
            ch.sources.iter().collect::<std::collections::HashSet<_>>()
        );
        println!();
    }

    // ── AttrPtr Profile Analysis ──
    // Group ATTR_GET_VALUE + ATTR_VALUE_SNAPSHOT by attrPtr to show stats
    let mut attr_ptr_profiles: HashMap<String, HashMap<String, f64>> = HashMap::new();
    let mut attr_ptr_counts: HashMap<String, u32> = HashMap::new();
    let mut attr_ptr_class: HashMap<String, String> = HashMap::new(); // attrPtr → className
    for ev in raw_events {
        let etype = ev.get("type").and_then(|v| v.as_str()).unwrap_or("");
        match etype {
            "ATTR_GET_VALUE" => {
                let attr_ptr = ev.get("attrPtr").and_then(|v| v.as_str()).unwrap_or("");
                if attr_ptr.is_empty() {
                    continue;
                }
                *attr_ptr_counts.entry(attr_ptr.to_string()).or_default() += 1;
                if let (Some(name), Some(val)) = (
                    ev.get("attributeName").and_then(|v| v.as_str()),
                    ev.get("value").and_then(|v| v.as_f64()),
                ) {
                    attr_ptr_profiles
                        .entry(attr_ptr.to_string())
                        .or_default()
                        .insert(name.to_string(), val);
                }
            }
            "ATTR_VALUE_SNAPSHOT" => {
                let attr_ptr = ev.get("attrPtr").and_then(|v| v.as_str()).unwrap_or("");
                if attr_ptr.is_empty() {
                    continue;
                }
                let call_count = ev.get("callCount").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
                *attr_ptr_counts.entry(attr_ptr.to_string()).or_default() += call_count;
                if let Some(attrs) = ev.get("attrs").and_then(|v| v.as_object()) {
                    let profile = attr_ptr_profiles.entry(attr_ptr.to_string()).or_default();
                    for (key, val) in attrs {
                        if let Some(n) = val.as_f64() {
                            profile.insert(key.clone(), n);
                        }
                    }
                }
            }
            "ATTR_PTR_FIRST_SEEN" => {
                // Capture class name for display
                if let Some(cn) = ev.get("attrClassName").and_then(|v| v.as_str())
                    && let Some(ap) = ev.get("attrPtr").and_then(|v| v.as_str())
                {
                    attr_ptr_class.insert(ap.to_string(), cn.to_string());
                }
            }
            _ => {}
        }
    }

    // Also collect ATTR_PTR_FIRST_SEEN events
    let first_seen_events: Vec<_> = raw_events
        .iter()
        .filter(|ev| ev.get("type").and_then(|v| v.as_str()) == Some("ATTR_PTR_FIRST_SEEN"))
        .collect();
    let ability_init_events: Vec<_> = raw_events
        .iter()
        .filter(|ev| {
            matches!(
                ev.get("type").and_then(|v| v.as_str()),
                Some("ABILITY_INIT_DEEP" | "ABILITY_INIT_ATTRIBUTES")
            )
        })
        .collect();

    // Build the attrPtr resolution map (for display)
    let attr_ptr_map = build_attr_ptr_map(raw_events);

    if !attr_ptr_profiles.is_empty() {
        // Classify attrPtrs into player/enemy/system
        let mut players: Vec<&String> = Vec::new();
        let mut enemies: Vec<&String> = Vec::new();
        let mut system: Vec<&String> = Vec::new();

        for p in attr_ptr_profiles.keys() {
            let stats = &attr_ptr_profiles[p];
            let str_val = stats.get("str").copied().unwrap_or(0.0);
            let wisd = stats.get("wisd").copied().unwrap_or(0.0);
            // Player: wisd=0, non-zero damage scalars OR str>=500 at level 1
            // Enemy: wisd=100 (rawData[3]=100 maps to wisd in GetValue)
            // System: str=1, agi=10
            if str_val <= 1.0 {
                system.push(p);
            } else if wisd >= 90.0 {
                enemies.push(p);
            } else {
                players.push(p);
            }
        }

        println!(
            "\n  ── AttrPtr Profiles ({} unique: {} player, {} enemy, {} system) ──",
            attr_ptr_profiles.len(),
            players.len(),
            enemies.len(),
            system.len()
        );
        println!(
            "  AttrPtr→Character resolution: {} mapped",
            attr_ptr_map.len()
        );

        // Show players first
        let display_group = |label: &str, ptrs: &[&String]| {
            if ptrs.is_empty() {
                return;
            }
            println!("\n  ── {} ({}) ──", label, ptrs.len());
            let mut sorted = ptrs.to_vec();
            sorted.sort();
            for p in &sorted {
                let stats = &attr_ptr_profiles[*p];
                let count = attr_ptr_counts.get(*p).copied().unwrap_or(0);
                let resolved = attr_ptr_map.get(*p);
                let class_name = attr_ptr_class.get(*p).map(|s| s.as_str()).unwrap_or("");

                let short_ptr = &p[p.len().saturating_sub(10)..];
                println!(
                    "\n    ╔══ attrPtr=..{} ({}× GetValue) {} {}",
                    short_ptr,
                    count,
                    resolved
                        .map(|r| format!("→ {}", r))
                        .unwrap_or_else(|| "→ ???".to_string()),
                    if class_name.is_empty() {
                        String::new()
                    } else {
                        format!("[{}]", class_name)
                    },
                );

                let core = ["hp", "maxHp", "atk", "def", "str", "agi", "wisd", "will"];
                for &s in &core {
                    if let Some(&v) = stats.get(s) {
                        println!("    ║   {:20} = {:.2}", s, v);
                    }
                }

                // Show damage scalars (unique per player character)
                let scalars = [
                    "physicalDamageIncrease",
                    "fireDamageIncrease",
                    "pulseDamageIncrease",
                    "crystDamageIncrease",
                    "naturalDamageIncrease",
                    "etherDamageIncrease",
                    "igniteDamageScalar",
                    "physicalInflictionDamageScalar",
                ];
                let has_scalars = scalars
                    .iter()
                    .any(|s| stats.get(*s).is_some_and(|v| *v != 0.0));
                if has_scalars {
                    println!("    ║   --- damage fingerprint ---");
                    for &s in &scalars {
                        if let Some(&v) = stats.get(s)
                            && v != 0.0
                        {
                            println!("    ║   {:40} = {:.4}", s, v);
                        }
                    }
                }

                println!("    ╚══");
            }
        };

        display_group("PLAYERS", &players);
        display_group("ENEMIES", &enemies);
        display_group("SYSTEM/DEFAULT", &system);
    }

    // AbilitySystem deep probe results
    if !ability_init_events.is_empty() {
        println!(
            "\n  ── AbilitySystem.InitAttributes: {} events ──",
            ability_init_events.len()
        );
        for (i, ev) in ability_init_events.iter().enumerate() {
            if let Some(as_data) = ev.get("abilitySystem") {
                let tid = as_data
                    .get("templateId")
                    .and_then(|v| v.as_str())
                    .unwrap_or("???");
                let attr_ptr = as_data
                    .get("attrPtr")
                    .and_then(|v| v.as_str())
                    .unwrap_or("none");
                let class_name = as_data
                    .get("className")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let tid_source = as_data
                    .get("templateIdSource")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let attr_ptrs = as_data.get("attrPtrs").and_then(|v| v.as_array());
                let n_attr_ptrs = attr_ptrs.map_or(0, |a| a.len());

                println!(
                    "    [{}] {} → attrPtr={} class={} ({}ptrs) {}",
                    i,
                    tid,
                    &attr_ptr[attr_ptr.len().saturating_sub(10)..],
                    class_name,
                    n_attr_ptrs,
                    if tid_source.is_empty() {
                        String::new()
                    } else {
                        format!("via {}", tid_source)
                    },
                );

                // Show string dumps (v7)
                if let Some(strings) = as_data.get("stringDump").and_then(|v| v.as_array()) {
                    println!("      String dump ({} strings):", strings.len());
                    for s in strings.iter().take(20) {
                        let off = s.get("offset").and_then(|v| v.as_i64()).unwrap_or(0);
                        let val = s.get("value").and_then(|v| v.as_str()).unwrap_or("?");
                        println!("        +{}: \"{}\"", off, val);
                    }
                    if strings.len() > 20 {
                        println!("        ... and {} more", strings.len() - 20);
                    }
                }
                if let Some(attr_strings) = as_data.get("attrStringDump").and_then(|v| v.as_array())
                {
                    println!("      Attributes string dump ({}):", attr_strings.len());
                    for s in attr_strings.iter().take(10) {
                        let off = s.get("offset").and_then(|v| v.as_i64()).unwrap_or(0);
                        let val = s.get("value").and_then(|v| v.as_str()).unwrap_or("?");
                        println!("        +{}: \"{}\"", off, val);
                    }
                }

                // Show reflection fields (only for first event)
                if i == 0
                    && let Some(fields) = as_data.get("reflectionFields").and_then(|v| v.as_array())
                {
                    println!("    Reflected fields ({}):", fields.len());
                    for f in fields {
                        let name = f.get("name").and_then(|v| v.as_str()).unwrap_or("?");
                        let offset = f.get("offset").and_then(|v| v.as_i64()).unwrap_or(0);
                        let ftype = f.get("type").and_then(|v| v.as_str()).unwrap_or("?");
                        let val_str = f.get("valueStr").and_then(|v| v.as_str());
                        print!("      {:30} off={:3} type={}", name, offset, ftype);
                        if let Some(vs) = val_str {
                            print!("  val=\"{}\"", vs);
                        }
                        println!();
                    }
                }
            }
        }
    }

    // ATTR_PTR_FIRST_SEEN summary
    if !first_seen_events.is_empty() {
        println!(
            "\n  ── ATTR_PTR_FIRST_SEEN: {} unique attrPtrs ──",
            first_seen_events.len()
        );
        for ev in &first_seen_events {
            let ptr = ev.get("attrPtr").and_then(|v| v.as_str()).unwrap_or("?");
            let tid = ev.get("templateId").and_then(|v| v.as_str());
            let during = ev
                .get("duringInitAttributes")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let back_src = ev
                .get("backProbeSource")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let class_name = ev
                .get("attrClassName")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let short_ptr = &ptr[ptr.len().saturating_sub(10)..];
            println!(
                "    ..{} → {} {} {} {}",
                short_ptr,
                tid.unwrap_or("unresolved"),
                if during { "[initAttr]" } else { "" },
                if back_src.is_empty() {
                    String::new()
                } else {
                    format!("(via {})", back_src)
                },
                if class_name.is_empty() {
                    String::new()
                } else {
                    format!("[{}]", class_name)
                },
            );
        }
    }

    // Save JSON output
    let json_output = serde_json::json!({
        "format": "gadget",
        "character_count": characters.len(),
        "characters": characters,
        "attr_ptr_map": attr_ptr_map,
        "attr_ptr_profiles": attr_ptr_profiles.len(),
        "attr_names": ATTR_NAMES,
    });

    let _ = fs::create_dir_all(output_dir);
    let json_path = format!("{}/character_stats.json", output_dir);
    match fs::write(
        &json_path,
        serde_json::to_string_pretty(&json_output).unwrap_or_default(),
    ) {
        Ok(()) => println!("  Saved: {}", json_path),
        Err(e) => println!("  ERROR saving {}: {}", json_path, e),
    }

    // Save per-character CSV for easy spreadsheet import
    let csv_path = format!("{}/character_stats.csv", output_dir);
    let mut csv_lines = Vec::new();

    // Collect all raw attribute names
    let mut all_attr_names: Vec<String> = chars
        .iter()
        .flat_map(|c| c.attributes.keys().cloned())
        .collect::<std::collections::HashSet<_>>()
        .into_iter()
        .collect();
    all_attr_names.sort();

    // Collect all final attribute names
    let mut all_final_names: Vec<String> = chars
        .iter()
        .filter_map(|c| c.computed_stats.as_ref())
        .flat_map(|cs| cs.final_attrs.keys().cloned())
        .collect::<std::collections::HashSet<_>>()
        .into_iter()
        .collect();
    all_final_names.sort();

    let scalar_cols = [
        "atkFromMain",
        "atkFromStr",
        "atkFromAgi",
        "atkFromWisd",
        "atkFromWill",
        "hpFromStr",
        "healFromWill",
        "physResistFromAgi",
        "energyResistFromWisd",
    ];

    // Header
    let raw_headers: Vec<String> = all_attr_names
        .iter()
        .map(|n| format!("raw_{}", n))
        .collect();
    let final_headers: Vec<String> = all_final_names
        .iter()
        .map(|n| format!("final_{}", n))
        .collect();
    let scalar_headers: Vec<String> = scalar_cols.iter().map(|s| s.to_string()).collect();

    csv_lines.push(format!(
        "templateId,level,breakStage,potentialLevel,mainAttrType,subAttrType,{},{},{}",
        raw_headers.join(","),
        final_headers.join(","),
        scalar_headers.join(","),
    ));

    for ch in &chars {
        let raw_values: Vec<String> = all_attr_names
            .iter()
            .map(|name| {
                ch.attributes
                    .get(name)
                    .map(|v| format!("{:.4}", v))
                    .unwrap_or_default()
            })
            .collect();

        let cs = ch.computed_stats.as_ref();

        let final_values: Vec<String> = all_final_names
            .iter()
            .map(|name| {
                cs.and_then(|c| c.final_attrs.get(name))
                    .map(|v| format!("{:.4}", v))
                    .unwrap_or_default()
            })
            .collect();

        let scalar_values: Vec<String> = if let Some(c) = cs {
            let s = &c.scalars;
            vec![
                s.atk_from_main
                    .map(|v| format!("{:.6}", v))
                    .unwrap_or_default(),
                s.atk_from_str
                    .map(|v| format!("{:.6}", v))
                    .unwrap_or_default(),
                s.atk_from_agi
                    .map(|v| format!("{:.6}", v))
                    .unwrap_or_default(),
                s.atk_from_wisd
                    .map(|v| format!("{:.6}", v))
                    .unwrap_or_default(),
                s.atk_from_will
                    .map(|v| format!("{:.6}", v))
                    .unwrap_or_default(),
                s.hp_from_str
                    .map(|v| format!("{:.6}", v))
                    .unwrap_or_default(),
                s.heal_from_will
                    .map(|v| format!("{:.6}", v))
                    .unwrap_or_default(),
                s.phys_resist_from_agi
                    .map(|v| format!("{:.6}", v))
                    .unwrap_or_default(),
                s.energy_resist_from_wisd
                    .map(|v| format!("{:.6}", v))
                    .unwrap_or_default(),
            ]
        } else {
            vec![String::new(); scalar_cols.len()]
        };

        let main_attr = cs
            .and_then(|c| c.main_attr_type)
            .map(|v| v.to_string())
            .unwrap_or_default();
        let sub_attr = cs
            .and_then(|c| c.sub_attr_type)
            .map(|v| v.to_string())
            .unwrap_or_default();

        csv_lines.push(format!(
            "{},{},{},{},{},{},{},{},{}",
            ch.template_id,
            ch.level,
            ch.break_stage,
            ch.potential_level,
            main_attr,
            sub_attr,
            raw_values.join(","),
            final_values.join(","),
            scalar_values.join(","),
        ));
    }

    match fs::write(&csv_path, csv_lines.join("\n")) {
        Ok(()) => println!("  Saved: {}", csv_path),
        Err(e) => println!("  ERROR saving {}: {}", csv_path, e),
    }
}

// ══════════════════════════════════════════════════════════════
// Config Table Analysis (hooks 7-10 events)
// ══════════════════════════════════════════════════════════════

#[derive(Debug, Default, Serialize)]
struct ConfigTableLookup {
    /// Category/attribute type ID (1=primary, 20=secondary, 6=tertiary)
    category: i32,
    sub_index: i32,
    /// Raw pointer to the Double[103] array (for grouping by character)
    array_ptr: String,
    arg3: i32,
    initialized: bool,
    /// IL2CPP array length
    arr_len: Option<i32>,
    /// Index 55 marker value
    marker: Option<f64>,
    /// Number of attribute slots (from index 55 marker)
    attr_count: Option<f64>,
    /// Decoded stat values keyed by ATTR_NAMES
    stats: Option<HashMap<String, f64>>,
    /// Diagnostic error string
    error: Option<String>,
    /// Debug: non-zero doubles from first few events
    debug_non_zero: Option<Vec<serde_json::Value>>,
    /// Stack backtrace (first 5 calls)
    backtrace: Option<Vec<String>>,
    /// Character ID found on stack probe
    stack_char_id: Option<String>,
    retval: i32,
    timestamp: u64,
}

#[derive(Debug, Default, Serialize)]
struct CalcBaseEvent {
    args: Vec<serde_json::Value>,
    retval: Option<serde_json::Value>,
    timestamp: u64,
}

#[derive(Debug, Default, Serialize)]
struct CalcFinalEvent {
    args: Vec<serde_json::Value>,
    retval: Option<serde_json::Value>,
    timestamp: u64,
}

fn pass_config_table_analysis(raw_events: &[serde_json::Value], output_dir: &str) {
    println!("\n{}", "═".repeat(62));
    println!("  Config Table Pipeline Analysis");
    println!("{}\n", "═".repeat(62));

    let mut table_lookups: Vec<ConfigTableLookup> = Vec::new();
    let mut base_calcs: Vec<CalcBaseEvent> = Vec::new();
    let mut final_calcs: Vec<CalcFinalEvent> = Vec::new();
    let mut table_dumps: Vec<serde_json::Value> = Vec::new();

    for ev in raw_events {
        let event_type = ev.get("type").and_then(|v| v.as_str()).unwrap_or("");

        match event_type {
            "CONFIG_TABLE_LOOKUP" => {
                // Decode stats object
                let stats = ev
                    .get("stats")
                    .and_then(|v| v.as_object())
                    .map(|obj| {
                        obj.iter()
                            .filter_map(|(k, v)| v.as_f64().map(|val| (k.clone(), val)))
                            .collect::<HashMap<String, f64>>()
                    })
                    .filter(|s| !s.is_empty());

                let lookup = ConfigTableLookup {
                    category: ev.get("category").and_then(|v| v.as_i64()).unwrap_or(0) as i32,
                    sub_index: ev.get("subIndex").and_then(|v| v.as_i64()).unwrap_or(0) as i32,
                    array_ptr: ev
                        .get("arrayPtr")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    arg3: ev.get("arg3").and_then(|v| v.as_i64()).unwrap_or(0) as i32,
                    initialized: ev
                        .get("initialized")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false),
                    arr_len: ev.get("arrLen").and_then(|v| v.as_i64()).map(|v| v as i32),
                    marker: ev.get("marker").and_then(|v| v.as_f64()),
                    attr_count: ev.get("attrCount").and_then(|v| v.as_f64()),
                    stats,
                    error: ev
                        .get("error")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string()),
                    debug_non_zero: ev.get("debugNonZero").and_then(|v| v.as_array()).cloned(),
                    backtrace: ev.get("backtrace").and_then(|v| v.as_array()).map(|arr| {
                        arr.iter()
                            .filter_map(|v| v.as_str().map(|s| s.to_string()))
                            .collect()
                    }),
                    stack_char_id: ev
                        .get("stackCharId")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string()),
                    retval: ev.get("retval").and_then(|v| v.as_i64()).unwrap_or(0) as i32,
                    timestamp: ev.get("timestamp").and_then(|v| v.as_u64()).unwrap_or(0),
                };

                table_lookups.push(lookup);
            }

            "CALC_BASE_ATTRIBUTE" => {
                let mut calc = CalcBaseEvent::default();
                if let Some(args) = ev.get("args").and_then(|v| v.as_array()) {
                    calc.args = args.clone();
                }
                calc.retval = ev.get("retval").cloned();
                calc.timestamp = ev.get("timestamp").and_then(|v| v.as_u64()).unwrap_or(0);
                base_calcs.push(calc);
            }

            "CALC_FINAL_ATTRIBUTE" => {
                let mut calc = CalcFinalEvent::default();
                if let Some(args) = ev.get("args").and_then(|v| v.as_array()) {
                    calc.args = args.clone();
                }
                calc.retval = ev.get("retval").cloned();
                calc.timestamp = ev.get("timestamp").and_then(|v| v.as_u64()).unwrap_or(0);
                final_calcs.push(calc);
            }

            "CHARACTER_ATTRIBUTE_TABLE_DUMP" => {
                table_dumps.push(ev.clone());
            }

            _ => {}
        }
    }

    // Report TABLE_DEEP_PROBE events (v10)
    let deep_probes: Vec<_> = raw_events
        .iter()
        .filter(|ev| ev.get("type").and_then(|v| v.as_str()) == Some("TABLE_DEEP_PROBE"))
        .collect();
    if !deep_probes.is_empty() {
        println!("\n  ── TABLE_DEEP_PROBE (v10 object graph) ──");
        for probe in &deep_probes {
            let class_name = probe
                .get("className")
                .and_then(|v| v.as_str())
                .unwrap_or("?");
            let table_ptr = probe
                .get("tablePtr")
                .and_then(|v| v.as_str())
                .unwrap_or("?");
            println!("    Table: {} @ {}", class_name, table_ptr);

            if let Some(graph) = probe.get("graph")
                && let Some(fields) = graph.get("fields").and_then(|v| v.as_array())
            {
                println!("    Fields ({}):", fields.len());
                for f in fields {
                    let offset = f.get("offset").and_then(|v| v.as_i64()).unwrap_or(0);
                    let f_ptr = f.get("ptr").and_then(|v| v.as_str()).unwrap_or("?");
                    let f_cn = f.get("className").and_then(|v| v.as_str());
                    let f_str = f.get("stringValue").and_then(|v| v.as_str());
                    let arr_len = f.get("possibleArrayLen").and_then(|v| v.as_i64());

                    let short_ptr = &f_ptr[f_ptr.len().saturating_sub(10)..];
                    print!("      +{:3}: ..{}", offset, short_ptr);
                    if let Some(cn) = f_cn {
                        print!("  [{}]", cn);
                    }
                    if let Some(s) = f_str {
                        print!("  =\"{}\"", s);
                    }
                    if let Some(al) = arr_len {
                        print!("  arrLen={}", al);
                    }
                    println!();

                    // Show children (depth 1)
                    if let Some(children) = f.get("children").and_then(|v| v.as_array()) {
                        for child in children.iter().take(10) {
                            let c_off = child.get("offset").and_then(|v| v.as_i64()).unwrap_or(0);
                            let c_cn = child.get("className").and_then(|v| v.as_str());
                            let c_str = child.get("stringValue").and_then(|v| v.as_str());
                            let c_arr = child.get("possibleArrayLen").and_then(|v| v.as_i64());
                            print!("        +{}→+{}", offset, c_off);
                            if let Some(cn) = c_cn {
                                print!("  [{}]", cn);
                            }
                            if let Some(s) = c_str {
                                print!("  =\"{}\"", s);
                            }
                            if let Some(al) = c_arr {
                                print!("  arrLen={}", al);
                            }
                            println!();
                        }
                        if children.len() > 10 {
                            println!("        ... +{} more", children.len() - 10);
                        }
                    }
                }
            }
        }
    }

    // Report TABLE_COLUMN_DUMP events (v11)
    let column_dumps: Vec<_> = raw_events
        .iter()
        .filter(|ev| ev.get("type").and_then(|v| v.as_str()) == Some("TABLE_COLUMN_DUMP"))
        .collect();
    if !column_dumps.is_empty() {
        println!("\n  ── TABLE_COLUMN_DUMP (v11 bare pointer recon) ──");
        for cd in &column_dumps {
            let col_idx = cd.get("columnIndex").and_then(|v| v.as_i64()).unwrap_or(-1);
            let col_off = cd.get("columnOffset").and_then(|v| v.as_i64()).unwrap_or(0);
            let klass_name = cd
                .get("klassName")
                .and_then(|v| v.as_str())
                .unwrap_or("(null)");
            let data_ptr = cd.get("dataPtr").and_then(|v| v.as_str()).unwrap_or("?");
            let klass_ptr = cd.get("klassPtr").and_then(|v| v.as_str()).unwrap_or("?");

            println!(
                "\n    Column {} (offset +{}): klass={} @ {}",
                col_idx, col_off, klass_name, klass_ptr
            );
            println!("      dataPtr: {}", data_ptr);

            if let Some(probe) = cd.get("probe") {
                // Hex dump — format as rows of 16 bytes (32 hex chars + 7 spaces)
                if let Some(hex) = probe.get("hex").and_then(|v| v.as_str()) {
                    println!("      Data hex dump (256 bytes):");
                    // Strip spaces for clean re-chunking
                    let clean: String = hex.chars().filter(|c| !c.is_whitespace()).collect();
                    for row in 0..16 {
                        let start = row * 32;
                        let end = (start + 32).min(clean.len());
                        if start >= clean.len() {
                            break;
                        }
                        let row_hex = &clean[start..end];
                        // Insert space every 2 chars for readability
                        let formatted: String = row_hex
                            .as_bytes()
                            .chunks(2)
                            .map(|c| std::str::from_utf8(c).unwrap_or("??"))
                            .collect::<Vec<_>>()
                            .join(" ");
                        println!("        {:04x}: {}", row * 16, formatted);
                    }
                }

                // Header hex (64 bytes before data ptr)
                if let Some(hdr) = probe.get("headerHex").and_then(|v| v.as_str())
                    && !hdr.is_empty()
                    && !hdr.starts_with('(')
                {
                    let clean: String = hdr.chars().filter(|c| !c.is_whitespace()).collect();
                    println!("      Header hex (64B before dataPtr):");
                    for row in 0..4 {
                        let start = row * 32;
                        let end = (start + 32).min(clean.len());
                        if start >= clean.len() {
                            break;
                        }
                        let row_hex = &clean[start..end];
                        let formatted: String = row_hex
                            .as_bytes()
                            .chunks(2)
                            .map(|c| std::str::from_utf8(c).unwrap_or("??"))
                            .collect::<Vec<_>>()
                            .join(" ");
                        println!("        -{:04x}: {}", (4 - row) * 16, formatted);
                    }
                }

                // Array header detection
                if let Some(ak) = probe.get("arrayHeaderKlass").and_then(|v| v.as_str()) {
                    let al = probe
                        .get("arrayHeaderLen")
                        .and_then(|v| v.as_i64())
                        .unwrap_or(0);
                    println!("      Array header: klass={} len={}", ak, al);
                }

                // Pointer fields
                if let Some(pfs) = probe.get("pointerFields").and_then(|v| v.as_array())
                    && !pfs.is_empty()
                {
                    println!("      Pointer fields ({}):", pfs.len());
                    for pf in pfs.iter().take(16) {
                        let off = pf.get("offset").and_then(|v| v.as_i64()).unwrap_or(0);
                        let p = pf.get("ptr").and_then(|v| v.as_str()).unwrap_or("?");
                        let short_p = &p[p.len().saturating_sub(12)..];
                        print!("        +{:3}: ..{}", off, short_p);
                        if let Some(cn) = pf.get("className").and_then(|v| v.as_str()) {
                            print!("  obj=[{}]", cn);
                        }
                        if let Some(kn) = pf.get("klassName").and_then(|v| v.as_str()) {
                            print!("  klass=[{}]", kn);
                        }
                        if let Some(sv) = pf.get("stringValue").and_then(|v| v.as_str()) {
                            print!("  str=\"{}\"", sv);
                        }
                        println!();
                    }
                    if pfs.len() > 16 {
                        println!("        ... +{} more", pfs.len() - 16);
                    }
                }

                // String fields
                if let Some(sfs) = probe.get("stringFields").and_then(|v| v.as_array())
                    && !sfs.is_empty()
                {
                    println!("      String fields ({}):", sfs.len());
                    for sf in sfs.iter().take(10) {
                        let off = sf.get("offset").and_then(|v| v.as_i64()).unwrap_or(0);
                        let val = sf.get("value").and_then(|v| v.as_str()).unwrap_or("?");
                        println!("        +{}: \"{}\"", off, val);
                    }
                }

                // Double values (stat-like)
                if let Some(dvs) = probe.get("doubleValues").and_then(|v| v.as_array())
                    && !dvs.is_empty()
                {
                    println!("      Double values ({}):", dvs.len());
                    for dv in dvs.iter().take(20) {
                        let off = dv.get("offset").and_then(|v| v.as_i64()).unwrap_or(0);
                        let idx = dv.get("index").and_then(|v| v.as_i64()).unwrap_or(0);
                        let val = dv.get("value").and_then(|v| v.as_f64()).unwrap_or(0.0);
                        println!("        +{} [{}]: {:.6}", off, idx, val);
                    }
                    if dvs.len() > 20 {
                        println!("        ... +{} more", dvs.len() - 20);
                    }
                }

                // Int32 fields
                if let Some(ifs) = probe.get("int32Fields").and_then(|v| v.as_array())
                    && !ifs.is_empty()
                {
                    println!("      Int32 fields ({}):", ifs.len());
                    for i32f in ifs.iter().take(16) {
                        let off = i32f.get("offset").and_then(|v| v.as_i64()).unwrap_or(0);
                        let val = i32f.get("value").and_then(|v| v.as_i64()).unwrap_or(0);
                        println!("        +{}: {} (0x{:X})", off, val, val as u32);
                    }
                }
            }
        }
    }

    // Report TABLE_BLOCK_HEADER events (v11)
    let block_headers: Vec<_> = raw_events
        .iter()
        .filter(|ev| ev.get("type").and_then(|v| v.as_str()) == Some("TABLE_BLOCK_HEADER"))
        .collect();
    if !block_headers.is_empty() {
        println!("\n  ── TABLE_BLOCK_HEADER (v11 contiguous allocation) ──");
        for bh in &block_headers {
            let lowest = bh
                .get("lowestDataPtr")
                .and_then(|v| v.as_str())
                .unwrap_or("?");
            let highest = bh
                .get("highestDataPtr")
                .and_then(|v| v.as_str())
                .unwrap_or("?");
            let klass_name = bh
                .get("klassName")
                .and_then(|v| v.as_str())
                .unwrap_or("(null)");
            let arr_len = bh
                .get("possibleArrayLen")
                .and_then(|v| v.as_i64())
                .unwrap_or(0);

            println!("    Range: {} .. {}", lowest, highest);
            println!(
                "    Block klass: {} | possibleArrayLen: {}",
                klass_name, arr_len
            );

            if let Some(hex) = bh.get("headerHex").and_then(|v| v.as_str()) {
                let clean: String = hex.chars().filter(|c| !c.is_whitespace()).collect();
                println!("    Header hex (32B before lowest dataPtr):");
                for row in 0..2 {
                    let start = row * 32;
                    let end = (start + 32).min(clean.len());
                    if start >= clean.len() {
                        break;
                    }
                    let row_hex = &clean[start..end];
                    let formatted: String = row_hex
                        .as_bytes()
                        .chunks(2)
                        .map(|c| std::str::from_utf8(c).unwrap_or("??"))
                        .collect::<Vec<_>>()
                        .join(" ");
                    println!("      -{:04x}: {}", (2 - row) * 16, formatted);
                }

                // Interpret as pointers
                if clean.len() >= 32 {
                    println!("    Interpreted as qwords:");
                    for qi in 0..4 {
                        let qstart = qi * 16;
                        let qend = (qstart + 16).min(clean.len());
                        if qstart >= clean.len() {
                            break;
                        }
                        let qhex = &clean[qstart..qend];
                        // Reverse bytes for little-endian qword
                        let le_bytes: String = qhex
                            .as_bytes()
                            .chunks(2)
                            .rev()
                            .map(|c| std::str::from_utf8(c).unwrap_or("??"))
                            .collect::<Vec<_>>()
                            .join("");
                        println!("      qword[{}]: 0x{} (LE: {})", qi, le_bytes, qhex);
                    }
                }
            }
        }
    }

    // Report TABLE_DICT_SUMMARY events (v10)
    let dict_summaries: Vec<_> = raw_events
        .iter()
        .filter(|ev| ev.get("type").and_then(|v| v.as_str()) == Some("TABLE_DICT_SUMMARY"))
        .collect();
    if !dict_summaries.is_empty() {
        println!("\n  ── Dictionary Summaries (v10) ──");
        for ds in &dict_summaries {
            let offset = ds
                .get("dictOffset")
                .and_then(|v| v.as_str())
                .or_else(|| ds.get("dictOffset").and_then(|v| v.as_i64()).map(|_| ""));
            let offset_str = offset.unwrap_or("?");
            // Handle both string and int offset
            let offset_display = if offset_str.is_empty() {
                ds.get("dictOffset")
                    .and_then(|v| v.as_i64())
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| "?".to_string())
            } else {
                offset_str.to_string()
            };
            let dict_cn = ds
                .get("dictClassName")
                .and_then(|v| v.as_str())
                .unwrap_or("?");
            let count = ds.get("count").and_then(|v| v.as_i64()).unwrap_or(0);
            let stride = ds.get("stride").and_then(|v| v.as_i64()).unwrap_or(0);
            let chr_count = ds
                .get("chrEntryCount")
                .and_then(|v| v.as_i64())
                .unwrap_or(0);

            println!(
                "    Dict at +{}: {} entries (stride={}), {} chr_ keys [{}]",
                offset_display, count, stride, chr_count, dict_cn
            );

            if let Some(keys) = ds.get("sampleKeys").and_then(|v| v.as_array()) {
                let key_strs: Vec<&str> = keys.iter().filter_map(|v| v.as_str()).take(10).collect();
                if !key_strs.is_empty() {
                    println!("      Sample keys: {}", key_strs.join(", "));
                }
            }
        }
    }

    // Report TABLE_CHARACTER_ENTRY events (v10)
    let table_entries: Vec<_> = raw_events
        .iter()
        .filter(|ev| ev.get("type").and_then(|v| v.as_str()) == Some("TABLE_CHARACTER_ENTRY"))
        .collect();
    if !table_entries.is_empty() {
        // Separate chr_ entries from others
        let chr_entries: Vec<_> = table_entries
            .iter()
            .filter(|ev| {
                ev.get("charId")
                    .and_then(|v| v.as_str())
                    .is_some_and(|s| s.starts_with("chr_"))
            })
            .collect();
        let other_entries: Vec<_> = table_entries
            .iter()
            .filter(|ev| {
                !ev.get("charId")
                    .and_then(|v| v.as_str())
                    .is_some_and(|s| s.starts_with("chr_"))
            })
            .collect();

        println!(
            "\n  ── Table Character Entries (v10): {} total ({} chr_, {} other) ──",
            table_entries.len(),
            chr_entries.len(),
            other_entries.len()
        );

        for entry in &chr_entries {
            let char_id = entry.get("charId").and_then(|v| v.as_str()).unwrap_or("?");
            let val_cn = entry
                .get("valueClassName")
                .and_then(|v| v.as_str())
                .unwrap_or("");

            print!("    {} [{}]", char_id, val_cn);

            if let Some(stats) = entry.get("stats").and_then(|v| v.as_object()) {
                let core = ["hp", "str", "agi", "wisd", "will", "atk", "def", "maxHp"];
                let mut shown = Vec::new();
                for &s in &core {
                    if let Some(v) = stats.get(s).and_then(|v| v.as_f64())
                        && v != 0.0
                    {
                        shown.push(format!("{}={:.0}", s, v));
                    }
                }
                if !shown.is_empty() {
                    print!(" — {}", shown.join(", "));
                }
                let total_stats = stats.len();
                if total_stats > shown.len() {
                    print!(" (+{} more)", total_stats - shown.len());
                }
            } else {
                print!(" — no stats");
            }
            println!();

            // Show string fields for identification
            if let Some(strings) = entry.get("stringFields").and_then(|v| v.as_array()) {
                for s in strings.iter().take(3) {
                    let soff = s.get("offset").and_then(|v| v.as_i64()).unwrap_or(0);
                    let sval = s.get("value").and_then(|v| v.as_str()).unwrap_or("?");
                    println!("      +{}: \"{}\"", soff, sval);
                }
            }
        }

        if !other_entries.is_empty() {
            println!("    ({} non-chr_ entries omitted)", other_entries.len());
        }
    }

    // Report table dumps
    if !table_dumps.is_empty() {
        println!("  Character Attribute Table Dumps: {}", table_dumps.len());
        for (i, dump) in table_dumps.iter().enumerate() {
            let status = dump.get("status").and_then(|v| v.as_str()).unwrap_or("?");
            println!("\n  ── Table Dump #{} (status: {}) ──", i + 1, status);

            if let Some(table) = dump.get("table") {
                if let Some(cn) = table.get("className").and_then(|v| v.as_str()) {
                    let ns = table
                        .get("classNamespace")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    println!("    Class: {}.{}", ns, cn);
                }
                if let Some(ptr_str) = table.get("ptr").and_then(|v| v.as_str()) {
                    println!("    Ptr: {}", ptr_str);
                }

                // Show fields from reflection
                if let Some(fields) = table.get("fields").and_then(|v| v.as_array()) {
                    println!("    Fields ({}):", fields.len());
                    for f in fields {
                        let name = f.get("name").and_then(|v| v.as_str()).unwrap_or("?");
                        let offset = f.get("offset").and_then(|v| v.as_i64()).unwrap_or(0);
                        let arr_len = f.get("arrayLength").and_then(|v| v.as_i64());
                        let val_ptr = f.get("valuePtr").and_then(|v| v.as_str());

                        print!("      {:30} offset={}", name, offset);
                        if let Some(vp) = val_ptr {
                            print!("  ptr={}", vp);
                        }
                        if let Some(al) = arr_len {
                            print!("  arrayLen={}", al);
                        }
                        println!();
                    }
                }

                // Show string fields from table scan
                if let Some(strings) = table.get("stringFields").and_then(|v| v.as_array()) {
                    println!("    String fields ({}):", strings.len());
                    for s in strings {
                        let off = s.get("offset").and_then(|v| v.as_i64()).unwrap_or(0);
                        let val = s.get("value").and_then(|v| v.as_str()).unwrap_or("?");
                        println!("      +{}: \"{}\"", off, val);
                    }
                }

                // Show dictionary probe results
                if let Some(count) = table.get("possibleDictEntries").and_then(|v| v.as_i64()) {
                    println!("    Possible dictionary entries: {}", count);
                }
                if let Some(samples) = table.get("sampleEntries").and_then(|v| v.as_array()) {
                    println!("    Sample entries ({}):", samples.len());
                    for (si, entry) in samples.iter().enumerate() {
                        let hash = entry.get("hashCode").and_then(|v| v.as_i64()).unwrap_or(0);
                        // Check multiple key offsets
                        let mut keys = Vec::new();
                        for koff in [8, 16, 24] {
                            let field = format!("keyAt{}", koff);
                            if let Some(k) = entry.get(&field).and_then(|v| v.as_str()) {
                                keys.push(format!("+{}=\"{}\"", koff, k));
                            }
                        }
                        // Fallback to old field name
                        if keys.is_empty()
                            && let Some(k) = entry.get("keyAsString").and_then(|v| v.as_str())
                        {
                            keys.push(format!("key=\"{}\"", k));
                        }
                        let key_str = if keys.is_empty() {
                            "(no string key)".to_string()
                        } else {
                            keys.join(" ")
                        };
                        println!("      [{}] hash={} {}", si, hash, key_str);
                        if let Some(vh) = entry.get("valueHex").and_then(|v| v.as_str()) {
                            println!("           valueHex: {}...", &vh[..vh.len().min(96)]);
                        }
                    }
                }

                // Show hex dump (truncated)
                if let Some(hex) = table.get("hexDump256").and_then(|v| v.as_str()) {
                    println!(
                        "    Hex dump (first 64 bytes): {}...",
                        &hex[..hex.len().min(192)]
                    );
                }
            }

            if let Some(err) = dump.get("error").and_then(|v| v.as_str()) {
                println!("    Error: {}", err);
            }
        }
    } else {
        println!("  No CHARACTER_ATTRIBUTE_TABLE_DUMP events.");
        println!("  DataManager.OnInit may not have fired yet.");
    }

    // Report table lookups
    println!("\n  Config Table Lookups: {}", table_lookups.len());
    if !table_lookups.is_empty() {
        let init_count = table_lookups.iter().filter(|l| l.initialized).count();
        let uninit_count = table_lookups.len() - init_count;
        println!(
            "    {} initialized, {} uninitialized",
            init_count, uninit_count
        );

        // Group by arrayPtr (each unique ptr = a distinct character's data)
        let mut by_ptr: HashMap<&str, Vec<&ConfigTableLookup>> = HashMap::new();
        for lookup in &table_lookups {
            by_ptr.entry(&lookup.array_ptr).or_default().push(lookup);
        }
        println!("    {} unique array pointers (≈ characters)", by_ptr.len());

        // Category distribution
        let mut cat_counts: HashMap<i32, usize> = HashMap::new();
        for lookup in &table_lookups {
            *cat_counts.entry(lookup.category).or_default() += 1;
        }
        let mut cats: Vec<_> = cat_counts.iter().collect();
        cats.sort_by_key(|(k, _)| **k);
        print!("    Categories:");
        for (cat, count) in &cats {
            print!(" {}(×{})", cat, count);
        }
        println!();

        // Print stat tables for initialized lookups
        for lookup in table_lookups.iter().filter(|l| l.initialized) {
            println!(
                "\n    ── Config Stats (cat={} ptr={}) ──",
                lookup.category,
                &lookup.array_ptr[lookup.array_ptr.len().saturating_sub(10)..]
            );
            if let Some(stats) = &lookup.stats {
                let mut entries: Vec<_> = stats.iter().collect();
                entries.sort_by_key(|(k, _)| k.to_string());
                for (name, val) in &entries {
                    if **val != 0.0 {
                        println!("      {:40} = {:.6}", name, val);
                    }
                }
            }
            println!(
                "      attrCount={:.0} retval=0x{:X}",
                lookup.attr_count.unwrap_or(0.0),
                lookup.retval as u32
            );
        }

        // Show first few lookup summaries with diagnostics
        println!(
            "\n    First {} lookup details:",
            table_lookups.len().min(10)
        );
        for (i, lookup) in table_lookups.iter().take(10).enumerate() {
            let stat_count = lookup.stats.as_ref().map_or(0, |s| s.len());
            println!(
                "      [{}] cat={} sub={} init={} stats={} arrLen={:?} marker={:?} ptr=..{} ret=0x{:X}",
                i,
                lookup.category,
                lookup.sub_index,
                lookup.initialized,
                stat_count,
                lookup.arr_len,
                lookup.marker,
                &lookup.array_ptr[lookup.array_ptr.len().saturating_sub(8)..],
                lookup.retval as u32,
            );
            if let Some(char_id) = &lookup.stack_char_id {
                println!("             >>> STACK CHAR ID: {}", char_id);
            }
            if let Some(bt) = &lookup.backtrace {
                println!("             backtrace:");
                for frame in bt.iter().take(5) {
                    println!("               {}", frame);
                }
            }
            if let Some(err) = &lookup.error {
                println!("             error: {}", err);
            }
            if let Some(nz) = &lookup.debug_non_zero {
                println!("             non-zero doubles ({}):", nz.len());
                for entry in nz.iter().take(20) {
                    let idx = entry.get("idx").and_then(|v| v.as_i64()).unwrap_or(-1);
                    let val = entry.get("val").and_then(|v| v.as_f64()).unwrap_or(0.0);
                    let name = if idx >= 56 && (idx - 56) < ATTR_NAMES.len() as i64 {
                        ATTR_NAMES[(idx - 56) as usize]
                    } else {
                        ""
                    };
                    if name.is_empty() {
                        println!("               [{:3}] = {:.6}", idx, val);
                    } else {
                        println!("               [{:3}] = {:.6}  ({})", idx, val, name);
                    }
                }
                if nz.len() > 20 {
                    println!("               ... and {} more", nz.len() - 20);
                }
            }
        }
    }

    // Report base attribute calculations
    println!("\n  Base Attribute Calculations: {}", base_calcs.len());
    if !base_calcs.is_empty() {
        println!("    First {} events:", base_calcs.len().min(5));
        for (i, calc) in base_calcs.iter().take(5).enumerate() {
            println!("      [{}] {} args:", i, calc.args.len());
            for (ai, arg) in calc.args.iter().enumerate() {
                let raw = arg.get("raw").and_then(|v| v.as_str()).unwrap_or("?");
                let as_int = arg.get("asInt32").and_then(|v| v.as_i64());
                print!("        arg[{}]: raw={}", ai, raw);
                if let Some(i) = as_int {
                    print!(" int32={}", i);
                }
                println!();
            }
            if let Some(ret) = &calc.retval {
                let raw = ret.get("raw").and_then(|v| v.as_str()).unwrap_or("?");
                println!("        retval: raw={}", raw);
            }
        }
    }

    // Report final attribute calculations
    println!("\n  Final Attribute Calculations: {}", final_calcs.len());
    if !final_calcs.is_empty() {
        println!("    First {} events:", final_calcs.len().min(5));
        for (i, calc) in final_calcs.iter().take(5).enumerate() {
            println!("      [{}] {} args:", i, calc.args.len());
            for (ai, arg) in calc.args.iter().enumerate() {
                let raw = arg.get("raw").and_then(|v| v.as_str()).unwrap_or("?");
                let as_int = arg.get("asInt32").and_then(|v| v.as_i64());
                print!("        arg[{}]: raw={}", ai, raw);
                if let Some(i) = as_int {
                    print!(" int32={}", i);
                }
                println!();
            }
            if let Some(ret) = &calc.retval {
                let raw = ret.get("raw").and_then(|v| v.as_str()).unwrap_or("?");
                println!("        retval: raw={}", raw);
            }
        }
    }

    // Save config table analysis JSON
    let unique_ptrs: std::collections::HashSet<&str> =
        table_lookups.iter().map(|l| l.array_ptr.as_str()).collect();
    let analysis = serde_json::json!({
        "table_dumps": table_dumps,
        "table_lookups": table_lookups,
        "base_calculations": base_calcs.len(),
        "final_calculations": final_calcs.len(),
        "initialized_lookups": table_lookups.iter().filter(|l| l.initialized).count(),
        "unique_array_ptrs": unique_ptrs.len(),
    });

    let _ = fs::create_dir_all(output_dir);
    let json_path = format!("{}/config_table_analysis.json", output_dir);
    match fs::write(
        &json_path,
        serde_json::to_string_pretty(&analysis).unwrap_or_default(),
    ) {
        Ok(()) => println!("\n  Saved: {}", json_path),
        Err(e) => println!("\n  ERROR saving {}: {}", json_path, e),
    }
}

// ══════════════════════════════════════════════════════════════
// Main
// ══════════════════════════════════════════════════════════════

fn main() {
    println!("{}", "═".repeat(62));
    println!("  IL2CPP Character Data Probe");
    println!("  Parse IL2CPP dumps + capture logs → character stats");
    println!("{}\n", "═".repeat(62));

    let args: Vec<String> = std::env::args().collect();

    let mut dump_path: Option<String> = None;
    let mut capture_path: Option<String> = None;
    let mut output_dir = "./output/character_stats".to_string();
    let mut suggest_mode = false;

    // Parse args
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--dump" | "-d" => {
                i += 1;
                if i < args.len() {
                    dump_path = Some(args[i].clone());
                }
            }
            "--capture" | "-c" => {
                i += 1;
                if i < args.len() {
                    capture_path = Some(args[i].clone());
                }
            }
            "--output-dir" | "-o" => {
                i += 1;
                if i < args.len() {
                    output_dir = args[i].clone();
                }
            }
            "--suggest" | "-s" => {
                suggest_mode = true;
            }
            "--help" | "-h" => {
                println!("Usage: {} [OPTIONS]", args[0]);
                println!();
                println!("Options:");
                println!("  --dump, -d <path>       IL2CPP dump JSON (from frida_il2cpp_scan.js)");
                println!("  --capture, -c <path>    Capture log (from frida_char_data.js)");
                println!(
                    "  --output-dir, -o <dir>  Output directory [default: ./output/character_stats]"
                );
                println!("  --suggest, -s           Generate hook target suggestions from dump");
                println!("  --help, -h              Show this help");
                println!();
                println!("Workflows:");
                println!("  1. Analyze dump only (suggest targets):");
                println!("     {} --dump il2cpp_full_dump.json --suggest", args[0]);
                println!();
                println!("  2. Full pipeline (dump + capture):");
                println!(
                    "     {} --dump il2cpp_full_dump.json --capture capture.log",
                    args[0]
                );
                return;
            }
            other => {
                // Positional: auto-detect by extension
                if other.ends_with(".jsonl") || other.ends_with(".log") {
                    // Capture log or JSONL events file
                    if capture_path.is_none() {
                        capture_path = Some(other.to_string());
                    }
                } else if dump_path.is_none() {
                    dump_path = Some(other.to_string());
                } else if capture_path.is_none() {
                    capture_path = Some(other.to_string());
                }
            }
        }
        i += 1;
    }

    if dump_path.is_none() && capture_path.is_none() {
        eprintln!("ERROR: --dump <path> and/or --capture <path> is required");
        eprintln!("Run with --help for usage info");
        std::process::exit(1);
    }

    let _ = fs::create_dir_all(&output_dir);

    // Pass 1+2: Parse dump (optional — skip if only capture provided)
    let dump;
    let scored;
    if let Some(dp) = &dump_path {
        dump = match pass_1_parse_dump(dp) {
            Some(d) => Some(d),
            None => {
                eprintln!("Failed to parse IL2CPP dump. Continuing without dump.");
                None
            }
        };
        scored = dump.as_ref().map(pass_2_rank);

        if suggest_mode && let Some(s) = &scored {
            suggest_targets(s, &output_dir);
        }
    } else {
        dump = None;
        scored = None;
    }

    // If no capture log, we're done (dump-only mode)
    if capture_path.is_none() {
        if !suggest_mode {
            println!("\n  No capture log provided. Use --suggest to generate hook targets.");
            println!("  Or provide --capture <path> for full analysis.");
        }
        println!("\n{}", "═".repeat(62));
        println!("  Probe Complete (dump-only mode)");
        println!("{}", "═".repeat(62));
        return;
    }

    // Pass 3: Parse capture
    let (old_events, raw_events) = pass_3_parse_capture(capture_path.as_ref().unwrap());

    if raw_events.is_empty() {
        println!("\n  No capture events found. Check log format.");
        println!("  Expected: IL2CPP_EVENT|{{json}} lines");
        println!("\n{}", "═".repeat(62));
        println!("  Probe Complete");
        println!("{}", "═".repeat(62));
        return;
    }

    // Check if gadget format
    let has_gadget_events = raw_events.iter().any(|ev| {
        let t = ev.get("type").and_then(|v| v.as_str()).unwrap_or("");
        matches!(
            t,
            "CHAR_ATTRIBUTE_REFRESH"
                | "CHAR_GET_ATTRIBUTES"
                | "ATTR_REFRESH_ALL"
                | "ATTR_DUMP"
                | "ABILITY_INIT_ATTRIBUTES"
                | "ABILITY_INIT_DEEP"
                | "ATTR_GET_VALUE"
                | "ATTR_PTR_FIRST_SEEN"
                | "FINAL_ATTRIBUTES"
                | "SCAN_CHAR_DATA"
                | "CONFIG_TABLE_LOOKUP"
                | "CALC_BASE_ATTRIBUTE"
                | "TABLE_DEEP_PROBE"
                | "TABLE_CHARACTER_ENTRY"
                | "TABLE_DICT_SUMMARY"
                | "CALC_FINAL_ATTRIBUTE"
                | "CHARACTER_ATTRIBUTE_TABLE_DUMP"
                | "FULL_CHARACTER_CAPTURE"
                | "INIT_ATTRIBUTES_UNRESOLVED"
                | "ATTR_REFRESH_UPDATE"
        )
    });

    if has_gadget_events {
        // Pass 4+5 (gadget): Direct stat extraction
        pass_gadget_extract(&raw_events, &output_dir);

        // Pass 6: Config table pipeline analysis (hooks 7-10)
        let has_config_events = raw_events.iter().any(|ev| {
            let t = ev.get("type").and_then(|v| v.as_str()).unwrap_or("");
            matches!(
                t,
                "CONFIG_TABLE_LOOKUP"
                    | "CALC_BASE_ATTRIBUTE"
                    | "CALC_FINAL_ATTRIBUTE"
                    | "CHARACTER_ATTRIBUTE_TABLE_DUMP"
                    | "TABLE_DEEP_PROBE"
                    | "TABLE_CHARACTER_ENTRY"
                    | "TABLE_DICT_SUMMARY"
            )
        });
        if has_config_events {
            pass_config_table_analysis(&raw_events, &output_dir);
        }
    } else if let (Some(s), Some(d)) = (&scored, &dump) {
        // Pass 4: Correlate (legacy — requires dump)
        pass_4_correlate(&old_events, s, d);

        // Pass 5: Extract stats (legacy)
        pass_5_extract_stats(&old_events, &output_dir);
    } else {
        println!("  Legacy format but no IL2CPP dump provided — skipping passes 4+5.");
        println!("  Provide --dump <path> for full legacy analysis.");
    }

    println!("\n{}", "═".repeat(62));
    println!("  Probe Complete");
    println!("{}", "═".repeat(62));
}
