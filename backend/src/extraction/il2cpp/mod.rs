//! A minimal IL2CPP reader over a Windows client's `GameAssembly.dll` and
//! `global-metadata.dat`, scoped to what the MemoryPack schema generator needs: type
//! definitions, fields with their IL2CPP offsets, instance sizes, properties, interfaces,
//! generic instances and custom attributes. No method bodies, no `DummyDll`.
//!
//! Ported from the reader the same author wrote for Arknights (Myrtle's
//! `unpacker/src/schema/il2cpp`). Format facts, measured on Endfield 1.6.4:
//!
//! - Metadata magic `0xFAB11BAF`, version 29. The header is the magic, the version and
//!   (offset, size) int32 pairs; the section count follows from where section 0 starts.
//!   Sections are contiguous in file order; the parser checks that.
//! - Typedef rows are 92 bytes, not v29's 88: one extra int32 at dword 16, after
//!   `interfaceOffsetsStart` and before the ushort counts. The stride is detected, never
//!   assumed: of 88 and 92, the one that divides the section, whose row count equals the
//!   images' summed `typeCount` (91,478 on 1.6.4), and whose row 0 is named `<Module>`.
//!   Every other table is standard v29: fields 12 bytes, properties 20, methods 32,
//!   parameters 12, generic containers 16, generic parameters 16, interfaces 4, images 40,
//!   field default values 12, attribute data ranges 8.
//! - v29 constants (enum values, attribute arguments) encode 4-byte integers in IL2CPP's
//!   compressed form and strings as a compressed length plus UTF-8.
//! - Field `static`/`literal` flags live in the `attrs` bits of the field's `Il2CppType` in
//!   the binary, not in the metadata row.
//! - `Il2CppMetadataRegistration` is found by scanning the PE for the qword run
//!   `typedefCount, ptr, typedefCount` (`fieldOffsetsCount`, `fieldOffsets`,
//!   `typeDefinitionsSizesCount`); the struct starts 10 qwords earlier, and the first seven
//!   count/pointer pairs must be sane. On 1.6.4 it is at VA 0x18da60e20 in `.rdata`; the
//!   `.tvm0` protector section does not disturb it. `CodeRegistration` is not needed.
//! - `fieldOffsets[t]` points at `int32` offsets for each field of typedef `t` (null for
//!   types without laid-out fields, e.g. generic definitions). Value-type offsets include
//!   the 16-byte object header, as does `Il2CppTypeDefinitionSizes::instance_size`.
//! - v27+ `Il2CppGenericClass` holds an `Il2CppType*` for the definition (not a typedef
//!   index), then `class_inst`.
//!
//! Any of these facts failing is a structural [`Error`]: the client changed shape and a
//! human must look.

pub mod attrs;
pub mod metadata;
pub mod pe;
pub mod registration;

use std::collections::HashMap;
use std::fmt;
use std::io;

use attrs::{Attribute, Value};
use metadata::{Blob, Metadata};
use pe::Pe;
use registration::{RawType, Registration, TypeRef, kind};

/// `FieldAttributes.Static`.
pub const FIELD_STATIC: u16 = 0x10;
/// `FieldAttributes.Literal` (`const`).
pub const FIELD_LITERAL: u16 = 0x40;

/// Size of the object header a boxed value type's offsets and instance size include.
const OBJECT_HEADER: i32 = 16;

/// A reader failure. `structural` means the files do not have the layout this reader knows
/// (a client update moved something); otherwise a single read went out of range.
#[derive(Debug, Clone)]
pub struct Error {
    structural: bool,
    message: String,
}

impl Error {
    pub fn structural(message: impl Into<String>) -> Self {
        Self {
            structural: true,
            message: message.into(),
        }
    }

    pub fn malformed(message: impl Into<String>) -> Self {
        Self {
            structural: false,
            message: message.into(),
        }
    }

    pub fn is_structural(&self) -> bool {
        self.structural
    }

    fn context(self, what: impl fmt::Display) -> Self {
        Self {
            structural: self.structural,
            message: format!("{what}: {}", self.message),
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.structural {
            write!(f, "IL2CPP structural detection failed: {}", self.message)
        } else {
            write!(f, "IL2CPP: {}", self.message)
        }
    }
}

impl std::error::Error for Error {}

impl From<Error> for io::Error {
    fn from(error: Error) -> Self {
        io::Error::new(io::ErrorKind::InvalidData, error)
    }
}

pub type Result<T> = std::result::Result<T, Error>;

/// What a typedef is, as C# would declare it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TypeKind {
    Class,
    Struct,
    Enum,
    Interface,
}

/// One field of a typedef.
#[derive(Debug, Clone)]
pub struct Field<'a> {
    /// Index into the metadata `fields` table.
    pub index: u32,
    pub name: &'a str,
    pub ty: TypeRef,
    /// `FieldAttributes` from the field's `Il2CppType`.
    pub attrs: u16,
    pub token: u32,
    /// Instance offset as C# sees it (value types without the object header); `None` when
    /// the binary records none.
    pub offset: Option<i32>,
}

impl Field<'_> {
    /// Static or `const`: not part of an instance.
    pub fn is_static(&self) -> bool {
        self.attrs & (FIELD_STATIC | FIELD_LITERAL) != 0
    }
}

/// One property of a typedef.
#[derive(Debug, Clone)]
pub struct Property<'a> {
    pub name: &'a str,
    pub has_getter: bool,
    pub has_setter: bool,
    /// The getter's return type, else the setter's parameter type.
    pub ty: Option<TypeRef>,
    pub token: u32,
}

pub struct Il2Cpp<'a> {
    pub meta: Metadata<'a>,
    pub pe: Pe<'a>,
    pub reg: Registration,
    typedef_image: Vec<u32>,
    attr_index: HashMap<(u32, u32), usize>,
}

impl<'a> Il2Cpp<'a> {
    /// Parse both files and validate the result (typedef stride, member ranges, exactly one
    /// registration candidate, every typedef round-tripping through `types[]`).
    pub fn new(dll: &'a [u8], metadata: &'a [u8]) -> Result<Self> {
        let meta = Metadata::parse(metadata)?;
        let pe = Pe::parse(dll)?;
        let reg = registration::find(&pe, meta.type_defs.len())?;

        let mut typedef_image = vec![u32::MAX; meta.type_defs.len()];
        for (k, img) in meta.images.iter().enumerate() {
            let range = img.type_start as usize..(img.type_start + img.type_count) as usize;
            for t in range {
                if let Some(slot) = typedef_image.get_mut(t) {
                    *slot = k as u32;
                }
            }
        }
        let mut attr_index = HashMap::new();
        for (k, img) in meta.images.iter().enumerate() {
            let start = img.custom_attribute_start as usize;
            for r in start..start + img.custom_attribute_count as usize {
                attr_index.insert((k as u32, meta.attribute_ranges[r].0), r);
            }
        }
        let il = Self {
            meta,
            pe,
            reg,
            typedef_image,
            attr_index,
        };
        il.check_byval_types()?;
        Ok(il)
    }

    /// Every typedef's `byvalTypeIndex` must point back at it through the binary's
    /// `types[]`; a mismatch means the registration or the typedef layout was misread.
    fn check_byval_types(&self) -> Result<()> {
        let bad = self
            .meta
            .type_defs
            .iter()
            .enumerate()
            .filter(|(k, t)| {
                // Built-ins (`System.Int32`, `System.String`, ...) carry their element type as
                // the kind; the data is still the typedef index.
                !matches!(self.raw_type(t.byval_type_index), Ok(r) if r.data == *k as u64)
            })
            .count();
        if bad > 0 {
            return Err(Error::structural(format!(
                "{bad} of {} typedefs do not round-trip through types[] (registration at {:#x})",
                self.meta.type_defs.len(),
                self.reg.va
            )));
        }
        if self.reg.field_offsets.len() != self.meta.type_defs.len()
            || self.reg.type_sizes.len() != self.meta.type_defs.len()
        {
            return Err(Error::structural(
                "fieldOffsets / typeDefinitionsSizes do not have one entry per typedef",
            ));
        }
        Ok(())
    }

    pub fn typedef_count(&self) -> usize {
        self.meta.type_defs.len()
    }

    fn def(&self, def: u32) -> &metadata::TypeDef {
        &self.meta.type_defs[def as usize]
    }

    pub fn raw_type(&self, index: i32) -> Result<RawType> {
        self.reg.raw(&self.pe, index)
    }

    /// Resolve a type index, filling each class generic parameter's position.
    pub fn type_ref(&self, index: i32) -> Result<TypeRef> {
        let r = registration::resolve(&self.pe, self.raw_type(index)?)?;
        Ok(self.fill_vars(r))
    }

    fn fill_vars(&self, t: TypeRef) -> TypeRef {
        match t {
            TypeRef::Var(p, _) => {
                let num = self
                    .meta
                    .generic_params
                    .get(p as usize)
                    .map_or(0, |g| g.num);
                TypeRef::Var(p, num)
            }
            TypeRef::Generic(d, args) => {
                TypeRef::Generic(d, args.into_iter().map(|a| self.fill_vars(a)).collect())
            }
            TypeRef::SzArray(e) => TypeRef::SzArray(Box::new(self.fill_vars(*e))),
            TypeRef::Array(e, r) => TypeRef::Array(Box::new(self.fill_vars(*e)), r),
            TypeRef::Ptr(e) => TypeRef::Ptr(Box::new(self.fill_vars(*e))),
            other => other,
        }
    }

    /// The metadata name, e.g. ``List`1``.
    pub fn name(&self, def: u32) -> &'a str {
        self.meta.string(self.def(def).name_index)
    }

    /// The name without its generic arity suffix.
    pub fn short_name(&self, def: u32) -> &'a str {
        let name = self.name(def);
        name.split_once('`').map_or(name, |(stem, _)| stem)
    }

    /// The enclosing typedef of a nested type.
    pub fn declaring(&self, def: u32) -> Option<u32> {
        let t = self.def(def);
        if t.declaring_type_index < 0 {
            return None;
        }
        let r = self.raw_type(t.declaring_type_index).ok()?;
        let outer = u32::try_from(r.data).ok()?;
        ((outer as usize) < self.typedef_count() && outer != def).then_some(outer)
    }

    /// The namespace, taken from the outermost type for nested types.
    pub fn namespace(&self, def: u32) -> &'a str {
        let mut d = def;
        for _ in 0..64 {
            match self.declaring(d) {
                Some(outer) => d = outer,
                None => break,
            }
        }
        self.meta.string(self.def(d).namespace_index)
    }

    /// The declared generic parameter names of a typedef, in order.
    pub fn generic_params_of(&self, def: u32) -> Vec<&'a str> {
        let t = self.def(def);
        let Some(c) = usize::try_from(t.generic_container_index)
            .ok()
            .and_then(|i| self.meta.generic_containers.get(i))
        else {
            return Vec::new();
        };
        (0..c.type_argc.max(0))
            .map(|k| self.generic_param_name((c.generic_parameter_start + k) as u32))
            .collect()
    }

    /// Name of a generic parameter by its `genericParameters` index.
    pub fn generic_param_name(&self, index: u32) -> &'a str {
        self.meta
            .generic_params
            .get(index as usize)
            .map_or("", |g| self.meta.string(g.name_index))
    }

    /// The name as Il2CppDumper's `dump.cs` declares it: enclosing types joined by `.`, the
    /// arity suffix replaced by the parameter list (`Outer.Inner`, `Optional<T>`).
    /// Enclosing types are spelled without their parameters, as the dump does.
    pub fn dump_name(&self, def: u32) -> String {
        let mut name = self.nesting_prefix(def);
        name.push_str(self.short_name(def));
        let params = self.generic_params_of(def);
        if !params.is_empty() {
            name.push('<');
            name.push_str(&params.join(", "));
            name.push('>');
        }
        name
    }

    /// `Outer.Inner.` for a type nested in `Outer.Inner`, else empty.
    fn nesting_prefix(&self, def: u32) -> String {
        let mut prefix = String::new();
        let mut d = def;
        for _ in 0..64 {
            let Some(outer) = self.declaring(d) else {
                break;
            };
            prefix.insert(0, '.');
            prefix.insert_str(0, self.short_name(outer));
            d = outer;
        }
        prefix
    }

    /// `Namespace.DumpName`, the key the dump-based tools used.
    pub fn full_name(&self, def: u32) -> String {
        let ns = self.namespace(def);
        let name = self.dump_name(def);
        if ns.is_empty() {
            name
        } else {
            format!("{ns}.{name}")
        }
    }

    pub fn kind(&self, def: u32) -> TypeKind {
        let t = self.def(def);
        if t.is_enum() {
            TypeKind::Enum
        } else if t.is_value_type() {
            TypeKind::Struct
        } else if t.is_interface() {
            TypeKind::Interface
        } else {
            TypeKind::Class
        }
    }

    pub fn is_abstract(&self, def: u32) -> bool {
        self.def(def).is_abstract()
    }

    /// The base type, if any.
    pub fn parent(&self, def: u32) -> Option<TypeRef> {
        let t = self.def(def);
        (t.parent_index >= 0)
            .then(|| self.type_ref(t.parent_index).ok())
            .flatten()
    }

    /// Directly implemented interfaces, in metadata order.
    pub fn interfaces(&self, def: u32) -> Result<Vec<TypeRef>> {
        let t = self.def(def);
        let start = usize::try_from(t.interfaces_start).unwrap_or(0);
        (0..usize::from(t.interfaces_count))
            .map(|k| self.type_ref(self.meta.interfaces[start + k]))
            .collect()
    }

    /// An enum's underlying element type (`kind::I4`, ...).
    pub fn enum_underlying(&self, def: u32) -> Option<u8> {
        let t = self.def(def);
        if !t.is_enum() {
            return None;
        }
        self.raw_type(t.element_type_index).ok().map(|r| r.kind)
    }

    /// `Il2CppTypeDefinitionSizes::instance_size`, header included for every type.
    pub fn instance_size(&self, def: u32) -> Result<u32> {
        let ptr = self.reg.type_sizes[def as usize];
        self.pe.u32_at(ptr)
    }

    /// A value type's managed size: its instance size without the object header.
    pub fn value_size(&self, def: u32) -> Option<u32> {
        let size = self.instance_size(def).ok()?;
        size.checked_sub(OBJECT_HEADER as u32)
    }

    /// The fields of a typedef in declaration order.
    pub fn fields(&self, def: u32) -> Result<Vec<Field<'a>>> {
        let t = self.def(def);
        let value_type = t.is_value_type();
        let offsets = self.reg.field_offsets[def as usize];
        let start = usize::try_from(t.field_start).unwrap_or(0);
        (0..usize::from(t.field_count))
            .map(|k| {
                let index = start + k;
                let f = &self.meta.fields[index];
                let raw = self.raw_type(f.type_index)?;
                let ty = self.fill_vars(registration::resolve(&self.pe, raw)?);
                let static_field = raw.attrs & FIELD_STATIC != 0;
                let offset = if offsets == 0 || raw.attrs & FIELD_LITERAL != 0 {
                    None
                } else {
                    let raw_offset = self.pe.i32_at(offsets + 4 * k as u64)?;
                    Some(if value_type && !static_field && raw_offset > 0 {
                        raw_offset - OBJECT_HEADER
                    } else {
                        raw_offset
                    })
                };
                Ok(Field {
                    index: index as u32,
                    name: self.meta.string(f.name_index),
                    ty,
                    attrs: raw.attrs,
                    token: f.token,
                    offset,
                })
            })
            .collect::<Result<_>>()
            .map_err(|e: Error| e.context(format!("fields of {}", self.full_name(def))))
    }

    /// The properties of a typedef in declaration order.
    pub fn properties(&self, def: u32) -> Result<Vec<Property<'a>>> {
        let t = self.def(def);
        let start = usize::try_from(t.property_start).unwrap_or(0);
        let method_start = t.method_start;
        (0..usize::from(t.property_count))
            .map(|k| {
                let p = &self.meta.properties[start + k];
                let method = |rel: i32| {
                    (rel >= 0)
                        .then(|| {
                            self.meta
                                .methods
                                .get(usize::try_from(method_start + rel).ok()?)
                        })
                        .flatten()
                };
                let getter = method(p.get);
                let setter = method(p.set);
                let ty = if let Some(g) = getter {
                    Some(self.type_ref(g.return_type)?)
                } else if let Some(s) = setter.filter(|s| s.parameter_count > 0) {
                    let param = usize::try_from(s.parameter_start)
                        .ok()
                        .and_then(|i| self.meta.parameters.get(i))
                        .ok_or_else(|| Error::malformed("setter parameter out of range"))?;
                    Some(self.type_ref(param.type_index)?)
                } else {
                    None
                };
                Ok(Property {
                    name: self.meta.string(p.name_index),
                    has_getter: getter.is_some(),
                    has_setter: setter.is_some(),
                    ty,
                    token: p.token,
                })
            })
            .collect::<Result<_>>()
            .map_err(|e: Error| e.context(format!("properties of {}", self.full_name(def))))
    }

    /// The image (assembly module) a typedef belongs to.
    pub fn image_of(&self, def: u32) -> u32 {
        self.typedef_image
            .get(def as usize)
            .copied()
            .unwrap_or(u32::MAX)
    }

    /// All custom attributes on the member with `token` in `def`'s image (the typedef's own
    /// token for the type itself).
    pub fn attributes(&self, def: u32, token: u32) -> Result<Vec<Attribute>> {
        let image = self.image_of(def);
        let Some(&r) = self.attr_index.get(&(image, token)) else {
            return Ok(Vec::new());
        };
        let start = self.meta.attribute_ranges[r].1;
        let end = self.meta.attribute_ranges.get(r + 1).map(|x| x.1);
        attrs::decode(self, self.meta.attribute_data(start, end)).map_err(|e| {
            e.context(format!(
                "attributes of token {token:#x} in {}",
                self.full_name(def)
            ))
        })
    }

    /// The typedef's own attributes.
    pub fn type_attributes(&self, def: u32) -> Result<Vec<Attribute>> {
        self.attributes(def, self.def(def).token)
    }

    /// The typedef a `typeof(X)` attribute argument names.
    pub fn type_arg_def(&self, value: &Value) -> Option<u32> {
        let Value::Type(index) = value else {
            return None;
        };
        match self.type_ref(*index).ok()? {
            TypeRef::Def(d) | TypeRef::Generic(d, _) => Some(d),
            _ => None,
        }
    }

    /// A field's constant (`fieldDefaultValues`), e.g. an enum member's value.
    pub fn field_default(&self, field_index: u32) -> Result<Option<Value>> {
        let Some(&(type_index, data)) = self.meta.field_defaults.get(&field_index) else {
            return Ok(None);
        };
        let k = self.raw_type(type_index)?.kind;
        let mut b = Blob::new(self.meta.default_value_data(data));
        attrs::read_value(self, &mut b, k).map(Some)
    }

    /// A type reference spelled as `dump.cs` spells it (`List<int>`, `Outer.Inner[]`).
    pub fn csharp_name(&self, t: &TypeRef) -> String {
        match t {
            TypeRef::Prim(k) => prim_keyword(*k).to_owned(),
            TypeRef::Def(d) => self.dump_name(*d),
            TypeRef::Generic(d, args) => {
                let mut name = self.nesting_prefix(*d);
                name.push_str(self.short_name(*d));
                let args: Vec<String> = args.iter().map(|a| self.csharp_name(a)).collect();
                format!("{name}<{}>", args.join(", "))
            }
            TypeRef::SzArray(e) => format!("{}[]", self.csharp_name(e)),
            TypeRef::Array(e, rank) => format!(
                "{}[{}]",
                self.csharp_name(e),
                ",".repeat(usize::from(rank.saturating_sub(1)))
            ),
            TypeRef::Ptr(e) => format!("{}*", self.csharp_name(e)),
            TypeRef::Var(p, _) => self.generic_param_name(*p).to_owned(),
            TypeRef::Other(k) => format!("<kind {k:#x}>"),
        }
    }
}

/// The C# keyword (or `dump.cs` name) of a built-in element type.
pub fn prim_keyword(k: u8) -> &'static str {
    match k {
        kind::VOID => "void",
        kind::BOOLEAN => "bool",
        kind::CHAR => "char",
        kind::I1 => "sbyte",
        kind::U1 => "byte",
        kind::I2 => "short",
        kind::U2 => "ushort",
        kind::I4 => "int",
        kind::U4 => "uint",
        kind::I8 => "long",
        kind::U8 => "ulong",
        kind::R4 => "float",
        kind::R8 => "double",
        kind::STRING => "string",
        kind::OBJECT => "object",
        kind::I => "IntPtr",
        kind::U => "UIntPtr",
        _ => "?",
    }
}
