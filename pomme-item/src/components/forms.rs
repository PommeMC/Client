//! The `CODEC` forms of the components pomme has no use for beyond hashing
//! them: each is read from its `STREAM_CODEC` bytes straight into the form.
// TODO: `can_place_on`, `can_break` and `lock` hold predicates, which have
// no form here yet.

use pomme_protocol::codec::{DecodeResult, Reader};
use pomme_registry::builtin::{
    BlockEntityKind, DataComponentKind, EntityKind, ItemKind, MobEffect, VillagerKind,
};
use pomme_registry::data::{
    BannerPatternKind, CatSoundVariant, CatVariant, ChickenSoundVariant, ChickenVariant,
    CowSoundVariant, CowVariant, DamageKind, FrogVariant, Instrument, JukeboxSong, PaintingVariant,
    PigSoundVariant, PigVariant, WolfSoundVariant, WolfVariant, ZombieNautilusVariant,
};
use pomme_registry::{DataRegistry, Identifier};

use super::codec::{
    Nbt, Text, compound_value, data_entry_value, entries_value, entry_value, holder_set,
    holder_set_value, identifier, registry, sound, sound_value, string, tag_value,
};
use super::{Component, ConsumeEffect, CustomData, DyeColor, FireworkExplosion};
use crate::hash::{Names, Record, Value};

/// The form of the `kind` value `bytes` hold; `None` for a kind this doesn't
/// cover, bytes that aren't one, or a value with something that has no
/// form here.
pub(super) fn value_of(kind: DataComponentKind, bytes: &[u8], names: &dyn Names) -> Option<Value> {
    let mut r = Reader::new(bytes);
    let value = read(kind, &mut r, names).ok()??;
    r.ensure_eof().ok()?;
    Some(value)
}

/// A registered entry of a registry the server sends, by its id.
fn data_entry<R: DataRegistry>(r: &mut Reader, names: &dyn Names) -> DecodeResult<Option<Value>> {
    Ok(data_entry_value(&registry::<R>(r)?, names))
}

/// `ByteBufCodecs.holder` over such a registry: the entry's id plus one.
// TODO: zero and a value sent in the entry's place has no form here.
fn data_holder<R: DataRegistry>(r: &mut Reader, names: &dyn Names) -> DecodeResult<Option<Value>> {
    Ok(match r.varint()?.checked_sub(1) {
        Some(id) => data_entry_value(&R::new_raw(id), names),
        None => None,
    })
}

/// `TypedEntityData`: the data with its type's name in it as `id`.
fn typed_data<R: pomme_registry::Registry + Into<Identifier>>(
    r: &mut Reader,
) -> DecodeResult<Option<Value>> {
    let kind: R = registry(r)?;
    let data = Nbt::read(r)?;
    Ok(data
        .compound()
        .map(|data| compound_value(&data, Some(("id", entry_value(&kind))))))
}

/// A component sent as the NBT its `CODEC` writes, having no `STREAM_CODEC`.
fn nbt(r: &mut Reader) -> DecodeResult<Option<Value>> {
    Ok(Nbt::read(r)?.tag().as_ref().map(tag_value))
}

/// `Filterable.codec`: the raw value and what the filter left of it, each
/// read by `read`.
fn filterable(
    r: &mut Reader,
    read: impl Fn(&mut Reader) -> DecodeResult<Option<Value>>,
) -> DecodeResult<Option<Value>> {
    let raw = read(r)?;
    let filtered = r.optional(&read)?;
    let Some(filtered) = filtered.map_or(Some(None), |filtered| filtered.map(Some)) else {
        return Ok(None);
    };
    Ok(raw.map(|raw| {
        Record::new()
            .field("raw", raw)
            .optional("filtered", filtered)
            .build()
    }))
}

/// A string of at most `max` bytes, for [`filterable`].
fn text(max: usize) -> impl Fn(&mut Reader) -> DecodeResult<Option<Value>> {
    move |r| Ok(Some(Value::string(r.utf(max)?)))
}

/// An enum sent by id as its name, an id that is none of them as the
/// first (`ByIdMap`'s zero).
fn by_id(r: &mut Reader, names: &[(u32, &str)]) -> DecodeResult<Value> {
    let id = r.varint()?;
    let name = names
        .iter()
        .find(|(known, _)| *known == id)
        .unwrap_or(&names[0]);
    Ok(Value::string(name.1))
}

fn read(kind: DataComponentKind, r: &mut Reader, names: &dyn Names) -> DecodeResult<Option<Value>> {
    use DataComponentKind as Kind;

    let template = |r: &mut Reader| Ok(super::template_value(&super::read_template(r)?, names));
    Ok(Some(match kind {
        Kind::ItemModel | Kind::TooltipStyle | Kind::NoteBlockSound => {
            Value::identifier(&identifier(r)?)
        }
        Kind::RepairCost | Kind::MapId | Kind::OminousBottleAmplifier => Value::Int(r.var_i32()?),
        Kind::Enchantable => Record::new()
            .field("value", Value::Int(r.var_i32()?))
            .build(),
        Kind::EnchantmentGlintOverride => Value::Bool(r.bool()?),
        Kind::PotionDurationScale => Value::Float(r.f32()?),
        // `Unit.CODEC`.
        Kind::Glider => Record::new().build(),
        Kind::BaseColor
        | Kind::WolfCollar
        | Kind::CatCollar
        | Kind::SheepColor
        | Kind::ShulkerColor
        | Kind::TropicalFishBaseColor
        | Kind::TropicalFishPatternColor => DyeColor::read_id(r)?.name_value(),
        Kind::BreakSound => sound_value(&sound(r)?),
        Kind::BucketEntityData => return Ok(CustomData::read(r)?.value(names)),
        Kind::EntityData => return typed_data::<EntityKind>(r),
        Kind::BlockEntityData => return typed_data::<BlockEntityKind>(r),
        Kind::VillagerVariant => entry_value(&registry::<VillagerKind>(r)?),
        Kind::WolfVariant => return data_entry::<WolfVariant>(r, names),
        Kind::WolfSoundVariant => return data_entry::<WolfSoundVariant>(r, names),
        Kind::PigVariant => return data_entry::<PigVariant>(r, names),
        Kind::PigSoundVariant => return data_entry::<PigSoundVariant>(r, names),
        Kind::CowVariant => return data_entry::<CowVariant>(r, names),
        Kind::CowSoundVariant => return data_entry::<CowSoundVariant>(r, names),
        Kind::ChickenVariant => return data_entry::<ChickenVariant>(r, names),
        Kind::ChickenSoundVariant => return data_entry::<ChickenSoundVariant>(r, names),
        Kind::ZombieNautilusVariant => return data_entry::<ZombieNautilusVariant>(r, names),
        Kind::FrogVariant => return data_entry::<FrogVariant>(r, names),
        Kind::CatVariant => return data_entry::<CatVariant>(r, names),
        Kind::CatSoundVariant => return data_entry::<CatSoundVariant>(r, names),
        Kind::DamageType => return data_entry::<DamageKind>(r, names),
        Kind::PaintingVariant => return data_holder::<PaintingVariant>(r, names),
        Kind::Instrument => return data_holder::<Instrument>(r, names),
        Kind::JukeboxPlayable => return data_holder::<JukeboxSong>(r, names),
        Kind::ProvidesBannerPatterns => {
            let patterns = holder_set::<BannerPatternKind>(r)?;
            return Ok(holder_set_value(&patterns, |pattern| {
                data_entry_value(pattern, names)
            }));
        }
        Kind::BannerPatterns => {
            let layers = r.list(usize::MAX, |r| {
                let pattern = data_holder::<BannerPatternKind>(r, names)?;
                let color = DyeColor::read_id(r)?.name_value();
                Ok(pattern.map(|pattern| {
                    Record::new()
                        .field("pattern", pattern)
                        .field("color", color)
                        .build()
                }))
            })?;
            return Ok(Value::try_list(layers));
        }
        Kind::Weapon => Record::new()
            .int("item_damage_per_attack", r.var_i32()?, 1)
            .float("disable_blocking_for_seconds", r.f32()?, 0.0)
            .build(),
        Kind::AttackRange => Record::new()
            .float("min_reach", r.f32()?, 0.0)
            .float("max_reach", r.f32()?, 3.0)
            .float("min_creative_reach", r.f32()?, 0.0)
            .float("max_creative_reach", r.f32()?, 5.0)
            .float("hitbox_margin", r.f32()?, 0.3)
            .float("mob_factor", r.f32()?, 1.0)
            .build(),
        Kind::Repairable => Record::new()
            .field("items", entries_value(&holder_set::<ItemKind>(r)?))
            .build(),
        Kind::DamageResistant => {
            let kinds = holder_set::<DamageKind>(r)?;
            let kinds = holder_set_value(&kinds, |kind| data_entry_value(kind, names));
            return Ok(kinds.map(|kinds| Record::new().field("types", kinds).build()));
        }
        Kind::UseRemainder | Kind::SulfurCubeContent => return template(r),
        Kind::DeathProtection => {
            let effects = r.list(usize::MAX, ConsumeEffect::read)?;
            Record::new()
                .unless("death_effects", effects.is_empty(), || {
                    Value::list(effects.iter().map(ConsumeEffect::value))
                })
                .build()
        }
        Kind::PiercingWeapon => Record::new()
            .flag("deals_knockback", r.bool()?, true)
            .flag("dismounts", r.bool()?, false)
            .optional("sound", r.optional(sound)?.as_ref().map(sound_value))
            .optional("hit_sound", r.optional(sound)?.as_ref().map(sound_value))
            .build(),
        Kind::SwingAnimation => {
            // `SwingAnimationType`, and `SwingAnimation.DEFAULT`'s whack.
            let kind = match r.varint()? {
                1 => "whack",
                2 => "stab",
                _ => "none",
            };
            Record::new()
                .unless("type", kind == "whack", || Value::string(kind))
                .int("duration", r.var_i32()?, 6)
                .build()
        }
        Kind::SuspiciousStewEffects => Value::list(r.list(usize::MAX, |r| {
            Ok(Record::new()
                .field("id", entry_value(&registry::<MobEffect>(r)?))
                .int("duration", r.var_i32()?, 160)
                .build())
        })?),
        Kind::WritableBookContent => {
            let pages = r.list(100, |r| filterable(r, text(1024)))?;
            let Some(pages) = pages.into_iter().collect::<Option<Vec<_>>>() else {
                return Ok(None);
            };
            Record::new()
                .unless("pages", pages.is_empty(), || Value::List(pages))
                .build()
        }
        Kind::Fireworks => {
            let flight = r.var_i32()?;
            let explosions = r.list(256, FireworkExplosion::read)?;
            let explosions = explosions.iter().map(|explosion| explosion.value(names));
            let Some(explosions) = explosions.collect::<Option<Vec<_>>>() else {
                return Ok(None);
            };
            Record::new()
                // `ExtraCodecs.UNSIGNED_BYTE`.
                .unless("flight_duration", flight == 0, || Value::Byte(flight as i8))
                .unless("explosions", explosions.is_empty(), || {
                    Value::List(explosions)
                })
                .build()
        }
        Kind::PotDecorations => {
            Value::list(r.list(4, registry::<ItemKind>)?.iter().map(entry_value))
        }
        Kind::Container => {
            // `ItemContainerContents.asSlots`: each filled slot with its
            // place.
            let slots = r.list(256, |r| r.optional(template))?;
            let slots = slots.into_iter().enumerate().filter_map(|(slot, item)| {
                Some(item?.map(|item| {
                    Record::new()
                        .field("slot", Value::Int(slot as i32))
                        .field("item", item)
                        .build()
                }))
            });
            return Ok(Value::try_list(slots));
        }
        Kind::FoxVariant => by_id(r, &[(0, "red"), (1, "snow")])?,
        Kind::SalmonSize => by_id(r, &[(0, "small"), (1, "medium"), (2, "large")])?,
        Kind::ParrotVariant => by_id(
            r,
            &[
                (0, "red_blue"),
                (1, "blue"),
                (2, "green"),
                (3, "yellow_blue"),
                (4, "gray"),
            ],
        )?,
        // `Pattern.getPackedId`: the base in the low byte, the index above.
        Kind::TropicalFishPattern => by_id(
            r,
            &[
                (0x000, "kob"),
                (0x100, "sunstreak"),
                (0x200, "snooper"),
                (0x300, "dasher"),
                (0x400, "brinely"),
                (0x500, "spotty"),
                (0x001, "flopper"),
                (0x101, "stripey"),
                (0x201, "glitter"),
                (0x301, "blockfish"),
                (0x401, "betty"),
                (0x501, "clayfish"),
            ],
        )?,
        Kind::MooshroomVariant => by_id(r, &[(0, "red"), (1, "brown")])?,
        Kind::RabbitVariant => by_id(
            r,
            &[
                (0, "brown"),
                (1, "white"),
                (2, "black"),
                (3, "white_splotched"),
                (4, "gold"),
                (5, "salt"),
                (99, "evil"),
            ],
        )?,
        Kind::HorseVariant => by_id(
            r,
            &[
                (0, "white"),
                (1, "creamy"),
                (2, "chestnut"),
                (3, "brown"),
                (4, "black"),
                (5, "gray"),
                (6, "dark_brown"),
            ],
        )?,
        Kind::LlamaVariant => by_id(r, &[(0, "creamy"), (1, "white"), (2, "brown"), (3, "gray")])?,
        Kind::AxolotlVariant => by_id(
            r,
            &[
                (0, "lucy"),
                (1, "wild"),
                (2, "gold"),
                (3, "cyan"),
                (4, "blue"),
            ],
        )?,
        Kind::WrittenBookContent => {
            let title = filterable(r, text(32))?;
            let author = string(r)?;
            let generation = r.var_i32()?;
            let pages = r.list(usize::MAX, |r| {
                filterable(r, |r| Ok(Text::read(r)?.value()))
            })?;
            let resolved = r.bool()?;
            let (Some(title), Some(pages)) = (title, pages.into_iter().collect::<Option<Vec<_>>>())
            else {
                return Ok(None);
            };
            Record::new()
                .field("title", title)
                .field("author", Value::String(author))
                .int("generation", generation, 0)
                .unless("pages", pages.is_empty(), || Value::List(pages))
                .flag("resolved", resolved, false)
                .build()
        }
        Kind::LodestoneTracker => {
            let target = r.optional(|r| {
                let dimension = identifier(r)?;
                let (x, y, z) = r.block_pos()?;
                Ok(Record::new()
                    .field("dimension", Value::identifier(&dimension))
                    .field("pos", Value::IntList(vec![x, y, z]))
                    .build())
            })?;
            Record::new()
                .optional("target", target)
                .flag("tracked", r.bool()?, true)
                .build()
        }
        Kind::IntangibleProjectile
        | Kind::MapDecorations
        | Kind::DebugStickState
        | Kind::Recipes
        | Kind::ContainerLoot => return nbt(r),
        _ => return Ok(None),
    }))
}
