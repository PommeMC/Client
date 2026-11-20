//! A value's `CODEC` form and its hash.
//!
//! [`Value`] is what a vanilla `CODEC` writes through any `DynamicOps`: the
//! saved-data shape of a component, with registry entries as names. Its
//! [`Value::hash`] is `HashOps`' over `Hashing.crc32c()`, whose hasher takes
//! numbers little-endian, which is how a click tells the server what the
//! client thinks each stack holds (`HashedPatchMap`).

use pomme_registry::Identifier;

const TAG_EMPTY: u8 = 1;
const TAG_MAP_START: u8 = 2;
const TAG_MAP_END: u8 = 3;
const TAG_LIST_START: u8 = 4;
const TAG_LIST_END: u8 = 5;
const TAG_BYTE: u8 = 6;
const TAG_SHORT: u8 = 7;
const TAG_INT: u8 = 8;
const TAG_LONG: u8 = 9;
const TAG_FLOAT: u8 = 10;
const TAG_DOUBLE: u8 = 11;
const TAG_STRING: u8 = 12;
const TAG_BOOLEAN: u8 = 13;
const TAG_BYTE_ARRAY_START: u8 = 14;
const TAG_BYTE_ARRAY_END: u8 = 15;
const TAG_INT_ARRAY_START: u8 = 16;
const TAG_INT_ARRAY_END: u8 = 17;
const TAG_LONG_ARRAY_START: u8 = 18;
const TAG_LONG_ARRAY_END: u8 = 19;

/// A value's hash, Guava's `HashCode` of a CRC32C.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Hash(pub u32);

impl Hash {
    fn of(tag: u8, payload: &[u8]) -> Self {
        Self(crc32c::crc32c_append(crc32c::crc32c(&[tag]), payload))
    }

    fn between(start: u8, payload: &[u8], end: u8) -> Self {
        Self(crc32c::crc32c_append(Self::of(start, payload).0, &[end]))
    }

    /// `HashCode.asInt`, as a click sends it.
    pub fn as_i32(self) -> i32 {
        self.0 as i32
    }
}

/// What a `CODEC` writes, each variant the `DynamicOps` method that makes
/// it.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Empty,
    Bool(bool),
    Byte(i8),
    Short(i16),
    Int(i32),
    Long(i64),
    Float(f32),
    Double(f64),
    String(String),
    List(Vec<Value>),
    Map(Vec<(Value, Value)>),
    ByteList(Vec<u8>),
    IntList(Vec<i32>),
    LongList(Vec<i64>),
}

impl Value {
    pub fn string(value: &str) -> Self {
        Self::String(value.to_owned())
    }

    /// `Identifier.CODEC`: its string.
    pub fn identifier(id: &Identifier) -> Self {
        Self::String(id.to_string())
    }

    pub fn list(values: impl IntoIterator<Item = Self>) -> Self {
        Self::List(values.into_iter().collect())
    }

    /// A list whose values may have no form, which leaves it without one.
    pub fn try_list(values: impl IntoIterator<Item = Option<Self>>) -> Option<Self> {
        values.into_iter().collect::<Option<_>>().map(Self::List)
    }

    pub fn byte_list(values: &[u8]) -> Self {
        Self::ByteList(values.to_vec())
    }

    pub fn int_list(values: impl IntoIterator<Item = i32>) -> Self {
        Self::IntList(values.into_iter().collect())
    }

    pub fn long_list(values: impl IntoIterator<Item = i64>) -> Self {
        Self::LongList(values.into_iter().collect())
    }

    /// `HashOps`: each variant as the method that hashes it.
    pub fn hash(&self) -> Hash {
        fn bytes(hashes: impl Iterator<Item = Hash>) -> Vec<u8> {
            hashes.flat_map(|hash| hash.0.to_le_bytes()).collect()
        }
        match self {
            Self::Empty => Hash(crc32c::crc32c(&[TAG_EMPTY])),
            Self::Bool(value) => Hash::of(TAG_BOOLEAN, &[u8::from(*value)]),
            Self::Byte(value) => Hash::of(TAG_BYTE, &value.to_le_bytes()),
            Self::Short(value) => Hash::of(TAG_SHORT, &value.to_le_bytes()),
            Self::Int(value) => Hash::of(TAG_INT, &value.to_le_bytes()),
            Self::Long(value) => Hash::of(TAG_LONG, &value.to_le_bytes()),
            Self::Float(value) => Hash::of(TAG_FLOAT, &value.to_le_bytes()),
            Self::Double(value) => Hash::of(TAG_DOUBLE, &value.to_le_bytes()),
            // `createString`: the length in UTF-16 units, then each unit.
            Self::String(value) => {
                let units: Vec<u16> = value.encode_utf16().collect();
                let mut payload = (units.len() as i32).to_le_bytes().to_vec();
                payload.extend(units.iter().flat_map(|unit| unit.to_le_bytes()));
                Hash::of(TAG_STRING, &payload)
            }
            Self::List(values) => {
                let payload = bytes(values.iter().map(Self::hash));
                Hash::between(TAG_LIST_START, &payload, TAG_LIST_END)
            }
            // `createMap`: the entries ordered by key hash, then value hash.
            Self::Map(entries) => {
                let mut hashes: Vec<_> = entries
                    .iter()
                    .map(|(key, value)| (key.hash(), value.hash()))
                    .collect();
                hashes.sort();
                let payload = bytes(hashes.into_iter().flat_map(|(key, value)| [key, value]));
                Hash::between(TAG_MAP_START, &payload, TAG_MAP_END)
            }
            Self::ByteList(values) => {
                Hash::between(TAG_BYTE_ARRAY_START, values, TAG_BYTE_ARRAY_END)
            }
            Self::IntList(values) => {
                let payload: Vec<u8> = values.iter().flat_map(|v| v.to_le_bytes()).collect();
                Hash::between(TAG_INT_ARRAY_START, &payload, TAG_INT_ARRAY_END)
            }
            Self::LongList(values) => {
                let payload: Vec<u8> = values.iter().flat_map(|v| v.to_le_bytes()).collect();
                Hash::between(TAG_LONG_ARRAY_START, &payload, TAG_LONG_ARRAY_END)
            }
        }
    }
}

/// A `RecordCodecBuilder`'s map, a field at a time.
#[derive(Default)]
pub struct Record {
    entries: Vec<(Value, Value)>,
}

impl Record {
    pub fn new() -> Self {
        Self::default()
    }

    /// `fieldOf`.
    pub fn field(mut self, name: &str, value: Value) -> Self {
        self.entries.push((Value::string(name), value));
        self
    }

    /// `optionalFieldOf` without a default: left out when there is none.
    pub fn optional(self, name: &str, value: Option<Value>) -> Self {
        match value {
            Some(value) => self.field(name, value),
            None => self,
        }
    }

    /// `optionalFieldOf` with a default: left out when the value is it.
    pub fn unless(self, name: &str, is_default: bool, value: impl FnOnce() -> Value) -> Self {
        if is_default {
            self
        } else {
            self.field(name, value())
        }
    }

    pub fn flag(self, name: &str, value: bool, default: bool) -> Self {
        self.unless(name, value == default, || Value::Bool(value))
    }

    pub fn int(self, name: &str, value: i32, default: i32) -> Self {
        self.unless(name, value == default, || Value::Int(value))
    }

    /// `Float.equals`, which tells the two zeros apart.
    pub fn float(self, name: &str, value: f32, default: f32) -> Self {
        self.unless(name, value.to_bits() == default.to_bits(), || {
            Value::Float(value)
        })
    }

    /// A `MapCodec`'s fields, which sit beside this record's own.
    pub fn merge(mut self, other: Self) -> Self {
        self.entries.extend(other.entries);
        self
    }

    pub fn build(self) -> Value {
        Value::Map(self.entries)
    }
}

/// The names of entries in the registries a server sends, which a value's
/// `CODEC` form holds in place of their ids.
pub trait Names {
    /// The entry numbered `id` in the registry at `registry`, like
    /// `enchantment`.
    fn name(&self, registry: &str, id: u32) -> Option<Identifier>;
}
