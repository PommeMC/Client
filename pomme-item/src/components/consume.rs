//! What using an item does: eating, drinking, potion effects and the
//! cooldown after.

use pomme_protocol::codec::{DecodeResult, Reader};
use pomme_protocol::wire::write_utf;
use pomme_registry::builtin::{ConsumeEffectKind, DataComponentKind, MobEffect, Potion};
use pomme_registry::{HolderSet, Identifier};

use super::Component;
use super::codec::{
    Sound, holder_set, id_enum, identifier, registry, sound, string, write_bool, write_f32,
    write_holder_set, write_i32, write_identifier, write_list, write_optional, write_registry,
    write_sound, write_var_i32,
};

/// Vanilla `FoodProperties`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Food {
    pub nutrition: i32,
    pub saturation: f32,
    pub can_always_eat: bool,
}

impl Component for Food {
    const KIND: DataComponentKind = DataComponentKind::Food;

    fn read(r: &mut Reader) -> DecodeResult<Self> {
        Ok(Self {
            nutrition: r.var_i32()?,
            saturation: r.f32()?,
            can_always_eat: r.bool()?,
        })
    }

    fn write(&self, buf: &mut Vec<u8>) {
        write_var_i32(buf, self.nutrition);
        write_f32(buf, self.saturation);
        write_bool(buf, self.can_always_eat);
    }
}

/// Vanilla `UseEffects`: how using an item slows and silences its holder.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct UseEffects {
    pub can_sprint: bool,
    pub interact_vibrations: bool,
    pub speed_multiplier: f32,
}

impl Default for UseEffects {
    /// `UseEffects.DEFAULT`.
    fn default() -> Self {
        Self {
            can_sprint: false,
            interact_vibrations: true,
            speed_multiplier: 0.2,
        }
    }
}

impl Component for UseEffects {
    const KIND: DataComponentKind = DataComponentKind::UseEffects;

    fn read(r: &mut Reader) -> DecodeResult<Self> {
        Ok(Self {
            can_sprint: r.bool()?,
            interact_vibrations: r.bool()?,
            speed_multiplier: r.f32()?,
        })
    }

    fn write(&self, buf: &mut Vec<u8>) {
        write_bool(buf, self.can_sprint);
        write_bool(buf, self.interact_vibrations);
        write_f32(buf, self.speed_multiplier);
    }
}

/// Vanilla `UseCooldown`: how long an item, or its group, can't be used
/// again.
#[derive(Clone, Debug, PartialEq)]
pub struct UseCooldown {
    pub seconds: f32,
    pub cooldown_group: Option<Identifier>,
}

impl Component for UseCooldown {
    const KIND: DataComponentKind = DataComponentKind::UseCooldown;

    fn read(r: &mut Reader) -> DecodeResult<Self> {
        Ok(Self {
            seconds: r.f32()?,
            cooldown_group: r.optional(identifier)?,
        })
    }

    fn write(&self, buf: &mut Vec<u8>) {
        write_f32(buf, self.seconds);
        write_optional(buf, &self.cooldown_group, write_identifier);
    }
}

/// Vanilla `MobEffectInstance.Details`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MobEffectDetails {
    pub amplifier: i32,
    pub duration: i32,
    pub ambient: bool,
    pub show_particles: bool,
    pub show_icon: bool,
    pub hidden_effect: Option<Box<MobEffectDetails>>,
}

impl MobEffectDetails {
    fn read(r: &mut Reader) -> DecodeResult<Self> {
        Ok(Self {
            amplifier: r.var_i32()?,
            duration: r.var_i32()?,
            ambient: r.bool()?,
            show_particles: r.bool()?,
            show_icon: r.bool()?,
            hidden_effect: r.optional(|r| Self::read(r).map(Box::new))?,
        })
    }

    fn write(&self, buf: &mut Vec<u8>) {
        write_var_i32(buf, self.amplifier);
        write_var_i32(buf, self.duration);
        write_bool(buf, self.ambient);
        write_bool(buf, self.show_particles);
        write_bool(buf, self.show_icon);
        write_optional(buf, &self.hidden_effect, |buf, hidden| hidden.write(buf));
    }
}

/// Vanilla `MobEffectInstance`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MobEffectInstance {
    pub id: MobEffect,
    pub details: MobEffectDetails,
}

impl MobEffectInstance {
    fn read(r: &mut Reader) -> DecodeResult<Self> {
        Ok(Self {
            id: registry(r)?,
            details: MobEffectDetails::read(r)?,
        })
    }

    fn write(&self, buf: &mut Vec<u8>) {
        write_registry(buf, &self.id);
        self.details.write(buf);
    }
}

/// Vanilla `PotionContents`: a potion with any effects, colour and name of
/// its own.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PotionContents {
    pub potion: Option<Potion>,
    pub custom_color: Option<i32>,
    pub custom_effects: Vec<MobEffectInstance>,
    pub custom_name: Option<String>,
}

impl Component for PotionContents {
    const KIND: DataComponentKind = DataComponentKind::PotionContents;

    fn read(r: &mut Reader) -> DecodeResult<Self> {
        Ok(Self {
            potion: r.optional(registry)?,
            custom_color: r.optional(Reader::i32)?,
            custom_effects: r.list(usize::MAX, MobEffectInstance::read)?,
            custom_name: r.optional(string)?,
        })
    }

    fn write(&self, buf: &mut Vec<u8>) {
        write_optional(buf, &self.potion, write_registry);
        write_optional(buf, &self.custom_color, |buf, color| write_i32(buf, *color));
        write_list(buf, &self.custom_effects, |buf, effect| effect.write(buf));
        write_optional(buf, &self.custom_name, |buf, name| write_utf(buf, name));
    }
}

id_enum!(
    /// Vanilla `ItemUseAnimation`.
    ItemUseAnimation {
        None = 0,
        Eat = 1,
        Drink = 2,
        Block = 3,
        Bow = 4,
        Trident = 5,
        Crossbow = 6,
        Spyglass = 7,
        TootHorn = 8,
        Brush = 9,
        Bundle = 10,
        Spear = 11,
    }
);

/// Vanilla `ConsumeEffect`: something that happens when an item is used up.
#[derive(Clone, Debug, PartialEq)]
pub enum ConsumeEffect {
    ApplyEffects {
        effects: Vec<MobEffectInstance>,
        probability: f32,
    },
    RemoveEffects {
        effects: HolderSet<MobEffect, Identifier>,
    },
    ClearAllEffects,
    TeleportRandomly {
        diameter: f32,
    },
    PlaySound {
        sound: Sound,
    },
}

impl ConsumeEffect {
    fn read(r: &mut Reader) -> DecodeResult<Self> {
        Ok(match registry(r)? {
            ConsumeEffectKind::ApplyEffects => Self::ApplyEffects {
                effects: r.list(usize::MAX, MobEffectInstance::read)?,
                probability: r.f32()?,
            },
            ConsumeEffectKind::RemoveEffects => Self::RemoveEffects {
                effects: holder_set(r)?,
            },
            ConsumeEffectKind::ClearAllEffects => Self::ClearAllEffects,
            ConsumeEffectKind::TeleportRandomly => Self::TeleportRandomly { diameter: r.f32()? },
            ConsumeEffectKind::PlaySound => Self::PlaySound { sound: sound(r)? },
        })
    }

    fn write(&self, buf: &mut Vec<u8>) {
        match self {
            Self::ApplyEffects {
                effects,
                probability,
            } => {
                write_registry(buf, &ConsumeEffectKind::ApplyEffects);
                write_list(buf, effects, |buf, effect| effect.write(buf));
                write_f32(buf, *probability);
            }
            Self::RemoveEffects { effects } => {
                write_registry(buf, &ConsumeEffectKind::RemoveEffects);
                write_holder_set(buf, effects);
            }
            Self::ClearAllEffects => write_registry(buf, &ConsumeEffectKind::ClearAllEffects),
            Self::TeleportRandomly { diameter } => {
                write_registry(buf, &ConsumeEffectKind::TeleportRandomly);
                write_f32(buf, *diameter);
            }
            Self::PlaySound { sound } => {
                write_registry(buf, &ConsumeEffectKind::PlaySound);
                write_sound(buf, sound);
            }
        }
    }
}

/// Vanilla `Consumable`: how an item is eaten or drunk and what that does.
#[derive(Clone, Debug, PartialEq)]
pub struct Consumable {
    pub consume_seconds: f32,
    pub animation: ItemUseAnimation,
    pub sound: Sound,
    pub has_consume_particles: bool,
    pub on_consume_effects: Vec<ConsumeEffect>,
}

impl Component for Consumable {
    const KIND: DataComponentKind = DataComponentKind::Consumable;

    fn read(r: &mut Reader) -> DecodeResult<Self> {
        Ok(Self {
            consume_seconds: r.f32()?,
            animation: ItemUseAnimation::read_id(r)?,
            sound: sound(r)?,
            has_consume_particles: r.bool()?,
            on_consume_effects: r.list(usize::MAX, ConsumeEffect::read)?,
        })
    }

    fn write(&self, buf: &mut Vec<u8>) {
        write_f32(buf, self.consume_seconds);
        self.animation.write_id(buf);
        write_sound(buf, &self.sound);
        write_bool(buf, self.has_consume_particles);
        write_list(buf, &self.on_consume_effects, |buf, effect| {
            effect.write(buf);
        });
    }
}
