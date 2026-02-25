/// Game data extraction with ManagedReferencesRegistry resolution (Phase M).
///
/// Pipeline: config → ZIP → .chk → VFS decrypt → UnityFS → SerializedFile → MonoBehaviour
///           → parse with ManagedReferencesRegistry → resolve rid references → JSON
///
/// Env vars:
///   SCAN_ALL=1      — process ALL .chk files (default: smallest per dir)
///   EXTRACT_RESOLVED=1 — run Pass 3+4 (extraction + summary)
///   DUMP_SCRIPT=<hash> — dump TypeTree + raw bytes for a specific script type
///   RAW_REGISTRY=1  — Phase N: extract raw ManagedReferencesRegistry data
use backend::core::endfield::{
    config::{ResolveParams, resolve_game_config},
    constants::{app_code, channel, sub_channel},
};
use backend::core::hypergryph::launcher::Platform;
use backend::extraction::unityfs_header;
use backend::extraction::vfs_decrypt;
use backend::extraction::zip_range::{SplitArchivePart, ZipCentralDirectory};
use reqwest::Client;
use std::collections::HashMap;
use unity_asset_binary::asset::{ObjectInfo, SerializedFile, SerializedFileParser};
use unity_asset_binary::object::ObjectHandle;
use unity_asset_binary::reader::{BinaryReader, ByteOrder};
use unity_asset_binary::typetree::{TypeTreeParseOptions, TypeTreeSerializer};

/// VFS container directories.
const VFS_DIRS: &[(&str, &str)] = &[("7064D8E2", "bundles/configs"), ("0CE8FA57", "textures")];

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
    format!("{:.2} {}", size, UNITS[i])
}

/// Get the script type name for a MonoBehaviour object.
fn get_script_type_name(sf: &SerializedFile, info: &ObjectInfo) -> String {
    if info.type_index >= 0
        && let Some(st) = sf.types.get(info.type_index as usize)
    {
        if !st.type_tree.nodes.is_empty() {
            let root_type = &st.type_tree.nodes[0].type_name;
            if !root_type.is_empty() && root_type != "MonoBehaviour" {
                return root_type.clone();
            }
        }
        if !st.class_name.is_empty() {
            return st.class_name.clone();
        }
        if st.script_id != [0u8; 16] {
            return format!(
                "script_{}",
                st.script_id
                    .iter()
                    .map(|b| format!("{b:02x}"))
                    .collect::<String>()
            );
        }
    }
    format!("class_{}", info.type_id)
}

fn sanitize_filename(name: &str) -> String {
    name.replace(['\\', ':', '*', '?', '"', '<', '>', '|'], "_")
}

/// All extracted contents from a .chk file.
struct ChkContents {
    sf_data: Vec<Vec<u8>>,
}

fn extract_chk_contents(data: &[u8]) -> Option<ChkContents> {
    if !vfs_decrypt::is_valid_header(data) {
        return None;
    }
    let assets = match unityfs_header::process_chk_file(data) {
        Ok(a) => a,
        Err(_) => return None,
    };

    let mut sf_data: Vec<Vec<u8>> = Vec::new();
    for asset in assets {
        if asset.is_unityfs {
            for entry in &asset.bundle_entries {
                let start = entry.offset as usize;
                let end = (start + entry.size as usize).min(asset.data.len());
                if start >= asset.data.len() {
                    continue;
                }
                if !entry.path.ends_with(".resS") && !entry.path.ends_with(".resource") {
                    sf_data.push(asset.data[start..end].to_vec());
                }
            }
        } else if !asset.vfs_name.ends_with(".resS") && !asset.vfs_name.ends_with(".resource") {
            sf_data.push(asset.data);
        }
    }

    Some(ChkContents { sf_data })
}

/// Pass 1: Census of ref_types available per SerializedFile.
fn pass1_ref_types_census(sf: &SerializedFile) -> (usize, Vec<(String, String, String)>) {
    let count = sf.ref_types.len();
    let types: Vec<(String, String, String)> = sf
        .ref_types
        .iter()
        .map(|t| {
            (
                t.class_name.clone(),
                t.namespace.clone(),
                t.assembly_name.clone(),
            )
        })
        .collect();
    (count, types)
}

/// Info about a MonoBehaviour's registry presence.
struct RegistryInfo {
    type_name: String,
    obj_size: usize,
    has_registry: bool,
}

/// Pass 2: Detect which MonoBehaviour objects have ManagedReferencesRegistry in their TypeTree.
fn pass2_registry_detection(sf: &SerializedFile) -> Vec<RegistryInfo> {
    let mut results = Vec::new();

    for obj in sf.objects_of_type(114) {
        let type_name = get_script_type_name(sf, obj);
        let obj_size = obj.byte_size as usize;

        // Check TypeTree for ManagedReferencesRegistry node
        let has_registry = if obj.type_index >= 0
            && let Some(st) = sf.types.get(obj.type_index as usize)
        {
            fn has_managed_ref(nodes: &[unity_asset_binary::typetree::TypeTreeNode]) -> bool {
                for node in nodes {
                    if node.type_name == "ManagedReferencesRegistry" {
                        return true;
                    }
                    if has_managed_ref(&node.children) {
                        return true;
                    }
                }
                false
            }
            has_managed_ref(&st.type_tree.nodes)
        } else {
            false
        };

        results.push(RegistryInfo {
            type_name,
            obj_size,
            has_registry,
        });
    }

    results
}

/// Walk a JSON value, resolving `{ "rid": N }` references using the registry's RefIds array.
fn resolve_rids(obj: &mut serde_json::Value, registry: &serde_json::Value) {
    let mut rid_map: HashMap<i64, serde_json::Value> = HashMap::new();
    if let Some(ref_ids) = registry.get("RefIds").and_then(|v| v.as_array()) {
        for entry in ref_ids {
            if let Some(rid) = entry.get("rid").and_then(|v| v.as_i64()) {
                // The actual data is in the "data" sub-object
                if let Some(data) = entry.get("data") {
                    rid_map.insert(rid, data.clone());
                }
            }
        }
    }

    fn walk(val: &mut serde_json::Value, map: &HashMap<i64, serde_json::Value>) {
        match val {
            serde_json::Value::Object(obj) => {
                if obj.len() == 1
                    && obj.contains_key("rid")
                    && let Some(rid) = obj.get("rid").and_then(|v| v.as_i64())
                {
                    if rid == -2 {
                        *val = serde_json::Value::Null;
                        return;
                    }
                    if let Some(resolved) = map.get(&rid) {
                        *val = resolved.clone();
                        return;
                    }
                }
                for v in obj.values_mut() {
                    walk(v, map);
                }
            }
            serde_json::Value::Array(arr) => {
                for v in arr.iter_mut() {
                    walk(v, map);
                }
            }
            _ => {}
        }
    }
    walk(obj, &rid_map);
}

/// Dump TypeTree structure and raw bytes for objects of a specific script type.
fn dump_script_type(sf: &SerializedFile, target_hash: &str) {
    for obj in sf.objects_of_type(114) {
        let type_name = get_script_type_name(sf, obj);
        if !type_name.contains(target_hash) {
            continue;
        }

        let handle = ObjectHandle::new(sf, obj);
        let obj_name = handle
            .peek_name()
            .ok()
            .flatten()
            .unwrap_or_else(|| format!("obj_{}", obj.path_id));

        println!(
            "  Object: {} (pathID={}, size={})",
            obj_name, obj.path_id, obj.byte_size
        );

        // Dump TypeTree structure
        if obj.type_index >= 0
            && let Some(st) = sf.types.get(obj.type_index as usize)
        {
            println!("    TypeTree ({} nodes):", st.type_tree.nodes.len());
            fn dump_tree(nodes: &[unity_asset_binary::typetree::TypeTreeNode], indent: usize) {
                for node in nodes {
                    let pad = "  ".repeat(indent);
                    println!(
                        "    {pad}{}: {} (size={}, flags=0x{:04x})",
                        node.name, node.type_name, node.byte_size, node.meta_flags
                    );
                    dump_tree(&node.children, indent + 1);
                }
            }
            dump_tree(&st.type_tree.nodes, 0);
        }

        // Dump raw bytes
        if let Ok(raw) = sf.object_bytes(obj) {
            println!("    Raw bytes ({} total):", raw.len());
            let show = raw.len().min(256);
            for chunk_start in (0..show).step_by(32) {
                let end = (chunk_start + 32).min(show);
                let hex: Vec<String> = raw[chunk_start..end]
                    .iter()
                    .map(|b| format!("{b:02x}"))
                    .collect();
                let ascii: String = raw[chunk_start..end]
                    .iter()
                    .map(|&b| {
                        if b.is_ascii_graphic() || b == b' ' {
                            b as char
                        } else {
                            '.'
                        }
                    })
                    .collect();
                println!("    {:04x}: {:48} |{}|", chunk_start, hex.join(" "), ascii);
            }
            if raw.len() > show {
                println!("    ... {} more bytes", raw.len() - show);
            }
        }
        println!();
    }
}

/// Game-data-relevant type name patterns.
fn is_gamedata_type(name: &str) -> bool {
    let lower = name.to_lowercase();
    let keywords = [
        "table",
        "config",
        "data",
        "character",
        "stat",
        "gacha",
        "skill",
        "weapon",
        "item",
        "buff",
        "level",
        "quest",
        "stage",
        "enemy",
        "equip",
        "talent",
        "recipe",
        "shop",
        "reward",
        "mission",
        "ability",
        "effect",
        "combat",
        "attr",
        "param",
        "const",
    ];
    keywords.iter().any(|kw| lower.contains(kw))
}

/// Character-relevant field patterns in JSON keys.
fn has_character_fields(json: &serde_json::Value) -> bool {
    let keywords = [
        "attack", "defense", "hp", "health", "level", "atk", "def", "star", "rarity", "cost",
        "maxlevel", "baseatk", "basedef",
    ];
    let json_str = json.to_string().to_lowercase();
    keywords.iter().any(|kw| json_str.contains(kw))
}

/// Pass 3: Extract MonoBehaviours with registry resolution.
struct ResolvedExport {
    type_name: String,
    obj_name: String,
    json: String,
    had_registry: bool,
    rid_count: usize,
}

/// Check if this MonoBehaviour's TypeTree has a ManagedReferencesRegistry node.
fn type_has_registry(sf: &SerializedFile, info: &ObjectInfo) -> bool {
    if info.type_index >= 0
        && let Some(st) = sf.types.get(info.type_index as usize)
    {
        fn check(nodes: &[unity_asset_binary::typetree::TypeTreeNode]) -> bool {
            for node in nodes {
                if node.type_name == "ManagedReferencesRegistry" {
                    return true;
                }
                if check(&node.children) {
                    return true;
                }
            }
            false
        }
        check(&st.type_tree.nodes)
    } else {
        false
    }
}

fn pass3_extract_resolved(sf: &SerializedFile) -> Vec<ResolvedExport> {
    let options = TypeTreeParseOptions {
        parse_managed_refs: true,
        ..Default::default()
    };

    let mut results = Vec::new();

    for obj in sf.objects_of_type(114) {
        let type_name = get_script_type_name(sf, obj);

        // Extract game-data-relevant types OR any type that has a registry
        if !is_gamedata_type(&type_name) && !type_has_registry(sf, obj) {
            continue;
        }

        let handle = ObjectHandle::new(sf, obj);
        let obj_name = handle
            .peek_name()
            .ok()
            .flatten()
            .filter(|n| !n.is_empty())
            .map_or_else(|| format!("obj_{}", obj.path_id), |n| sanitize_filename(&n));

        let unity_obj = match handle.read_with_options(options) {
            Ok(o) => o,
            Err(_) => continue,
        };

        let props = unity_obj.class.properties();
        let mut json: serde_json::Value = match serde_json::to_value(props) {
            Ok(j) => j,
            Err(_) => continue,
        };

        // Find the registry — it appears under a TypeTree node name (e.g., "references"),
        // not necessarily "ManagedReferencesRegistry". Look for any top-level object
        // containing a "RefIds" array.
        let registry = json
            .as_object()
            .and_then(|top| {
                top.values().find(|v| {
                    v.get("RefIds")
                        .and_then(|r| r.as_array())
                        .is_some_and(|a| !a.is_empty())
                })
            })
            .cloned();

        let had_registry = registry.is_some();

        // Count rid references before resolution
        let json_str = json.to_string();
        let rid_count = json_str.matches("\"rid\"").count();

        if let Some(reg) = registry {
            resolve_rids(&mut json, &reg);
        }

        let pretty = match serde_json::to_string_pretty(&json) {
            Ok(j) => j,
            Err(_) => continue,
        };

        results.push(ResolvedExport {
            type_name: sanitize_filename(&type_name),
            obj_name,
            json: pretty,
            had_registry,
            rid_count,
        });
    }

    results
}

// ─── Phase N: Manual ManagedReferencesRegistry Parser ───

/// A single entry from the ManagedReferencesRegistry, parsed from raw binary.
#[derive(Debug)]
struct RawRegistryEntry {
    rid: i64,
    class_name: String,
    namespace: String,
    assembly_name: String,
    data_bytes: Vec<u8>,
}

/// Heuristic field extraction results from raw binary data.
#[derive(Debug, serde::Serialize)]
struct HeuristicFields {
    strings: Vec<String>,
    /// (offset, value) pairs
    rids: Vec<(usize, i64)>,
}

/// Try to parse an entry header at the given byte position.
/// Returns the number of bytes consumed (up to end of 3rd aligned string) if valid.
fn try_parse_entry_header(data: &[u8], byte_order: ByteOrder) -> Option<usize> {
    if data.len() < 20 {
        return None;
    }
    let mut reader = BinaryReader::new(data, byte_order);

    // Read rid (i64) — any value is plausible
    let _rid = reader.read_i64().ok()?;

    // Read 3 aligned strings (class, ns, asm)
    let mut strings = Vec::with_capacity(3);
    for _ in 0..3 {
        let len = reader.read_i32().ok()?;
        if !(0..=200).contains(&len) {
            return None;
        }
        if reader.remaining() < len as usize {
            return None;
        }
        let s = reader.read_bytes(len as usize).ok()?;
        // Validate: all bytes are valid C# type name characters
        if !s.iter().all(|&b| {
            b.is_ascii_alphanumeric()
                || b == b'_'
                || b == b'.'
                || b == b'/'
                || b == b'`'
                || b == b'<'
                || b == b'>'
                || b == b','
                || b == b' '
                || b == b'['
                || b == b']'
        }) {
            return None;
        }
        strings.push(s);
        // Align to 4
        reader.align_to(4).ok()?;
    }

    let class = &strings[0];
    let ns = &strings[1];
    let asm = &strings[2];

    // Allow null entries: all 3 strings empty (managed reference with rid=-2)
    if class.is_empty() && ns.is_empty() && asm.is_empty() {
        return Some(reader.position() as usize);
    }

    // Class must be non-empty for non-null entries
    if class.is_empty() {
        return None;
    }

    // For non-null entries, at least one of namespace/assembly must contain a dot
    // (all real C# types have dotted ns or asm like "Beyond.Gameplay").
    // This rejects false positives from inline data strings like "HeadBar"/"FootBar"/"Lock".
    let has_dot = ns.contains(&b'.') || asm.contains(&b'.');
    let all_empty_ns_asm = ns.is_empty() && asm.is_empty();
    if !has_dot && !all_empty_ns_asm {
        return None;
    }

    Some(reader.position() as usize)
}

/// Parse the ManagedReferencesRegistry from raw binary bytes.
/// Supports both version 1 (no size prefix) and version 2 (with data size prefix).
fn parse_registry_manual(bytes: &[u8], byte_order: ByteOrder) -> Vec<RawRegistryEntry> {
    if bytes.len() < 8 {
        return Vec::new();
    }

    let mut reader = BinaryReader::new(bytes, byte_order);

    let version = match reader.read_i32() {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };
    let count = match reader.read_i32() {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };

    if count <= 0 || count > 100_000 {
        return Vec::new();
    }
    let count = count as usize;

    let mut entries = Vec::with_capacity(count);

    // Heuristic boundary detection for ALL versions.
    // The binary format has NO explicit data-size prefix; the data size is implicit
    // from the resolved TypeTree (which requires ref_types). Since ref_types is empty
    // in this game, we scan for entry header patterns to find boundaries.
    let header_end = reader.position() as usize;
    let mut entry_starts: Vec<usize> = Vec::with_capacity(count);
    entry_starts.push(header_end);

    if count > 1
        && let Some(hdr_size) = try_parse_entry_header(&bytes[header_end..], byte_order)
    {
        // Skip past entry[0]'s header to start scanning for entry[1]
        let scan_start = header_end + hdr_size;
        let mut pos = scan_start;
        while entry_starts.len() < count && pos + 20 < bytes.len() {
            if let Some(next_hdr) = try_parse_entry_header(&bytes[pos..], byte_order) {
                entry_starts.push(pos);
                // Skip past this entry's header to search for the next
                pos += next_hdr;
                continue;
            }
            pos += 1;
        }
    }

    if entry_starts.len() != count {
        eprintln!(
            "  [WARN] registry v{}: expected {} entries but found {} boundaries. Using found count.",
            version,
            count,
            entry_starts.len(),
        );
        // Don't truncate — use whatever boundaries we found
    }

    for (i, &start) in entry_starts.iter().enumerate() {
        let mut r = BinaryReader::new(&bytes[start..], byte_order);

        let rid = match r.read_i64() {
            Ok(v) => v,
            Err(_) => continue,
        };
        let class_name = match r.read_aligned_string() {
            Ok(s) => s,
            Err(_) => continue,
        };
        let namespace = match r.read_aligned_string() {
            Ok(s) => s,
            Err(_) => continue,
        };
        let assembly_name = match r.read_aligned_string() {
            Ok(s) => s,
            Err(_) => continue,
        };

        let data_start = start + r.position() as usize;
        let data_end = if i + 1 < entry_starts.len() {
            entry_starts[i + 1]
        } else {
            bytes.len()
        };

        let data_bytes = if data_start <= data_end {
            bytes[data_start..data_end].to_vec()
        } else {
            Vec::new()
        };

        entries.push(RawRegistryEntry {
            rid,
            class_name,
            namespace,
            assembly_name,
            data_bytes,
        });
    }

    entries
}

/// Extract heuristic fields from raw binary data.
fn extract_heuristic_fields(data: &[u8], byte_order: ByteOrder) -> HeuristicFields {
    let mut strings = Vec::new();
    let mut rids = Vec::new();

    // Scan for aligned strings: [i32 len][ascii chars][align padding]
    let mut pos = 0;
    while pos + 4 < data.len() {
        let len_bytes: [u8; 4] = match data[pos..pos + 4].try_into() {
            Ok(b) => b,
            Err(_) => {
                pos += 4;
                continue;
            }
        };
        let len = match byte_order {
            ByteOrder::Little => i32::from_le_bytes(len_bytes),
            ByteOrder::Big => i32::from_be_bytes(len_bytes),
        };

        if (1..=500).contains(&len) && pos + 4 + len as usize <= data.len() {
            let str_bytes = &data[pos + 4..pos + 4 + len as usize];
            if str_bytes.iter().all(|&b| b.is_ascii_graphic() || b == b' ')
                && let Ok(s) = std::str::from_utf8(str_bytes)
            {
                strings.push(s.to_string());
                // Skip past string + alignment
                let aligned = (pos + 4 + len as usize + 3) & !3;
                pos = aligned;
                continue;
            }
        }
        pos += 4;
    }

    // Scan for managedReference rids: i64 values that look like they could be rids
    // (non-zero, large absolute value — Unity uses hash-based rids)
    let mut pos = 0;
    while pos + 8 <= data.len() {
        let val_bytes: [u8; 8] = match data[pos..pos + 8].try_into() {
            Ok(b) => b,
            Err(_) => {
                pos += 8;
                continue;
            }
        };
        let val = match byte_order {
            ByteOrder::Little => i64::from_le_bytes(val_bytes),
            ByteOrder::Big => i64::from_be_bytes(val_bytes),
        };

        // Rids are typically large positive values or -2 (null ref)
        if val == -2 || (val > 1_000_000 && val.unsigned_abs() > 0x1_0000_0000) {
            rids.push((pos, val));
        }
        pos += 8;
    }

    HeuristicFields { strings, rids }
}

/// Pass 5: Raw registry extraction — bypasses the broken serializer for ReferencedObjectData.
/// Returns: (type_name, obj_name, registry_entries, prefix_json)
fn pass5_raw_registry_extraction(
    sf: &SerializedFile,
) -> Vec<(String, String, Vec<RawRegistryEntry>, serde_json::Value)> {
    let byte_order = sf.header.byte_order();
    let mut results = Vec::new();

    for obj in sf.objects_of_type(114) {
        // Only process objects with ManagedReferencesRegistry
        if !type_has_registry(sf, obj) {
            continue;
        }

        let type_name = get_script_type_name(sf, obj);
        let handle = ObjectHandle::new(sf, obj);
        let obj_name = handle
            .peek_name()
            .ok()
            .flatten()
            .filter(|n| !n.is_empty())
            .map_or_else(|| format!("obj_{}", obj.path_id), |n| sanitize_filename(&n));

        // Get raw object bytes
        let raw = match sf.object_bytes(obj) {
            Ok(b) => b,
            Err(_) => continue,
        };

        // Find registry child index in TypeTree
        let st = match sf
            .types
            .get(obj.type_index as usize)
            .filter(|_| obj.type_index >= 0)
        {
            Some(st) => st,
            None => continue,
        };

        let root = match st.type_tree.nodes.first() {
            Some(r) => r,
            None => continue,
        };

        let registry_idx = root
            .children
            .iter()
            .position(|c| c.type_name == "ManagedReferencesRegistry")
            .unwrap_or(root.children.len());

        // Parse MonoBehaviour prefix (everything before registry) to advance reader
        let serializer = TypeTreeSerializer::new(&st.type_tree);
        let mut reader = BinaryReader::new(raw, byte_order);
        let _prefix = match serializer.parse_object_prefix_detailed(
            &mut reader,
            TypeTreeParseOptions::default(),
            registry_idx,
        ) {
            Ok(p) => p,
            Err(e) => {
                eprintln!("  [WARN] prefix parse error for {}: {}", obj_name, e);
                continue;
            }
        };

        // Diagnostic: show TypeTree structure for first object of this type
        static SHOWN_TYPES: std::sync::LazyLock<
            std::sync::Mutex<std::collections::HashSet<String>>,
        > = std::sync::LazyLock::new(|| std::sync::Mutex::new(std::collections::HashSet::new()));
        {
            let type_name = get_script_type_name(sf, obj);
            let mut shown = SHOWN_TYPES.lock().unwrap();
            if !shown.contains(&type_name) {
                shown.insert(type_name.clone());
                eprintln!(
                    "  [TypeTree] {} — {} root children, obj_size={}, prefix_consumed={}",
                    type_name,
                    root.children.len(),
                    raw.len(),
                    reader.position()
                );
                for (i, c) in root.children.iter().enumerate() {
                    eprintln!(
                        "    [{}] {}: {} (size={}, children={})",
                        i,
                        c.name,
                        c.type_name,
                        c.byte_size,
                        c.children.len()
                    );
                }
            }
        }

        // Remaining bytes are the ManagedReferencesRegistry
        let registry_offset = reader.position() as usize;
        if registry_offset >= raw.len() {
            continue;
        }
        let registry_bytes = &raw[registry_offset..];

        // Parse registry manually
        let entries = parse_registry_manual(registry_bytes, byte_order);

        // Convert prefix properties to JSON for the output
        let prefix_json: serde_json::Value =
            serde_json::to_value(&_prefix.properties).unwrap_or_default();

        if !entries.is_empty() {
            results.push((type_name, obj_name, entries, prefix_json));
        }
    }

    results
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("=== Game Data Probe (Phase M+N) ===\n");

    let scan_all = std::env::var("SCAN_ALL").is_ok();
    let extract_resolved = std::env::var("EXTRACT_RESOLVED").is_ok();
    let raw_registry = std::env::var("RAW_REGISTRY").is_ok();
    let dump_script = std::env::var("DUMP_SCRIPT").ok();

    if scan_all {
        println!("SCAN_ALL: processing ALL .chk files");
    }
    if extract_resolved {
        println!("EXTRACT_RESOLVED: will run Pass 3+4 extraction");
    }
    if raw_registry {
        println!("RAW_REGISTRY: will run Phase N raw registry extraction");
    }
    println!();

    let client = Client::new();
    let params = ResolveParams {
        app_code: app_code::game::OS_WIN_REL,
        launcher_app_code: app_code::launcher::OS_WIN_REL,
        channel: channel::OS_WIN_REL,
        sub_channel: sub_channel::OS_WIN_REL,
        launcher_sub_channel: sub_channel::OS_WIN_REL,
        platform: Platform::Windows,
    };

    println!("[1] Resolving game config...");
    let config = resolve_game_config(&client, &params).await?;
    println!("    Version: {}", config.version);

    println!("\n[2] Parsing ZIP central directory...");
    let parts: Vec<SplitArchivePart> = config
        .packs
        .iter()
        .map(|p| SplitArchivePart {
            url: p.url.clone(),
            size: p.package_size.parse::<u64>().expect("invalid package_size"),
        })
        .collect();
    let cd = ZipCentralDirectory::from_split_archive(&client, parts).await?;
    println!("    Total ZIP entries: {}", cd.len());

    // ─── Pass 1: ref_types Census ───
    println!("\n=== Pass 1: ref_types Census ===\n");

    let mut total_sf = 0u64;
    let mut total_ref_types = 0u64;
    let mut all_ref_type_names: HashMap<String, u32> = HashMap::new();

    // Track MonoBehaviour registry info across all passes
    let mut registry_by_type: HashMap<String, (u32, u32, usize)> = HashMap::new(); // (total, with_registry, max_size)

    for (dir, desc) in VFS_DIRS {
        println!("--- {} ({}) ---", dir, desc);

        let mut candidates: Vec<(String, u64)> = cd
            .filenames()
            .filter(|f| f.ends_with(".chk") && f.contains(dir))
            .map(|f| {
                let size = cd.get(f).map_or(0, |e| e.uncompressed_size);
                (f.to_string(), size)
            })
            .collect();
        candidates.sort_by_key(|(_n, s)| *s);

        if !scan_all {
            candidates.truncate(1);
        }

        println!("  Processing {} .chk file(s)\n", candidates.len());

        for (name, size) in &candidates {
            println!("  {} ({})", name, format_bytes(*size));

            let data = match cd.extract(&client, name).await {
                Ok(d) => d,
                Err(e) => {
                    println!("    Download error: {e}");
                    continue;
                }
            };

            let chk = match extract_chk_contents(&data) {
                Some(c) => c,
                None => {
                    println!("    Not a valid VFS container");
                    continue;
                }
            };

            for sf_bytes in &chk.sf_data {
                let sf = match SerializedFileParser::from_bytes(sf_bytes.clone()) {
                    Ok(sf) => sf,
                    Err(_) => continue,
                };

                total_sf += 1;
                let (count, types) = pass1_ref_types_census(&sf);
                total_ref_types += count as u64;

                if count > 0 {
                    println!("    SF: {} ref_types", count);
                    for (class, ns, asm) in &types {
                        let key = format!("{class}|{ns}|{asm}");
                        *all_ref_type_names.entry(key.clone()).or_insert(0) += 1;
                        println!("      - {class} [{ns}] ({asm})");
                    }
                }

                // Pass 2: Registry detection
                let registry_infos = pass2_registry_detection(&sf);
                for info in registry_infos {
                    let entry = registry_by_type
                        .entry(info.type_name.clone())
                        .or_insert((0, 0, 0));
                    entry.0 += 1;
                    if info.has_registry {
                        entry.1 += 1;
                    }
                    if info.obj_size > entry.2 {
                        entry.2 = info.obj_size;
                    }
                }
            }
        }
        println!();
    }

    println!("=== Pass 1 Summary ===");
    println!("  SerializedFiles scanned: {total_sf}");
    println!("  Total ref_types entries: {total_ref_types}");
    println!("  Unique ref type keys:    {}", all_ref_type_names.len());
    if !all_ref_type_names.is_empty() {
        let mut sorted: Vec<_> = all_ref_type_names.iter().collect();
        sorted.sort_by_key(|(_, count)| std::cmp::Reverse(**count));
        println!("\n  Top ref_types:");
        for (key, count) in sorted.iter().take(20) {
            println!("    [{count}x] {key}");
        }
    }

    // ─── Pass 2: Registry Detection Summary ───
    println!("\n=== Pass 2: Registry Detection ===\n");

    let total_mono: u32 = registry_by_type.values().map(|(t, _, _)| t).sum();
    let total_with_reg: u32 = registry_by_type.values().map(|(_, r, _)| r).sum();
    println!("  Total MonoBehaviours: {total_mono}");
    println!("  With ManagedReferencesRegistry: {total_with_reg}");

    // Show types that have registries
    let mut types_with_reg: Vec<_> = registry_by_type
        .iter()
        .filter(|(_, (_, r, _))| *r > 0)
        .collect();
    types_with_reg.sort_by_key(|(_, (_, r, _))| std::cmp::Reverse(*r));

    if !types_with_reg.is_empty() {
        println!("\n  Types with registries:");
        for (type_name, (total, with_reg, max_size)) in &types_with_reg {
            let tag = if is_gamedata_type(type_name) {
                " [GAMEDATA]"
            } else {
                ""
            };
            println!(
                "    {type_name}: {with_reg}/{total} objects, max {} {tag}",
                format_bytes(*max_size as u64)
            );
        }
    }

    // ─── Pass 3+4: Extraction (if enabled) ───
    if extract_resolved {
        println!("\n=== Pass 3: Resolved Extraction ===\n");

        let output_dir = "./output/gamedata_resolved";
        std::fs::create_dir_all(output_dir)?;

        let mut total_extracted = 0u64;
        let mut total_with_registry = 0u64;
        let mut total_rids_resolved = 0u64;
        let mut type_stats: HashMap<String, (u32, bool)> = HashMap::new(); // (count, has_char_fields)

        for (dir, desc) in VFS_DIRS {
            println!("--- {} ({}) ---", dir, desc);

            let mut candidates: Vec<(String, u64)> = cd
                .filenames()
                .filter(|f| f.ends_with(".chk") && f.contains(dir))
                .map(|f| {
                    let size = cd.get(f).map_or(0, |e| e.uncompressed_size);
                    (f.to_string(), size)
                })
                .collect();
            candidates.sort_by_key(|(_n, s)| *s);

            if !scan_all {
                candidates.truncate(1);
            }

            for (name, size) in &candidates {
                println!("  {} ({})", name, format_bytes(*size));

                let data = match cd.extract(&client, name).await {
                    Ok(d) => d,
                    Err(e) => {
                        println!("    Download error: {e}");
                        continue;
                    }
                };

                let chk = match extract_chk_contents(&data) {
                    Some(c) => c,
                    None => continue,
                };

                for sf_bytes in &chk.sf_data {
                    let sf = match SerializedFileParser::from_bytes(sf_bytes.clone()) {
                        Ok(sf) => sf,
                        Err(_) => continue,
                    };

                    let exports = pass3_extract_resolved(&sf);
                    for export in exports {
                        let type_dir = format!("{output_dir}/{}", export.type_name);
                        let _ = std::fs::create_dir_all(&type_dir);

                        let path = format!("{type_dir}/{}.json", export.obj_name);
                        if std::fs::write(&path, &export.json).is_ok() {
                            total_extracted += 1;
                            if export.had_registry {
                                total_with_registry += 1;
                                total_rids_resolved += export.rid_count as u64;
                            }

                            // Check for character fields
                            let json_val: serde_json::Value =
                                serde_json::from_str(&export.json).unwrap_or_default();
                            let has_char = has_character_fields(&json_val);

                            let entry = type_stats
                                .entry(export.type_name.clone())
                                .or_insert((0, false));
                            entry.0 += 1;
                            if has_char {
                                entry.1 = true;
                            }

                            let tag = if export.had_registry {
                                format!(" [REG, {} rids]", export.rid_count)
                            } else {
                                String::new()
                            };
                            println!(
                                "    [OK] {}/{} ({}){tag}",
                                export.type_name,
                                export.obj_name,
                                format_bytes(export.json.len() as u64)
                            );
                        }
                    }
                }
            }
            println!();
        }

        // ─── Pass 4: Data Discovery Summary ───
        println!("=== Pass 4: Data Discovery Summary ===\n");
        println!("  Total game-data objects extracted: {total_extracted}");
        println!("  Objects with resolved registries:  {total_with_registry}");
        println!("  Total rid references resolved:     {total_rids_resolved}");
        println!("  Output directory: {output_dir}");

        if !type_stats.is_empty() {
            let mut sorted: Vec<_> = type_stats.iter().collect();
            sorted.sort_by_key(|(_, (count, _))| std::cmp::Reverse(*count));

            println!("\n  Extracted types:");
            for (type_name, (count, has_char)) in &sorted {
                let tag = if *has_char {
                    " *** CHARACTER DATA ***"
                } else {
                    ""
                };
                println!("    {type_name}: {count} objects{tag}");
            }
        }
    }

    // ─── Optional: TypeTree + Raw Bytes Dump ───
    if let Some(ref target_hash) = dump_script {
        println!(
            "\n=== TypeTree + Raw Dump for script containing '{}' ===\n",
            target_hash
        );

        let mut found = false;
        for (dir, desc) in VFS_DIRS {
            let mut candidates: Vec<(String, u64)> = cd
                .filenames()
                .filter(|f| f.ends_with(".chk") && f.contains(dir))
                .map(|f| {
                    let size = cd.get(f).map_or(0, |e| e.uncompressed_size);
                    (f.to_string(), size)
                })
                .collect();
            candidates.sort_by_key(|(_n, s)| *s);

            if !scan_all {
                candidates.truncate(1);
            }

            for (name, _size) in &candidates {
                let data = match cd.extract(&client, name).await {
                    Ok(d) => d,
                    Err(_) => continue,
                };

                let chk = match extract_chk_contents(&data) {
                    Some(c) => c,
                    None => continue,
                };

                for sf_bytes in &chk.sf_data {
                    let sf = match SerializedFileParser::from_bytes(sf_bytes.clone()) {
                        Ok(sf) => sf,
                        Err(_) => continue,
                    };

                    // Check if any object matches
                    let has_match = sf
                        .objects_of_type(114)
                        .iter()
                        .any(|obj| get_script_type_name(&sf, obj).contains(target_hash.as_str()));

                    if has_match {
                        println!("Found in {} ({}):", name, desc);
                        dump_script_type(&sf, target_hash);
                        found = true;
                    }
                }

                if found {
                    break;
                }
            }
            if found {
                break;
            }
        }

        if !found {
            println!("  No objects found for script hash '{}'", target_hash);
            println!("  (Try SCAN_ALL=1 to search all .chk files)");
        }
    }

    // ─── Phase N: Raw Registry Extraction ───
    if raw_registry {
        println!("\n=== Phase N: Raw Registry Extraction ===\n");

        let output_dir = "./output/gamedata_raw";
        std::fs::create_dir_all(output_dir)?;

        let mut total_objects = 0u64;
        let mut total_entries = 0u64;
        let mut total_data_bytes = 0u64;
        let mut type_class_counts: HashMap<String, u32> = HashMap::new();

        for (dir, desc) in VFS_DIRS {
            println!("--- {} ({}) ---", dir, desc);

            let mut candidates: Vec<(String, u64)> = cd
                .filenames()
                .filter(|f| f.ends_with(".chk") && f.contains(dir))
                .map(|f| {
                    let size = cd.get(f).map_or(0, |e| e.uncompressed_size);
                    (f.to_string(), size)
                })
                .collect();
            candidates.sort_by_key(|(_n, s)| *s);

            if !scan_all {
                candidates.truncate(1);
            }

            for (name, size) in &candidates {
                println!("  {} ({})", name, format_bytes(*size));

                let data = match cd.extract(&client, name).await {
                    Ok(d) => d,
                    Err(e) => {
                        println!("    Download error: {e}");
                        continue;
                    }
                };

                let chk = match extract_chk_contents(&data) {
                    Some(c) => c,
                    None => continue,
                };

                for sf_bytes in &chk.sf_data {
                    let sf = match SerializedFileParser::from_bytes(sf_bytes.clone()) {
                        Ok(sf) => sf,
                        Err(_) => continue,
                    };

                    let byte_order = sf.header.byte_order();
                    let raw_results = pass5_raw_registry_extraction(&sf);

                    for (type_name, obj_name, entries, prefix_json) in &raw_results {
                        total_objects += 1;
                        total_entries += entries.len() as u64;

                        let type_dir = format!("{output_dir}/{}", sanitize_filename(type_name));
                        let _ = std::fs::create_dir_all(&type_dir);

                        // Build JSON output for this object
                        let mut obj_json = serde_json::json!({
                            "_meta": {
                                "object_name": obj_name,
                                "type_name": type_name,
                                "entry_count": entries.len(),
                            },
                            "prefix": prefix_json,
                            "entries": [],
                        });

                        let entries_arr = obj_json["entries"].as_array_mut().unwrap();

                        for entry in entries {
                            let data_size = entry.data_bytes.len();
                            total_data_bytes += data_size as u64;

                            *type_class_counts
                                .entry(entry.class_name.clone())
                                .or_insert(0) += 1;

                            let heuristic = extract_heuristic_fields(&entry.data_bytes, byte_order);

                            // Full data as hex
                            let hex_preview: String = entry
                                .data_bytes
                                .iter()
                                .map(|b| format!("{b:02x}"))
                                .collect::<Vec<_>>()
                                .chunks(2)
                                .map(|pair| pair.join(""))
                                .collect::<Vec<_>>()
                                .join(" ");

                            entries_arr.push(serde_json::json!({
                                "rid": entry.rid,
                                "type": {
                                    "class": entry.class_name,
                                    "ns": entry.namespace,
                                    "asm": entry.assembly_name,
                                },
                                "data_size": data_size,
                                "data_hex_preview": hex_preview,
                                "heuristic_strings": heuristic.strings,
                                "heuristic_rids": heuristic.rids.iter()
                                    .map(|(off, val)| serde_json::json!({"offset": off, "rid": val}))
                                    .collect::<Vec<_>>(),
                            }));
                        }

                        let path = format!("{type_dir}/{}.json", sanitize_filename(obj_name));
                        if let Ok(pretty) = serde_json::to_string_pretty(&obj_json) {
                            let _ = std::fs::write(&path, &pretty);
                        }

                        // Summary line
                        let entry_types: Vec<&str> =
                            entries.iter().map(|e| e.class_name.as_str()).collect();
                        let unique_types: std::collections::HashSet<&str> =
                            entry_types.iter().copied().collect();
                        let types_summary = if unique_types.len() <= 3 {
                            unique_types.into_iter().collect::<Vec<_>>().join(", ")
                        } else {
                            format!("{} types", unique_types.len())
                        };

                        let total_data: usize = entries.iter().map(|e| e.data_bytes.len()).sum();
                        println!(
                            "    [OK] {}/{} — {} entries ({}), types: {}",
                            type_name,
                            obj_name,
                            entries.len(),
                            format_bytes(total_data as u64),
                            types_summary,
                        );
                    }
                }
            }
            println!();
        }

        println!("=== Phase N Summary ===");
        println!("  Objects with raw registry: {total_objects}");
        println!("  Total registry entries:    {total_entries}");
        println!(
            "  Total data bytes:          {}",
            format_bytes(total_data_bytes)
        );
        println!("  Output directory: {output_dir}");

        if !type_class_counts.is_empty() {
            let mut sorted: Vec<_> = type_class_counts.iter().collect();
            sorted.sort_by_key(|(_, count)| std::cmp::Reverse(**count));
            println!("\n  Referenced type classes:");
            for (class, count) in sorted.iter().take(30) {
                println!("    [{count}x] {class}");
            }
        }
    }

    println!("\n=== Done ===");
    Ok(())
}
