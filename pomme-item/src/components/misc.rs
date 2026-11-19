//! Fireworks, beehives and player heads.

use pomme_protocol::codec::{DecodeResult, Reader};
use pomme_protocol::wire::write_utf;
use pomme_registry::Identifier;
use pomme_registry::builtin::{DataComponentKind, EntityKind};

use super::Component;
use super::codec::{
    Nbt, id_enum, identifier, registry, write_bool, write_i32, write_identifier, write_list,
    write_optional, write_registry, write_var_i32,
};

id_enum!(
    /// Vanilla `FireworkExplosion.Shape`.
    FireworkExplosionShape {
        SmallBall = 0,
        LargeBall = 1,
        Star = 2,
        Creeper = 3,
        Burst = 4,
    }
);

/// Vanilla `FireworkExplosion`: one burst of a firework.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FireworkExplosion {
    pub shape: FireworkExplosionShape,
    pub colors: Vec<i32>,
    pub fade_colors: Vec<i32>,
    pub has_trail: bool,
    pub has_twinkle: bool,
}

impl Component for FireworkExplosion {
    const KIND: DataComponentKind = DataComponentKind::FireworkExplosion;

    fn read(r: &mut Reader) -> DecodeResult<Self> {
        Ok(Self {
            shape: FireworkExplosionShape::read(r)?,
            colors: r.list(usize::MAX, Reader::i32)?,
            fade_colors: r.list(usize::MAX, Reader::i32)?,
            has_trail: r.bool()?,
            has_twinkle: r.bool()?,
        })
    }

    fn write(&self, buf: &mut Vec<u8>) {
        self.shape.write(buf);
        write_list(buf, &self.colors, |buf, color| write_i32(buf, *color));
        write_list(buf, &self.fade_colors, |buf, color| write_i32(buf, *color));
        write_bool(buf, self.has_trail);
        write_bool(buf, self.has_twinkle);
    }
}

/// Vanilla `BeehiveBlockEntity.Occupant`: a bee inside a hive.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BeehiveOccupant {
    /// `TypedEntityData`: the entity's type, then its data.
    pub entity: EntityKind,
    pub entity_data: Nbt,
    pub ticks_in_hive: i32,
    pub min_ticks_in_hive: i32,
}

/// Vanilla `Bees`: the bees a hive item holds.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Bees {
    pub occupants: Vec<BeehiveOccupant>,
}

impl Component for Bees {
    const KIND: DataComponentKind = DataComponentKind::Bees;

    fn read(r: &mut Reader) -> DecodeResult<Self> {
        let occupants = r.list(usize::MAX, |r| {
            Ok(BeehiveOccupant {
                entity: registry(r)?,
                entity_data: Nbt::read(r)?,
                ticks_in_hive: r.var_i32()?,
                min_ticks_in_hive: r.var_i32()?,
            })
        })?;
        Ok(Self { occupants })
    }

    fn write(&self, buf: &mut Vec<u8>) {
        write_list(buf, &self.occupants, |buf, occupant| {
            write_registry(buf, &occupant.entity);
            occupant.entity_data.write(buf);
            write_var_i32(buf, occupant.ticks_in_hive);
            write_var_i32(buf, occupant.min_ticks_in_hive);
        });
    }
}

/// One of a game profile's properties, like its signed `textures`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProfileProperty {
    pub name: String,
    pub value: String,
    pub signature: Option<String>,
}

/// `ByteBufCodecs.GAME_PROFILE_PROPERTIES`.
fn properties(r: &mut Reader) -> DecodeResult<Vec<ProfileProperty>> {
    const MAX_PROPERTIES: usize = 16;
    r.list(MAX_PROPERTIES, |r| {
        Ok(ProfileProperty {
            name: r.utf(64)?.to_owned(),
            value: r.utf(32767)?.to_owned(),
            signature: r.optional(|r| r.utf(1024).map(str::to_owned))?,
        })
    })
}

fn write_properties(buf: &mut Vec<u8>, properties: &[ProfileProperty]) {
    write_list(buf, properties, |buf, property| {
        write_utf(buf, &property.name);
        write_utf(buf, &property.value);
        write_optional(buf, &property.signature, |buf, signature| {
            write_utf(buf, signature);
        });
    });
}

/// `ByteBufCodecs.PLAYER_NAME`'s length limit.
const MAX_PLAYER_NAME: usize = 16;

/// Vanilla `ResolvableProfile`'s two forms: a whole game profile, or the
/// parts of one that are known.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProfileContents {
    Full {
        id: u128,
        name: String,
        properties: Vec<ProfileProperty>,
    },
    Partial {
        name: Option<String>,
        id: Option<u128>,
        properties: Vec<ProfileProperty>,
    },
}

/// Vanilla `PlayerSkin.Patch`: textures and a model that replace the
/// profile's own.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PlayerSkinPatch {
    pub body: Option<Identifier>,
    pub cape: Option<Identifier>,
    pub elytra: Option<Identifier>,
    /// `PlayerModelType`: whether the model is the slim one.
    pub slim: Option<bool>,
}

/// Vanilla `ResolvableProfile`: whose head a player head shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Profile {
    pub contents: ProfileContents,
    pub skin_patch: PlayerSkinPatch,
}

impl Component for Profile {
    const KIND: DataComponentKind = DataComponentKind::Profile;

    fn read(r: &mut Reader) -> DecodeResult<Self> {
        // `ByteBufCodecs.either`: true for the left value, the full profile.
        let contents = if r.bool()? {
            ProfileContents::Full {
                id: r.uuid()?,
                name: r.utf(MAX_PLAYER_NAME)?.to_owned(),
                properties: properties(r)?,
            }
        } else {
            ProfileContents::Partial {
                name: r.optional(|r| r.utf(MAX_PLAYER_NAME).map(str::to_owned))?,
                id: r.optional(Reader::uuid)?,
                properties: properties(r)?,
            }
        };
        let skin_patch = PlayerSkinPatch {
            body: r.optional(identifier)?,
            cape: r.optional(identifier)?,
            elytra: r.optional(identifier)?,
            slim: r.optional(Reader::bool)?,
        };
        Ok(Self {
            contents,
            skin_patch,
        })
    }

    fn write(&self, buf: &mut Vec<u8>) {
        let write_uuid = |buf: &mut Vec<u8>, id: &u128| buf.extend_from_slice(&id.to_be_bytes());
        match &self.contents {
            ProfileContents::Full {
                id,
                name,
                properties,
            } => {
                write_bool(buf, true);
                write_uuid(buf, id);
                write_utf(buf, name);
                write_properties(buf, properties);
            }
            ProfileContents::Partial {
                name,
                id,
                properties,
            } => {
                write_bool(buf, false);
                write_optional(buf, name, |buf, name| write_utf(buf, name));
                write_optional(buf, id, write_uuid);
                write_properties(buf, properties);
            }
        }
        let patch = &self.skin_patch;
        for texture in [&patch.body, &patch.cape, &patch.elytra] {
            write_optional(buf, texture, write_identifier);
        }
        write_optional(buf, &patch.slim, |buf, slim| write_bool(buf, *slim));
    }
}
