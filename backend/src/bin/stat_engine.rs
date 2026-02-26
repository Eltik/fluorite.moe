//! Offline Stat Computation Engine
//!
//! Computes character stats from calibration data captured by frida_calibration.js.
//! No game runtime or Frida needed — pure offline calculation.
//!
//! Usage:
//!   cargo run --bin stat-engine -- --calibration calibration_data.json
//!   cargo run --bin stat-engine -- --calibration calibration_data.json --char chr_0004_pelica --level 60
//!
//! Modes:
//!   1. Summary: Show all calibrated characters and their stats
//!   2. Query: Compute stats for a specific character at a given level
//!   3. Export: Generate full stat tables for all characters

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;

// ══════════════════════════════════════════════════════════════
// Calibration Data Structures
// ══════════════════════════════════════════════════════════════

#[derive(Debug, Deserialize)]
struct CalibrationFile {
    version: u32,
    #[allow(dead_code)]
    timestamp: String,
    characters: HashMap<String, CalibrationRecord>,
    #[serde(rename = "calculatorInterceptions", default)]
    calculator_interceptions: Vec<CalcInterception>,
    #[serde(rename = "charMap", default)]
    #[allow(dead_code)]
    char_map: HashMap<String, serde_json::Value>,
}

#[derive(Debug, Deserialize)]
struct CalibrationRecord {
    #[serde(rename = "templateId")]
    template_id: String,
    snapshots: Vec<CalibrationSnapshot>,
}

#[derive(Debug, Deserialize, Clone)]
struct CalibrationSnapshot {
    #[allow(dead_code)]
    timestamp: u64,
    level: Option<i64>,
    #[serde(rename = "breakStage")]
    break_stage: Option<i64>,
    #[serde(rename = "rawData")]
    raw_data: Option<Vec<f64>>,
    #[serde(rename = "finalCachedData")]
    final_cached_data: Option<CacheData>,
    #[serde(rename = "baseNonConverted")]
    base_non_converted: Option<CacheData>,
    #[serde(rename = "finalNonConverted")]
    #[allow(dead_code)]
    final_non_converted: Option<CacheData>,
    #[serde(rename = "mainAttrType")]
    main_attr_type: Option<NullableAttrType>,
    #[serde(rename = "subAttrType")]
    sub_attr_type: Option<NullableAttrType>,
    scalars: Option<HashMap<String, f64>>,
    #[serde(default)]
    modifiers: Vec<ModifierData>,
    #[serde(rename = "dataMin")]
    #[allow(dead_code)]
    data_min: Option<Vec<f64>>,
    #[serde(rename = "dataMax")]
    #[allow(dead_code)]
    data_max: Option<Vec<f64>>,
}

#[derive(Debug, Deserialize, Clone)]
struct CacheData {
    data: Option<Vec<f64>>,
    #[serde(rename = "namedAttrs", default)]
    #[allow(dead_code)]
    named_attrs: HashMap<String, f64>,
}

#[derive(Debug, Deserialize, Clone)]
struct NullableAttrType {
    value: i32,
    #[serde(rename = "hasValue")]
    has_value: bool,
}

#[derive(Debug, Deserialize, Clone)]
#[allow(dead_code)]
struct ModifierData {
    index: u32,
    #[serde(rename = "baseAdditions")]
    base_additions: Option<Vec<f64>>,
    #[serde(rename = "baseMultipliers")]
    base_multipliers: Option<Vec<f64>>,
    #[serde(rename = "baseFinalAdditions")]
    base_final_additions: Option<Vec<f64>>,
    #[serde(rename = "baseFinalMultipliers")]
    base_final_multipliers: Option<Vec<f64>>,
    additions: Option<Vec<f64>>,
    multipliers: Option<Vec<f64>>,
    #[serde(rename = "finalAdditions")]
    final_additions: Option<Vec<f64>>,
    #[serde(rename = "finalMultipliers")]
    final_multipliers: Option<Vec<f64>>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct CalcInterception {
    method: String,
    args: Vec<String>,
    retval: Option<String>,
    timestamp: u64,
}

// ══════════════════════════════════════════════════════════════
// Computed Output Structures
// ══════════════════════════════════════════════════════════════

#[derive(Debug, Serialize)]
struct CharacterStatSheet {
    template_id: String,
    level: i64,
    break_stage: i64,
    main_attr_type: Option<String>,
    sub_attr_type: Option<String>,
    raw_stats: HashMap<String, f64>,
    final_stats: HashMap<String, f64>,
    scalars: HashMap<String, f64>,
    modifier_summary: ModifierSummary,
    data_source: String,
}

#[derive(Debug, Default, Serialize)]
struct ModifierSummary {
    modifier_count: usize,
    total_base_additions: HashMap<String, f64>,
    total_multipliers: HashMap<String, f64>,
    total_final_additions: HashMap<String, f64>,
}

/// Attribute names indexed by position
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

fn attr_name(idx: usize) -> String {
    if idx < ATTR_NAMES.len() {
        ATTR_NAMES[idx].to_string()
    } else {
        format!("attr_{}", idx)
    }
}

fn named_from_array(arr: &[f64]) -> HashMap<String, f64> {
    let mut map = HashMap::new();
    for (i, &v) in arr.iter().enumerate() {
        if v != 0.0 {
            map.insert(attr_name(i), v);
        }
    }
    map
}

// ══════════════════════════════════════════════════════════════
// Stat Computation
// ══════════════════════════════════════════════════════════════

/// Compute the 8-layer modifier pipeline for a single attribute index.
///
/// Formula per attribute:
///   base = rawValue
///   base += sum(baseAdditions[i])
///   base *= product(1 + baseMultipliers[i])
///   base += sum(baseFinalAdditions[i])
///   base *= product(1 + baseFinalMultipliers[i])
///   value = base
///   value += sum(additions[i])
///   value *= product(1 + multipliers[i])
///   value += sum(finalAdditions[i])
///   value *= product(1 + finalMultipliers[i])
fn apply_modifiers(raw_value: f64, attr_idx: usize, modifiers: &[ModifierData]) -> f64 {
    let get_layer = |layer: &Option<Vec<f64>>, idx: usize| -> f64 {
        layer.as_ref().and_then(|v| v.get(idx).copied()).unwrap_or(0.0)
    };

    let mut base = raw_value;

    // Base phase
    let mut base_add = 0.0;
    let mut base_mul = 1.0;
    let mut base_final_add = 0.0;
    let mut base_final_mul = 1.0;
    let mut add = 0.0;
    let mut mul = 1.0;
    let mut final_add = 0.0;
    let mut final_mul = 1.0;

    for m in modifiers {
        base_add += get_layer(&m.base_additions, attr_idx);
        let bm = get_layer(&m.base_multipliers, attr_idx);
        if bm != 0.0 {
            base_mul *= 1.0 + bm;
        }
        base_final_add += get_layer(&m.base_final_additions, attr_idx);
        let bfm = get_layer(&m.base_final_multipliers, attr_idx);
        if bfm != 0.0 {
            base_final_mul *= 1.0 + bfm;
        }
        add += get_layer(&m.additions, attr_idx);
        let mm = get_layer(&m.multipliers, attr_idx);
        if mm != 0.0 {
            mul *= 1.0 + mm;
        }
        final_add += get_layer(&m.final_additions, attr_idx);
        let fm = get_layer(&m.final_multipliers, attr_idx);
        if fm != 0.0 {
            final_mul *= 1.0 + fm;
        }
    }

    base += base_add;
    base *= base_mul;
    base += base_final_add;
    base *= base_final_mul;

    let mut value = base;
    value += add;
    value *= mul;
    value += final_add;
    value *= final_mul;

    value
}

/// Apply cross-attribute scaling (STR -> HP add, mainAttr -> ATK scalar, etc.)
fn apply_cross_attribute_scaling(
    stats: &mut HashMap<String, f64>,
    scalars: &HashMap<String, f64>,
) {
    // HP bonus from STR
    if let Some(&hp_from_str) = scalars.get("hpFromStr")
        && hp_from_str != 0.0 {
            let current_hp = stats.get("hp").copied().unwrap_or(0.0);
            stats.insert("hp".into(), current_hp + hp_from_str);
            let current_max = stats.get("maxHp").copied().unwrap_or(0.0);
            stats.insert("maxHp".into(), current_max + hp_from_str);
        }

    // ATK scaling from main + sub attributes
    let total_atk_scalar = {
        let from_main = scalars.get("atkFromMain").copied().unwrap_or(0.0);
        let from_str = scalars.get("atkFromStr").copied().unwrap_or(0.0);
        let from_agi = scalars.get("atkFromAgi").copied().unwrap_or(0.0);
        let from_wisd = scalars.get("atkFromWisd").copied().unwrap_or(0.0);
        let from_will = scalars.get("atkFromWill").copied().unwrap_or(0.0);
        // atkFromMain already includes one of the per-stat contributions
        // The individual per-stat values are components; use the max of
        // (atkFromMain) vs (sum of per-stat) to avoid double-counting
        let per_stat_sum = from_str + from_agi + from_wisd + from_will;
        from_main.max(per_stat_sum)
    };

    if total_atk_scalar != 0.0
        && let Some(atk) = stats.get_mut("atk") {
            *atk *= 1.0 + total_atk_scalar;
        }
}

/// Build a full stat sheet from a calibration snapshot.
fn build_stat_sheet(record: &CalibrationRecord, snapshot_idx: Option<usize>) -> Option<CharacterStatSheet> {
    let snap = if let Some(idx) = snapshot_idx {
        record.snapshots.get(idx)?
    } else {
        // Use the latest snapshot
        record.snapshots.last()?
    };

    let level = snap.level.unwrap_or(1);
    let break_stage = snap.break_stage.unwrap_or(0);

    // Raw stats from m_rawData
    let raw_stats = snap.raw_data.as_ref()
        .map(|d| named_from_array(d))
        .unwrap_or_default();

    // Final stats: prefer finalCachedData if available, else compute from modifiers
    let final_stats = if let Some(fcd) = &snap.final_cached_data
        && let Some(data) = &fcd.data
    {
        named_from_array(data)
    } else if let Some(raw) = &snap.raw_data {
        // Compute from raw + modifiers
        let mut computed = HashMap::new();
        for (i, &v) in raw.iter().enumerate() {
            let final_val = apply_modifiers(v, i, &snap.modifiers);
            if final_val != 0.0 {
                computed.insert(attr_name(i), final_val);
            }
        }
        // Apply cross-attribute scaling
        if let Some(sc) = &snap.scalars {
            apply_cross_attribute_scaling(&mut computed, sc);
        }
        computed
    } else {
        HashMap::new()
    };

    // Scalars
    let scalars = snap.scalars.clone().unwrap_or_default();

    // Main/sub attr types
    let main_attr = snap.main_attr_type.as_ref()
        .filter(|t| t.has_value)
        .map(|t| {
            if (t.value as usize) < ATTR_NAMES.len() {
                format!("{} ({})", ATTR_NAMES[t.value as usize], t.value)
            } else {
                format!("attr_{}", t.value)
            }
        });
    let sub_attr = snap.sub_attr_type.as_ref()
        .filter(|t| t.has_value)
        .map(|t| {
            if (t.value as usize) < ATTR_NAMES.len() {
                format!("{} ({})", ATTR_NAMES[t.value as usize], t.value)
            } else {
                format!("attr_{}", t.value)
            }
        });

    // Modifier summary
    let mut mod_summary = ModifierSummary {
        modifier_count: snap.modifiers.len(),
        ..Default::default()
    };

    for m in &snap.modifiers {
        if let Some(ba) = &m.base_additions {
            for (i, &v) in ba.iter().enumerate() {
                if v != 0.0 {
                    *mod_summary.total_base_additions.entry(attr_name(i)).or_insert(0.0) += v;
                }
            }
        }
        if let Some(ml) = &m.multipliers {
            for (i, &v) in ml.iter().enumerate() {
                if v != 0.0 {
                    *mod_summary.total_multipliers.entry(attr_name(i)).or_insert(0.0) += v;
                }
            }
        }
        if let Some(fa) = &m.final_additions {
            for (i, &v) in fa.iter().enumerate() {
                if v != 0.0 {
                    *mod_summary.total_final_additions.entry(attr_name(i)).or_insert(0.0) += v;
                }
            }
        }
    }

    let data_source = if snap.final_cached_data.as_ref().is_some_and(|f| f.data.is_some()) {
        "finalCachedData (direct capture)"
    } else if !snap.modifiers.is_empty() {
        "computed (raw + modifiers)"
    } else {
        "raw only (no modifiers captured)"
    };

    Some(CharacterStatSheet {
        template_id: record.template_id.clone(),
        level,
        break_stage,
        main_attr_type: main_attr,
        sub_attr_type: sub_attr,
        raw_stats,
        final_stats,
        scalars,
        modifier_summary: mod_summary,
        data_source: data_source.into(),
    })
}

// ══════════════════════════════════════════════════════════════
// Growth Curve Estimation (from multi-level snapshots)
// ══════════════════════════════════════════════════════════════

#[derive(Debug, Serialize)]
struct GrowthCurve {
    template_id: String,
    levels: Vec<i64>,
    per_level_growth: HashMap<String, f64>,
    base_at_level_1: HashMap<String, f64>,
}

fn estimate_growth_curve(record: &CalibrationRecord) -> Option<GrowthCurve> {
    // Need at least 2 snapshots at different levels
    let mut level_snaps: Vec<(i64, &CalibrationSnapshot)> = record.snapshots.iter()
        .filter_map(|s| s.level.map(|l| (l, s)))
        .collect();
    level_snaps.sort_by_key(|(l, _)| *l);
    level_snaps.dedup_by_key(|(l, _)| *l);

    if level_snaps.len() < 2 {
        return None;
    }

    let levels: Vec<i64> = level_snaps.iter().map(|(l, _)| *l).collect();

    // For each attribute, compute average growth per level
    let mut per_level = HashMap::new();
    let mut base_stats = HashMap::new();

    // Use base_non_converted for clean growth calculation (pre-modifier)
    let first = level_snaps.first()?;
    let last = level_snaps.last()?;

    let first_raw = first.1.base_non_converted.as_ref()
        .and_then(|c| c.data.as_ref())
        .or(first.1.raw_data.as_ref())?;
    let last_raw = last.1.base_non_converted.as_ref()
        .and_then(|c| c.data.as_ref())
        .or(last.1.raw_data.as_ref())?;

    let level_delta = last.0 - first.0;
    if level_delta <= 0 {
        return None;
    }

    for (i, (&first_val, &last_val)) in first_raw.iter().zip(last_raw.iter()).enumerate() {
        if first_val != 0.0 || last_val != 0.0 {
            let name = attr_name(i);
            let growth = (last_val - first_val) / level_delta as f64;
            if growth.abs() > 0.001 {
                per_level.insert(name.clone(), growth);
            }
            // Extrapolate back to level 1
            let base = first_val - growth * (first.0 - 1) as f64;
            base_stats.insert(name, base);
        }
    }

    Some(GrowthCurve {
        template_id: record.template_id.clone(),
        levels,
        per_level_growth: per_level,
        base_at_level_1: base_stats,
    })
}

// ══════════════════════════════════════════════════════════════
// Display
// ══════════════════════════════════════════════════════════════

fn display_stat_sheet(sheet: &CharacterStatSheet) {
    println!("  ╔══ {} (Lv{}, Break{}) ══", sheet.template_id, sheet.level, sheet.break_stage);
    println!("  ║ Data source: {}", sheet.data_source);

    if let Some(ref mat) = sheet.main_attr_type {
        println!("  ║ Main attr: {}", mat);
    }
    if let Some(ref sat) = sheet.sub_attr_type {
        println!("  ║ Sub attr:  {}", sat);
    }

    // Core stats comparison
    let core = ["hp", "maxHp", "atk", "def", "str", "agi", "wisd", "will"];
    println!("  ║");
    println!("  ║ {:20} {:>12} {:>12} {:>8}", "Stat", "Raw", "Final", "Ratio");
    println!("  ║ {}", "-".repeat(56));
    for &stat in &core {
        let raw = sheet.raw_stats.get(stat).copied().unwrap_or(0.0);
        let fin = sheet.final_stats.get(stat).copied().unwrap_or(0.0);
        if raw != 0.0 || fin != 0.0 {
            let ratio = if raw != 0.0 { fin / raw } else { 0.0 };
            println!("  ║ {:20} {:>12.2} {:>12.2} {:>8.3}x", stat, raw, fin, ratio);
        }
    }

    // Secondary stats
    let shown: std::collections::HashSet<&str> = core.iter().copied().collect();
    let mut other: Vec<_> = sheet.final_stats.iter()
        .filter(|(k, _)| !shown.contains(k.as_str()))
        .collect();
    other.sort_by(|a, b| a.0.cmp(b.0));
    if !other.is_empty() {
        println!("  ║");
        println!("  ║ Other final stats:");
        for (k, v) in &other {
            let raw = sheet.raw_stats.get(k.as_str()).copied().unwrap_or(0.0);
            if raw != 0.0 {
                println!("  ║   {:36} = {:>12.4} (raw={:.4})", k, v, raw);
            } else {
                println!("  ║   {:36} = {:>12.4}", k, v);
            }
        }
    }

    // Scalars
    if !sheet.scalars.is_empty() {
        println!("  ║");
        println!("  ║ Cross-attribute scalars:");
        for (k, v) in &sheet.scalars {
            println!("  ║   {:36} = {:.6}", k, v);
        }
    }

    // Modifier summary
    let ms = &sheet.modifier_summary;
    if ms.modifier_count > 0 {
        println!("  ║");
        println!("  ║ Modifiers: {} active", ms.modifier_count);
        if !ms.total_base_additions.is_empty() {
            println!("  ║   Base additions:");
            for (k, v) in &ms.total_base_additions {
                println!("  ║     {:32} += {:.4}", k, v);
            }
        }
        if !ms.total_multipliers.is_empty() {
            println!("  ║   Multipliers:");
            for (k, v) in &ms.total_multipliers {
                println!("  ║     {:32} *= {:.4}", k, 1.0 + v);
            }
        }
    }

    println!("  ╚{}", "═".repeat(50));
    println!();
}

// ══════════════════════════════════════════════════════════════
// Main
// ══════════════════════════════════════════════════════════════

fn main() {
    println!("{}", "═".repeat(62));
    println!("  Offline Stat Computation Engine");
    println!("  Computes character stats from calibration data");
    println!("{}\n", "═".repeat(62));

    let args: Vec<String> = std::env::args().collect();

    let mut calibration_path: Option<String> = None;
    let mut query_char: Option<String> = None;
    let mut output_dir = "./output/stat_engine".to_string();
    let mut export_mode = false;

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--calibration" | "-C" => {
                i += 1;
                if i < args.len() {
                    calibration_path = Some(args[i].clone());
                }
            }
            "--char" | "-c" => {
                i += 1;
                if i < args.len() {
                    query_char = Some(args[i].clone());
                }
            }
            "--output-dir" | "-o" => {
                i += 1;
                if i < args.len() {
                    output_dir = args[i].clone();
                }
            }
            "--export" | "-e" => {
                export_mode = true;
            }
            "--help" | "-h" => {
                println!("Usage: {} [OPTIONS]", args[0]);
                println!();
                println!("Options:");
                println!("  --calibration, -C <path>   Calibration data JSON (from frida_calibration.js)");
                println!("  --char, -c <id>            Query specific character (e.g., chr_0004_pelica)");
                println!("  --output-dir, -o <dir>     Output directory [default: ./output/stat_engine]");
                println!("  --export, -e               Export full stat tables as JSON + CSV");
                println!("  --help, -h                 Show this help");
                println!();
                println!("Workflows:");
                println!("  1. View all characters:");
                println!("     {} -C calibration_data.json", args[0]);
                println!();
                println!("  2. Query one character:");
                println!("     {} -C calibration_data.json -c chr_0004_pelica", args[0]);
                println!();
                println!("  3. Export stat tables:");
                println!("     {} -C calibration_data.json --export", args[0]);
                return;
            }
            other => {
                if calibration_path.is_none() {
                    calibration_path = Some(other.to_string());
                }
            }
        }
        i += 1;
    }

    let cal_path = match calibration_path {
        Some(p) => p,
        None => {
            eprintln!("ERROR: --calibration <path> is required");
            eprintln!("Run with --help for usage info");
            std::process::exit(1);
        }
    };

    // ── Pass 1: Load calibration data ────────────────────────

    println!("  Pass 1: Loading calibration data...");

    let content = match fs::read_to_string(&cal_path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("  ERROR: Cannot read {}: {}", cal_path, e);
            std::process::exit(1);
        }
    };

    // Handle two formats: direct JSON or CALIBRATION_DATA|{json} log lines
    let cal: CalibrationFile = if content.starts_with('{') {
        serde_json::from_str(&content).unwrap_or_else(|e| {
            eprintln!("  ERROR: JSON parse failed: {}", e);
            std::process::exit(1);
        })
    } else {
        // Search for CALIBRATION_DATA| line
        let mut found = None;
        for line in content.lines() {
            if let Some(json_str) = line.trim().strip_prefix("CALIBRATION_DATA|") {
                found = Some(json_str.to_string());
                break;
            }
        }
        let json_str = match found {
            Some(s) => s,
            None => {
                eprintln!("  ERROR: No CALIBRATION_DATA| line found in {}", cal_path);
                std::process::exit(1);
            }
        };
        serde_json::from_str(&json_str).unwrap_or_else(|e| {
            eprintln!("  ERROR: JSON parse failed: {}", e);
            std::process::exit(1);
        })
    };

    println!("  Loaded: {} ({} characters, {} calculator calls, v{})",
        cal_path,
        cal.characters.len(),
        cal.calculator_interceptions.len(),
        cal.version,
    );

    if cal.characters.is_empty() {
        println!("\n  No character data in calibration file.");
        println!("  Run frida_calibration.js and call exportCalibration() in the REPL.");
        return;
    }

    // ── Pass 2: Build stat sheets ────────────────────────────

    println!("\n  Pass 2: Computing stat sheets...\n");

    let mut sheets: Vec<CharacterStatSheet> = Vec::new();

    let mut records: Vec<&CalibrationRecord> = cal.characters.values().collect();
    records.sort_by(|a, b| a.template_id.cmp(&b.template_id));

    for record in &records {
        // Filter by query if specified
        if let Some(ref qc) = query_char
            && !record.template_id.contains(qc.as_str()) {
                continue;
            }

        if let Some(sheet) = build_stat_sheet(record, None) {
            sheets.push(sheet);
        }
    }

    if sheets.is_empty() {
        if let Some(ref qc) = query_char {
            println!("  No matching character for '{}'. Available:", qc);
            for r in &records {
                println!("    - {}", r.template_id);
            }
        } else {
            println!("  No stat sheets could be built from calibration data.");
        }
        return;
    }

    // ── Pass 3: Display ──────────────────────────────────────

    println!("  {} character stat sheet(s):\n", sheets.len());

    for sheet in &sheets {
        display_stat_sheet(sheet);
    }

    // ── Pass 4: Growth curves (if multi-level data) ──────────

    let mut curves: Vec<GrowthCurve> = Vec::new();
    for record in &records {
        if let Some(ref qc) = query_char
            && !record.template_id.contains(qc.as_str()) {
                continue;
            }
        if let Some(curve) = estimate_growth_curve(record) {
            curves.push(curve);
        }
    }

    if !curves.is_empty() {
        println!("\n  Growth curves ({} characters with multi-level data):\n", curves.len());
        for curve in &curves {
            println!("  {} (levels: {:?})", curve.template_id, curve.levels);
            println!("    Per-level growth:");
            let mut growth: Vec<_> = curve.per_level_growth.iter().collect();
            growth.sort_by(|a, b| b.1.partial_cmp(a.1).unwrap_or(std::cmp::Ordering::Equal));
            for (k, v) in &growth {
                println!("      {:20} += {:.4}/level", k, v);
            }
            println!();
        }
    }

    // ── Pass 5: Export (if requested) ────────────────────────

    if export_mode {
        let _ = fs::create_dir_all(&output_dir);

        // JSON export
        let json_output = serde_json::json!({
            "generated": chrono::Utc::now().to_rfc3339(),
            "characters": sheets,
            "growth_curves": curves,
            "calculator_interceptions": cal.calculator_interceptions.len(),
        });
        let json_path = format!("{}/stat_sheets.json", output_dir);
        match fs::write(&json_path, serde_json::to_string_pretty(&json_output).unwrap_or_default()) {
            Ok(()) => println!("  Exported: {}", json_path),
            Err(e) => println!("  ERROR: {}: {}", json_path, e),
        }

        // CSV export
        let csv_path = format!("{}/stat_sheets.csv", output_dir);
        let mut csv = Vec::new();

        // Collect all stat names
        let mut all_stats: Vec<String> = sheets.iter()
            .flat_map(|s| s.final_stats.keys().cloned())
            .collect::<std::collections::HashSet<_>>()
            .into_iter()
            .collect();
        all_stats.sort();

        let scalar_keys = [
            "atkFromMain", "atkFromStr", "atkFromAgi", "atkFromWisd", "atkFromWill",
            "hpFromStr", "healFromWill", "physResistFromAgi", "energyResistFromWisd",
        ];

        csv.push(format!(
            "templateId,level,breakStage,mainAttr,subAttr,dataSource,{},{}",
            all_stats.iter().map(|s| format!("final_{}", s)).collect::<Vec<_>>().join(","),
            scalar_keys.join(","),
        ));

        for s in &sheets {
            let stat_vals: Vec<String> = all_stats.iter()
                .map(|name| s.final_stats.get(name).map(|v| format!("{:.4}", v)).unwrap_or_default())
                .collect();
            let scalar_vals: Vec<String> = scalar_keys.iter()
                .map(|k| s.scalars.get(*k).map(|v| format!("{:.6}", v)).unwrap_or_default())
                .collect();

            csv.push(format!(
                "{},{},{},{},{},{},{},{}",
                s.template_id, s.level, s.break_stage,
                s.main_attr_type.as_deref().unwrap_or(""),
                s.sub_attr_type.as_deref().unwrap_or(""),
                s.data_source,
                stat_vals.join(","),
                scalar_vals.join(","),
            ));
        }

        match fs::write(&csv_path, csv.join("\n")) {
            Ok(()) => println!("  Exported: {}", csv_path),
            Err(e) => println!("  ERROR: {}: {}", csv_path, e),
        }
    }

    // ── Summary ──────────────────────────────────────────────

    println!("\n{}", "═".repeat(62));
    println!("  Stat Engine Complete");
    println!("  {} characters, {} growth curves",
        sheets.len(),
        curves.len(),
    );
    println!("{}", "═".repeat(62));
}
