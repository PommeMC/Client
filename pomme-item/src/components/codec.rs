//! The codecs components share, each ported from the `ByteBufCodecs` or
//! `STREAM_CODEC` it names.

use std::io::Cursor;

use pomme_protocol::codec::{DecodeResult, Reader};
use pomme_protocol::wire::{write_utf, write_varint};
use pomme_registry::builtin::SoundEvent;
use pomme_registry::{Holder, HolderSet, Identifier, Registry};
use pomme_text::Component as TextComponent;
use simdnbt::owned::NbtTag;

/// An enum sent by an `idMapper` over `ByIdMap.continuous(.., ZERO)`: its
/// variants with their ids, an id out of range reading as the one with id 0,
/// which is listed first.
macro_rules! id_enum {
    ($(#[$meta:meta])* $name:ident { $($variant:ident = $id:literal,)* }) => {
        $(#[$meta])*
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
        pub enum $name {
            $($variant,)*
        }

        impl $name {
            pub const ALL: &'static [Self] = &[$(Self::$variant,)*];

            pub fn from_id(id: u32) -> Self {
                match id {
                    $($id => Self::$variant,)*
                    _ => Self::ALL[0],
                }
            }

            pub fn id(self) -> u32 {
                match self {
                    $(Self::$variant => $id,)*
                }
            }

            pub(crate) fn read(
                r: &mut pomme_protocol::codec::Reader,
            ) -> pomme_protocol::codec::DecodeResult<Self> {
                r.varint().map(Self::from_id)
            }

            pub(crate) fn write(self, buf: &mut Vec<u8>) {
                pomme_protocol::wire::write_varint(buf, self.id());
            }
        }
    };
}
pub(crate) use id_enum;

/// `ByteBufCodecs.registry`: an entry by id.
pub(crate) fn registry<R: Registry>(r: &mut Reader) -> DecodeResult<R> {
    let id = r.varint()?;
    R::from_u32(id).ok_or_else(|| r.error("unknown registry entry"))
}

pub(crate) fn write_registry<R: Registry>(buf: &mut Vec<u8>, entry: &R) {
    write_varint(buf, entry.to_u32());
}

/// `Identifier.STREAM_CODEC`.
pub(crate) fn identifier(r: &mut Reader) -> DecodeResult<Identifier> {
    r.identifier().map(Identifier::new)
}

pub(crate) fn write_identifier(buf: &mut Vec<u8>, id: &Identifier) {
    write_utf(buf, &id.to_string());
}

pub(crate) fn string(r: &mut Reader) -> DecodeResult<String> {
    r.utf(pomme_protocol::codec::MAX_STRING_LENGTH)
        .map(str::to_owned)
}

pub(crate) fn write_bool(buf: &mut Vec<u8>, value: bool) {
    buf.push(u8::from(value));
}

pub(crate) fn write_f32(buf: &mut Vec<u8>, value: f32) {
    buf.extend_from_slice(&value.to_be_bytes());
}

pub(crate) fn write_i32(buf: &mut Vec<u8>, value: i32) {
    buf.extend_from_slice(&value.to_be_bytes());
}

pub(crate) fn write_var_i32(buf: &mut Vec<u8>, value: i32) {
    write_varint(buf, value as u32);
}

/// `ByteBufCodecs.optional`: whether there is a value, then the value.
pub(crate) fn write_optional<T>(
    buf: &mut Vec<u8>,
    value: &Option<T>,
    write: impl FnOnce(&mut Vec<u8>, &T),
) {
    write_bool(buf, value.is_some());
    if let Some(value) = value {
        write(buf, value);
    }
}

/// `ByteBufCodecs.collection`: a count, then each element.
pub(crate) fn write_list<T>(
    buf: &mut Vec<u8>,
    values: &[T],
    mut write: impl FnMut(&mut Vec<u8>, &T),
) {
    write_varint(buf, values.len() as u32);
    for value in values {
        write(buf, value);
    }
}

/// `ByteBufCodecs.holder`: zero and a value sent in the entry's place, else
/// the entry's id plus one.
pub(crate) fn holder<R: Registry, D>(
    r: &mut Reader,
    direct: impl FnOnce(&mut Reader) -> DecodeResult<D>,
) -> DecodeResult<Holder<R, D>> {
    match r.varint()? {
        0 => direct(r).map(Holder::Direct),
        id => R::from_u32(id - 1)
            .map(Holder::Reference)
            .ok_or_else(|| r.error("unknown registry entry")),
    }
}

pub(crate) fn write_holder<R: Registry, D>(
    buf: &mut Vec<u8>,
    holder: &Holder<R, D>,
    direct: impl FnOnce(&mut Vec<u8>, &D),
) {
    match holder {
        Holder::Reference(entry) => write_varint(buf, entry.to_u32() + 1),
        Holder::Direct(value) => {
            write_varint(buf, 0);
            direct(buf, value);
        }
    }
}

/// `ByteBufCodecs.holderSet`: zero and a tag's name, else a count plus one
/// and that many entries.
pub(crate) fn holder_set<R: Registry>(r: &mut Reader) -> DecodeResult<HolderSet<R, Identifier>> {
    match r.varint()? {
        0 => Ok(HolderSet::Named {
            key: identifier(r)?,
            contents: Vec::new(),
        }),
        n => {
            let n = r.len_of(n as usize - 1)?;
            let contents = (0..n).map(|_| registry(r)).collect::<DecodeResult<_>>()?;
            Ok(HolderSet::Direct { contents })
        }
    }
}

pub(crate) fn write_holder_set<R: Registry>(buf: &mut Vec<u8>, set: &HolderSet<R, Identifier>) {
    match set {
        HolderSet::Named { key, .. } => {
            write_varint(buf, 0);
            write_identifier(buf, key);
        }
        HolderSet::Direct { contents } => {
            write_varint(buf, contents.len() as u32 + 1);
            for entry in contents {
                write_registry(buf, entry);
            }
        }
    }
}

/// Vanilla `SoundEvent` when it isn't a registry entry: a sound file's
/// location and the range it carries, if fixed.
#[derive(Clone, Debug, PartialEq)]
pub struct CustomSound {
    pub location: Identifier,
    pub fixed_range: Option<f32>,
}

/// `SoundEvent.STREAM_CODEC`: a registered sound, or one sent in its place.
pub type Sound = Holder<SoundEvent, CustomSound>;

pub(crate) fn sound(r: &mut Reader) -> DecodeResult<Sound> {
    holder(r, |r| {
        Ok(CustomSound {
            location: identifier(r)?,
            fixed_range: r.optional(Reader::f32)?,
        })
    })
}

pub(crate) fn write_sound(buf: &mut Vec<u8>, sound: &Sound) {
    write_holder(buf, sound, |buf, custom| {
        write_identifier(buf, &custom.location);
        write_optional(buf, &custom.fixed_range, |buf, range| {
            write_f32(buf, *range)
        });
    });
}

/// `ByteBufCodecs.COMPOUND_TAG` and `TAG`: NBT kept as its bytes, so it goes
/// back out as it came.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Nbt {
    bytes: Box<[u8]>,
}

impl Nbt {
    pub fn new(tag: &NbtTag) -> Self {
        let mut bytes = Vec::new();
        tag.write(&mut bytes);
        Self {
            bytes: bytes.into(),
        }
    }

    /// The value; `None` when the bytes aren't NBT.
    pub fn tag(&self) -> Option<NbtTag> {
        simdnbt::owned::read_tag(&mut Cursor::new(&self.bytes)).ok()
    }

    /// The value when it is a compound.
    pub fn compound(&self) -> Option<simdnbt::owned::NbtCompound> {
        match self.tag()? {
            NbtTag::Compound(compound) => Some(compound),
            _ => None,
        }
    }

    pub(crate) fn read(r: &mut Reader) -> DecodeResult<Self> {
        let bytes = r.nbt()?.into();
        Ok(Self { bytes })
    }

    pub(crate) fn write(&self, buf: &mut Vec<u8>) {
        buf.extend_from_slice(&self.bytes);
    }
}

/// A text component as `ComponentSerialization.STREAM_CODEC` sends it: one
/// NBT value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Text {
    nbt: Nbt,
}

impl Text {
    /// Plain text, which the codec writes as a string tag.
    pub fn literal(text: &str) -> Self {
        Self {
            nbt: Nbt::new(&NbtTag::String(text.into())),
        }
    }

    /// The text; `None` when the NBT isn't a text component.
    pub fn component(&self) -> Option<TextComponent> {
        TextComponent::from_nbt_tag(&self.nbt.tag()?).ok()
    }

    pub(crate) fn read(r: &mut Reader) -> DecodeResult<Self> {
        Nbt::read(r).map(|nbt| Self { nbt })
    }

    pub(crate) fn write(&self, buf: &mut Vec<u8>) {
        self.nbt.write(buf);
    }
}
