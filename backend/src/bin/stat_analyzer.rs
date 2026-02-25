/// Offline post-processing of raw registry data to decode numeric fields and identify character stats.
///
/// Reads JSON files from `./output/gamedata_raw/` (produced by game_data_probe Phase N).
/// No network access required — purely offline analysis.
///
/// Env vars:
///   PASS=1|2|3|4|5  — run specific pass (default: all)
///   INPUT_DIR=<path> — override input directory (default: ./output/gamedata_raw)
///   CHAR_ONLY=1     — only process character-related objects
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;

// ─── Data Structures ───

/// Parsed entry from a raw registry JSON file.
#[derive(Debug, Clone)]
struct ParsedEntry {
    rid: i64,
    class: String,
    ns: String,
    asm: String,
    data_size: usize,
    data_bytes: Vec<u8>,
    heuristic_strings: Vec<String>,
    heuristic_rids: Vec<(usize, i64)>,
}

/// Parsed object (one JSON file = one MonoBehaviour with its registry entries).
#[derive(Debug)]
struct ParsedObject {
    object_name: String,
    #[allow(dead_code)]
    type_name: String,
    entry_count: usize,
    entries: Vec<ParsedEntry>,
}

/// A decoded value at a specific byte offset.
#[derive(Debug, Clone)]
enum DecodedValue {
    Float(f32),
    Int(i32),
    Rid(i64),
    AlignedString(String),
}

impl std::fmt::Display for DecodedValue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DecodedValue::Float(v) => write!(f, "{v:.4}"),
            DecodedValue::Int(v) => write!(f, "{v}"),
            DecodedValue::Rid(v) => write!(f, "rid:{v}"),
            DecodedValue::AlignedString(s) => write!(f, "\"{s}\""),
        }
    }
}

/// Field template: what values appear at each offset across all instances of a type.
#[derive(Debug, Default)]
struct FieldTemplate {
    /// offset → list of (object_name, decoded_value)
    values: BTreeMap<usize, Vec<(String, DecodedValue)>>,
}

// ─── Helpers ───

fn format_bytes(bytes: u64) -> String {
    const UNITS: &[&str] = &["B", "KB", "MB", "GB", "TB"];
    if bytes == 0 {
        return "0 B".into();
    }
    let mut size = bytes as f64;
    let mut i = 0;
    while size >= 1024.0 && i < UNITS.len() - 1 {
        size /= 1024.0;
        i += 1;
    }
    format!("{size:.2} {}", UNITS[i])
}

/// Decode a potential f32 at the given offset.
fn decode_f32(bytes: &[u8], offset: usize) -> Option<f32> {
    if offset + 4 > bytes.len() {
        return None;
    }
    let val = f32::from_le_bytes(bytes[offset..offset + 4].try_into().ok()?);
    if val.is_finite() && val != 0.0 && val.abs() > 1e-10 && val.abs() < 1e10 {
        Some(val)
    } else if val == 0.0 {
        Some(0.0)
    } else {
        None
    }
}

/// Decode a potential i32 at the given offset.
fn decode_i32(bytes: &[u8], offset: usize) -> Option<i32> {
    if offset + 4 > bytes.len() {
        return None;
    }
    let val = i32::from_le_bytes(bytes[offset..offset + 4].try_into().ok()?);
    // Accept small integers and common game values
    if (0..=100_000).contains(&val) {
        Some(val)
    } else {
        None
    }
}

/// Decode a potential i64 (RID) at the given offset.
fn decode_rid(bytes: &[u8], offset: usize) -> Option<i64> {
    if offset + 8 > bytes.len() {
        return None;
    }
    let val = i64::from_le_bytes(bytes[offset..offset + 8].try_into().ok()?);
    // RIDs are large values or -2 (null ref)
    if val == -2 || (val > 1_000_000 && val.unsigned_abs() > 0x1_0000_0000) {
        Some(val)
    } else {
        None
    }
}

/// Decode an aligned string at the given offset: [i32 len][ascii][align].
/// Returns (string, bytes_consumed).
fn decode_aligned_string(bytes: &[u8], offset: usize) -> Option<(String, usize)> {
    if offset + 4 > bytes.len() {
        return None;
    }
    let len = i32::from_le_bytes(bytes[offset..offset + 4].try_into().ok()?);
    if !(1..=500).contains(&len) {
        return None;
    }
    let len = len as usize;
    if offset + 4 + len > bytes.len() {
        return None;
    }
    let str_bytes = &bytes[offset + 4..offset + 4 + len];
    if str_bytes.iter().all(|&b| b.is_ascii_graphic() || b == b' ')
        && let Ok(s) = std::str::from_utf8(str_bytes)
    {
        let aligned = (offset + 4 + len + 3) & !3;
        let consumed = aligned - offset;
        return Some((s.to_string(), consumed));
    }
    None
}

/// Decode all structured values from a byte buffer.
fn decode_all_values(bytes: &[u8]) -> Vec<(usize, DecodedValue)> {
    let mut result = Vec::new();
    let mut skip_until = 0usize;

    let mut offset = 0;
    while offset < bytes.len() {
        if offset < skip_until {
            offset += 4;
            continue;
        }

        // Try aligned string first (most specific)
        if let Some((s, consumed)) = decode_aligned_string(bytes, offset) {
            result.push((offset, DecodedValue::AlignedString(s)));
            skip_until = offset + consumed;
            offset = skip_until;
            // Re-align to 4
            offset = (offset + 3) & !3;
            continue;
        }

        // Try float FIRST — if both halves of a potential 8-byte RID are valid floats,
        // prefer the float interpretation (avoids misinterpreting adjacent f32 pairs as RIDs)
        if let Some(fval) = decode_f32(bytes, offset) {
            // Only record float if it doesn't also look like a plausible small integer
            // that we'd prefer as int. Prefer float for non-integral values.
            let ival = decode_i32(bytes, offset);
            if fval.fract() != 0.0 || fval.abs() > 100_000.0 {
                result.push((offset, DecodedValue::Float(fval)));
            } else if let Some(iv) = ival {
                result.push((offset, DecodedValue::Int(iv)));
            } else {
                result.push((offset, DecodedValue::Float(fval)));
            }
            offset += 4;
            continue;
        }

        // Try int
        if let Some(ival) = decode_i32(bytes, offset) {
            result.push((offset, DecodedValue::Int(ival)));
            offset += 4;
            continue;
        }

        // Try RID (8-byte) as last resort — only if neither 4-byte half is a valid float/int
        if let Some(rid) = decode_rid(bytes, offset) {
            result.push((offset, DecodedValue::Rid(rid)));
            skip_until = offset + 8;
            offset += 8;
            continue;
        }

        offset += 4;
    }

    result
}

/// Parse hex preview string back into bytes.
/// Format: "0d00 0000 6368 725f ..." (space-separated 4-char hex pairs)
fn hex_preview_to_bytes(hex: &str) -> Vec<u8> {
    let mut bytes = Vec::new();
    for chunk in hex.split_whitespace() {
        // Each chunk is 4 hex chars = 2 bytes
        let clean = chunk.replace(' ', "");
        let mut i = 0;
        while i + 2 <= clean.len() {
            if let Ok(b) = u8::from_str_radix(&clean[i..i + 2], 16) {
                bytes.push(b);
            }
            i += 2;
        }
    }
    bytes
}

/// Load all raw JSON files from the input directory.
fn load_objects(input_dir: &str, char_only: bool) -> Vec<ParsedObject> {
    let mut objects = Vec::new();
    let base = PathBuf::from(input_dir);

    let dirs = match std::fs::read_dir(&base) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("Cannot read {input_dir}: {e}");
            return objects;
        }
    };

    for dir_entry in dirs.flatten() {
        if !dir_entry.file_type().is_ok_and(|ft| ft.is_dir()) {
            continue;
        }

        let files = match std::fs::read_dir(dir_entry.path()) {
            Ok(f) => f,
            Err(_) => continue,
        };

        for file_entry in files.flatten() {
            let path = file_entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }

            let content = match std::fs::read_to_string(&path) {
                Ok(c) => c,
                Err(_) => continue,
            };

            let json: serde_json::Value = match serde_json::from_str(&content) {
                Ok(v) => v,
                Err(_) => continue,
            };

            let meta = &json["_meta"];
            let object_name = meta["object_name"].as_str().unwrap_or("").to_string();
            let type_name = meta["type_name"].as_str().unwrap_or("").to_string();
            let entry_count = meta["entry_count"].as_u64().unwrap_or(0) as usize;

            if char_only && !object_name.contains("chr_") {
                continue;
            }

            let mut entries = Vec::new();
            if let Some(arr) = json["entries"].as_array() {
                for e in arr {
                    let rid = e["rid"].as_i64().unwrap_or(0);
                    let typ = &e["type"];
                    let class = typ["class"].as_str().unwrap_or("").to_string();
                    let ns = typ["ns"].as_str().unwrap_or("").to_string();
                    let asm = typ["asm"].as_str().unwrap_or("").to_string();
                    let data_size = e["data_size"].as_u64().unwrap_or(0) as usize;
                    let hex = e["data_hex_preview"].as_str().unwrap_or("");
                    let data_bytes = hex_preview_to_bytes(hex);

                    let heuristic_strings: Vec<String> = e["heuristic_strings"]
                        .as_array()
                        .map(|a| {
                            a.iter()
                                .filter_map(|v| v.as_str().map(String::from))
                                .collect()
                        })
                        .unwrap_or_default();

                    let heuristic_rids: Vec<(usize, i64)> = e["heuristic_rids"]
                        .as_array()
                        .map(|a| {
                            a.iter()
                                .filter_map(|v| {
                                    let off = v["offset"].as_u64()? as usize;
                                    let rid = v["rid"].as_i64()?;
                                    Some((off, rid))
                                })
                                .collect()
                        })
                        .unwrap_or_default();

                    entries.push(ParsedEntry {
                        rid,
                        class,
                        ns,
                        asm,
                        data_size,
                        data_bytes,
                        heuristic_strings,
                        heuristic_rids,
                    });
                }
            }

            objects.push(ParsedObject {
                object_name,
                type_name,
                entry_count,
                entries,
            });
        }
    }

    objects.sort_by(|a, b| a.object_name.cmp(&b.object_name));
    objects
}

// ─── Pass 1: Type Catalog ───

fn pass1_type_catalog(objects: &[ParsedObject]) {
    println!("=== Pass 1: Type Catalog ===\n");

    // (class, ns, asm) → (count, total_data_bytes, objects containing it)
    type TypeInfo = HashMap<(String, String, String), (u32, u64, HashSet<String>)>;
    let mut type_info: TypeInfo = HashMap::new();
    let mut empty_type_count = 0u32;
    let mut total_entries = 0u32;

    for obj in objects {
        for entry in &obj.entries {
            total_entries += 1;
            if entry.class.is_empty() {
                empty_type_count += 1;
                continue;
            }
            let key = (entry.class.clone(), entry.ns.clone(), entry.asm.clone());
            let info = type_info.entry(key).or_insert((0, 0, HashSet::new()));
            info.0 += 1;
            info.1 += entry.data_size as u64;
            info.2.insert(obj.object_name.clone());
        }
    }

    let mut sorted: Vec<_> = type_info.iter().collect();
    sorted.sort_by_key(|(_, (count, _, _))| std::cmp::Reverse(*count));

    println!(
        "  Total objects: {}, Total entries: {}, Entries with type: {}, Empty type: {}",
        objects.len(),
        total_entries,
        total_entries - empty_type_count,
        empty_type_count
    );
    println!();

    println!(
        "  {:<45} {:>6} {:>10} {:>6}",
        "Type (class)", "Count", "Data", "Objs"
    );
    println!("  {}", "-".repeat(72));
    for ((class, ns, asm), (count, data, objs)) in &sorted {
        let ns_asm = if ns.is_empty() && asm.is_empty() {
            String::new()
        } else {
            format!(" ({ns}/{asm})")
        };
        println!(
            "  {:<45} {:>6} {:>10} {:>6}{}",
            class,
            count,
            format_bytes(*data),
            objs.len(),
            ns_asm
        );
    }

    // Types of interest for stats
    println!("\n  --- Stat-relevant types ---");
    let stat_keywords = [
        "Movement",
        "Ability",
        "Combat",
        "Stat",
        "Attribute",
        "Health",
        "Attack",
        "Defense",
        "Speed",
        "Damage",
        "Character",
        "Rotator",
        "Model",
        "Navigation",
    ];
    for ((class, _, _), (count, data, objs)) in &sorted {
        if stat_keywords
            .iter()
            .any(|kw| class.to_lowercase().contains(&kw.to_lowercase()))
        {
            println!(
                "  STAT_RELEVANT: {class} — {count}x, {}, in {} objects",
                format_bytes(*data),
                objs.len()
            );
        }
    }

    println!();
}

// ─── Pass 2: Numeric Decode ───

fn pass2_numeric_decode(objects: &[ParsedObject]) {
    println!("=== Pass 2: Numeric Decode (Field Templates) ===\n");

    // Group entries by class name, build field templates
    let mut templates: HashMap<String, FieldTemplate> = HashMap::new();
    let mut class_sizes: HashMap<String, Vec<usize>> = HashMap::new();

    for obj in objects {
        for entry in &obj.entries {
            let class_key = if entry.class.is_empty() {
                format!("_unknown_{}b", entry.data_size)
            } else {
                entry.class.clone()
            };

            class_sizes
                .entry(class_key.clone())
                .or_default()
                .push(entry.data_size);

            if entry.data_bytes.is_empty() {
                continue;
            }

            let decoded = decode_all_values(&entry.data_bytes);
            let template = templates.entry(class_key).or_default();

            for (offset, value) in decoded {
                template
                    .values
                    .entry(offset)
                    .or_default()
                    .push((obj.object_name.clone(), value));
            }
        }
    }

    // Sort by class name
    let mut sorted_classes: Vec<_> = templates.keys().cloned().collect();
    sorted_classes.sort();

    let mut output_data: serde_json::Map<String, serde_json::Value> = serde_json::Map::new();

    for class in &sorted_classes {
        let template = &templates[class];
        let sizes = class_sizes.get(class).unwrap();
        let instance_count = sizes.len();
        let avg_size = sizes.iter().sum::<usize>() / instance_count.max(1);

        if template.values.is_empty() {
            continue;
        }

        println!("  Type: {class} ({instance_count} instances, avg {avg_size} bytes)");

        let mut field_entries = serde_json::Map::new();

        for (offset, values) in &template.values {
            // Categorize: how many unique values at this offset?
            let unique_count = values
                .iter()
                .map(|(_, v)| format!("{v}"))
                .collect::<HashSet<_>>()
                .len();

            let variability = if unique_count == 1 {
                "CONSTANT"
            } else if unique_count as f64 / instance_count as f64 > 0.8 {
                "VARIES"
            } else {
                "PARTIAL"
            };

            // Sample values
            let samples: Vec<String> = values.iter().take(3).map(|(_, v)| format!("{v}")).collect();

            let kind = match values.first() {
                Some((_, DecodedValue::Float(_))) => "f32",
                Some((_, DecodedValue::Int(_))) => "i32",
                Some((_, DecodedValue::Rid(_))) => "rid",
                Some((_, DecodedValue::AlignedString(_))) => "str",
                None => "?",
            };

            if variability == "VARIES" || variability == "PARTIAL" {
                println!(
                    "    +{offset:04}: {kind} [{variability}] {unique_count}/{instance_count} unique — {}",
                    samples.join(", ")
                );
            }

            field_entries.insert(
                format!("{offset}"),
                serde_json::json!({
                    "kind": kind,
                    "variability": variability,
                    "unique": unique_count,
                    "total": instance_count,
                    "samples": samples,
                }),
            );
        }

        output_data.insert(class.clone(), serde_json::Value::Object(field_entries));
        println!();
    }

    // Write field templates to JSON
    let output_path = "./output/field_templates.json";
    if let Ok(pretty) = serde_json::to_string_pretty(&output_data) {
        let _ = std::fs::create_dir_all("./output");
        let _ = std::fs::write(output_path, &pretty);
        println!("  Field templates written to {output_path}");
    }
    println!();
}

/// Entries grouped by class: Vec<(name, decoded_values, data_size)>
type ClassInstances = Vec<(String, Vec<(usize, DecodedValue)>, usize)>;

// ─── Pass 3: Character Comparison (by class name) ───

/// Compare entries across characters grouped by CLASS NAME, not index.
fn pass3_character_comparison(objects: &[ParsedObject]) {
    println!("=== Pass 3: Character Comparison (by class) ===\n");

    // Find character template objects (data_chr_XXXX pattern only)
    let char_objects: Vec<&ParsedObject> = objects
        .iter()
        .filter(|o| o.object_name.starts_with("data_chr_"))
        .collect();

    if char_objects.is_empty() {
        println!("  No character objects found!");
        return;
    }

    println!("  Found {} character objects:", char_objects.len());
    for obj in &char_objects {
        println!("    {} — {} entries", obj.object_name, obj.entry_count);
    }
    println!();

    // Group entries by class name across all characters.
    // Key: class name → Vec<(char_name, decoded_values, data_size)>
    let mut by_class: BTreeMap<String, ClassInstances> = BTreeMap::new();

    for obj in &char_objects {
        let char_name = obj
            .object_name
            .strip_prefix("data_")
            .unwrap_or(&obj.object_name)
            .to_string();

        for entry in &obj.entries {
            let class_key = if entry.class.is_empty() {
                // Group unknown-type entries by data_size to avoid mixing
                format!("_unknown_{}b", entry.data_size)
            } else {
                entry.class.clone()
            };

            let decoded = decode_all_values(&entry.data_bytes);
            by_class.entry(class_key).or_default().push((
                char_name.clone(),
                decoded,
                entry.data_size,
            ));
        }
    }

    let mut stat_candidates: Vec<serde_json::Value> = Vec::new();

    for (class_name, instances) in &by_class {
        // Skip types that appear in fewer than 2 characters (can't compare)
        let unique_chars: HashSet<&str> = instances.iter().map(|(n, _, _)| n.as_str()).collect();
        if unique_chars.len() < 2 {
            continue;
        }

        // Collect all unique offsets
        let all_offsets: BTreeMap<usize, ()> = instances
            .iter()
            .flat_map(|(_, vals, _)| vals.iter().map(|(off, _)| (*off, ())))
            .collect();

        if all_offsets.is_empty() {
            continue;
        }

        // For each offset, check if values differ between characters
        type VaryingField = (usize, String, Vec<(String, String)>);
        let mut varying_fields: Vec<VaryingField> = Vec::new();
        let mut constant_count = 0usize;

        for &offset in all_offsets.keys() {
            let values_at_offset: Vec<(String, String)> = instances
                .iter()
                .filter_map(|(name, vals, _)| {
                    vals.iter()
                        .find(|(off, _)| *off == offset)
                        .map(|(_, v)| (name.clone(), format!("{v}")))
                })
                .collect();

            if values_at_offset.is_empty() {
                continue;
            }

            let unique: HashSet<&str> = values_at_offset.iter().map(|(_, v)| v.as_str()).collect();
            if unique.len() > 1 {
                let kind = instances
                    .iter()
                    .find_map(|(_, vals, _)| {
                        vals.iter().find(|(off, _)| *off == offset).map(|(_, v)| {
                            match v {
                                DecodedValue::Float(_) => "f32",
                                DecodedValue::Int(_) => "i32",
                                DecodedValue::Rid(_) => "rid",
                                DecodedValue::AlignedString(_) => "str",
                            }
                            .to_string()
                        })
                    })
                    .unwrap_or_else(|| "?".to_string());
                varying_fields.push((offset, kind, values_at_offset));
            } else {
                constant_count += 1;
            }
        }

        if varying_fields.is_empty() {
            continue;
        }

        let avg_size = instances.iter().map(|(_, _, s)| s).sum::<usize>() / instances.len();

        println!(
            "  {class_name} ({} chars, avg {avg_size} bytes) — {} varying, {constant_count} constant",
            unique_chars.len(),
            varying_fields.len(),
        );

        for (offset, kind, values) in &varying_fields {
            // Skip rid/str fields for concise output — focus on numeric stats
            if kind == "rid" || kind == "str" {
                // Still record in JSON but don't print
                stat_candidates.push(serde_json::json!({
                    "class": class_name,
                    "offset": offset,
                    "kind": kind,
                    "char_count": values.len(),
                    "values": values.iter()
                        .map(|(name, val)| serde_json::json!({"char": name, "value": val}))
                        .collect::<Vec<_>>(),
                }));
                continue;
            }

            println!("    STAT +{offset:04} ({kind}):");
            for (char_name, val) in values {
                println!("      {char_name:<30} = {val}");
            }

            stat_candidates.push(serde_json::json!({
                "class": class_name,
                "offset": offset,
                "kind": kind,
                "char_count": values.len(),
                "values": values.iter()
                    .map(|(name, val)| serde_json::json!({"char": name, "value": val}))
                    .collect::<Vec<_>>(),
            }));
        }
        println!();
    }

    // Write stat candidates to JSON
    let output_path = "./output/stat_candidates.json";
    if let Ok(pretty) = serde_json::to_string_pretty(&stat_candidates) {
        let _ = std::fs::create_dir_all("./output");
        let _ = std::fs::write(output_path, &pretty);
        println!(
            "  {} stat candidates written to {output_path}",
            stat_candidates.len()
        );
    }
    println!();
}

// ─── Pass 5: Combat Stat Hunt ───

/// Dedicated search for combat-like stats across ALL object types (not just characters).
/// Looks for numeric patterns typical of game stats: HP, ATK, DEF, elemental values, etc.
fn pass5_combat_stat_hunt(objects: &[ParsedObject]) {
    println!("=== Pass 5: Combat Stat Hunt ===\n");

    // Strategy 1: Check enemy template data — enemies always have visible HP/ATK
    let enemy_objects: Vec<&ParsedObject> = objects
        .iter()
        .filter(|o| o.object_name.contains("eny_"))
        .collect();

    println!(
        "  --- Enemy Template Analysis ({} objects) ---",
        enemy_objects.len()
    );

    // Group enemy entries by class, look for stat-like types
    let stat_type_keywords = [
        "Stat",
        "Attribute",
        "Health",
        "HP",
        "Attack",
        "ATK",
        "Defense",
        "DEF",
        "Damage",
        "Element",
        "Level",
        "Ability",
        "Combat",
        "Power",
        "Resist",
        "Shield",
        "Crit",
        "Speed",
        "Range",
        "Poise",
    ];

    let mut enemy_by_class: BTreeMap<String, ClassInstances> = BTreeMap::new();

    for obj in &enemy_objects {
        let name = obj
            .object_name
            .strip_prefix("data_")
            .unwrap_or(&obj.object_name)
            .to_string();
        for entry in &obj.entries {
            if !entry.class.is_empty() {
                let decoded = decode_all_values(&entry.data_bytes);
                enemy_by_class
                    .entry(entry.class.clone())
                    .or_default()
                    .push((name.clone(), decoded, entry.data_size));
            }
        }
    }

    // Print all enemy component types with their instance count and size
    println!("  Enemy component types:");
    let mut enemy_sorted: Vec<_> = enemy_by_class.iter().collect();
    enemy_sorted.sort_by_key(|(_, v)| std::cmp::Reverse(v.len()));
    for (class, instances) in &enemy_sorted {
        let avg_size = if instances.is_empty() {
            0
        } else {
            instances.iter().map(|(_, _, s)| s).sum::<usize>() / instances.len()
        };
        let is_stat = stat_type_keywords
            .iter()
            .any(|kw| class.to_lowercase().contains(&kw.to_lowercase()));
        let marker = if is_stat { " <<<" } else { "" };
        println!(
            "    {class:<55} {:>4}x  avg {:>6} bytes{marker}",
            instances.len(),
            avg_size
        );
    }
    println!();

    // Strategy 2: Look for large int/float values typical of game stats
    // HP: 1000-100000, ATK: 100-5000, DEF: 100-5000
    println!("  --- Stat-Range Value Scan (characters) ---");

    let char_objects: Vec<&ParsedObject> = objects
        .iter()
        .filter(|o| o.object_name.starts_with("data_chr_"))
        .collect();

    // Scan ALL entries (including unknown-type) for values in stat ranges
    for obj in &char_objects {
        let char_name = obj
            .object_name
            .strip_prefix("data_")
            .unwrap_or(&obj.object_name);

        let mut stat_hits: Vec<(usize, &str, usize, String, DecodedValue)> = Vec::new();

        for (idx, entry) in obj.entries.iter().enumerate() {
            if entry.data_bytes.is_empty() {
                continue;
            }
            let decoded = decode_all_values(&entry.data_bytes);
            for (offset, val) in &decoded {
                let is_stat_range = match val {
                    // Large ints typical of HP/ATK/DEF
                    DecodedValue::Int(v) => *v >= 100 && *v <= 100_000,
                    // Floats in multiplier range (but not tiny or huge)
                    DecodedValue::Float(v) => *v >= 0.5 && *v <= 10000.0 && *v != 1.0,
                    _ => false,
                };
                if is_stat_range {
                    let class = if entry.class.is_empty() {
                        "_unknown"
                    } else {
                        entry.class.as_str()
                    };
                    stat_hits.push((idx, class, *offset, format!("{val}"), val.clone()));
                }
            }
        }

        if !stat_hits.is_empty() {
            println!("  {char_name}: {} stat-range values", stat_hits.len());
            for (idx, class, offset, val_str, _) in stat_hits.iter().take(20) {
                println!("    entry[{idx}] {class} +{offset:04}: {val_str}");
            }
            if stat_hits.len() > 20 {
                println!("    ... and {} more", stat_hits.len() - 20);
            }
        }
    }
    println!();

    // Strategy 3: Compare same-class entries between chars AND enemies
    // If CharacterMovementComponentData exists in both, the shared structure reveals field meanings
    println!("  --- Character vs Enemy shared types ---");

    let mut char_classes: HashSet<String> = HashSet::new();
    for obj in &char_objects {
        for entry in &obj.entries {
            if !entry.class.is_empty() {
                char_classes.insert(entry.class.clone());
            }
        }
    }

    let mut enemy_classes: HashSet<String> = HashSet::new();
    for obj in &enemy_objects {
        for entry in &obj.entries {
            if !entry.class.is_empty() {
                enemy_classes.insert(entry.class.clone());
            }
        }
    }

    let shared: Vec<&String> = char_classes.intersection(&enemy_classes).collect();
    let mut shared_sorted: Vec<&&String> = shared.iter().collect();
    shared_sorted.sort();
    println!(
        "  Shared types between characters and enemies ({}):",
        shared.len()
    );
    for class in &shared_sorted {
        let char_count = char_objects
            .iter()
            .flat_map(|o| o.entries.iter())
            .filter(|e| &e.class == **class)
            .count();
        let enemy_count = enemy_objects
            .iter()
            .flat_map(|o| o.entries.iter())
            .filter(|e| &e.class == **class)
            .count();
        println!(
            "    {:<50} chr:{char_count:>3}  eny:{enemy_count:>3}",
            class
        );
    }
    println!();

    // Strategy 4: Dump ALL unique types across the entire dataset with their avg data sizes
    // Types with data_size 0 can't hold stats
    println!("  --- All types with numeric data (sorted by avg size) ---");
    let mut all_types: BTreeMap<String, (usize, u64)> = BTreeMap::new();
    for obj in objects {
        for entry in &obj.entries {
            if !entry.class.is_empty() && entry.data_size > 0 {
                let info = all_types.entry(entry.class.clone()).or_insert((0, 0));
                info.0 += 1;
                info.1 += entry.data_size as u64;
            }
        }
    }
    let mut types_by_size: Vec<_> = all_types.iter().collect();
    types_by_size.sort_by_key(|(_, (count, total))| std::cmp::Reverse(*total / (*count as u64)));

    for (class, (count, total)) in types_by_size.iter().take(40) {
        let avg = *total / (*count as u64);
        let is_stat = stat_type_keywords
            .iter()
            .any(|kw| class.to_lowercase().contains(&kw.to_lowercase()));
        let marker = if is_stat { " <<<" } else { "" };
        println!(
            "    {class:<55} {:>5}x  avg {:>6} bytes{marker}",
            count, avg
        );
    }

    println!("\n=== Pass 5 Done ===\n");
}

// ─── Pass 4: Cross-Reference Graph ───

fn pass4_cross_reference(objects: &[ParsedObject]) {
    println!("=== Pass 4: Cross-Reference Graph ===\n");

    // Build RID → (object_name, entry_idx, class) mapping
    let mut rid_to_source: HashMap<i64, (String, usize, String)> = HashMap::new();

    for obj in objects {
        for (idx, entry) in obj.entries.iter().enumerate() {
            rid_to_source.insert(
                entry.rid,
                (obj.object_name.clone(), idx, entry.class.clone()),
            );
        }
    }

    println!("  Total RIDs indexed: {}", rid_to_source.len());

    // Build forward references: for each entry, which RIDs does it reference?
    let mut forward_refs: HashMap<i64, Vec<i64>> = HashMap::new();

    for obj in objects {
        for entry in &obj.entries {
            let referenced_rids: Vec<i64> = entry
                .heuristic_rids
                .iter()
                .map(|(_, rid)| *rid)
                .filter(|rid| *rid != -2 && rid_to_source.contains_key(rid))
                .collect();

            if !referenced_rids.is_empty() {
                forward_refs.insert(entry.rid, referenced_rids);
            }
        }
    }

    println!("  Entries with forward references: {}", forward_refs.len());

    // Build object-level graph for character template objects
    let char_objects: Vec<&ParsedObject> = objects
        .iter()
        .filter(|o| o.object_name.starts_with("data_chr_"))
        .collect();

    let mut graph_json: Vec<serde_json::Value> = Vec::new();

    for obj in &char_objects {
        let mut obj_graph = serde_json::json!({
            "object": obj.object_name,
            "entries": [],
        });

        let entries_arr = obj_graph["entries"].as_array_mut().unwrap();

        for (idx, entry) in obj.entries.iter().enumerate() {
            let refs: Vec<serde_json::Value> = entry
                .heuristic_rids
                .iter()
                .filter_map(|(off, rid)| {
                    if *rid == -2 {
                        return None;
                    }
                    let target = rid_to_source.get(rid);
                    Some(serde_json::json!({
                        "offset": off,
                        "rid": rid,
                        "target": target.map(|(name, tidx, cls)| {
                            serde_json::json!({
                                "object": name,
                                "entry_idx": tidx,
                                "class": cls,
                            })
                        }),
                    }))
                })
                .collect();

            entries_arr.push(serde_json::json!({
                "idx": idx,
                "rid": entry.rid,
                "class": entry.class,
                "data_size": entry.data_size,
                "strings": entry.heuristic_strings,
                "references": refs,
            }));
        }

        // Trace the full reference chain from entry 0
        let mut chain: Vec<String> = Vec::new();
        let mut visited: HashSet<i64> = HashSet::new();
        let mut queue: Vec<i64> = vec![obj.entries.first().map_or(0, |e| e.rid)];

        while let Some(rid) = queue.pop() {
            if !visited.insert(rid) {
                continue;
            }
            if let Some((_, _, cls)) = rid_to_source.get(&rid) {
                let label = if cls.is_empty() {
                    format!("rid:{rid}")
                } else {
                    cls.clone()
                };
                chain.push(label);

                if let Some(refs) = forward_refs.get(&rid) {
                    for r in refs {
                        queue.push(*r);
                    }
                }
            }
        }

        println!(
            "  {} — {} entries, chain: {}",
            obj.object_name,
            obj.entries.len(),
            chain.join(" → ")
        );

        obj_graph["reference_chain"] = serde_json::json!(chain);
        graph_json.push(obj_graph);
    }

    // Write graph to JSON
    let output_path = "./output/reference_graph.json";
    if let Ok(pretty) = serde_json::to_string_pretty(&graph_json) {
        let _ = std::fs::create_dir_all("./output");
        let _ = std::fs::write(output_path, &pretty);
        println!(
            "\n  Reference graph written to {output_path} ({} characters)",
            graph_json.len()
        );
    }
    println!();
}

// ─── Main ───

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let input_dir = std::env::var("INPUT_DIR").unwrap_or_else(|_| "./output/gamedata_raw".into());
    let char_only = std::env::var("CHAR_ONLY").is_ok_and(|v| v == "1");
    let pass_filter = std::env::var("PASS").ok();

    let should_run = |pass: &str| -> bool {
        pass_filter
            .as_ref()
            .is_none_or(|f| f.split(',').any(|p| p.trim() == pass))
    };

    println!("=== Stat Analyzer ===");
    println!("  Input: {input_dir}");
    println!("  Char only: {char_only}");
    if let Some(ref f) = pass_filter {
        println!("  Pass filter: {f}");
    }
    println!();

    // Load all objects
    println!("Loading objects from {input_dir}...");
    let objects = load_objects(&input_dir, char_only);
    let total_entries: usize = objects.iter().map(|o| o.entries.len()).sum();
    let total_data: u64 = objects
        .iter()
        .flat_map(|o| o.entries.iter())
        .map(|e| e.data_size as u64)
        .sum();

    // Check for truncated entries
    let truncated: Vec<_> = objects
        .iter()
        .flat_map(|o| {
            o.entries.iter().filter_map(|e| {
                if e.data_bytes.len() < e.data_size {
                    Some((
                        o.object_name.as_str(),
                        &e.class,
                        e.data_bytes.len(),
                        e.data_size,
                    ))
                } else {
                    None
                }
            })
        })
        .collect();

    println!(
        "  Loaded {} objects, {} entries, {} data",
        objects.len(),
        total_entries,
        format_bytes(total_data)
    );
    if truncated.is_empty() {
        println!("  All entry data is complete (no truncation).");
    } else {
        println!(
            "  WARNING: {} entries have truncated data (hex_preview < data_size):",
            truncated.len()
        );
        for (obj, class, have, need) in truncated.iter().take(10) {
            let class_label = if class.is_empty() {
                "<unknown>"
            } else {
                class.as_str()
            };
            println!("    {obj} / {class_label}: {have}/{need} bytes");
        }
        if truncated.len() > 10 {
            println!("    ... and {} more", truncated.len() - 10);
        }
        println!("  Re-run game_data_probe with RAW_REGISTRY=1 to get full data.");
    }
    println!();

    if objects.is_empty() {
        eprintln!(
            "No objects found in {input_dir}. Run game_data_probe with RAW_REGISTRY=1 first."
        );
        return Ok(());
    }

    if should_run("1") {
        pass1_type_catalog(&objects);
    }

    if should_run("2") {
        pass2_numeric_decode(&objects);
    }

    if should_run("3") {
        pass3_character_comparison(&objects);
    }

    if should_run("4") {
        pass4_cross_reference(&objects);
    }

    if should_run("5") {
        pass5_combat_stat_hunt(&objects);
    }

    println!("=== Done ===");
    Ok(())
}
