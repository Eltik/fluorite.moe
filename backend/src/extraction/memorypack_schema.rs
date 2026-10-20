//! Generate the compiled MemoryPack schema that [`super::memorypack`] decodes with, straight
//! from a client's `GameAssembly.dll` and `global-metadata.dat`.
//!
//! This replaces the offline pipeline in `backend/scripts/memorypack-schema/` (a patched
//! Il2CppDumper, then Python over its `dump.cs`). The rules are the same, but every type
//! reference is the exact `Il2CppType` from the binary instead of a C# name resolved by
//! heuristics:
//!
//! - Wrappers are the classes in `Beyond.MemoryPack` named `*ForMemoryPack` that implement
//!   `IMemoryPackDeSerializeWrapper<T>`; `T` is the type they serialize, possibly a closed
//!   generic. Every non-generic `T` is a root.
//! - A wrapper's members are its setter-only properties named `__x__`, in declaration order,
//!   after the base wrapper's members. `[MemoryPackAllowSerialize]` on a member lets a
//!   blittable value be copied raw even when its type has a wrapper.
//! - `[MemoryPackUnion(tag, typeof(W))]` on a wrapper makes it a union of the wrappers `W`.
//! - A member's wire format follows [`Compiler::compile_value`], in the order the Python
//!   compiler (`compile.py::_value`) used, so the decoded JSON keeps its shape.
//! - Blittable structs are laid out at their IL2CPP field offsets within their IL2CPP
//!   instance size; closed generic structs (which have no recorded layout) use natural
//!   alignment.
//!
//! A file's root type is not recorded in the data. [`learn_dir_roots`] decodes a real
//! `JsonData/Json` tree and records, per directory, the root types that decoded its files.

use super::il2cpp::registration::{TypeRef, kind};
use super::il2cpp::{Il2Cpp, TypeKind};
use super::memorypack::{self, Schema, StringPool};
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::io::{self, Write};
use std::path::Path;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

/// Schema format revision written (the one [`memorypack::Schema`] reads).
pub const SCHEMA_FORMAT: u64 = 1;

const WRAPPER_NAMESPACE: &str = "Beyond.MemoryPack";
const WRAPPER_SUFFIX: &str = "ForMemoryPack";
const WRAPPER_INTERFACE: &str = "IMemoryPackDeSerializeWrapper`1";
const MEMORYPACK_NAMESPACE: &str = "MemoryPack";
const UNION_ATTRIBUTE: &str = "MemoryPackUnionAttribute";
const ALLOW_SERIALIZE_ATTRIBUTE: &str = "MemoryPackAllowSerializeAttribute";

/// Generic collections serialized as `i32` count + elements (matched by top-level name).
const LISTS: [&str; 12] = [
    "List",
    "HashSet",
    "Queue",
    "Stack",
    "LinkedList",
    "IList",
    "ICollection",
    "IEnumerable",
    "IReadOnlyList",
    "IReadOnlyCollection",
    "SortedSet",
    "ISet",
];
/// Beyond's dictionaries whose formatter writes an object header first.
const HEADED_DICTS: [&str; 3] = [
    "SerializeFieldDictionary",
    "SerializeFieldDictionaryPaired",
    "SerializeFieldDictionarySorted",
];
const DICTS: [&str; 5] = [
    "Dictionary",
    "SortedDictionary",
    "IDictionary",
    "IReadOnlyDictionary",
    "ConcurrentDictionary",
];

/// Undecoded files listed by name in a [`LearnReport`] (all are counted).
const LEARN_REPORTED_FAILURES: usize = 200;

// ── Nodes ───────────────────────────────────────────────

/// One compiled type layout; child types are node indexes (see `memorypack.rs`).
#[derive(Debug, Clone)]
enum Node {
    /// Placeholder while a recursive type is being compiled.
    Pending,
    Prim(&'static str),
    Str,
    List(usize),
    Dict {
        hdr: bool,
        key: usize,
        val: usize,
    },
    Raw {
        size: u32,
        fields: Vec<(u32, String, usize)>,
    },
    Nullable {
        off: u32,
        value: usize,
    },
    Ref(usize),
    Obj(Vec<(String, usize)>),
    Union(BTreeMap<u16, (String, usize)>),
    Curve,
    Gradient,
    RectOffset,
    Unsupported(String),
}

impl Node {
    fn kind(&self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Prim(_) => "p",
            Self::Str => "str",
            Self::List(_) => "list",
            Self::Dict { .. } => "dict",
            Self::Raw { .. } => "raw",
            Self::Nullable { .. } => "nullable",
            Self::Ref(_) => "ref",
            Self::Obj(_) => "obj",
            Self::Union(_) => "union",
            Self::Curve => "curve",
            Self::Gradient => "gradient",
            Self::RectOffset => "rect_offset",
            Self::Unsupported(_) => "unsupported",
        }
    }

    fn to_json(&self) -> Value {
        let k = self.kind();
        match self {
            Self::Prim(t) => json!({"k": k, "t": t}),
            Self::List(e) => json!({"k": k, "e": e}),
            Self::Dict { hdr, key, val } => json!({"k": k, "hdr": hdr, "key": key, "val": val}),
            Self::Raw { size, fields } => json!({
                "k": k,
                "size": size,
                "f": fields.iter().map(|(off, name, n)| json!([off, name, n])).collect::<Vec<_>>(),
            }),
            Self::Nullable { off, value } => json!({"k": k, "off": off, "v": value}),
            Self::Ref(to) => json!({"k": k, "to": to}),
            Self::Obj(members) => json!({
                "k": k,
                "m": members.iter().map(|(name, n)| json!([name, n])).collect::<Vec<_>>(),
            }),
            Self::Union(cases) => {
                let c: serde_json::Map<String, Value> = cases
                    .iter()
                    .map(|(tag, (name, n))| (tag.to_string(), json!([name, n])))
                    .collect();
                json!({"k": k, "c": c})
            }
            Self::Unsupported(why) => json!({"k": k, "why": why}),
            Self::Pending | Self::Str | Self::Curve | Self::Gradient | Self::RectOffset => {
                json!({"k": k})
            }
        }
    }
}

/// The `memorypack.rs` primitive name of a built-in element type.
fn prim_name(k: u8) -> Option<&'static str> {
    Some(match k {
        kind::BOOLEAN => "bool",
        kind::U1 => "u8",
        kind::I1 => "i8",
        kind::I2 => "i16",
        kind::U2 | kind::CHAR => "u16",
        kind::I4 => "i32",
        kind::U4 => "u32",
        kind::I8 => "i64",
        kind::U8 => "u64",
        kind::R4 => "f32",
        kind::R8 => "f64",
        _ => return None,
    })
}

/// Size (= alignment) of a blittable built-in element type.
fn prim_size(k: u8) -> Option<u32> {
    Some(match k {
        kind::BOOLEAN | kind::U1 | kind::I1 => 1,
        kind::I2 | kind::U2 | kind::CHAR => 2,
        kind::I4 | kind::U4 | kind::R4 => 4,
        kind::I8 | kind::U8 | kind::R8 => 8,
        _ => return None,
    })
}

/// A blittable layout: size, alignment and `(offset, name, type)` fields.
#[derive(Debug, Clone)]
struct Layout {
    size: u32,
    align: u32,
    fields: Vec<(u32, String, TypeRef)>,
}

/// One wrapper member: name without the `__` decoration, type, `[MemoryPackAllowSerialize]`.
type Member = (String, TypeRef, bool);

// ── Compiler ────────────────────────────────────────────

struct Compiler<'il, 'a> {
    il: &'il Il2Cpp<'a>,
    /// Original (non-generic) type -> its wrapper.
    wrapper_of: HashMap<u32, u32>,
    /// Closed generic type -> its wrapper.
    generic_wrappers: HashMap<TypeRef, u32>,
    /// Wrapper -> the type it serializes.
    orig_of: HashMap<u32, TypeRef>,
    nodes: Vec<Node>,
    values: HashMap<(TypeRef, bool), usize>,
    objects: HashMap<u32, usize>,
    members: HashMap<u32, Vec<Member>>,
    unmanaged: HashMap<u32, Option<Layout>>,
    /// Structs whose layout is being computed (a self-containing struct is not blittable).
    in_progress: HashSet<u32>,
    notes: Vec<String>,
}

impl<'il, 'a> Compiler<'il, 'a> {
    fn new(il: &'il Il2Cpp<'a>) -> Self {
        let mut c = Self {
            il,
            wrapper_of: HashMap::new(),
            generic_wrappers: HashMap::new(),
            orig_of: HashMap::new(),
            nodes: Vec::new(),
            values: HashMap::new(),
            objects: HashMap::new(),
            members: HashMap::new(),
            unmanaged: HashMap::new(),
            in_progress: HashSet::new(),
            notes: Vec::new(),
        };
        c.find_wrappers();
        c.note_custom_formatters();
        c
    }

    /// Hand-written `MemoryPackFormatter<T>` classes for non-wrapper types (e.g.
    /// `SendLuaEvent1.SendLuaEvent1Formatter`): their wire format is code, not metadata, so
    /// whatever the schema says for `T` is a guess.
    fn note_custom_formatters(&mut self) {
        let il = self.il;
        for def in 0..il.typedef_count() as u32 {
            let Some(TypeRef::Generic(base, args)) = il.parent(def) else {
                continue;
            };
            if il.name(base) != "MemoryPackFormatter`1"
                || il.namespace(base) != MEMORYPACK_NAMESPACE
            {
                continue;
            }
            if let [TypeRef::Def(target) | TypeRef::Generic(target, _)] = args.as_slice()
                && !self.orig_of.contains_key(target)
                && il.namespace(*target) != MEMORYPACK_NAMESPACE
                && il.namespace(def) != MEMORYPACK_NAMESPACE
                && !il.namespace(def).starts_with("MemoryPack.")
                // Formats the compiler implements by hand.
                && !HEADED_DICTS.contains(&il.short_name(*target))
                && il.short_name(*target) != "StringPathHash"
            {
                self.notes.push(format!(
                    "{} has a hand-written formatter ({}); its layout is not derivable",
                    il.full_name(*target),
                    il.full_name(def)
                ));
            }
        }
    }

    fn is_wrapper_class(&self, def: u32) -> bool {
        self.il.kind(def) == TypeKind::Class
            && self.il.declaring(def).is_none()
            && self.il.name(def).ends_with(WRAPPER_SUFFIX)
            && self.il.namespace(def) == WRAPPER_NAMESPACE
    }

    fn find_wrappers(&mut self) {
        let il = self.il;
        for def in 0..il.typedef_count() as u32 {
            if !self.is_wrapper_class(def) {
                continue;
            }
            let interfaces = match il.interfaces(def) {
                Ok(interfaces) => interfaces,
                Err(e) => {
                    self.notes.push(format!("{}: {e}", il.full_name(def)));
                    continue;
                }
            };
            let orig = interfaces.into_iter().find_map(|i| match i {
                TypeRef::Generic(d, mut args)
                    if il.name(d) == WRAPPER_INTERFACE
                        && il.namespace(d) == WRAPPER_NAMESPACE
                        && args.len() == 1 =>
                {
                    args.pop()
                }
                _ => None,
            });
            match orig {
                Some(TypeRef::Def(o)) => {
                    if let Some(previous) = self.wrapper_of.insert(o, def) {
                        self.notes.push(format!(
                            "{} has two wrappers, {} and {}",
                            il.full_name(o),
                            il.full_name(previous),
                            il.full_name(def)
                        ));
                    }
                    self.orig_of.insert(def, TypeRef::Def(o));
                }
                Some(generic @ TypeRef::Generic(..)) => {
                    self.generic_wrappers.insert(generic.clone(), def);
                    self.orig_of.insert(def, generic);
                }
                other => self.notes.push(format!(
                    "{} implements no usable {WRAPPER_INTERFACE} ({other:?})",
                    il.full_name(def)
                )),
            }
        }
    }

    fn push(&mut self, node: Node) -> usize {
        self.nodes.push(node);
        self.nodes.len() - 1
    }

    /// Node of a value of type `t`; `raw_ok` lets blittable values be copied raw.
    fn value(&mut self, t: &TypeRef, raw_ok: bool) -> usize {
        let key = (t.clone(), raw_ok);
        if let Some(&slot) = self.values.get(&key) {
            return slot;
        }
        let slot = self.push(Node::Pending);
        self.values.insert(key, slot);
        self.nodes[slot] = self.compile_value(t, raw_ok);
        slot
    }

    fn raw(&mut self, layout: Layout) -> Node {
        let fields = layout
            .fields
            .iter()
            .map(|(off, name, ty)| (*off, name.clone(), self.value(ty, true)))
            .collect();
        Node::Raw {
            size: layout.size,
            fields,
        }
    }

    /// Short name of a top-level generic definition (`List` for ``System.Collections.Generic.List`1``).
    fn top_level_generic(&self, def: u32) -> Option<&'a str> {
        self.il
            .declaring(def)
            .is_none()
            .then(|| self.il.short_name(def))
    }

    /// The wire format of a value of type `t`, in `compile.py::_value`'s dispatch order.
    fn compile_value(&mut self, t: &TypeRef, raw_ok: bool) -> Node {
        let il = self.il;
        match t {
            // `T[]` of a blittable `T` goes through MemoryPack's raw array formatter.
            TypeRef::SzArray(element) => Node::List(self.value(element, true)),
            TypeRef::Prim(kind::STRING) => Node::Str,
            TypeRef::Prim(k) => match prim_name(*k) {
                Some(name) => Node::Prim(name),
                None => Node::Unsupported(format!("unknown type {}", il.csharp_name(t))),
            },
            // StringPathHashFormatter writes only the 64-bit path hash.
            TypeRef::Def(d) if il.short_name(*d) == "StringPathHash" => Node::Prim("i64"),
            TypeRef::Generic(d, args) => self.compile_generic(t, *d, args, raw_ok),
            TypeRef::Def(d) => self.compile_def(*d, raw_ok),
            _ => Node::Unsupported(format!("unknown type {}", il.csharp_name(t))),
        }
    }

    fn compile_generic(&mut self, t: &TypeRef, def: u32, args: &[TypeRef], raw_ok: bool) -> Node {
        let il = self.il;
        let base = self.top_level_generic(def);
        match (base, args) {
            (Some(b), [element]) if LISTS.contains(&b) => {
                return Node::List(self.value(element, false));
            }
            (Some(b), [key, val]) if HEADED_DICTS.contains(&b) => {
                // Beyond's own formatter: header byte, then key and value through their
                // formatters, even for unmanaged pairs.
                return Node::Dict {
                    hdr: true,
                    key: self.value(key, false),
                    val: self.value(val, false),
                };
            }
            (Some(b), [key, val]) if DICTS.contains(&b) => {
                // MemoryPack's KeyValuePairFormatter copies an unmanaged pair raw, so blittable
                // keys and values are raw even when their type has a wrapper.
                let raw = match self.natural_layout(&[key, val]) {
                    Some((offsets, size))
                        if offsets[1] == offsets[0] + self.size_of(key)
                            && size == offsets[1] + self.size_of(val) =>
                    {
                        true
                    }
                    Some(_) => {
                        // The pair has padding, which format 1 cannot express: read the key and
                        // value back to back, which only fits empty dictionaries.
                        self.notes.push(format!(
                            "{} is an unmanaged pair with padding; its entries are read unpadded \
                             (needs a decoder extension)",
                            il.csharp_name(t)
                        ));
                        false
                    }
                    None => false,
                };
                return Node::Dict {
                    hdr: false,
                    key: self.value(key, raw),
                    val: self.value(val, raw),
                };
            }
            // ValueTuple's formatter writes the items back to back; for blittable items without
            // padding that is the raw struct.
            (Some("ValueTuple"), _) if il.namespace(def) == "System" => {
                if let Some(layout) = self.generic_struct(def, args)
                    && layout.size
                        == layout
                            .fields
                            .iter()
                            .map(|f| self.size_of(&f.2))
                            .sum::<u32>()
                {
                    return self.raw(layout);
                }
            }
            _ => {}
        }
        if raw_ok && let Some(layout) = self.generic_struct(def, args) {
            return self.raw(layout);
        }
        if let Some(&wrapper) = self.generic_wrappers.get(t) {
            return Node::Ref(self.object(wrapper));
        }
        if base == Some("Nullable")
            && let [inner] = args
        {
            // An unmanaged `Nullable<T>` is copied raw: hasValue, padding to T's alignment, T.
            let Some((_, align)) = self.blittable(inner) else {
                return Node::Unsupported(format!(
                    "nullable {} of non-blittable type",
                    il.csharp_name(t)
                ));
            };
            return Node::Nullable {
                off: align,
                value: self.value(inner, true),
            };
        }
        Node::Unsupported(format!("unknown type {}", il.csharp_name(t)))
    }

    fn compile_def(&mut self, def: u32, raw_ok: bool) -> Node {
        let il = self.il;
        // Types with hand-written MemoryPack formatters (MemoryPack's Unity extensions).
        match il.short_name(def) {
            "AnimationCurve" => return Node::Curve,
            "Gradient" => return Node::Gradient,
            "RectOffset" => return Node::RectOffset,
            _ => {}
        }
        let type_kind = il.kind(def);
        if type_kind == TypeKind::Enum {
            return match il.enum_underlying(def).and_then(prim_name) {
                Some(name) => Node::Prim(name),
                None => {
                    Node::Unsupported(format!("enum {} has no integer type", il.full_name(def)))
                }
            };
        }
        let wrapper = self.wrapper_of.get(&def).copied();
        if type_kind == TypeKind::Struct
            && (raw_ok || wrapper.is_none())
            && let Some(layout) = self.unmanaged(def)
        {
            return self.raw(layout);
        }
        if let Some(wrapper) = wrapper {
            return Node::Ref(self.object(wrapper));
        }
        Node::Unsupported(format!(
            "unsupported type {} ({type_kind:?})",
            il.full_name(def)
        ))
    }

    /// `(size, align)` of a blittable type.
    fn blittable(&mut self, t: &TypeRef) -> Option<(u32, u32)> {
        match t {
            TypeRef::Prim(k) => prim_size(*k).map(|s| (s, s)),
            TypeRef::Def(d) => match self.il.kind(*d) {
                TypeKind::Enum => self
                    .il
                    .enum_underlying(*d)
                    .and_then(prim_size)
                    .map(|s| (s, s)),
                TypeKind::Struct => self.unmanaged(*d).map(|l| (l.size, l.align)),
                _ => None,
            },
            TypeRef::Generic(d, args) => self.generic_struct(*d, args).map(|l| (l.size, l.align)),
            _ => None,
        }
    }

    /// Size of a type known to be blittable (0 otherwise).
    fn size_of(&mut self, t: &TypeRef) -> u32 {
        self.blittable(t).map_or(0, |(size, _)| size)
    }

    /// Natural-alignment field offsets and size of a sequence of blittable types.
    fn natural_layout(&mut self, types: &[&TypeRef]) -> Option<(Vec<u32>, u32)> {
        let (mut off, mut align) = (0u32, 1u32);
        let mut offsets = Vec::with_capacity(types.len());
        for t in types {
            let (size, field_align) = self.blittable(t)?;
            off = off.div_ceil(field_align) * field_align;
            offsets.push(off);
            off += size;
            align = align.max(field_align);
        }
        Some((offsets, off.div_ceil(align) * align))
    }

    /// Raw layout of a blittable struct: its IL2CPP field offsets and instance size.
    fn unmanaged(&mut self, def: u32) -> Option<Layout> {
        if let Some(layout) = self.unmanaged.get(&def) {
            return layout.clone();
        }
        if !self.in_progress.insert(def) {
            return None;
        }
        let layout = self.compute_unmanaged(def);
        self.in_progress.remove(&def);
        self.unmanaged.insert(def, layout.clone());
        layout
    }

    fn compute_unmanaged(&mut self, def: u32) -> Option<Layout> {
        let il = self.il;
        if il.kind(def) != TypeKind::Struct {
            return None;
        }
        let fields: Vec<_> = il
            .fields(def)
            .ok()?
            .into_iter()
            .filter(|f| !f.is_static())
            .collect();
        if fields.is_empty() {
            return None;
        }
        let (mut end, mut align) = (0, 1);
        let mut out = Vec::with_capacity(fields.len());
        for f in fields {
            let offset = u32::try_from(f.offset?).ok()?;
            let (size, field_align) = self.blittable(&f.ty)?;
            end = end.max(offset + size);
            align = align.max(field_align);
            out.push((offset, f.name.to_owned(), f.ty));
        }
        // The IL2CPP instance size is authoritative (explicit `Pack`/`Size` included).
        let size = il
            .value_size(def)
            .filter(|&s| s > 0)
            .unwrap_or_else(|| end.div_ceil(align) * align);
        Some(Layout {
            size,
            align,
            fields: out,
        })
    }

    /// Raw layout of a closed generic struct such as `Optional<Vector3>`. Generic instances
    /// have no recorded layout, so the fields get natural alignment.
    fn generic_struct(&mut self, def: u32, args: &[TypeRef]) -> Option<Layout> {
        let il = self.il;
        if il.kind(def) != TypeKind::Struct {
            return None;
        }
        let fields = il.fields(def).ok()?;
        let (mut off, mut align) = (0u32, 1u32);
        let mut out = Vec::new();
        for f in fields.into_iter().filter(|f| !f.is_static()) {
            let ty = substitute(&f.ty, args)?;
            let (size, field_align) = self.blittable(&ty)?;
            off = off.div_ceil(field_align) * field_align;
            out.push((off, f.name.to_owned(), ty));
            off += size;
            align = align.max(field_align);
        }
        if out.is_empty() {
            return None;
        }
        Some(Layout {
            size: off.div_ceil(align) * align,
            align,
            fields: out,
        })
    }

    /// The members a wrapper serializes: the base wrapper's, then its own `__x__` setters.
    fn wrapper_members(&mut self, wrapper: u32) -> Result<Vec<Member>, String> {
        if let Some(members) = self.members.get(&wrapper) {
            return Ok(members.clone());
        }
        let il = self.il;
        let mut out = Vec::new();
        if let Some(TypeRef::Def(base)) = il.parent(wrapper)
            && base != wrapper
            && self.is_wrapper_class(base)
        {
            out = self.wrapper_members(base)?;
        }
        let properties = il.properties(wrapper).map_err(|e| e.to_string())?;
        for p in properties {
            let Some(name) = p.name.strip_prefix("__").and_then(|n| n.strip_suffix("__")) else {
                continue;
            };
            if p.has_getter || !p.has_setter || name.is_empty() {
                continue;
            }
            let ty =
                p.ty.ok_or_else(|| format!("{}.{} has no type", il.full_name(wrapper), p.name))?;
            let allow = il
                .attributes(wrapper, p.token)
                .map_err(|e| e.to_string())?
                .iter()
                .any(|a| self.is_memorypack_attribute(a.type_def, ALLOW_SERIALIZE_ATTRIBUTE));
            out.push((name.to_owned(), ty, allow));
        }
        self.members.insert(wrapper, out.clone());
        Ok(out)
    }

    fn is_memorypack_attribute(&self, def: u32, name: &str) -> bool {
        self.il.name(def) == name && self.il.namespace(def) == MEMORYPACK_NAMESPACE
    }

    /// `[MemoryPackUnion(tag, typeof(W))]` cases of a wrapper.
    fn unions(&self, wrapper: u32) -> Result<BTreeMap<u16, u32>, String> {
        let il = self.il;
        let mut out = BTreeMap::new();
        for a in il.type_attributes(wrapper).map_err(|e| e.to_string())? {
            if !self.is_memorypack_attribute(a.type_def, UNION_ATTRIBUTE) {
                continue;
            }
            let (Some(tag), Some(case)) = (
                a.args.first().and_then(|v| v.as_i64()),
                a.args.get(1).and_then(|v| il.type_arg_def(v)),
            ) else {
                return Err(format!(
                    "malformed MemoryPackUnion on {}",
                    il.full_name(wrapper)
                ));
            };
            if !self.orig_of.contains_key(&case) {
                return Err(format!(
                    "union member {} has no wrapper",
                    il.dump_name(case)
                ));
            }
            let tag = u16::try_from(tag).map_err(|_| format!("union tag {tag} out of range"))?;
            out.insert(tag, case);
        }
        Ok(out)
    }

    /// `$type` of a union case: the serialized type's `dump.cs` name.
    fn case_name(&self, wrapper: u32) -> String {
        match self.orig_of.get(&wrapper) {
            Some(TypeRef::Def(d) | TypeRef::Generic(d, _)) => self.il.dump_name(*d),
            _ => self.il.dump_name(wrapper),
        }
    }

    /// Node of the object (or union) a wrapper serializes.
    fn object(&mut self, wrapper: u32) -> usize {
        if let Some(&slot) = self.objects.get(&wrapper) {
            return slot;
        }
        let slot = self.push(Node::Pending);
        self.objects.insert(wrapper, slot);
        self.nodes[slot] = match self.compile_object(wrapper) {
            Ok(node) => node,
            Err(why) => Node::Unsupported(why),
        };
        slot
    }

    fn compile_object(&mut self, wrapper: u32) -> Result<Node, String> {
        let unions = self.unions(wrapper)?;
        if !unions.is_empty() {
            let mut cases = BTreeMap::new();
            for (tag, case) in unions {
                cases.insert(tag, (self.case_name(case), self.object(case)));
            }
            return Ok(Node::Union(cases));
        }
        let members = self.wrapper_members(wrapper)?;
        let compiled = members
            .iter()
            .map(|(name, ty, allow)| (name.clone(), self.value(ty, *allow)))
            .collect();
        Ok(Node::Obj(compiled))
    }
}

/// Replace class generic parameters by `args`; `None` when one is out of range.
fn substitute(t: &TypeRef, args: &[TypeRef]) -> Option<TypeRef> {
    Some(match t {
        TypeRef::Var(_, num) => args.get(usize::from(*num))?.clone(),
        TypeRef::Generic(d, inner) => TypeRef::Generic(
            *d,
            inner
                .iter()
                .map(|a| substitute(a, args))
                .collect::<Option<_>>()?,
        ),
        TypeRef::SzArray(e) => TypeRef::SzArray(Box::new(substitute(e, args)?)),
        TypeRef::Array(e, rank) => TypeRef::Array(Box::new(substitute(e, args)?), *rank),
        TypeRef::Ptr(e) => TypeRef::Ptr(Box::new(substitute(e, args)?)),
        other => other.clone(),
    })
}

// ── Compiled schema ─────────────────────────────────────

/// A root type of the compiled schema.
#[derive(Debug, Clone)]
pub struct RootInfo {
    /// Node index.
    pub node: usize,
    /// The type's `dump.cs` name (no namespace), which the learner matches paths against.
    pub short_name: String,
    /// Member count for an object root; `None` for a union (or unsupported) root.
    pub members: Option<usize>,
    pub union: bool,
    pub is_abstract: bool,
}

/// The schema before root learning.
pub struct CompiledSchema {
    nodes: Vec<Node>,
    /// Root type full name -> root.
    pub roots: BTreeMap<String, RootInfo>,
    pub typedefs: usize,
    pub registration_va: u64,
    pub wrappers: usize,
    pub generic_wrappers: usize,
    /// Things the compiler noticed but worked around (duplicate wrappers, name clashes).
    pub notes: Vec<String>,
}

impl CompiledSchema {
    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    /// Node count per kind (`"obj"`, `"raw"`, ...).
    pub fn node_kinds(&self) -> BTreeMap<&'static str, usize> {
        let mut kinds = BTreeMap::new();
        for n in &self.nodes {
            *kinds.entry(n.kind()).or_default() += 1;
        }
        kinds
    }

    /// Every unsupported node's reason, with how many nodes give it.
    pub fn unsupported(&self) -> BTreeMap<String, usize> {
        let mut out = BTreeMap::new();
        for n in &self.nodes {
            if let Node::Unsupported(why) = n {
                *out.entry(why.clone()).or_default() += 1;
            }
        }
        out
    }

    /// The format-1 schema document with the given `dir_roots`.
    pub fn to_json(&self, game_version: &str, dir_roots: &BTreeMap<String, Vec<String>>) -> Value {
        let roots: serde_json::Map<String, Value> = self
            .roots
            .iter()
            .map(|(name, r)| (name.clone(), json!(r.node)))
            .collect();
        json!({
            "format": SCHEMA_FORMAT,
            "game_version": game_version,
            "nodes": self.nodes.iter().map(Node::to_json).collect::<Vec<_>>(),
            "roots": roots,
            "dir_roots": dir_roots,
        })
    }
}

/// Compile every wrapper reachable from a root into nodes.
pub fn compile(il: &Il2Cpp) -> CompiledSchema {
    let mut c = Compiler::new(il);
    let mut origs: Vec<(u32, u32)> = c.wrapper_of.iter().map(|(&o, &w)| (o, w)).collect();
    origs.sort_unstable();
    let mut roots = BTreeMap::new();
    for (orig, wrapper) in origs {
        let node = c.object(wrapper);
        let name = il.full_name(orig);
        let (members, union) = match &c.nodes[node] {
            Node::Obj(m) => (Some(m.len()), false),
            Node::Union(_) => (None, true),
            _ => (None, false),
        };
        let info = RootInfo {
            node,
            short_name: il.dump_name(orig),
            members,
            union,
            is_abstract: il.is_abstract(wrapper),
        };
        if roots.contains_key(&name) {
            c.notes.push(format!(
                "two root types are named {name}; keeping the first"
            ));
            continue;
        }
        roots.insert(name, info);
    }
    debug_assert!(!c.nodes.iter().any(|n| matches!(n, Node::Pending)));
    CompiledSchema {
        roots,
        typedefs: il.typedef_count(),
        registration_va: il.reg.va,
        wrappers: c.orig_of.len(),
        generic_wrappers: c.generic_wrappers.len(),
        notes: c.notes,
        nodes: c.nodes,
    }
}

// ── Root learning ───────────────────────────────────────

/// Lowercased words of an identifier or path, split like `[A-Z]?[a-z]+|[A-Z]+(?![a-z])|\d+`.
pub fn words(s: &str) -> HashSet<String> {
    let b = s.as_bytes();
    let mut out = HashSet::new();
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        let start = i;
        if c.is_ascii_uppercase() && b.get(i + 1).is_some_and(u8::is_ascii_lowercase) {
            i += 1;
            while b.get(i).is_some_and(u8::is_ascii_lowercase) {
                i += 1;
            }
        } else if c.is_ascii_lowercase() {
            while b.get(i).is_some_and(u8::is_ascii_lowercase) {
                i += 1;
            }
        } else if c.is_ascii_uppercase() {
            while b.get(i).is_some_and(u8::is_ascii_uppercase) {
                i += 1;
            }
            // An upper run followed by lowercase leaves its last letter to the next word.
            if b.get(i).is_some_and(u8::is_ascii_lowercase) {
                i -= 1;
            }
        } else if c.is_ascii_digit() {
            while b.get(i).is_some_and(u8::is_ascii_digit) {
                i += 1;
            }
        } else {
            i += 1;
            continue;
        }
        out.insert(s[start..i].to_ascii_lowercase());
    }
    out
}

/// Totals from [`learn_dir_roots`].
#[derive(Debug, Default, Clone)]
pub struct LearnReport {
    /// MemoryPack files seen (`.mpk` sources, or `.json` files that are not JSON).
    pub files: usize,
    pub decoded: usize,
    /// Relative paths of files no root decodes (the first [`LEARN_REPORTED_FAILURES`]).
    pub failed: Vec<String>,
    pub failed_count: usize,
    pub dirs: usize,
    pub elapsed: Duration,
}

/// The MemoryPack source of a `.json` file: its `.mpk` copy when the tree was decoded in
/// place, else the file itself when it is not JSON.
pub fn memorypack_source(json_path: &Path) -> io::Result<Option<Vec<u8>>> {
    let data = match fs::read(json_path.with_extension("mpk")) {
        Ok(data) => data,
        Err(e) if e.kind() == io::ErrorKind::NotFound => fs::read(json_path)?,
        Err(e) => return Err(e),
    };
    Ok((!data.is_empty() && data[0] != b'{').then_some(data))
}

/// For every directory of `json_dir` (a `JsonData/Json` tree), the root types that decode
/// its MemoryPack files, most frequent first. Directories are keyed relative to `json_dir`,
/// `/`-separated, `.` for `json_dir` itself. Nothing is written.
///
/// Within a directory, files are tried in name order against the roots learned so far, then
/// every root whose header fits (objects with as many members as the first byte, then
/// unions), those whose name shares the most words with the file's path first.
pub fn learn_dir_roots(
    compiled: &CompiledSchema,
    schema: &Schema,
    json_dir: &Path,
) -> io::Result<(BTreeMap<String, Vec<String>>, LearnReport)> {
    let started = Instant::now();
    let pool = StringPool::load(&json_dir.join("StringPool"))?;
    let mut by_dir: BTreeMap<String, Vec<std::path::PathBuf>> = BTreeMap::new();
    for (path, rel_dir) in memorypack::json_files(json_dir)? {
        by_dir.entry(rel_dir).or_default().push(path);
    }
    let mut by_count: HashMap<usize, Vec<&str>> = HashMap::new();
    let mut unions: Vec<&str> = Vec::new();
    for (name, root) in &compiled.roots {
        if root.union {
            unions.push(name);
        } else if let Some(n) = root.members.filter(|_| !root.is_abstract) {
            by_count.entry(n).or_default().push(name);
        }
    }
    let name_words: HashMap<&str, HashSet<String>> = compiled
        .roots
        .iter()
        .map(|(name, r)| (name.as_str(), words(&r.short_name)))
        .collect();
    let candidates = Candidates {
        by_count,
        unions,
        name_words,
    };

    // Big directories first, one directory per task: files within one stay sequential.
    let mut dirs: Vec<(String, Vec<std::path::PathBuf>)> = by_dir.into_iter().collect();
    dirs.sort_by_key(|(_, files)| std::cmp::Reverse(files.len()));
    for (_, files) in &mut dirs {
        files.sort();
    }
    let next = AtomicUsize::new(0);
    let results = Mutex::new(Vec::new());
    let workers = std::thread::available_parallelism().map_or(4, usize::from);
    std::thread::scope(|scope| -> io::Result<()> {
        let handles: Vec<_> = (0..workers)
            .map(|_| {
                scope.spawn(|| -> io::Result<()> {
                    loop {
                        let k = next.fetch_add(1, Ordering::Relaxed);
                        let Some((rel_dir, files)) = dirs.get(k) else {
                            return Ok(());
                        };
                        let outcome = learn_dir(schema, &pool, json_dir, files, &candidates)?;
                        results
                            .lock()
                            .map_err(|_| io::Error::other("learner lock poisoned"))?
                            .push((rel_dir.clone(), outcome));
                    }
                })
            })
            .collect();
        for h in handles {
            h.join()
                .map_err(|_| io::Error::other("root learner worker panicked"))??;
        }
        Ok(())
    })?;

    let mut report = LearnReport::default();
    let mut dir_roots = BTreeMap::new();
    let mut results = results
        .into_inner()
        .map_err(|_| io::Error::other("learner lock poisoned"))?;
    results.sort_by(|a, b| a.0.cmp(&b.0));
    for (rel_dir, outcome) in results {
        report.files += outcome.files;
        report.decoded += outcome.decoded;
        report.failed_count += outcome.failed.len();
        for f in outcome.failed {
            if report.failed.len() < LEARN_REPORTED_FAILURES {
                report.failed.push(f);
            }
        }
        if !outcome.winners.is_empty() {
            dir_roots.insert(rel_dir, outcome.winners);
        }
    }
    report.dirs = dir_roots.len();
    report.elapsed = started.elapsed();
    Ok((dir_roots, report))
}

struct DirOutcome {
    files: usize,
    decoded: usize,
    failed: Vec<String>,
    /// Most frequent first.
    winners: Vec<String>,
}

/// The roots a file with no learned root may be: by header byte, plus every union.
struct Candidates<'c> {
    /// Non-abstract object roots by member count, in name order.
    by_count: HashMap<usize, Vec<&'c str>>,
    unions: Vec<&'c str>,
    /// Words of each root's type name, matched against the file's path.
    name_words: HashMap<&'c str, HashSet<String>>,
}

fn learn_dir(
    schema: &Schema,
    pool: &StringPool,
    json_dir: &Path,
    files: &[std::path::PathBuf],
    candidates: &Candidates,
) -> io::Result<DirOutcome> {
    // (root, count), kept sorted by count, ties by first success.
    let mut winners: Vec<(String, usize)> = Vec::new();
    let mut outcome = DirOutcome {
        files: 0,
        decoded: 0,
        failed: Vec::new(),
        winners: Vec::new(),
    };
    for path in files {
        let Some(data) = memorypack_source(path)? else {
            continue;
        };
        outcome.files += 1;
        let rel = path
            .strip_prefix(json_dir)
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/");
        let mut found = winners
            .iter()
            .position(|(root, _)| memorypack::decode_as(schema, pool, &data, root).is_some());
        if found.is_none() {
            let hint = words(&rel.replace(['/', '_'], " "));
            let mut rest: Vec<&str> = candidates
                .by_count
                .get(&usize::from(data[0]))
                .into_iter()
                .flatten()
                .chain(&candidates.unions)
                .copied()
                .filter(|r| !winners.iter().any(|(w, _)| w == r))
                .collect();
            // Stable: equal scores keep name order.
            rest.sort_by_key(|r| {
                std::cmp::Reverse(
                    candidates
                        .name_words
                        .get(r)
                        .map_or(0, |w| w.intersection(&hint).count()),
                )
            });
            if let Some(root) = rest
                .into_iter()
                .find(|root| memorypack::decode_as(schema, pool, &data, root).is_some())
            {
                winners.push((root.to_owned(), 0));
                found = Some(winners.len() - 1);
            }
        }
        match found {
            Some(k) => {
                outcome.decoded += 1;
                winners[k].1 += 1;
                // Keep most frequent first; a stable sort keeps first successes ahead on ties.
                winners.sort_by_key(|(_, count)| std::cmp::Reverse(*count));
            }
            None => outcome.failed.push(rel),
        }
    }
    outcome.winners = winners.into_iter().map(|(root, _)| root).collect();
    Ok(outcome)
}

// ── Entry point ─────────────────────────────────────────

/// What [`generate_schema`] built.
#[derive(Debug, Clone)]
pub struct SchemaReport {
    pub typedefs: usize,
    pub registration_va: u64,
    /// Wrapper classes found (closed generic ones included).
    pub wrappers: usize,
    pub generic_wrappers: usize,
    pub roots: usize,
    pub nodes: usize,
    pub node_kinds: BTreeMap<&'static str, usize>,
    /// Unsupported node reasons and how many nodes give each.
    pub unsupported: BTreeMap<String, usize>,
    pub notes: Vec<String>,
    pub compile_time: Duration,
    /// Present when a `Json/` tree was given to learn `dir_roots` from.
    pub learn: Option<LearnReport>,
}

/// Build the format-1 MemoryPack schema JSON from the client's `GameAssembly.dll` and
/// `global-metadata.dat`, learning `dir_roots` from `json_dir` (a `JsonData/Json` tree,
/// only read) when given. Without it `dir_roots` is empty and the decoder falls back to
/// trying every root whose header fits.
pub fn generate_schema(
    dll: &[u8],
    metadata: &[u8],
    game_version: &str,
    json_dir: Option<&Path>,
) -> io::Result<(Vec<u8>, SchemaReport)> {
    let started = Instant::now();
    let il = Il2Cpp::new(dll, metadata)?;
    let compiled = compile(&il);
    let compile_time = started.elapsed();
    let mut dir_roots = BTreeMap::new();
    let mut learn = None;
    if let Some(dir) = json_dir {
        let bare = serde_json::to_vec(&compiled.to_json(game_version, &dir_roots))?;
        let schema = Schema::from_json(&bare).map_err(io::Error::other)?;
        let (learned, report) = learn_dir_roots(&compiled, &schema, dir)?;
        dir_roots = learned;
        learn = Some(report);
    }
    let json = serde_json::to_vec(&compiled.to_json(game_version, &dir_roots))?;
    let report = SchemaReport {
        typedefs: compiled.typedefs,
        registration_va: compiled.registration_va,
        wrappers: compiled.wrappers,
        generic_wrappers: compiled.generic_wrappers,
        roots: compiled.roots.len(),
        nodes: compiled.node_count(),
        node_kinds: compiled.node_kinds(),
        unsupported: compiled.unsupported(),
        notes: compiled.notes.clone(),
        compile_time,
        learn,
    };
    Ok((json, report))
}

/// Gzip a schema the way `backend/schema/memorypack/*.json.gz` is stored.
pub fn gzip(json: &[u8]) -> io::Result<Vec<u8>> {
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::best());
    encoder.write_all(json)?;
    encoder.finish()
}
