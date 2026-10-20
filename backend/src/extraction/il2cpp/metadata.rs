//! `global-metadata.dat` v29: header, section table and the rows the schema generator reads.
//! Layout facts are in the parent module's doc.

use std::collections::HashMap;

use super::{Error, Result};

pub const MAGIC: u32 = 0xFAB1_1BAF;
pub const VERSION: u32 = 29;
/// Sections a standard v29 header lists; some builds list more, which are not read.
pub const STANDARD_SECTIONS: usize = 31;

// Section indices (v29 order).
const S_STRING: usize = 2;
const S_PROPERTIES: usize = 4;
const S_METHODS: usize = 5;
const S_FIELD_DEFAULT_VALUES: usize = 7;
const S_DEFAULT_VALUE_DATA: usize = 8;
const S_PARAMETERS: usize = 10;
const S_FIELDS: usize = 11;
const S_GENERIC_PARAMETERS: usize = 12;
const S_GENERIC_CONTAINERS: usize = 14;
const S_INTERFACES: usize = 16;
const S_TYPE_DEFINITIONS: usize = 19;
const S_IMAGES: usize = 20;
const S_ATTRIBUTE_DATA: usize = 24;
const S_ATTRIBUTE_DATA_RANGE: usize = 25;

/// The standard v29 `Il2CppTypeDefinition` row; some builds add one int32 (92).
pub const TYPEDEF_STRIDE_STANDARD: usize = 88;
const TYPEDEF_STRIDE_CANDIDATES: [usize; 2] = [88, 92];

/// `TypeAttributes.Interface`.
pub const TYPE_INTERFACE: u32 = 0x20;
/// `TypeAttributes.Abstract`.
pub const TYPE_ABSTRACT: u32 = 0x80;

#[derive(Debug, Clone, Copy)]
pub struct Section {
    pub offset: usize,
    pub size: usize,
}

#[derive(Debug, Clone)]
pub struct TypeDef {
    pub name_index: u32,
    pub namespace_index: u32,
    pub byval_type_index: i32,
    pub declaring_type_index: i32,
    pub parent_index: i32,
    pub element_type_index: i32,
    pub generic_container_index: i32,
    pub flags: u32,
    pub field_start: i32,
    pub method_start: i32,
    pub property_start: i32,
    pub interfaces_start: i32,
    pub method_count: u16,
    pub property_count: u16,
    pub field_count: u16,
    pub interfaces_count: u16,
    pub bitfield: u32,
    pub token: u32,
}

impl TypeDef {
    /// `bitfield` bit 0 is `valuetype`.
    pub const fn is_value_type(&self) -> bool {
        self.bitfield & 1 != 0
    }

    /// `bitfield` bit 1 is `enumtype`.
    pub const fn is_enum(&self) -> bool {
        self.bitfield & 2 != 0
    }

    pub const fn is_interface(&self) -> bool {
        self.flags & TYPE_INTERFACE != 0
    }

    pub const fn is_abstract(&self) -> bool {
        self.flags & TYPE_ABSTRACT != 0
    }
}

#[derive(Debug, Clone)]
pub struct FieldDef {
    pub name_index: u32,
    pub type_index: i32,
    pub token: u32,
}

#[derive(Debug, Clone)]
pub struct PropertyDef {
    pub name_index: u32,
    /// Method index relative to the declaring type's `method_start`; -1 none.
    pub get: i32,
    pub set: i32,
    pub token: u32,
}

#[derive(Debug, Clone)]
pub struct MethodDef {
    pub name_index: u32,
    pub declaring_type: i32,
    pub return_type: i32,
    pub parameter_start: i32,
    pub flags: u16,
    pub parameter_count: u16,
}

#[derive(Debug, Clone)]
pub struct ParameterDef {
    pub name_index: u32,
    pub type_index: i32,
}

#[derive(Debug, Clone)]
pub struct GenericContainer {
    pub type_argc: i32,
    pub generic_parameter_start: i32,
}

#[derive(Debug, Clone)]
pub struct GenericParam {
    pub name_index: u32,
    pub num: u16,
}

#[derive(Debug, Clone)]
pub struct ImageDef {
    pub type_start: u32,
    pub type_count: u32,
    pub custom_attribute_start: u32,
    pub custom_attribute_count: u32,
}

/// A parsed `global-metadata.dat`.
pub struct Metadata<'a> {
    data: &'a [u8],
    pub version: u32,
    pub sections: Vec<Section>,
    pub typedef_stride: usize,
    pub type_defs: Vec<TypeDef>,
    pub fields: Vec<FieldDef>,
    pub properties: Vec<PropertyDef>,
    pub methods: Vec<MethodDef>,
    pub parameters: Vec<ParameterDef>,
    pub generic_containers: Vec<GenericContainer>,
    pub generic_params: Vec<GenericParam>,
    /// `interfaces[]`: type indexes, sliced per typedef by `interfaces_start/count`.
    pub interfaces: Vec<i32>,
    pub images: Vec<ImageDef>,
    /// Field index -> (type index, offset into the default-value data).
    pub field_defaults: HashMap<u32, (i32, u32)>,
    /// `attributeDataRange` rows: (token, start offset into attribute data).
    pub attribute_ranges: Vec<(u32, u32)>,
}

fn rd_u16(d: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([d[o], d[o + 1]])
}

fn rd_u32(d: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([d[o], d[o + 1], d[o + 2], d[o + 3]])
}

fn rd_i32(d: &[u8], o: usize) -> i32 {
    rd_u32(d, o) as i32
}

/// Read the header: magic, version and the section table. The section count is implied by
/// where section 0 starts (`(offset - 8) / 8`).
pub fn parse_header(d: &[u8]) -> Result<(u32, Vec<Section>)> {
    if d.len() < 16 {
        return Err(Error::structural(format!(
            "metadata is {} bytes, too short for a header",
            d.len()
        )));
    }
    let magic = rd_u32(d, 0);
    if magic != MAGIC {
        return Err(Error::structural(format!(
            "metadata magic {magic:#010x} != {MAGIC:#010x}: not a plain global-metadata.dat"
        )));
    }
    let version = rd_u32(d, 4);
    if version != VERSION {
        return Err(Error::structural(format!(
            "metadata version {version}, this reader knows only v{VERSION}; the client moved to \
             a new IL2CPP version and the row layouts must be re-checked"
        )));
    }
    let first = rd_u32(d, 8) as usize;
    if first < 8 + 8 * STANDARD_SECTIONS || !(first - 8).is_multiple_of(8) || first > d.len() {
        return Err(Error::structural(format!(
            "section 0 starts at {first:#x}; a v29 header needs at least {STANDARD_SECTIONS} \
             sections ({:#x})",
            8 + 8 * STANDARD_SECTIONS
        )));
    }
    let count = (first - 8) / 8;
    let sections: Vec<Section> = (0..count)
        .map(|i| Section {
            offset: rd_u32(d, 8 + 8 * i) as usize,
            size: rd_u32(d, 12 + 8 * i) as usize,
        })
        .collect();
    // Contiguity: every section inside the file, in offset order without overlap, starting
    // right after the header.
    let mut order: Vec<usize> = (0..count).collect();
    order.sort_by_key(|&i| (sections[i].offset, sections[i].size));
    let mut end = first;
    for &i in &order {
        let s = sections[i];
        if s.offset < end {
            return Err(Error::structural(format!(
                "section {i} at {:#x} overlaps the previous section ending at {end:#x}",
                s.offset
            )));
        }
        if s.offset - end > 16 {
            return Err(Error::structural(format!(
                "section {i} at {:#x} leaves a {}-byte gap after {end:#x}",
                s.offset,
                s.offset - end
            )));
        }
        end = s.offset + s.size;
        if end > d.len() {
            return Err(Error::structural(format!(
                "section {i} ends at {end:#x}, past the file end {:#x}",
                d.len()
            )));
        }
    }
    Ok((version, sections))
}

fn rows(s: Section, stride: usize, what: &str) -> Result<usize> {
    if !s.size.is_multiple_of(stride) {
        return Err(Error::structural(format!(
            "{what} section is {} bytes, not a multiple of the {stride}-byte row",
            s.size
        )));
    }
    Ok(s.size / stride)
}

fn c_str(d: &[u8], start: usize) -> &[u8] {
    let rest = &d[start.min(d.len())..];
    let end = rest.iter().position(|&b| b == 0).unwrap_or(rest.len());
    &rest[..end]
}

/// Pick the typedef row size: the candidate that divides the section, makes the images'
/// `typeCount` sum equal the row count, and names row 0 `<Module>`. Exactly one must pass.
pub fn detect_typedef_stride(d: &[u8], sections: &[Section]) -> Result<usize> {
    let td = sections[S_TYPE_DEFINITIONS];
    let img = sections[S_IMAGES];
    let strings = sections[S_STRING];
    if !img.size.is_multiple_of(40) {
        return Err(Error::structural(format!(
            "images section is {} bytes, not a multiple of 40",
            img.size
        )));
    }
    let image_types: u64 = (0..img.size / 40)
        .map(|k| u64::from(rd_u32(d, img.offset + k * 40 + 12)))
        .sum();
    let mut passing = Vec::new();
    let mut tried = Vec::new();
    for stride in TYPEDEF_STRIDE_CANDIDATES {
        if !td.size.is_multiple_of(stride) || td.size < stride {
            tried.push(format!("{stride}: section size {} not a multiple", td.size));
            continue;
        }
        let n = (td.size / stride) as u64;
        let name_idx = rd_u32(d, td.offset) as usize;
        let name = if name_idx < strings.size {
            c_str(
                &d[..strings.offset + strings.size],
                strings.offset + name_idx,
            )
        } else {
            &[]
        };
        if n == image_types && name == b"<Module>" {
            passing.push(stride);
        } else {
            tried.push(format!(
                "{stride}: {n} rows vs {image_types} in the images, row 0 named {:?}",
                String::from_utf8_lossy(name)
            ));
        }
    }
    match passing.as_slice() {
        [one] => Ok(*one),
        _ => Err(Error::structural(format!(
            "typedef row size undetermined ({} candidates pass; {})",
            passing.len(),
            tried.join("; ")
        ))),
    }
}

impl<'a> Metadata<'a> {
    pub fn parse(data: &'a [u8]) -> Result<Self> {
        let (version, sections) = parse_header(data)?;
        let d = data;
        let stride = detect_typedef_stride(d, &sections)?;
        let extra = stride - TYPEDEF_STRIDE_STANDARD;

        let td = sections[S_TYPE_DEFINITIONS];
        let n = td.size / stride;
        let type_defs: Vec<TypeDef> = (0..n)
            .map(|k| {
                let o = td.offset + k * stride;
                // The ushort counts follow the extra column, when there is one.
                let c = o + 64 + extra;
                TypeDef {
                    name_index: rd_u32(d, o),
                    namespace_index: rd_u32(d, o + 4),
                    byval_type_index: rd_i32(d, o + 8),
                    declaring_type_index: rd_i32(d, o + 12),
                    parent_index: rd_i32(d, o + 16),
                    element_type_index: rd_i32(d, o + 20),
                    generic_container_index: rd_i32(d, o + 24),
                    flags: rd_u32(d, o + 28),
                    field_start: rd_i32(d, o + 32),
                    method_start: rd_i32(d, o + 36),
                    property_start: rd_i32(d, o + 44),
                    interfaces_start: rd_i32(d, o + 52),
                    method_count: rd_u16(d, c),
                    property_count: rd_u16(d, c + 2),
                    field_count: rd_u16(d, c + 4),
                    interfaces_count: rd_u16(d, c + 12),
                    bitfield: rd_u32(d, c + 16),
                    token: rd_u32(d, c + 20),
                }
            })
            .collect();

        let table = |index: usize, stride: usize, what: &str| -> Result<(Section, usize)> {
            let s = sections[index];
            Ok((s, rows(s, stride, what)?))
        };

        let (s, count) = table(S_FIELDS, 12, "fields")?;
        let fields = (0..count)
            .map(|k| {
                let o = s.offset + k * 12;
                FieldDef {
                    name_index: rd_u32(d, o),
                    type_index: rd_i32(d, o + 4),
                    token: rd_u32(d, o + 8),
                }
            })
            .collect();

        let (s, count) = table(S_PROPERTIES, 20, "properties")?;
        let properties = (0..count)
            .map(|k| {
                let o = s.offset + k * 20;
                PropertyDef {
                    name_index: rd_u32(d, o),
                    get: rd_i32(d, o + 4),
                    set: rd_i32(d, o + 8),
                    token: rd_u32(d, o + 16),
                }
            })
            .collect();

        let (s, count) = table(S_METHODS, 32, "methods")?;
        let methods = (0..count)
            .map(|k| {
                let o = s.offset + k * 32;
                MethodDef {
                    name_index: rd_u32(d, o),
                    declaring_type: rd_i32(d, o + 4),
                    return_type: rd_i32(d, o + 8),
                    parameter_start: rd_i32(d, o + 12),
                    flags: rd_u16(d, o + 24),
                    parameter_count: rd_u16(d, o + 30),
                }
            })
            .collect();

        let (s, count) = table(S_PARAMETERS, 12, "parameters")?;
        let parameters = (0..count)
            .map(|k| {
                let o = s.offset + k * 12;
                ParameterDef {
                    name_index: rd_u32(d, o),
                    type_index: rd_i32(d, o + 8),
                }
            })
            .collect();

        let (s, count) = table(S_GENERIC_CONTAINERS, 16, "genericContainers")?;
        let generic_containers = (0..count)
            .map(|k| {
                let o = s.offset + k * 16;
                GenericContainer {
                    type_argc: rd_i32(d, o + 4),
                    generic_parameter_start: rd_i32(d, o + 12),
                }
            })
            .collect();

        let (s, count) = table(S_GENERIC_PARAMETERS, 16, "genericParameters")?;
        let generic_params = (0..count)
            .map(|k| {
                let o = s.offset + k * 16;
                GenericParam {
                    name_index: rd_u32(d, o + 4),
                    num: rd_u16(d, o + 12),
                }
            })
            .collect();

        let (s, count) = table(S_INTERFACES, 4, "interfaces")?;
        let interfaces = (0..count).map(|k| rd_i32(d, s.offset + k * 4)).collect();

        let (s, count) = table(S_IMAGES, 40, "images")?;
        let images = (0..count)
            .map(|k| {
                let o = s.offset + k * 40;
                ImageDef {
                    type_start: rd_u32(d, o + 8),
                    type_count: rd_u32(d, o + 12),
                    custom_attribute_start: rd_u32(d, o + 32),
                    custom_attribute_count: rd_u32(d, o + 36),
                }
            })
            .collect();

        let (s, count) = table(S_FIELD_DEFAULT_VALUES, 12, "fieldDefaultValues")?;
        let field_defaults = (0..count)
            .map(|k| {
                let o = s.offset + k * 12;
                (rd_u32(d, o), (rd_i32(d, o + 4), rd_u32(d, o + 8)))
            })
            .collect();

        let (s, count) = table(S_ATTRIBUTE_DATA_RANGE, 8, "attributeDataRange")?;
        let attribute_ranges = (0..count)
            .map(|k| {
                let o = s.offset + k * 8;
                (rd_u32(d, o), rd_u32(d, o + 4))
            })
            .collect();

        let meta = Self {
            data,
            version,
            sections,
            typedef_stride: stride,
            type_defs,
            fields,
            properties,
            methods,
            parameters,
            generic_containers,
            generic_params,
            interfaces,
            images,
            field_defaults,
            attribute_ranges,
        };
        meta.check_ranges()?;
        Ok(meta)
    }

    /// Every typedef's member ranges must sit inside their tables.
    fn check_ranges(&self) -> Result<()> {
        let fits = |start: i32, count: u16, len: usize| {
            count == 0 || (start >= 0 && start as usize + count as usize <= len)
        };
        for (k, t) in self.type_defs.iter().enumerate() {
            if !fits(t.field_start, t.field_count, self.fields.len())
                || !fits(t.method_start, t.method_count, self.methods.len())
                || !fits(t.property_start, t.property_count, self.properties.len())
                || !fits(
                    t.interfaces_start,
                    t.interfaces_count,
                    self.interfaces.len(),
                )
            {
                return Err(Error::structural(format!(
                    "typedef {k} member ranges fall outside their tables (row layout changed?)"
                )));
            }
        }
        for (k, m) in self.methods.iter().enumerate() {
            let count = m.parameter_count;
            if count > 0 && !fits(m.parameter_start, count, self.parameters.len()) {
                return Err(Error::structural(format!(
                    "method {k} parameters fall outside their table (row layout changed?)"
                )));
            }
        }
        for (k, img) in self.images.iter().enumerate() {
            if (img.custom_attribute_start + img.custom_attribute_count) as usize
                > self.attribute_ranges.len()
            {
                return Err(Error::structural(format!(
                    "image {k} attribute ranges fall outside attributeDataRange"
                )));
            }
        }
        Ok(())
    }

    /// A NUL-terminated string from the string table.
    pub fn string(&self, index: u32) -> &'a str {
        let s = self.sections[S_STRING];
        let i = index as usize;
        if i >= s.size {
            return "";
        }
        std::str::from_utf8(c_str(&self.data[..s.offset + s.size], s.offset + i)).unwrap_or("")
    }

    /// The default-value data blob from `offset` on.
    pub fn default_value_data(&self, offset: u32) -> &'a [u8] {
        let s = self.sections[S_DEFAULT_VALUE_DATA];
        let o = (offset as usize).min(s.size);
        &self.data[s.offset + o..s.offset + s.size]
    }

    /// Attribute blob `[start, end)` of the attribute-data section.
    pub fn attribute_data(&self, start: u32, end: Option<u32>) -> &'a [u8] {
        let s = self.sections[S_ATTRIBUTE_DATA];
        let a = (start as usize).min(s.size);
        let b = end.map_or(s.size, |e| (e as usize).min(s.size)).max(a);
        &self.data[s.offset + a..s.offset + b]
    }
}

/// A little-endian cursor over a metadata blob, with IL2CPP's compressed integer encoding.
pub struct Blob<'a> {
    d: &'a [u8],
    pub pos: usize,
}

impl<'a> Blob<'a> {
    pub const fn new(d: &'a [u8]) -> Self {
        Self { d, pos: 0 }
    }

    pub fn bytes(&mut self, n: usize) -> Result<&'a [u8]> {
        if self.pos + n > self.d.len() {
            return Err(Error::malformed(format!(
                "blob read of {n} bytes at {} past its end {}",
                self.pos,
                self.d.len()
            )));
        }
        let s = &self.d[self.pos..self.pos + n];
        self.pos += n;
        Ok(s)
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N]> {
        let b = self.bytes(N)?;
        Ok(b.try_into().unwrap_or([0; N]))
    }

    pub fn u8(&mut self) -> Result<u8> {
        Ok(self.bytes(1)?[0])
    }

    pub fn u16(&mut self) -> Result<u16> {
        self.array().map(u16::from_le_bytes)
    }

    pub fn u32(&mut self) -> Result<u32> {
        self.array().map(u32::from_le_bytes)
    }

    pub fn u64(&mut self) -> Result<u64> {
        self.array().map(u64::from_le_bytes)
    }

    pub fn f32(&mut self) -> Result<f32> {
        self.array().map(f32::from_le_bytes)
    }

    pub fn f64(&mut self) -> Result<f64> {
        self.array().map(f64::from_le_bytes)
    }

    /// `ReadCompressedUInt32`: 1, 2 or 4 bytes big-endian by prefix bits, `0xF0` + raw u32,
    /// `0xFE`/`0xFF` for `u32::MAX - 1`/`u32::MAX`.
    pub fn compressed_u32(&mut self) -> Result<u32> {
        let b = self.u8()?;
        Ok(if b & 0x80 == 0 {
            u32::from(b)
        } else if b & 0xC0 == 0x80 {
            (u32::from(b & 0x3F) << 8) | u32::from(self.u8()?)
        } else if b & 0xE0 == 0xC0 {
            let r = self.bytes(3)?;
            (u32::from(b & 0x1F) << 24)
                | (u32::from(r[0]) << 16)
                | (u32::from(r[1]) << 8)
                | u32::from(r[2])
        } else if b == 0xF0 {
            self.u32()?
        } else if b == 0xFE {
            u32::MAX - 1
        } else if b == 0xFF {
            u32::MAX
        } else {
            return Err(Error::malformed(format!(
                "bad compressed integer prefix {b:#04x}"
            )));
        })
    }

    /// `ReadCompressedInt32`: zigzag-like, the low bit is the sign.
    pub fn compressed_i32(&mut self) -> Result<i32> {
        let e = self.compressed_u32()?;
        if e == u32::MAX {
            return Ok(i32::MIN);
        }
        let v = (e >> 1) as i32;
        Ok(if e & 1 != 0 { -v - 1 } else { v })
    }
}
