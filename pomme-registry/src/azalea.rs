//! Azalea's traits for these types: its wire trait, and the serde and NBT
//! forms its crates hash and parse them with. The azalea crates still in the
//! tree (and `pomme-inventory`, until it is rewritten) read registries
//! through these; pomme's own code doesn't. Goes when they do.

use std::io::{self, Cursor, Write};

use azalea_buf::{AzBuf, AzBufVar, BufReadError};
use serde::{Deserialize, Deserializer, Serialize, Serializer, de};
use simdnbt::borrow::NbtTag;
use simdnbt::{FromNbtTag, ToNbtTag};

use crate::{Holder, HolderSet, Identifier, Registry};

/// A registry id past the registry's end.
pub(crate) fn unknown_id(id: u32) -> BufReadError {
    BufReadError::UnexpectedEnumVariant { id: id as i32 }
}

macro_rules! builtin_registry {
    ($name:ident) => {
        impl azalea_buf::AzBuf for $name {
            fn azalea_read(
                buf: &mut std::io::Cursor<&[u8]>,
            ) -> Result<Self, azalea_buf::BufReadError> {
                let id = <u32 as azalea_buf::AzBufVar>::azalea_read_var(buf)?;
                $crate::Registry::from_u32(id).ok_or_else(|| $crate::azalea::unknown_id(id))
            }

            fn azalea_write(&self, buf: &mut impl std::io::Write) -> std::io::Result<()> {
                azalea_buf::AzBufVar::azalea_write_var(&(*self as u32), buf)
            }
        }

        impl serde::Serialize for $name {
            fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                serializer.serialize_str(self.to_str())
            }
        }

        impl<'de> serde::Deserialize<'de> for $name {
            fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                let name = String::deserialize(deserializer)?;
                name.parse().map_err(|()| {
                    serde::de::Error::custom(format!(
                        "{name:?} is not a valid {}",
                        stringify!($name)
                    ))
                })
            }
        }

        impl simdnbt::FromNbtTag for $name {
            fn from_nbt_tag(tag: simdnbt::borrow::NbtTag) -> Option<Self> {
                tag.string()?.to_str().parse().ok()
            }
        }

        impl simdnbt::ToNbtTag for $name {
            fn to_nbt_tag(self) -> simdnbt::owned::NbtTag {
                simdnbt::owned::NbtTag::String(self.to_str().into())
            }
        }
    };
}
pub(crate) use builtin_registry;

macro_rules! data_registry {
    ($registry:ident, $name:literal, $key:ident) => {
        impl azalea_buf::AzBuf for $registry {
            fn azalea_read(
                buf: &mut std::io::Cursor<&[u8]>,
            ) -> Result<Self, azalea_buf::BufReadError> {
                <u32 as azalea_buf::AzBufVar>::azalea_read_var(buf).map(|id| Self { id })
            }

            fn azalea_write(&self, buf: &mut impl std::io::Write) -> std::io::Result<()> {
                azalea_buf::AzBufVar::azalea_write_var(&self.id, buf)
            }
        }

        // The form azalea-core's `ChecksumSerializer` resolves to the
        // entry's name before hashing (`serialize_newtype_variant`).
        impl serde::Serialize for $registry {
            fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                serializer.serialize_newtype_variant(concat!("minecraft:", $name), self.id, "", &())
            }
        }

        impl simdnbt::FromNbtTag for $key {
            fn from_nbt_tag(tag: simdnbt::borrow::NbtTag) -> Option<Self> {
                <$crate::Identifier as simdnbt::FromNbtTag>::from_nbt_tag(tag).map(Self::from)
            }
        }
    };
}
pub(crate) use data_registry;

impl AzBuf for Identifier {
    fn azalea_read(buf: &mut Cursor<&[u8]>) -> Result<Self, BufReadError> {
        String::azalea_read(buf).map(Identifier::new)
    }

    fn azalea_write(&self, buf: &mut impl Write) -> io::Result<()> {
        self.to_string().azalea_write(buf)
    }
}

impl Serialize for Identifier {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

/// Only with its namespace written out.
impl<'de> Deserialize<'de> for Identifier {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        if !text.contains(':') {
            return Err(de::Error::invalid_value(
                de::Unexpected::Str(&text),
                &"a namespaced identifier",
            ));
        }
        Ok(Identifier::new(text))
    }
}

impl FromNbtTag for Identifier {
    fn from_nbt_tag(tag: NbtTag) -> Option<Self> {
        Some(Identifier::new(tag.string()?.to_str()))
    }
}

impl ToNbtTag for Identifier {
    fn to_nbt_tag(self) -> simdnbt::owned::NbtTag {
        simdnbt::owned::NbtTag::String(self.to_string().into())
    }
}

/// `ByteBufCodecs.holder`: the entry's id plus one, or zero and the value.
impl<R: Registry, Direct: AzBuf> AzBuf for Holder<R, Direct> {
    fn azalea_read(buf: &mut Cursor<&[u8]>) -> Result<Self, BufReadError> {
        match u32::azalea_read_var(buf)? {
            0 => Direct::azalea_read(buf).map(Self::Direct),
            id => R::from_u32(id - 1)
                .map(Self::Reference)
                .ok_or_else(|| unknown_id(id - 1)),
        }
    }

    fn azalea_write(&self, buf: &mut impl Write) -> io::Result<()> {
        match self {
            Self::Reference(entry) => (entry.to_u32() + 1).azalea_write_var(buf),
            Self::Direct(value) => {
                0u32.azalea_write_var(buf)?;
                value.azalea_write(buf)
            }
        }
    }
}

impl<R: Serialize, Direct: Serialize> Serialize for Holder<R, Direct> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Reference(entry) => entry.serialize(serializer),
            Self::Direct(value) => value.serialize(serializer),
        }
    }
}

impl<R: FromNbtTag, Direct: FromNbtTag> FromNbtTag for Holder<R, Direct> {
    fn from_nbt_tag(tag: NbtTag) -> Option<Self> {
        R::from_nbt_tag(tag)
            .map(Self::Reference)
            .or_else(|| Direct::from_nbt_tag(tag).map(Self::Direct))
    }
}

/// `ByteBufCodecs.holderSet`: the entry count plus one then the entries, or
/// zero and the tag's key.
impl<R: Registry + AzBuf, Key: AzBuf> AzBuf for HolderSet<R, Key> {
    fn azalea_read(buf: &mut Cursor<&[u8]>) -> Result<Self, BufReadError> {
        match u32::azalea_read_var(buf)? {
            0 => Ok(Self::Named {
                key: Key::azalea_read(buf)?,
                contents: Vec::new(),
            }),
            count => (1..count)
                .map(|_| R::azalea_read(buf))
                .collect::<Result<_, _>>()
                .map(|contents| Self::Direct { contents }),
        }
    }

    fn azalea_write(&self, buf: &mut impl Write) -> io::Result<()> {
        match self {
            Self::Direct { contents } => {
                (contents.len() as u32 + 1).azalea_write_var(buf)?;
                contents
                    .iter()
                    .try_for_each(|entry| entry.azalea_write(buf))
            }
            Self::Named { key, .. } => {
                0u32.azalea_write_var(buf)?;
                key.azalea_write(buf)
            }
        }
    }
}

/// A lone entry as itself, as vanilla's codec writes a one-element set.
impl<R: Serialize, Key: Serialize> Serialize for HolderSet<R, Key> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Direct { contents } => match contents.as_slice() {
                [entry] => entry.serialize(serializer),
                entries => entries.serialize(serializer),
            },
            Self::Named { key, .. } => key.serialize(serializer),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builtin::ItemKind;
    use crate::data::Biome;

    fn written(value: &impl AzBuf) -> Vec<u8> {
        let mut buf = Vec::new();
        value.azalea_write(&mut buf).unwrap();
        buf
    }

    fn read<T: AzBuf>(bytes: &[u8]) -> Result<T, BufReadError> {
        T::azalea_read(&mut Cursor::new(bytes))
    }

    #[test]
    fn registries_are_their_id_as_a_varint() {
        assert_eq!(written(&ItemKind::Stone), [1]);
        assert_eq!(read::<ItemKind>(&[1]).unwrap(), ItemKind::Stone);
        assert!(read::<ItemKind>(&[0xff, 0xff, 0x7f]).is_err());
        assert_eq!(written(&Biome::from(300)), [0xac, 0x02]);
        assert_eq!(read::<Biome>(&[0xac, 0x02]).unwrap(), Biome::from(300));
    }

    #[test]
    fn an_identifier_is_its_full_text() {
        let stone = Identifier::new("stone");
        assert_eq!(written(&stone)[1..], *b"minecraft:stone");
        assert_eq!(read::<Identifier>(&written(&stone)).unwrap(), stone);
    }

    #[test]
    fn a_holder_is_an_id_plus_one_or_zero_and_the_value() {
        type Sound = Holder<ItemKind, Identifier>;
        assert_eq!(written(&Sound::Reference(ItemKind::Stone)), [2]);
        assert_eq!(
            read::<Sound>(&[2]).unwrap(),
            Holder::Reference(ItemKind::Stone)
        );
        let direct = Sound::Direct(Identifier::new("a:b"));
        assert_eq!(written(&direct), [0, 3, b'a', b':', b'b']);
        assert_eq!(read::<Sound>(&written(&direct)).unwrap(), direct);
    }

    #[test]
    fn a_holder_set_is_a_count_plus_one_or_zero_and_the_tag() {
        type Items = HolderSet<ItemKind, Identifier>;
        let direct = Items::from(vec![ItemKind::Stone, ItemKind::Air]);
        assert_eq!(written(&direct), [3, 1, 0]);
        assert_eq!(read::<Items>(&[3, 1, 0]).unwrap(), direct);
        assert_eq!(read::<Items>(&[1]).unwrap(), Items::default());
        let named = Items::Named {
            key: Identifier::new("a:b"),
            contents: Vec::new(),
        };
        assert_eq!(written(&named), [0, 3, b'a', b':', b'b']);
        assert_eq!(read::<Items>(&written(&named)).unwrap(), named);
    }
}
