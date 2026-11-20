//! Vanilla `HashOps`: the hash a value's `CODEC` form comes to, which is how
//! a click tells the server what the client thinks each stack holds
//! (`HashedPatchMap`). Each builder here is the `HashOps` method it names,
//! over `Hashing.crc32c()`, whose hasher takes numbers little-endian.

use pomme_registry::Identifier;

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

    pub fn bool(value: bool) -> Self {
        Self::of(TAG_BOOLEAN, &[u8::from(value)])
    }

    pub fn byte(value: i8) -> Self {
        Self::of(TAG_BYTE, &value.to_le_bytes())
    }

    pub fn short(value: i16) -> Self {
        Self::of(TAG_SHORT, &value.to_le_bytes())
    }

    pub fn int(value: i32) -> Self {
        Self::of(TAG_INT, &value.to_le_bytes())
    }

    pub fn long(value: i64) -> Self {
        Self::of(TAG_LONG, &value.to_le_bytes())
    }

    pub fn float(value: f32) -> Self {
        Self::of(TAG_FLOAT, &value.to_le_bytes())
    }

    pub fn double(value: f64) -> Self {
        Self::of(TAG_DOUBLE, &value.to_le_bytes())
    }

    /// `createString`: the length in UTF-16 units, then each unit.
    pub fn string(value: &str) -> Self {
        let units: Vec<u16> = value.encode_utf16().collect();
        let mut payload = (units.len() as i32).to_le_bytes().to_vec();
        payload.extend(units.iter().flat_map(|unit| unit.to_le_bytes()));
        Self::of(TAG_STRING, &payload)
    }

    /// `Identifier.CODEC`: its string.
    pub fn identifier(id: &Identifier) -> Self {
        Self::string(&id.to_string())
    }

    pub fn list(values: impl IntoIterator<Item = Self>) -> Self {
        let payload: Vec<u8> = values.into_iter().flat_map(|v| v.0.to_le_bytes()).collect();
        Self::between(TAG_LIST_START, &payload, TAG_LIST_END)
    }

    /// A list whose values may have no hash, which leaves it without one.
    pub fn try_list(values: impl IntoIterator<Item = Option<Self>>) -> Option<Self> {
        let values: Option<Vec<_>> = values.into_iter().collect();
        values.map(Self::list)
    }

    /// `createMap`: the entries ordered by key hash, then value hash.
    pub fn map(mut entries: Vec<(Self, Self)>) -> Self {
        entries.sort();
        let payload: Vec<u8> = entries
            .iter()
            .flat_map(|(key, value)| [key.0.to_le_bytes(), value.0.to_le_bytes()])
            .flatten()
            .collect();
        Self::between(TAG_MAP_START, &payload, TAG_MAP_END)
    }

    pub fn byte_list(values: &[u8]) -> Self {
        Self::between(TAG_BYTE_ARRAY_START, values, TAG_BYTE_ARRAY_END)
    }

    pub fn int_list(values: impl IntoIterator<Item = i32>) -> Self {
        let payload: Vec<u8> = values.into_iter().flat_map(i32::to_le_bytes).collect();
        Self::between(TAG_INT_ARRAY_START, &payload, TAG_INT_ARRAY_END)
    }

    pub fn long_list(values: impl IntoIterator<Item = i64>) -> Self {
        let payload: Vec<u8> = values.into_iter().flat_map(i64::to_le_bytes).collect();
        Self::between(TAG_LONG_ARRAY_START, &payload, TAG_LONG_ARRAY_END)
    }
}

/// A `RecordCodecBuilder`'s map, a field at a time.
#[derive(Default)]
pub struct Record {
    entries: Vec<(Hash, Hash)>,
}

impl Record {
    pub fn new() -> Self {
        Self::default()
    }

    /// `fieldOf`.
    pub fn field(mut self, name: &str, value: Hash) -> Self {
        self.entries.push((Hash::string(name), value));
        self
    }

    /// `optionalFieldOf` without a default: left out when there is none.
    pub fn optional(self, name: &str, value: Option<Hash>) -> Self {
        match value {
            Some(value) => self.field(name, value),
            None => self,
        }
    }

    /// `optionalFieldOf` with a default: left out when the value is it.
    pub fn unless(self, name: &str, is_default: bool, value: impl FnOnce() -> Hash) -> Self {
        if is_default {
            self
        } else {
            self.field(name, value())
        }
    }

    pub fn flag(self, name: &str, value: bool, default: bool) -> Self {
        self.unless(name, value == default, || Hash::bool(value))
    }

    pub fn int(self, name: &str, value: i32, default: i32) -> Self {
        self.unless(name, value == default, || Hash::int(value))
    }

    /// `Float.equals`, which tells the two zeros apart.
    pub fn float(self, name: &str, value: f32, default: f32) -> Self {
        self.unless(name, value.to_bits() == default.to_bits(), || {
            Hash::float(value)
        })
    }

    /// A `MapCodec`'s fields, which sit beside this record's own.
    pub fn merge(mut self, other: Self) -> Self {
        self.entries.extend(other.entries);
        self
    }

    pub fn build(self) -> Hash {
        Hash::map(self.entries)
    }
}

/// The names of entries in the registries a server sends, which a value's
/// `CODEC` form holds in place of their ids.
pub trait Names {
    /// The entry numbered `id` in the registry at `registry`, like
    /// `enchantment`.
    fn name(&self, registry: &str, id: u32) -> Option<Identifier>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_negative_zero_is_not_the_default_zero() {
        let field = |value| Record::new().float("speed", value, 0.0).build();
        let empty = Record::new().build();
        assert_eq!(field(0.0), empty);
        assert_ne!(field(-0.0), empty);
    }
}
