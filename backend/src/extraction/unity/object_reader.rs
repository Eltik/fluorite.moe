// Adapted from Myrtle (assets/unpacker/src/unity).
use crate::extraction::unity::endian_reader::EndianReader;
use crate::extraction::unity::serialized_file::SerializedType;
use crate::extraction::unity::type_tree::TypeTreeNode;
use base64::Engine;
use serde_json::{Map, Value, json};
use std::io;

/// State carried through one object's read.
pub struct ReadContext<'a> {
    /// The file's `m_RefTypes`, used to resolve `[SerializeReference]` payloads.
    ref_types: &'a [SerializedType],
    /// Set once the object's `ManagedReferencesRegistry` has been read.
    has_registry: bool,
    /// Keep `pair` arrays as `[first, second]` arrays instead of a JSON object, so multimaps
    /// with duplicate keys (an AssetBundle's `m_Container`) lose no entries.
    pair_arrays: bool,
}

impl<'a> ReadContext<'a> {
    #[must_use]
    pub const fn new(ref_types: &'a [SerializedType], pair_arrays: bool) -> Self {
        Self {
            ref_types,
            has_registry: false,
            pair_arrays,
        }
    }
}

fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}

pub fn read_value(
    r: &mut EndianReader,
    node: &TypeTreeNode,
    ctx: &mut ReadContext,
) -> Result<Value, io::Error> {
    let val = match node.type_name.as_str() {
        "bool" => Value::Bool(r.read_bool()?),
        "SInt8" | "char" => Value::Number(r.read_i8()?.into()),
        "UInt8" => Value::Number(r.read_u8()?.into()),
        "SInt16" | "short" => Value::Number(r.read_i16()?.into()),
        "UInt16" | "unsigned short" => Value::Number(r.read_u16()?.into()),
        "SInt32" | "int" => Value::Number(r.read_i32()?.into()),
        "UInt32" | "unsigned int" | "Type*" => Value::Number(r.read_u32()?.into()),
        "SInt64" | "long long" => Value::Number(r.read_i64()?.into()),
        "UInt64" | "unsigned long long" | "FileSize" => {
            let v = r.read_u64()?;
            if v > i64::MAX as u64 {
                Value::String(v.to_string())
            } else {
                Value::Number((v as i64).into())
            }
        }
        "float" => json!(r.read_f32()?),
        "double" => json!(r.read_f64()?),
        "string" => {
            let len = r.read_i32()?;
            if len < 0 {
                return Err(invalid(format!("negative string length: {len}")));
            }
            let bytes = r.read_bytes(len as usize)?;
            r.align(4);
            // If valid UTF-8, store as string; otherwise store as base64
            match String::from_utf8(bytes.clone()) {
                Ok(s) => Value::String(s),
                Err(_) => Value::String(format!(
                    "base64:{}",
                    base64::engine::general_purpose::STANDARD.encode(&bytes)
                )),
            }
        }
        "TypelessData" => {
            let size = r.read_i32()?;
            if size < 0 {
                return Err(invalid(format!("negative TypelessData size: {size}")));
            }
            let bytes = r.read_bytes(size as usize)?;
            r.align(4);
            Value::String(base64::engine::general_purpose::STANDARD.encode(&bytes))
        }
        // One `[SerializeReference]` entry of a `ManagedReferencesRegistry`.
        "ReferencedObject" => read_referenced_object(r, node, ctx)?,
        _ if !node.children.is_empty() && node.children[0].type_name == "Array" => {
            // This is a container with an Array child (e.g., vector, set, etc.)
            read_array(r, &node.children[0], ctx)?
        }
        _ => read_struct(r, node, ctx)?,
    };

    // Post-read alignment
    if node.meta_flag & 0x4000 != 0 {
        r.align(4);
    }

    Ok(val)
}

/// Struct: read each child into a JSON object
pub fn read_struct(
    r: &mut EndianReader,
    node: &TypeTreeNode,
    ctx: &mut ReadContext,
) -> Result<Value, io::Error> {
    let mut map = Map::new();
    for child in &node.children {
        if let Some(value) = read_field(r, child, ctx)? {
            map.insert(child.name.clone(), value);
        }
    }
    Ok(Value::Object(map))
}

/// Read one struct field. The object's `ManagedReferencesRegistry` is read with each payload
/// resolved through `m_RefTypes`; any later registry — the copies embedded in referenced
/// types' own trees — holds no data and yields `None` without reading, as UnityPy does.
pub fn read_field(
    r: &mut EndianReader,
    node: &TypeTreeNode,
    ctx: &mut ReadContext,
) -> Result<Option<Value>, io::Error> {
    if node.type_name == "ManagedReferencesRegistry" {
        if ctx.has_registry {
            return Ok(None);
        }
        ctx.has_registry = true;
    }
    read_value(r, node, ctx).map(Some)
}

/// `rid` and `type` (class/namespace/assembly) are read first; `data` is then read with the
/// matching `m_RefTypes` tree, or with the placeholder node when no ref type matches (a null
/// reference has an empty class and no payload).
fn read_referenced_object(
    r: &mut EndianReader,
    node: &TypeTreeNode,
    ctx: &mut ReadContext,
) -> Result<Value, io::Error> {
    let mut map = Map::new();
    for child in &node.children {
        let value = if child.type_name == "ReferencedObjectData" {
            let ref_types = ctx.ref_types;
            match resolve_ref_type(&map, ref_types) {
                Some(tree) => {
                    let value = read_struct(r, tree, ctx)?;
                    if child.meta_flag & 0x4000 != 0 {
                        r.align(4);
                    }
                    value
                }
                None => read_value(r, child, ctx)?,
            }
        } else {
            read_value(r, child, ctx)?
        };
        map.insert(child.name.clone(), value);
    }
    Ok(Value::Object(map))
}

fn resolve_ref_type<'a>(
    referenced_object: &Map<String, Value>,
    ref_types: &'a [SerializedType],
) -> Option<&'a TypeTreeNode> {
    let managed_type = referenced_object.get("type")?.as_object()?;
    let field = |keys: [&str; 2]| {
        keys.iter()
            .find_map(|key| managed_type.get(*key)?.as_str())
            .unwrap_or("")
    };
    let class = field(["class", "m_ClassName"]);
    if class.is_empty() {
        return None;
    }
    let namespace = field(["ns", "m_NameSpace"]);
    let assembly = field(["asm", "m_AssemblyName"]);
    ref_types
        .iter()
        .filter(|t| {
            t.class_name == class && t.namespace == namespace && t.assembly_name == assembly
        })
        .find_map(|t| t.type_tree.as_ref())
}

fn read_array(
    r: &mut EndianReader,
    array_node: &TypeTreeNode,
    ctx: &mut ReadContext,
) -> Result<Value, io::Error> {
    // array_node.children[0] = "size" (int), children[1] = element type
    let size = r.read_i32()?;
    if size < 0 {
        return Err(invalid(format!("negative array size: {size}")));
    }
    let size = size as usize;
    let element_node = array_node
        .children
        .get(1)
        .ok_or_else(|| invalid("array node has no element type"))?;

    // Bound the element count against the bytes left in this object. Every array
    // element reads at least one byte, so a well-formed array of `size` elements
    // needs at least `size` bytes remaining. If the declared count exceeds what's
    // left, the type-tree was misread and `size` is garbage — reject it now.
    //
    // Without this, a bogus count (hundreds of millions) over an element type
    // that happens to read zero bytes — e.g. an empty struct node, `read_value`'s
    // struct fallback above — drives the loop to push that many `serde_json::Value`
    // nodes with no EOF to stop it, allocating ~10 GB and OOM-killing the process.
    // The self-limiting "fails on EOF if data is short" assumption below only holds
    // when each element actually consumes input; this guard covers when it doesn't.
    if size > r.remaining() {
        return Err(invalid(format!(
            "array size {size} exceeds {} remaining bytes (misread type-tree)",
            r.remaining()
        )));
    }

    // Cap initial capacity to avoid huge allocations when size is valid but large;
    // the loop will still read exactly `size` elements (failing on EOF if data is short).
    const MAX_INITIAL_CAP: usize = 1024;

    // Check if this is a map (element type is "pair")
    let result = if element_node.type_name == "pair" {
        let [first, second] = element_node.children.as_slice() else {
            return Err(invalid("pair node does not have two children"));
        };
        if ctx.pair_arrays {
            let mut arr = Vec::with_capacity(size.min(MAX_INITIAL_CAP));
            for _ in 0..size {
                let key = read_value(r, first, ctx)?;
                let val = read_value(r, second, ctx)?;
                arr.push(Value::Array(vec![key, val]));
            }
            Value::Array(arr)
        } else {
            let mut map = Map::with_capacity(size.min(MAX_INITIAL_CAP));
            for _ in 0..size {
                let key = read_value(r, first, ctx)?;
                let val = read_value(r, second, ctx)?;
                let key_str = match key {
                    Value::String(s) => s,
                    other => other.to_string(),
                };
                map.insert(key_str, val);
            }
            Value::Object(map)
        }
    } else {
        let mut arr = Vec::with_capacity(size.min(MAX_INITIAL_CAP));
        for _ in 0..size {
            arr.push(read_value(r, element_node, ctx)?);
        }
        Value::Array(arr)
    };

    // Unity's `kAlignBytes` (0x4000) flag for an array lives on the inner `Array`
    // node, not on the outer container (vector/set/...). It means "align to 4
    // after the whole array". Applying it here is required for arrays of packed
    // primitives whose byte length isn't a multiple of 4 (e.g. an odd-length
    // `m_IndexBuffer`); omitting it leaves the reader misaligned and corrupts
    // every subsequent field. align(4) on an already-aligned position is a no-op,
    // so this never over-shifts arrays that were already aligned.
    if array_node.meta_flag & 0x4000 != 0 {
        r.align(4);
    }

    Ok(result)
}
