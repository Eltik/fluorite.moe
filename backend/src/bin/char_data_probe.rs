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
    "hp", "str", "agi", "wisd", "will", "atk", "def", "maxHp",
    "maxUltimateSp", "maxPoise", "poiseRecTime", "poiseRecTimeScalar",
    "physicalDamageTakenScalar", "fireDamageTakenScalar",
    "pulseDamageTakenScalar", "crystDamageTakenScalar",
    "naturalDamageTakenScalar", "etherDamageTakenScalar",
    "weight", "criticalRate", "criticalDamageIncrease",
    "moveSpeedMultiplier", "turnRateMultiplier", "attackRate",
    "skillCooldownScalar", "normalAttackRange",
    "hpRecoveryPerSec", "hpRecoveryPerSecByHpRatio",
    "poiseTakenScalar", "poiseOutputScalar",
    "breakingAttackDamageTakenScalar", "knockDownTimeAddition",
    "healOutputIncrease", "healTakenIncrease", "lifeSteal",
    "ultimateSpGainScalar", "atbCostAddition", "skillCooldownAddition",
    "comboSkillCooldownScalar", "igniteDamageScalar",
    "physicalInflictionDamageScalar",
    "physicalDamageIncrease", "fireDamageIncrease",
    "pulseDamageIncrease", "crystDamageIncrease",
    "naturalDamageIncrease", "etherDamageIncrease",
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

fn extract_gadget_stats(events_raw: &[serde_json::Value]) -> HashMap<String, GadgetCharacterStats> {
    let mut characters: HashMap<String, GadgetCharacterStats> = HashMap::new();

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

                let entry = characters.entry(template_id.clone()).or_insert_with(|| {
                    GadgetCharacterStats {
                        template_id: template_id.clone(),
                        ..Default::default()
                    }
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
                            && let Some(m_attrs) = dump_result.get("m_attrs").and_then(|v| v.as_array())
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

            "ABILITY_INIT_ATTRIBUTES" => {
                if let Some(as_data) = ev.get("abilitySystem")
                    && let Some(attrs) = as_data.get("attributes")
                {
                    let template_id = attrs
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
                        extract_attributes_into(attrs, entry);
                        entry.capture_count += 1;
                        entry.sources.push("ABILITY_INIT_ATTRIBUTES".to_string());
                    }
                }
            }

            "ATTR_GET_VALUE" => {
                let template_id = ev
                    .get("templateId")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();

                if template_id.is_empty() {
                    continue;
                }

                let attr_name = ev
                    .get("attributeName")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown");
                let value = ev.get("value").and_then(|v| v.as_f64());

                if let Some(val) = value {
                    let entry = characters.entry(template_id.clone()).or_insert_with(|| {
                        GadgetCharacterStats {
                            template_id: template_id.clone(),
                            ..Default::default()
                        }
                    });
                    entry.attributes.insert(attr_name.to_string(), val);
                }
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

                let entry = characters.entry(template_id.clone()).or_insert_with(|| {
                    GadgetCharacterStats {
                        template_id: template_id.clone(),
                        ..Default::default()
                    }
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
                    computed.scalars.atk_from_main = scalars.get("atkFromMain").and_then(|v| v.as_f64());
                    computed.scalars.atk_from_str = scalars.get("atkFromStr").and_then(|v| v.as_f64());
                    computed.scalars.atk_from_agi = scalars.get("atkFromAgi").and_then(|v| v.as_f64());
                    computed.scalars.atk_from_wisd = scalars.get("atkFromWisd").and_then(|v| v.as_f64());
                    computed.scalars.atk_from_will = scalars.get("atkFromWill").and_then(|v| v.as_f64());
                    computed.scalars.hp_from_str = scalars.get("hpFromStr").and_then(|v| v.as_f64());
                    computed.scalars.heal_from_will = scalars.get("healFromWill").and_then(|v| v.as_f64());
                    computed.scalars.phys_resist_from_agi = scalars.get("physResistFromAgi").and_then(|v| v.as_f64());
                    computed.scalars.energy_resist_from_wisd = scalars.get("energyResistFromWisd").and_then(|v| v.as_f64());
                }

                // Extract main/sub attr types
                if let Some(mat) = ev.get("mainAttrType")
                    && let Some(has) = mat.get("hasValue").and_then(|v| v.as_bool())
                    && has
                {
                    computed.main_attr_type = mat.get("value").and_then(|v| v.as_i64()).map(|v| v as i32);
                }
                if let Some(sat) = ev.get("subAttrType")
                    && let Some(has) = sat.get("hasValue").and_then(|v| v.as_bool())
                    && has
                {
                    computed.sub_attr_type = sat.get("value").and_then(|v| v.as_i64()).map(|v| v as i32);
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
    "hp", "atk", "def", "attack", "defense", "health",
    "crit", "damage", "speed", "resist", "maxhp", "baseatk",
    "basedef", "level", "exp", "growth", "stat", "power",
    "strength", "agility", "stamina", "energy", "shield",
    "armor", "penetration", "lifesteal", "regen",
];

const STAT_METHOD_KEYWORDS: &[&str] = &[
    "get", "set", "calc", "compute", "load", "init",
    "parse", "merge", "deserialize", "read", "decode",
    "apply", "update", "refresh", "setup",
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
        if (ret_lower.contains("int") || ret_lower.contains("float") || ret_lower.contains("double"))
            && (lower.contains("stat") || lower.contains("hp") || lower.contains("atk")
                || lower.contains("def") || lower.contains("get"))
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
            "charId", "characterId", "id", "Id", "CharId", "char_id",
            "character_id", "unitId", "unit_id", "name", "Name",
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
    println!("    GA size:          {} bytes", dump.meta.game_assembly_size);
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
        if let Some(json_str) = trimmed.strip_prefix("IL2CPP_EVENT|") {
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
                            | "FINAL_ATTRIBUTES"
                            | "SCAN_CHAR_DATA"
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

fn pass_4_correlate(
    events: &[CaptureEvent],
    scored: &[ScoredClass],
    dump: &Il2cppDump,
) {
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
                    println!("    Score: {} | Reasons: {}", sc.score, sc.reasons.join("; "));
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
    match fs::write(&json_path, serde_json::to_string_pretty(&json_output).unwrap_or_default()) {
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

    for ch in &chars {
        println!(
            "  ╔══ {} (lv{}, break{}, potential{}) ══",
            ch.template_id, ch.level, ch.break_stage, ch.potential_level
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
            "criticalRate", "criticalDamageIncrease", "attackRate",
            "moveSpeedMultiplier", "hpRecoveryPerSec", "maxPoise",
            "weight", "healOutputIncrease", "lifeSteal",
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
        let shown: std::collections::HashSet<&str> = core_stats
            .iter()
            .chain(secondary.iter())
            .copied()
            .collect();
        let remaining: Vec<_> = ch
            .attributes
            .iter()
            .filter(|(k, v)| !shown.contains(k.as_str()) && !k.contains("Damage") && !k.contains("Resist") && !k.contains("Infliction") && **v != 0.0)
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
            let has_scalars = s.atk_from_main.is_some()
                || s.atk_from_str.is_some()
                || s.hp_from_str.is_some();
            if has_scalars {
                println!("  ║ Cross-attribute scalars:");
                if let Some(v) = s.atk_from_main { println!("  ║   atkFromMain       = {:.4}", v); }
                if let Some(v) = s.atk_from_str  { println!("  ║   atkFromStr        = {:.4}", v); }
                if let Some(v) = s.atk_from_agi  { println!("  ║   atkFromAgi        = {:.4}", v); }
                if let Some(v) = s.atk_from_wisd { println!("  ║   atkFromWisd       = {:.4}", v); }
                if let Some(v) = s.atk_from_will { println!("  ║   atkFromWill       = {:.4}", v); }
                if let Some(v) = s.hp_from_str   { println!("  ║   hpFromStr         = {:.4}", v); }
                if let Some(v) = s.heal_from_will { println!("  ║   healFromWill      = {:.4}", v); }
                if let Some(v) = s.phys_resist_from_agi { println!("  ║   physResistFromAgi = {:.4}", v); }
                if let Some(v) = s.energy_resist_from_wisd { println!("  ║   energyResistWisd  = {:.4}", v); }
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

    // Save JSON output
    let json_output = serde_json::json!({
        "format": "gadget",
        "character_count": characters.len(),
        "characters": characters,
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
        "atkFromMain", "atkFromStr", "atkFromAgi", "atkFromWisd", "atkFromWill",
        "hpFromStr", "healFromWill", "physResistFromAgi", "energyResistFromWisd",
    ];

    // Header
    let raw_headers: Vec<String> = all_attr_names.iter().map(|n| format!("raw_{}", n)).collect();
    let final_headers: Vec<String> = all_final_names.iter().map(|n| format!("final_{}", n)).collect();
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
                s.atk_from_main.map(|v| format!("{:.6}", v)).unwrap_or_default(),
                s.atk_from_str.map(|v| format!("{:.6}", v)).unwrap_or_default(),
                s.atk_from_agi.map(|v| format!("{:.6}", v)).unwrap_or_default(),
                s.atk_from_wisd.map(|v| format!("{:.6}", v)).unwrap_or_default(),
                s.atk_from_will.map(|v| format!("{:.6}", v)).unwrap_or_default(),
                s.hp_from_str.map(|v| format!("{:.6}", v)).unwrap_or_default(),
                s.heal_from_will.map(|v| format!("{:.6}", v)).unwrap_or_default(),
                s.phys_resist_from_agi.map(|v| format!("{:.6}", v)).unwrap_or_default(),
                s.energy_resist_from_wisd.map(|v| format!("{:.6}", v)).unwrap_or_default(),
            ]
        } else {
            vec![String::new(); scalar_cols.len()]
        };

        let main_attr = cs.and_then(|c| c.main_attr_type).map(|v| v.to_string()).unwrap_or_default();
        let sub_attr = cs.and_then(|c| c.sub_attr_type).map(|v| v.to_string()).unwrap_or_default();

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
                println!("  --output-dir, -o <dir>  Output directory [default: ./output/character_stats]");
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
                // Positional: first = dump, second = capture
                if dump_path.is_none() {
                    dump_path = Some(other.to_string());
                } else if capture_path.is_none() {
                    capture_path = Some(other.to_string());
                }
            }
        }
        i += 1;
    }

    if dump_path.is_none() {
        eprintln!("ERROR: --dump <path> is required");
        eprintln!("Run with --help for usage info");
        std::process::exit(1);
    }

    let _ = fs::create_dir_all(&output_dir);

    // Pass 1: Parse dump
    let dump = match pass_1_parse_dump(dump_path.as_ref().unwrap()) {
        Some(d) => d,
        None => {
            eprintln!("Failed to parse IL2CPP dump. Aborting.");
            std::process::exit(1);
        }
    };

    // Pass 2: Rank classes
    let scored = pass_2_rank(&dump);

    // Suggest mode
    if suggest_mode {
        suggest_targets(&scored, &output_dir);
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
                | "ATTR_GET_VALUE"
                | "FINAL_ATTRIBUTES"
                | "SCAN_CHAR_DATA"
        )
    });

    if has_gadget_events {
        // Pass 4+5 (gadget): Direct stat extraction
        pass_gadget_extract(&raw_events, &output_dir);
    } else {
        // Pass 4: Correlate (legacy)
        pass_4_correlate(&old_events, &scored, &dump);

        // Pass 5: Extract stats (legacy)
        pass_5_extract_stats(&old_events, &output_dir);
    }

    println!("\n{}", "═".repeat(62));
    println!("  Probe Complete");
    println!("{}", "═".repeat(62));
}
