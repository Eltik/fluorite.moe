//! Object access on a parsed `SerializedFile`.
use crate::extraction::unity::endian_reader::{EndianReader, invalid_data};
use crate::extraction::unity::object_reader::{ReadContext, read_field, read_struct};
use crate::extraction::unity::serialized_file::{ObjectInfo, SerializedFile};
use crate::extraction::unity::type_tree::TypeTreeNode;
use base64::Engine;
use serde_json::{Map, Value};
use std::io;

impl SerializedFile {
    /// Objects of one Unity class ID, in file order.
    pub fn objects_of_type(&self, class_id: i32) -> impl Iterator<Item = &ObjectInfo> {
        self.objects
            .iter()
            .filter(move |obj| obj.class_id == class_id)
    }

    /// The object's serialized bytes.
    pub fn object_bytes(&self, obj: &ObjectInfo) -> io::Result<&[u8]> {
        let start = usize::try_from(obj.byte_start).ok();
        start
            .and_then(|start| {
                self.data
                    .get(start..start.checked_add(obj.byte_size as usize)?)
            })
            .ok_or_else(|| invalid_data(format!("object {} data out of bounds", obj.path_id)))
    }

    /// Deserialize an object through its type tree. Any read error fails the whole object.
    pub fn read_object(&self, obj: &ObjectInfo) -> io::Result<Value> {
        self.read_object_with(obj, false)
    }

    /// Like `read_object`, but `pair` arrays stay arrays of `[first, second]` so multimaps
    /// with duplicate keys (an AssetBundle's `m_Container`) keep every entry.
    pub fn read_object_keep_pairs(&self, obj: &ObjectInfo) -> io::Result<Value> {
        self.read_object_with(obj, true)
    }

    fn read_object_with(&self, obj: &ObjectInfo, pair_arrays: bool) -> io::Result<Value> {
        let tree = self
            .type_tree(obj)
            .ok_or_else(|| invalid_data("no type tree"))?;
        let mut r = EndianReader::new(self.object_bytes(obj)?, self.big_endian);
        let mut ctx = ReadContext::new(&self.ref_types, pair_arrays);
        read_struct(&mut r, tree, &mut ctx)
    }

    /// Read only the root fields up to `m_Name` (or `name`) and return it.
    ///
    /// `Ok(None)` when the object has no type tree or no name field.
    pub fn peek_name(&self, obj: &ObjectInfo) -> io::Result<Option<String>> {
        let Some(mut fields) = self.read_prefix(obj, &["m_Name", "name"])? else {
            return Ok(None);
        };
        let name = fields.remove("m_Name").or_else(|| fields.remove("name"));
        Ok(match name {
            Some(Value::String(name)) => Some(name),
            _ => None,
        })
    }

    /// Read the root fields in order, stopping after the first one named in `last`.
    ///
    /// `Ok(None)` when the object has no type tree or no such field.
    pub fn read_prefix(
        &self,
        obj: &ObjectInfo,
        last: &[&str],
    ) -> io::Result<Option<Map<String, Value>>> {
        let Some(tree) = self.type_tree(obj) else {
            return Ok(None);
        };
        if !tree
            .children
            .iter()
            .any(|c| last.contains(&c.name.as_str()))
        {
            return Ok(None);
        }
        let mut r = EndianReader::new(self.object_bytes(obj)?, self.big_endian);
        let mut ctx = ReadContext::new(&self.ref_types, false);
        let mut fields = Map::new();
        for child in &tree.children {
            if let Some(value) = read_field(&mut r, child, &mut ctx)? {
                fields.insert(child.name.clone(), value);
            }
            if last.contains(&child.name.as_str()) {
                break;
            }
        }
        Ok(Some(fields))
    }

    fn type_tree(&self, obj: &ObjectInfo) -> Option<&TypeTreeNode> {
        if !self.enable_type_tree {
            return None;
        }
        self.types.get(obj.type_index)?.type_tree.as_ref()
    }
}

/// Bytes of a `TypelessData` (base64 string) or `vector<UInt8>` (array of numbers) value.
#[must_use]
pub fn value_bytes(value: &Value) -> Option<Vec<u8>> {
    match value {
        Value::String(s) => base64::engine::general_purpose::STANDARD.decode(s).ok(),
        Value::Array(items) => items
            .iter()
            .map(|v| v.as_u64().and_then(|n| u8::try_from(n).ok()))
            .collect(),
        _ => None,
    }
}
