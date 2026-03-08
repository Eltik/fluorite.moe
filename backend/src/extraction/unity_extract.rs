/// Unity asset extraction helpers: ChkContents, TextAsset parsing, class names.
use crate::extraction::{unityfs_header, vfs_decrypt};
use std::collections::HashMap;
use unity_asset_binary::asset::{ObjectInfo, SerializedFile};

/// All extracted contents from a .chk file: SerializedFile data + shared resource streams.
pub struct ChkContents {
    /// Individual SerializedFile blobs.
    pub sf_data: Vec<Vec<u8>>,
    /// Resource streams keyed by entry path (e.g., "CAB-<hash>.resS").
    pub resources: HashMap<String, Vec<u8>>,
}

/// Extract all SerializedFile + resource stream data from a .chk file.
///
/// Resource streams (.resS) can come from two places:
/// 1. As directory entries INSIDE a UnityFS bundle (alongside the SerializedFile)
/// 2. As separate VFS files alongside the UnityFS bundle (NOT wrapped in UnityFS)
pub fn extract_chk_contents(data: &[u8]) -> Option<ChkContents> {
    if !vfs_decrypt::is_valid_header(data) {
        return None;
    }
    let assets = match unityfs_header::process_chk_file(data) {
        Ok(a) => a,
        Err(_) => return None,
    };

    let mut resources: HashMap<String, Vec<u8>> = HashMap::new();
    let mut sf_data: Vec<Vec<u8>> = Vec::new();

    for asset in assets {
        if asset.is_unityfs {
            for entry in &asset.bundle_entries {
                let start = entry.offset as usize;
                let end = (start + entry.size as usize).min(asset.data.len());
                if start >= asset.data.len() {
                    continue;
                }
                if entry.path.ends_with(".resS") || entry.path.ends_with(".resource") {
                    resources.insert(entry.path.clone(), asset.data[start..end].to_vec());
                } else {
                    sf_data.push(asset.data[start..end].to_vec());
                }
            }
        } else if asset.vfs_name.ends_with(".resS") || asset.vfs_name.ends_with(".resource") {
            resources.insert(asset.vfs_name, asset.data);
        } else {
            sf_data.push(asset.data);
        }
    }

    Some(ChkContents { sf_data, resources })
}

/// Parse a TextAsset from raw object bytes (little-endian).
///
/// Binary layout:
/// ```text
/// u32  m_Name length
/// [N]  m_Name bytes (UTF-8)
///      align to 4 bytes
/// u32  m_Script length
/// [N]  m_Script bytes
/// ```
pub fn parse_text_asset(bytes: &[u8]) -> Option<(String, Vec<u8>)> {
    if bytes.len() < 8 {
        return None;
    }

    let name_len = u32::from_le_bytes(bytes[0..4].try_into().ok()?) as usize;
    if name_len > 10_000 || 4 + name_len > bytes.len() {
        return None;
    }
    let name = String::from_utf8_lossy(&bytes[4..4 + name_len]).to_string();

    let aligned = (4 + name_len + 3) & !3;
    if aligned + 4 > bytes.len() {
        return None;
    }

    let script_len = u32::from_le_bytes(bytes[aligned..aligned + 4].try_into().ok()?) as usize;
    let script_start = aligned + 4;
    if script_len > 100_000_000 || script_start + script_len > bytes.len() {
        return None;
    }

    let script = bytes[script_start..script_start + script_len].to_vec();
    Some((name, script))
}

/// Get Unity class name from class ID.
pub fn class_name(id: i32) -> String {
    unity_asset_core::get_class_name(id).unwrap_or_else(|| format!("Class_{}", id))
}

/// Get the script type name for a MonoBehaviour object from TypeTree metadata.
pub fn get_script_type_name(sf: &SerializedFile, info: &ObjectInfo) -> String {
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
