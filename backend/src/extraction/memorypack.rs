//! MemoryPack decoding for the `JsonData` BLC group.
//!
//! Most of the group's `.json` files are binary MemoryPack payloads. Their layouts come from
//! a schema compiled from the game's IL2CPP metadata (`backend/scripts/memorypack-schema/`),
//! and every string is an index into a global pool stored under `Json/StringPool/`.
//!
//! A file carries no type tag, so each candidate root type is tried in turn; a decode only
//! counts when it consumes every byte. Decoded files keep their original bytes beside them
//! as `<stem>.mpk`, which later runs decode from so a schema update can re-decode in place.
use super::utils::write_file;
use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// BLC group holding the MemoryPack files.
pub const JSON_DATA_GROUP: &str = "JsonData";

/// Compiled schema for the game version the decoder was built against.
const EMBEDDED_SCHEMA: &[u8] = include_bytes!("../../schema/memorypack/1.6.4.json.gz");

/// Schema format revision this decoder understands.
const SCHEMA_FORMAT: u64 = 1;

/// Nesting limit; the schema is a graph, so hostile data could otherwise recurse forever.
const MAX_DEPTH: usize = 128;

/// `USPC`, the magic of every string pool file.
const POOL_MAGIC: i32 = 0x4350_5355;

/// String pool files are probed as `StringPool0.byte`, `StringPool1.byte`, ... up to this many.
const MAX_POOL_FILES: usize = 64;

fn invalid(msg: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg.into())
}

fn read_i32(data: &[u8], pos: usize) -> Option<i32> {
    let bytes = data.get(pos..pos.checked_add(4)?)?;
    Some(i32::from_le_bytes(bytes.try_into().ok()?))
}

fn read_len(data: &[u8], pos: usize, what: &str) -> io::Result<usize> {
    let value =
        read_i32(data, pos).ok_or_else(|| invalid(format!("string pool: {what} truncated")))?;
    usize::try_from(value).map_err(|_| invalid(format!("string pool: negative {what} {value}")))
}

// ── String pool ─────────────────────────────────────────

/// The global string table that MemoryPack string fields index into.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct StringPool {
    strings: Vec<String>,
}

impl StringPool {
    /// A pool holding `strings` in index order.
    pub fn new(strings: Vec<String>) -> Self {
        Self { strings }
    }

    /// Number of strings in the pool.
    pub fn len(&self) -> usize {
        self.strings.len()
    }

    /// Whether the pool holds no strings.
    pub fn is_empty(&self) -> bool {
        self.strings.is_empty()
    }

    /// String at global index `index`.
    pub fn get(&self, index: usize) -> Option<&str> {
        self.strings.get(index).map(String::as_str)
    }

    /// Parse the contents of `StringPool0.byte`, `StringPool1.byte`, ... in file order.
    ///
    /// The first file starts with a 4-byte file count, which is ignored: the files present
    /// are what counts.
    pub fn from_files<B: AsRef<[u8]>>(files: &[B]) -> io::Result<Self> {
        let mut strings = Vec::new();
        for (index, file) in files.iter().enumerate() {
            let block = pool_block(file.as_ref(), index == 0)?;
            pool_strings(&block, &mut strings)?;
        }
        Ok(Self { strings })
    }

    /// Load every `StringPool<k>.byte` in `dir`, stopping at the first missing index.
    pub fn load(dir: &Path) -> io::Result<Self> {
        let mut files = Vec::new();
        for k in 0..MAX_POOL_FILES {
            match fs::read(dir.join(format!("StringPool{k}.byte"))) {
                Ok(bytes) => files.push(bytes),
                Err(error) if error.kind() == io::ErrorKind::NotFound => break,
                Err(error) => return Err(error),
            }
        }
        Self::from_files(&files)
    }
}

/// Decompressed string block of one pool file.
///
/// The header is six LE `i32`: magic, version, flags (bit 0 = LZ4 block), string count, raw
/// length, stored length.
fn pool_block(file: &[u8], first: bool) -> io::Result<Cow<'_, [u8]>> {
    let start = if first { 4 } else { 0 };
    let header: Vec<i32> = (0..6)
        .map(|i| read_i32(file, start + i * 4))
        .collect::<Option<_>>()
        .ok_or_else(|| invalid("string pool: header truncated"))?;
    if header[0] != POOL_MAGIC {
        return Err(invalid(format!("string pool: bad magic {:#x}", header[0])));
    }
    let flags = header[2];
    let raw_len =
        usize::try_from(header[4]).map_err(|_| invalid("string pool: negative raw length"))?;
    let stored =
        usize::try_from(header[5]).map_err(|_| invalid("string pool: negative stored length"))?;
    let body = start + 24;
    let data = file
        .get(body..body + stored)
        .ok_or_else(|| invalid("string pool: data truncated"))?;
    if flags & 1 == 0 {
        return Ok(Cow::Borrowed(data));
    }
    let raw = lz4_flex::block::decompress(data, raw_len)
        .map_err(|e| invalid(format!("string pool: LZ4: {e}")))?;
    if raw.len() != raw_len {
        return Err(invalid(format!(
            "string pool: decompressed {} bytes, expected {raw_len}",
            raw.len()
        )));
    }
    Ok(Cow::Owned(raw))
}

/// Append a block's strings: `i32 n`, `n` offsets, then at `4 + 4n + offset` an `i32` byte
/// length and the UTF-8 bytes.
fn pool_strings(block: &[u8], out: &mut Vec<String>) -> io::Result<()> {
    let count = read_len(block, 0, "string count")?;
    let base = count
        .checked_mul(4)
        .and_then(|n| n.checked_add(4))
        .filter(|&base| base <= block.len())
        .ok_or_else(|| invalid("string pool: offset table truncated"))?;
    out.reserve(count);
    for k in 0..count {
        let offset = read_len(block, 4 + k * 4, "string offset")?;
        let at = base
            .checked_add(offset)
            .ok_or_else(|| invalid("string pool: offset overflow"))?;
        let len = read_len(block, at, "string length")?;
        let bytes = block
            .get(at + 4..at + 4 + len)
            .ok_or_else(|| invalid("string pool: string out of bounds"))?;
        let text = std::str::from_utf8(bytes).map_err(|_| invalid("string pool: invalid UTF-8"))?;
        out.push(text.to_owned());
    }
    Ok(())
}

// ── Schema ──────────────────────────────────────────────

#[derive(Debug, Clone, Copy)]
enum Prim {
    Bool,
    U8,
    I8,
    I16,
    U16,
    I32,
    U32,
    I64,
    U64,
    F32,
    F64,
}

impl Prim {
    fn parse(name: &str) -> Option<Self> {
        Some(match name {
            "bool" => Self::Bool,
            "u8" => Self::U8,
            "i8" => Self::I8,
            "i16" => Self::I16,
            "u16" => Self::U16,
            "i32" => Self::I32,
            "u32" => Self::U32,
            "i64" => Self::I64,
            "u64" => Self::U64,
            "f32" => Self::F32,
            "f64" => Self::F64,
            _ => return None,
        })
    }
}

/// One compiled type layout; child types are node indexes.
#[derive(Debug)]
enum Node {
    Prim(Prim),
    /// `i32` pool index; `-1` is null and `-2` the empty string.
    Str,
    List(usize),
    /// `hdr`: a leading `255` byte means null.
    Dict {
        hdr: bool,
        key: usize,
        val: usize,
    },
    /// Unmanaged struct: fields at fixed offsets within `size` bytes.
    Raw {
        size: usize,
        fields: Vec<(usize, String, usize)>,
    },
    /// `bool` has-value flag, then the value at `off` from the flag (decoded either way).
    Nullable {
        off: usize,
        value: usize,
    },
    Ref(usize),
    /// Member-count header byte, then members in order.
    Obj(Vec<(String, usize)>),
    /// Tag byte (`250` = a wide `u16` tag follows) selecting a case.
    Union(HashMap<u16, (String, usize)>),
    Curve,
    Gradient,
    RectOffset,
    Unsupported,
}

/// A compiled MemoryPack schema (`{format, game_version, nodes, roots, dir_roots}`).
#[derive(Debug)]
pub struct Schema {
    /// Game version the schema was compiled from, when recorded.
    game_version: Option<String>,
    nodes: Vec<Node>,
    roots: HashMap<String, usize>,
    dir_roots: HashMap<String, Vec<String>>,
    /// Object roots by member count, each list sorted by type name.
    obj_roots: HashMap<usize, Vec<String>>,
    /// Union roots sorted by type name.
    union_roots: Vec<String>,
}

fn index_of(value: &serde_json::Value) -> Option<usize> {
    value.as_u64().and_then(|v| usize::try_from(v).ok())
}

fn parse_node(node: &serde_json::Value) -> Option<Node> {
    let field = |name: &str| node.get(name).and_then(index_of);
    Some(match node.get("k")?.as_str()? {
        "p" => match Prim::parse(node.get("t")?.as_str()?) {
            Some(prim) => Node::Prim(prim),
            None => Node::Unsupported,
        },
        "str" => Node::Str,
        "list" => Node::List(field("e")?),
        "dict" => Node::Dict {
            hdr: node.get("hdr")?.as_bool()?,
            key: field("key")?,
            val: field("val")?,
        },
        "raw" => Node::Raw {
            size: field("size")?,
            fields: node
                .get("f")?
                .as_array()?
                .iter()
                .map(|f| {
                    Some((
                        index_of(f.get(0)?)?,
                        f.get(1)?.as_str()?.to_owned(),
                        index_of(f.get(2)?)?,
                    ))
                })
                .collect::<Option<_>>()?,
        },
        "nullable" => Node::Nullable {
            off: field("off")?,
            value: field("v")?,
        },
        "ref" => Node::Ref(field("to")?),
        "obj" => Node::Obj(
            node.get("m")?
                .as_array()?
                .iter()
                .map(|m| Some((m.get(0)?.as_str()?.to_owned(), index_of(m.get(1)?)?)))
                .collect::<Option<_>>()?,
        ),
        "union" => Node::Union(
            node.get("c")?
                .as_object()?
                .iter()
                .filter_map(|(tag, case)| {
                    // A tag that is not a u16 can never be read, so it is dropped.
                    let tag = tag.parse::<u16>().ok()?;
                    Some((tag, case))
                })
                .map(|(tag, case)| {
                    Some((
                        tag,
                        (case.get(0)?.as_str()?.to_owned(), index_of(case.get(1)?)?),
                    ))
                })
                .collect::<Option<_>>()?,
        ),
        "curve" => Node::Curve,
        "gradient" => Node::Gradient,
        "rect_offset" => Node::RectOffset,
        _ => Node::Unsupported,
    })
}

impl Schema {
    /// Parse a compiled schema from its (uncompressed) JSON.
    pub fn from_json(json: &[u8]) -> Result<Self, String> {
        let value: serde_json::Value =
            serde_json::from_slice(json).map_err(|e| format!("schema JSON: {e}"))?;
        let format = value.get("format").and_then(serde_json::Value::as_u64);
        if format != Some(SCHEMA_FORMAT) {
            return Err(format!("unsupported schema format {format:?}"));
        }
        let nodes = value
            .get("nodes")
            .and_then(serde_json::Value::as_array)
            .ok_or("schema has no nodes")?
            .iter()
            .enumerate()
            .map(|(i, node)| {
                parse_node(node).ok_or_else(|| format!("schema node {i} is malformed"))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let roots: HashMap<String, usize> = value
            .get("roots")
            .and_then(serde_json::Value::as_object)
            .ok_or("schema has no roots")?
            .iter()
            .map(|(name, index)| {
                index_of(index)
                    .filter(|&i| i < nodes.len())
                    .map(|i| (name.clone(), i))
                    .ok_or_else(|| format!("schema root {name} is malformed"))
            })
            .collect::<Result<_, _>>()?;
        let dir_roots = value
            .get("dir_roots")
            .and_then(serde_json::Value::as_object)
            .ok_or("schema has no dir_roots")?
            .iter()
            .map(|(dir, names)| {
                names
                    .as_array()
                    .and_then(|names| {
                        names
                            .iter()
                            .map(|n| n.as_str().map(str::to_owned))
                            .collect::<Option<Vec<_>>>()
                    })
                    .map(|names| (dir.clone(), names))
                    .ok_or_else(|| format!("schema dir_roots {dir} is malformed"))
            })
            .collect::<Result<_, _>>()?;

        let mut obj_roots: HashMap<usize, Vec<String>> = HashMap::new();
        let mut union_roots = Vec::new();
        for (name, &index) in &roots {
            match &nodes[index] {
                Node::Obj(members) => obj_roots
                    .entry(members.len())
                    .or_default()
                    .push(name.clone()),
                Node::Union(_) => union_roots.push(name.clone()),
                _ => {}
            }
        }
        // Byte-wise `String` order is code point order, which is how the reference sorts.
        obj_roots.values_mut().for_each(|names| names.sort());
        union_roots.sort();
        let game_version = value
            .get("game_version")
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned);
        Ok(Self {
            game_version,
            nodes,
            roots,
            dir_roots,
            obj_roots,
            union_roots,
        })
    }

    /// Parse a gzip-compressed compiled schema.
    pub fn from_gzip(bytes: &[u8]) -> Result<Self, String> {
        let mut json = Vec::new();
        flate2::read::GzDecoder::new(bytes)
            .read_to_end(&mut json)
            .map_err(|e| format!("schema gzip: {e}"))?;
        Self::from_json(&json)
    }

    /// The schema bundled with this build, parsed on first use.
    pub fn embedded() -> io::Result<&'static Schema> {
        static SCHEMA: OnceLock<Result<Schema, String>> = OnceLock::new();
        SCHEMA
            .get_or_init(|| Schema::from_gzip(EMBEDDED_SCHEMA))
            .as_ref()
            .map_err(|e| invalid(format!("embedded MemoryPack schema: {e}")))
    }

    /// Game version the schema was compiled from (`"1.6.4"`), if it records one.
    pub fn game_version(&self) -> Option<&str> {
        self.game_version.as_deref()
    }

    /// Root types to try for a file in `rel_dir` whose first byte is `first`, in order: the
    /// directory's known roots, then objects with `first` members, then unions.
    fn candidates<'s>(&'s self, rel_dir: &str, first: u8) -> Vec<&'s str> {
        let known: Vec<&str> = self
            .dir_roots
            .get(rel_dir)
            .map(|names| names.iter().map(String::as_str).collect())
            .unwrap_or_default();
        let seen: HashSet<&str> = known.iter().copied().collect();
        let objs = self
            .obj_roots
            .get(&usize::from(first))
            .into_iter()
            .flatten();
        let rest = objs
            .chain(&self.union_roots)
            .map(String::as_str)
            .filter(|name| !seen.contains(name));
        known.iter().copied().chain(rest).collect()
    }
}

// ── Decoded values ──────────────────────────────────────

/// A decoded value. Objects keep their members in order (schema order, `$type` first),
/// which `serde_json::Value` would not without the crate-wide `preserve_order` feature.
#[derive(Debug, Clone, PartialEq)]
enum Value<'a> {
    Null,
    Bool(bool),
    Int(i64),
    UInt(u64),
    Float(f64),
    Str(&'a str),
    List(Vec<Value<'a>>),
    Object(Vec<(Cow<'a, str>, Value<'a>)>),
}

/// Set `key` like a Python dict assignment: a repeated key keeps its first position.
fn insert<'a>(members: &mut Vec<(Cow<'a, str>, Value<'a>)>, key: Cow<'a, str>, value: Value<'a>) {
    match members.iter_mut().find(|(k, _)| *k == key) {
        Some(slot) => slot.1 = value,
        None => members.push((key, value)),
    }
}

/// The shortest decimal that rounds back to the same `f32` (so `0.1f32` becomes `0.1`),
/// widened to `f64`; at equal length the correctly rounded digits win.
fn shortest_f32(v: f32) -> f64 {
    if !v.is_finite() {
        return f64::from(v);
    }
    for digits in 0..9 {
        let Ok(candidate) = format!("{:.digits$e}", f64::from(v)).parse::<f64>() else {
            continue;
        };
        if (candidate as f32).to_bits() == v.to_bits() {
            return candidate;
        }
    }
    f64::from(v)
}

/// Serialize like `serde_json::to_vec_pretty`: two-space indent, `"key": value`.
fn write_pretty(value: &Value, indent: usize, out: &mut Vec<u8>) {
    fn newline(out: &mut Vec<u8>, indent: usize) {
        out.push(b'\n');
        out.resize(out.len() + indent * 2, b' ');
    }
    // Writing scalars into a Vec cannot fail.
    match value {
        Value::Null => out.extend_from_slice(b"null"),
        Value::Bool(b) => out.extend_from_slice(if *b { b"true" } else { b"false" }),
        Value::Int(n) => out.extend_from_slice(n.to_string().as_bytes()),
        Value::UInt(n) => out.extend_from_slice(n.to_string().as_bytes()),
        Value::Float(f) => serde_json::to_writer(&mut *out, f).unwrap_or_default(),
        Value::Str(s) => serde_json::to_writer(&mut *out, s).unwrap_or_default(),
        Value::List(items) if items.is_empty() => out.extend_from_slice(b"[]"),
        Value::Object(members) if members.is_empty() => out.extend_from_slice(b"{}"),
        Value::List(items) => {
            out.push(b'[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(b',');
                }
                newline(out, indent + 1);
                write_pretty(item, indent + 1, out);
            }
            newline(out, indent);
            out.push(b']');
        }
        Value::Object(members) => {
            out.push(b'{');
            for (i, (key, member)) in members.iter().enumerate() {
                if i > 0 {
                    out.push(b',');
                }
                newline(out, indent + 1);
                serde_json::to_writer(&mut *out, key.as_ref()).unwrap_or_default();
                out.extend_from_slice(b": ");
                write_pretty(member, indent + 1, out);
            }
            newline(out, indent);
            out.push(b'}');
        }
    }
}

// ── Decoder ─────────────────────────────────────────────

/// The data does not fit the candidate type.
struct Fail;

type Decoded<'a> = Result<Value<'a>, Fail>;

struct Decoder<'a> {
    schema: &'a Schema,
    pool: &'a StringPool,
    data: &'a [u8],
    pos: usize,
}

impl<'a> Decoder<'a> {
    fn take<const N: usize>(&mut self) -> Result<[u8; N], Fail> {
        let end = self.pos.checked_add(N).ok_or(Fail)?;
        let bytes = self.data.get(self.pos..end).ok_or(Fail)?;
        self.pos = end;
        bytes.try_into().map_err(|_| Fail)
    }

    fn u8(&mut self) -> Result<u8, Fail> {
        Ok(self.take::<1>()?[0])
    }

    fn i32(&mut self) -> Result<i32, Fail> {
        self.take().map(i32::from_le_bytes)
    }

    /// Collection length; `None` for a null collection (`-1`).
    fn count(&mut self) -> Result<Option<usize>, Fail> {
        let count = self.i32()?;
        if count == -1 {
            return Ok(None);
        }
        // Every element takes at least a byte in practice, so more than remain is garbage.
        let count = usize::try_from(count).map_err(|_| Fail)?;
        if count > self.data.len() - self.pos {
            return Err(Fail);
        }
        Ok(Some(count))
    }

    fn prim(&mut self, prim: Prim) -> Decoded<'a> {
        // Non-finite floats have no JSON form.
        let float = |f: f64| {
            if f.is_finite() {
                Value::Float(f)
            } else {
                Value::Null
            }
        };
        Ok(match prim {
            Prim::Bool => Value::Bool(self.u8()? != 0),
            Prim::U8 => Value::Int(self.u8()?.into()),
            Prim::I8 => Value::Int(i8::from_le_bytes(self.take()?).into()),
            Prim::I16 => Value::Int(i16::from_le_bytes(self.take()?).into()),
            Prim::U16 => Value::Int(u16::from_le_bytes(self.take()?).into()),
            Prim::I32 => Value::Int(self.i32()?.into()),
            Prim::U32 => Value::Int(u32::from_le_bytes(self.take()?).into()),
            Prim::I64 => Value::Int(i64::from_le_bytes(self.take()?)),
            Prim::U64 => Value::UInt(u64::from_le_bytes(self.take()?)),
            Prim::F32 => float(shortest_f32(f32::from_le_bytes(self.take()?))),
            Prim::F64 => float(f64::from_le_bytes(self.take()?)),
        })
    }

    /// A fixed record of named primitives.
    fn record(&mut self, fields: &[(&'static str, Prim)]) -> Decoded<'a> {
        let mut members = Vec::with_capacity(fields.len());
        for &(name, prim) in fields {
            members.push((Cow::Borrowed(name), self.prim(prim)?));
        }
        Ok(Value::Object(members))
    }

    /// `count` records, or null.
    fn records(&mut self, fields: &[(&'static str, Prim)]) -> Decoded<'a> {
        let Some(count) = self.count()? else {
            return Ok(Value::Null);
        };
        (0..count)
            .map(|_| self.record(fields))
            .collect::<Result<_, _>>()
            .map(Value::List)
    }

    fn value(&mut self, index: usize, depth: usize) -> Decoded<'a> {
        if depth > MAX_DEPTH {
            return Err(Fail);
        }
        let schema = self.schema;
        match schema.nodes.get(index).ok_or(Fail)? {
            Node::Prim(prim) => self.prim(*prim),
            Node::Str => match self.i32()? {
                -1 => Ok(Value::Null),
                -2 => Ok(Value::Str("")),
                index => {
                    let index = usize::try_from(index).map_err(|_| Fail)?;
                    self.pool.get(index).map(Value::Str).ok_or(Fail)
                }
            },
            Node::List(element) => {
                let Some(count) = self.count()? else {
                    return Ok(Value::Null);
                };
                (0..count)
                    .map(|_| self.value(*element, depth + 1))
                    .collect::<Result<_, _>>()
                    .map(Value::List)
            }
            Node::Dict { hdr, key, val } => {
                if *hdr && self.u8()? == 255 {
                    return Ok(Value::Null);
                }
                let Some(count) = self.count()? else {
                    return Ok(Value::Null);
                };
                let mut pairs = Vec::with_capacity(count);
                for _ in 0..count {
                    let k = self.value(*key, depth + 1)?;
                    let v = self.value(*val, depth + 1)?;
                    pairs.push((k, v));
                }
                Ok(dict_value(pairs))
            }
            Node::Raw { size, fields } => {
                let start = self.pos;
                let end = start.checked_add(*size).ok_or(Fail)?;
                if end > self.data.len() {
                    return Err(Fail);
                }
                let mut members = Vec::with_capacity(fields.len());
                for (offset, name, field) in fields {
                    self.pos = start.checked_add(*offset).ok_or(Fail)?;
                    let value = self.value(*field, depth + 1)?;
                    insert(&mut members, Cow::Borrowed(name.as_str()), value);
                }
                self.pos = end;
                Ok(Value::Object(members))
            }
            Node::Nullable { off, value } => {
                let start = self.pos;
                let has = self.u8()? != 0;
                self.pos = start.checked_add(*off).ok_or(Fail)?;
                let value = self.value(*value, depth + 1)?;
                Ok(if has { value } else { Value::Null })
            }
            Node::Ref(to) => self.value(*to, depth + 1),
            Node::Obj(members) => {
                let header = self.u8()?;
                if header == 255 {
                    return Ok(Value::Null);
                }
                if usize::from(header) != members.len() {
                    return Err(Fail);
                }
                let mut out = Vec::with_capacity(members.len());
                for (name, member) in members {
                    let value = self.value(*member, depth + 1)?;
                    insert(&mut out, Cow::Borrowed(name.as_str()), value);
                }
                Ok(Value::Object(out))
            }
            Node::Union(cases) => {
                let tag = match self.u8()? {
                    255 => return Ok(Value::Null),
                    250 => u16::from_le_bytes(self.take()?),
                    tag => tag.into(),
                };
                let (type_name, case) = cases.get(&tag).ok_or(Fail)?;
                Ok(match self.value(*case, depth + 1)? {
                    Value::Object(members) => {
                        let mut out = Vec::with_capacity(members.len() + 1);
                        out.push((Cow::Borrowed("$type"), Value::Str(type_name.as_str())));
                        for (key, value) in members {
                            // A member named `$type` overrides the tag, as `{**val}` would.
                            if key == "$type" {
                                out[0].1 = value;
                            } else {
                                out.push((key, value));
                            }
                        }
                        Value::Object(out)
                    }
                    other => other,
                })
            }
            Node::Curve => {
                if self.u8()? == 255 {
                    return Ok(Value::Null);
                }
                let post = Value::Int(self.i32()?.into());
                let pre = Value::Int(self.i32()?.into());
                let keys = self.records(&[
                    ("time", Prim::F32),
                    ("value", Prim::F32),
                    ("inTangent", Prim::F32),
                    ("outTangent", Prim::F32),
                    ("weightedMode", Prim::I32),
                    ("inWeight", Prim::F32),
                    ("outWeight", Prim::F32),
                ])?;
                Ok(Value::Object(vec![
                    (Cow::Borrowed("postWrapMode"), post),
                    (Cow::Borrowed("preWrapMode"), pre),
                    (Cow::Borrowed("keys"), keys),
                ]))
            }
            Node::Gradient => {
                if self.u8()? == 255 {
                    return Ok(Value::Null);
                }
                let colors = self.records(&[
                    ("r", Prim::F32),
                    ("g", Prim::F32),
                    ("b", Prim::F32),
                    ("a", Prim::F32),
                    ("time", Prim::F32),
                ])?;
                let alphas = self.records(&[("alpha", Prim::F32), ("time", Prim::F32)])?;
                let mode = self.prim(Prim::I32)?;
                Ok(Value::Object(vec![
                    (Cow::Borrowed("colorKeys"), colors),
                    (Cow::Borrowed("alphaKeys"), alphas),
                    (Cow::Borrowed("mode"), mode),
                ]))
            }
            Node::RectOffset => {
                if self.u8()? == 255 {
                    return Ok(Value::Null);
                }
                self.record(&[
                    ("left", Prim::I32),
                    ("right", Prim::I32),
                    ("top", Prim::I32),
                    ("bottom", Prim::I32),
                ])
            }
            Node::Unsupported => Err(Fail),
        }
    }
}

/// A dictionary becomes a JSON object when every key is a string or (non-bool) integer and
/// the keys stay unique as text; otherwise a list of `[key, value]` pairs.
fn dict_value<'a>(pairs: Vec<(Value<'a>, Value<'a>)>) -> Value<'a> {
    let keys: Option<Vec<Cow<'a, str>>> = pairs
        .iter()
        .map(|(key, _)| match key {
            Value::Str(s) => Some(Cow::Borrowed(*s)),
            Value::Int(n) => Some(Cow::Owned(n.to_string())),
            Value::UInt(n) => Some(Cow::Owned(n.to_string())),
            _ => None,
        })
        .collect();
    if let Some(keys) = keys {
        let unique: HashSet<&str> = keys.iter().map(AsRef::as_ref).collect();
        if unique.len() == keys.len() {
            let members = keys.into_iter().zip(pairs.into_iter().map(|(_, v)| v));
            return Value::Object(members.collect());
        }
    }
    Value::List(
        pairs
            .into_iter()
            .map(|(k, v)| Value::List(vec![k, v]))
            .collect(),
    )
}

/// Decode one MemoryPack file to pretty-printed JSON, or `None` when no candidate root type
/// consumes it exactly.
///
/// `rel_dir` is the file's directory relative to `Json/`, `/`-separated, `.` for `Json/`
/// itself; it selects the directory's known root types.
pub fn decode_bytes(
    schema: &Schema,
    pool: &StringPool,
    data: &[u8],
    rel_dir: &str,
) -> Option<Vec<u8>> {
    let first = *data.first()?;
    for root in schema.candidates(rel_dir, first) {
        let Some(&index) = schema.roots.get(root) else {
            continue;
        };
        let mut decoder = Decoder {
            schema,
            pool,
            data,
            pos: 0,
        };
        let Ok(value) = decoder.value(index, 0) else {
            continue;
        };
        if decoder.pos == data.len() {
            let mut out = Vec::with_capacity(data.len() * 4);
            write_pretty(&value, 0, &mut out);
            return Some(out);
        }
    }
    None
}

// ── Tree conversion ─────────────────────────────────────

/// Totals from [`decode_tree`].
#[derive(Debug, Default, PartialEq, Eq)]
pub struct TreeDecodeResult {
    /// Files written (or already current) as decoded JSON.
    pub decoded: u64,
    /// MemoryPack files no root type decodes; left untouched.
    pub failed: u64,
    pub write_errors: u64,
}

/// `.json` files under `root`, each paired with its directory relative to `root`
/// (`/`-separated, `.` for `root` itself).
pub fn json_files(root: &Path) -> io::Result<Vec<(PathBuf, String)>> {
    let mut files = Vec::new();
    let mut pending = vec![(root.to_path_buf(), String::from("."))];
    while let Some((dir, rel)) = pending.pop() {
        for entry in fs::read_dir(&dir)? {
            let entry = entry?;
            let path = entry.path();
            if entry.file_type()?.is_dir() {
                let name = entry.file_name().to_string_lossy().into_owned();
                let child = if rel == "." {
                    name
                } else {
                    format!("{rel}/{name}")
                };
                pending.push((path, child));
            } else if path.extension().is_some_and(|ext| ext == "json") {
                files.push((path, rel.clone()));
            }
        }
    }
    Ok(files)
}

/// Decode, in place, every MemoryPack `.json` under `group_dir/Json`.
///
/// Each decoded `Foo.json` first has its original bytes saved as `Foo.mpk`, and an existing
/// `Foo.mpk` is decoded instead of `Foo.json`, so rerunning (e.g. after a schema update)
/// re-decodes from the source and is otherwise a no-op. Plain JSON and files that fail to
/// decode are left alone. A group without `Json/` or a string pool yields zero counts.
pub fn decode_tree(group_dir: &Path) -> io::Result<TreeDecodeResult> {
    if !has_string_pool(group_dir) {
        return Ok(TreeDecodeResult::default());
    }
    decode_tree_with(Schema::embedded()?, group_dir)
}

fn has_string_pool(group_dir: &Path) -> bool {
    let json_root = group_dir.join("Json");
    json_root.is_dir() && json_root.join("StringPool/StringPool0.byte").is_file()
}

/// [`decode_tree`] with an explicit schema.
pub fn decode_tree_with(schema: &Schema, group_dir: &Path) -> io::Result<TreeDecodeResult> {
    if !has_string_pool(group_dir) {
        return Ok(TreeDecodeResult::default());
    }
    let json_root = group_dir.join("Json");
    let pool = StringPool::load(&json_root.join("StringPool"))?;
    let files = json_files(&json_root)?;

    // Files are independent, so each worker decodes a slice and the totals are summed.
    let workers = std::thread::available_parallelism().map_or(4, usize::from);
    let chunk = files.len().div_ceil(workers).max(1);
    let pool = &pool;
    std::thread::scope(|scope| {
        let handles: Vec<_> = files
            .chunks(chunk)
            .map(|batch| {
                scope.spawn(move || -> io::Result<TreeDecodeResult> {
                    let mut totals = TreeDecodeResult::default();
                    for (path, rel_dir) in batch {
                        match decode_file(schema, pool, path, rel_dir)? {
                            FileOutcome::Skipped => {}
                            FileOutcome::Decoded => totals.decoded += 1,
                            FileOutcome::Failed => totals.failed += 1,
                            FileOutcome::WriteError => totals.write_errors += 1,
                        }
                    }
                    Ok(totals)
                })
            })
            .collect();
        let mut totals = TreeDecodeResult::default();
        for handle in handles {
            let part = handle
                .join()
                .map_err(|_| io::Error::other("MemoryPack decode worker panicked"))??;
            totals.decoded += part.decoded;
            totals.failed += part.failed;
            totals.write_errors += part.write_errors;
        }
        Ok(totals)
    })
}

enum FileOutcome {
    /// Not MemoryPack (plain JSON or empty).
    Skipped,
    Decoded,
    Failed,
    WriteError,
}

fn decode_file(
    schema: &Schema,
    pool: &StringPool,
    path: &Path,
    rel_dir: &str,
) -> io::Result<FileOutcome> {
    let mpk = path.with_extension("mpk");
    let (data, has_mpk) = match fs::read(&mpk) {
        Ok(data) => (data, true),
        Err(error) if error.kind() == io::ErrorKind::NotFound => (fs::read(path)?, false),
        Err(error) => return Err(error),
    };
    if data.first().is_none_or(|&b| b == b'{') {
        return Ok(FileOutcome::Skipped);
    }
    let Some(json) = decode_bytes(schema, pool, &data, rel_dir) else {
        return Ok(FileOutcome::Failed);
    };
    // Save the source before replacing it, so a crash in between loses nothing.
    if !has_mpk && !write_file(&mpk, &data) {
        return Ok(FileOutcome::WriteError);
    }
    let current = if has_mpk { fs::read(path).ok() } else { None };
    if current.as_deref() != Some(json.as_slice()) && !write_file(path, &json) {
        return Ok(FileOutcome::WriteError);
    }
    Ok(FileOutcome::Decoded)
}
