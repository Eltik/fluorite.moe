/// SparkBuffer binary table format parser.
/// Converts .bytes SparkBuffer files to JSON.
/// Based on BydTools (github.com/SherkeyXD/BydTools) SparkBuffer implementation.
use serde_json::{Map, Number, Value};
use std::collections::HashMap;
use std::io::{self, Cursor, Read, Seek, SeekFrom};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
enum SparkType {
    Bool = 0,
    Byte = 1,
    Int = 2,
    Long = 3,
    Float = 4,
    Double = 5,
    Enum = 6,
    String = 7,
    Bean = 8,
    Array = 9,
    Map = 10,
}

impl SparkType {
    fn from_u8(v: u8) -> io::Result<Self> {
        match v {
            0 => Ok(Self::Bool),
            1 => Ok(Self::Byte),
            2 => Ok(Self::Int),
            3 => Ok(Self::Long),
            4 => Ok(Self::Float),
            5 => Ok(Self::Double),
            6 => Ok(Self::Enum),
            7 => Ok(Self::String),
            8 => Ok(Self::Bean),
            9 => Ok(Self::Array),
            10 => Ok(Self::Map),
            _ => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("unknown SparkType {v}"),
            )),
        }
    }

    fn is_enum_or_bean(self) -> bool {
        matches!(self, Self::Enum | Self::Bean)
    }
}

#[derive(Debug, Clone)]
struct EnumItem {
    name: String,
    value: i32,
}

#[derive(Debug, Clone)]
struct EnumType {
    type_hash: i32,
    #[allow(dead_code)]
    name: String,
    enums: Vec<EnumItem>,
}

#[derive(Debug, Clone)]
struct BeanField {
    name: String,
    typ: SparkType,
    type2: Option<SparkType>,
    type3: Option<SparkType>,
    type_hash: Option<i32>,
    type_hash2: Option<i32>,
}

#[derive(Debug, Clone)]
struct BeanType {
    type_hash: i32,
    #[allow(dead_code)]
    name: String,
    fields: Vec<BeanField>,
}

struct SparkReader<'a> {
    cursor: Cursor<&'a [u8]>,
    bean_types: HashMap<i32, BeanType>,
    enum_types: HashMap<i32, EnumType>,
}

impl<'a> SparkReader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self {
            cursor: Cursor::new(data),
            bean_types: HashMap::new(),
            enum_types: HashMap::new(),
        }
    }

    fn pos(&self) -> u64 {
        self.cursor.position()
    }

    fn seek_to(&mut self, pos: u64) {
        self.cursor.set_position(pos);
    }

    fn read_u8(&mut self) -> io::Result<u8> {
        let mut buf = [0u8; 1];
        self.cursor.read_exact(&mut buf)?;
        Ok(buf[0])
    }

    fn read_i32(&mut self) -> io::Result<i32> {
        let mut buf = [0u8; 4];
        self.cursor.read_exact(&mut buf)?;
        Ok(i32::from_le_bytes(buf))
    }

    fn read_i64(&mut self) -> io::Result<i64> {
        let mut buf = [0u8; 8];
        self.cursor.read_exact(&mut buf)?;
        Ok(i64::from_le_bytes(buf))
    }

    fn read_f32(&mut self) -> io::Result<f32> {
        let mut buf = [0u8; 4];
        self.cursor.read_exact(&mut buf)?;
        Ok(f32::from_le_bytes(buf))
    }

    fn read_f64(&mut self) -> io::Result<f64> {
        let mut buf = [0u8; 8];
        self.cursor.read_exact(&mut buf)?;
        Ok(f64::from_le_bytes(buf))
    }

    fn read_bool(&mut self) -> io::Result<bool> {
        Ok(self.read_u8()? != 0)
    }

    fn read_spark_type(&mut self) -> io::Result<SparkType> {
        SparkType::from_u8(self.read_u8()?)
    }

    // Null-terminated UTF-8 string
    fn read_string(&mut self) -> io::Result<String> {
        let mut bytes = Vec::new();
        loop {
            let b = self.read_u8()?;
            if b == 0 {
                break;
            }
            bytes.push(b);
        }
        String::from_utf8(bytes).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
    }

    // String via i32 offset indirection
    fn read_string_offset(&mut self) -> io::Result<String> {
        let offset = self.read_i32()?;
        if offset == -1 {
            return Ok(String::new());
        }
        let saved = self.pos();
        self.seek_to(offset as u64);
        let s = self.read_string()?;
        self.seek_to(saved);
        Ok(s)
    }

    fn align4(&mut self) {
        let p = self.pos();
        if p == 0 {
            return;
        }
        let aligned = (p - 1) + (4 - ((p - 1) % 4));
        self.seek_to(aligned);
    }

    fn align8(&mut self) {
        let p = self.pos();
        if p == 0 {
            return;
        }
        let aligned = (p - 1) + (8 - ((p - 1) % 8));
        self.seek_to(aligned);
    }

    fn read_long_aligned(&mut self) -> io::Result<i64> {
        self.align8();
        self.read_i64()
    }

    fn read_double_aligned(&mut self) -> io::Result<f64> {
        self.align8();
        self.read_f64()
    }

    fn read_enum_value(&mut self, type_hash: i32) -> io::Result<Value> {
        let val = self.read_i32()?;
        if let Some(et) = self.enum_types.get(&type_hash)
            && let Some(item) = et.enums.iter().find(|e| e.value == val)
        {
            return Ok(Value::String(item.name.clone()));
        }
        Ok(Value::Number(Number::from(val)))
    }

    fn read_type_definitions(&mut self) -> io::Result<()> {
        let count = self.read_i32()?;
        for _ in 0..count {
            let spark_type = self.read_spark_type()?;
            self.align4();
            match spark_type {
                SparkType::Enum => {
                    let et = self.read_enum_type()?;
                    self.enum_types.insert(et.type_hash, et);
                }
                SparkType::Bean => {
                    let bt = self.read_bean_type()?;
                    self.bean_types.insert(bt.type_hash, bt);
                }
                _ => {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!("unexpected type {spark_type:?} in type definitions"),
                    ));
                }
            }
        }
        Ok(())
    }

    fn read_enum_type(&mut self) -> io::Result<EnumType> {
        let type_hash = self.read_i32()?;
        let name = self.read_string()?;
        self.align4();
        let count = self.read_i32()?;
        let mut enums = Vec::with_capacity(count as usize);
        for _ in 0..count {
            let item_name = self.read_string()?;
            self.align4();
            let value = self.read_i32()?;
            enums.push(EnumItem {
                name: item_name,
                value,
            });
        }
        Ok(EnumType {
            type_hash,
            name,
            enums,
        })
    }

    fn read_bean_type(&mut self) -> io::Result<BeanType> {
        let type_hash = self.read_i32()?;
        let name = self.read_string()?;
        self.align4();
        let field_count = self.read_i32()?;
        let mut fields = Vec::with_capacity(field_count as usize);
        for _ in 0..field_count {
            let field_name = self.read_string()?;
            let typ = self.read_spark_type()?;
            let mut type2 = None;
            let mut type3 = None;
            let mut type_hash = None;
            let mut type_hash2 = None;

            match typ {
                SparkType::Enum | SparkType::Bean => {
                    self.align4();
                    type_hash = Some(self.read_i32()?);
                }
                SparkType::Array => {
                    type2 = Some(self.read_spark_type()?);
                    if type2.unwrap().is_enum_or_bean() {
                        self.align4();
                        type_hash = Some(self.read_i32()?);
                    }
                }
                SparkType::Map => {
                    type2 = Some(self.read_spark_type()?);
                    type3 = Some(self.read_spark_type()?);
                    if type2.unwrap().is_enum_or_bean() {
                        self.align4();
                        type_hash = Some(self.read_i32()?);
                    }
                    if type3.unwrap().is_enum_or_bean() {
                        self.align4();
                        type_hash2 = Some(self.read_i32()?);
                    }
                }
                _ => {}
            }

            fields.push(BeanField {
                name: field_name,
                typ,
                type2,
                type3,
                type_hash,
                type_hash2,
            });
        }
        Ok(BeanType {
            type_hash,
            name,
            fields,
        })
    }

    fn read_bean_value(&mut self, type_hash: i32, pointer: bool) -> io::Result<Value> {
        let mut pointer_origin = None;
        if pointer {
            let offset = self.read_i32()?;
            if offset == -1 {
                return Ok(Value::Null);
            }
            pointer_origin = Some(self.pos());
            self.seek_to(offset as u64);
        }

        let bean = self
            .bean_types
            .get(&type_hash)
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("unknown bean type hash {type_hash}"),
                )
            })?
            .clone();

        let mut obj = Map::new();
        let field_count = bean.fields.len();

        for (fi, field) in bean.fields.iter().enumerate() {
            let mut array_origin = None;
            if field.typ == SparkType::Array {
                let offset = self.read_i32()?;
                if offset == -1 {
                    obj.insert(field.name.clone(), Value::Null);
                    continue;
                }
                array_origin = Some(self.pos());
                self.seek_to(offset as u64);
            }

            let value = match field.typ {
                SparkType::Int => Value::Number(Number::from(self.read_i32()?)),
                SparkType::Enum => self.read_enum_value(field.type_hash.unwrap_or(0))?,
                SparkType::Long => {
                    let v = self.read_long_aligned()?;
                    Value::Number(Number::from(v))
                }
                SparkType::Float => {
                    let v = self.read_f32()?;
                    json_number_f64(v as f64)
                }
                SparkType::Double => {
                    let v = self.read_double_aligned()?;
                    json_number_f64(v)
                }
                SparkType::String => Value::String(self.read_string_offset()?),
                SparkType::Bean => self.read_bean_value(field.type_hash.unwrap_or(0), true)?,
                SparkType::Bool => {
                    let v = self.read_bool()?;
                    // Align after bool if next field is not bool
                    if fi + 1 < field_count && bean.fields[fi + 1].typ != SparkType::Bool {
                        self.align4();
                    }
                    Value::Bool(v)
                }
                SparkType::Array => {
                    let item_count = self.read_i32()?;
                    let mut arr = Vec::with_capacity(item_count as usize);
                    let item_type = field.type2.unwrap_or(SparkType::Int);
                    for _ in 0..item_count {
                        let item = match item_type {
                            SparkType::String => Value::String(self.read_string_offset()?),
                            SparkType::Bean => {
                                self.read_bean_value(field.type_hash.unwrap_or(0), true)?
                            }
                            SparkType::Float => json_number_f64(self.read_f32()? as f64),
                            SparkType::Long => {
                                Value::Number(Number::from(self.read_long_aligned()?))
                            }
                            SparkType::Int | SparkType::Enum => {
                                Value::Number(Number::from(self.read_i32()?))
                            }
                            SparkType::Bool => Value::Bool(self.read_bool()?),
                            SparkType::Double => json_number_f64(self.read_double_aligned()?),
                            other => {
                                return Err(io::Error::new(
                                    io::ErrorKind::InvalidData,
                                    format!(
                                        "unsupported array element type {other:?} at pos {}",
                                        self.pos()
                                    ),
                                ));
                            }
                        };
                        arr.push(item);
                    }
                    Value::Array(arr)
                }
                SparkType::Map => {
                    let offset = self.read_i32()?;
                    let map_origin = self.pos();
                    self.seek_to(offset as u64);
                    let v = self.read_map_value(field)?;
                    self.seek_to(map_origin);
                    v
                }
                SparkType::Byte => {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!("byte type not supported at pos {}", self.pos()),
                    ));
                }
            };

            obj.insert(field.name.clone(), value);

            if let Some(origin) = array_origin {
                self.seek_to(origin);
            }
        }

        if let Some(origin) = pointer_origin {
            self.seek_to(origin);
        }

        Ok(Value::Object(obj))
    }

    fn read_map_value(&mut self, field: &BeanField) -> io::Result<Value> {
        let kv_count = self.read_i32()?;
        // Skip index table (kv_count * 8 bytes)
        let skip = kv_count as i64 * 8;
        self.cursor.seek(SeekFrom::Current(skip))?;

        let key_type = field.type2.unwrap_or(SparkType::String);
        let val_type = field.type3.unwrap_or(SparkType::String);

        let mut obj = Map::new();
        for _ in 0..kv_count {
            let key = match key_type {
                SparkType::String => self.read_string_offset()?,
                SparkType::Int => self.read_i32()?.to_string(),
                SparkType::Long => self.read_long_aligned()?.to_string(),
                other => {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!("unsupported map key type {other:?}"),
                    ));
                }
            };
            let value = match val_type {
                SparkType::Bean => self.read_bean_value(field.type_hash2.unwrap_or(0), true)?,
                SparkType::String => Value::String(self.read_string_offset()?),
                SparkType::Int => Value::Number(Number::from(self.read_i32()?)),
                SparkType::Float => json_number_f64(self.read_f32()? as f64),
                SparkType::Enum => self.read_enum_value(field.type_hash2.unwrap_or(0))?,
                SparkType::Bool => {
                    let v = self.read_bool()?;
                    self.align4();
                    Value::Bool(v)
                }
                other => {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!("unsupported map value type {other:?}"),
                    ));
                }
            };
            obj.insert(key, value);
        }

        Ok(Value::Object(obj))
    }

    fn parse(mut self) -> io::Result<Value> {
        let type_def_offset = self.read_i32()?;
        let root_def_offset = self.read_i32()?;
        let data_offset = self.read_i32()?;

        self.seek_to(type_def_offset as u64);
        self.read_type_definitions()?;

        self.seek_to(root_def_offset as u64);
        let root_type = self.read_spark_type()?;
        let _root_name = self.read_string()?;

        let mut root_type_hash = None;
        let mut root_type2 = None;
        let mut root_type3 = None;
        let mut root_type_hash2 = None;

        if root_type.is_enum_or_bean() {
            self.align4();
            root_type_hash = Some(self.read_i32()?);
        }
        if root_type == SparkType::Map {
            root_type2 = Some(self.read_spark_type()?);
            root_type3 = Some(self.read_spark_type()?);
            if root_type2.unwrap().is_enum_or_bean() {
                self.align4();
                root_type_hash = Some(self.read_i32()?);
            }
            if root_type3.unwrap().is_enum_or_bean() {
                self.align4();
                root_type_hash2 = Some(self.read_i32()?);
            }
        }

        self.seek_to(data_offset as u64);
        match root_type {
            SparkType::Bean => self.read_bean_value(root_type_hash.unwrap_or(0), false),
            SparkType::Map => {
                let field = BeanField {
                    name: String::new(),
                    typ: SparkType::Map,
                    type2: root_type2,
                    type3: root_type3,
                    type_hash: root_type_hash,
                    type_hash2: root_type_hash2,
                };
                self.read_map_value(&field)
            }
            other => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("unsupported root type {other:?}"),
            )),
        }
    }
}

fn json_number_f64(v: f64) -> Value {
    Number::from_f64(v).map_or(Value::Null, Value::Number)
}

/// Parse a SparkBuffer .bytes file and return JSON.
pub fn parse_sparkbuffer(data: &[u8]) -> io::Result<Value> {
    if data.len() < 12 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "file too small for SparkBuffer",
        ));
    }
    SparkReader::new(data).parse()
}
