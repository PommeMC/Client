//! What an item does in a hand: mining, blocking and charging attacks.

use pomme_protocol::codec::{DecodeResult, Reader};
use pomme_registry::builtin::{BlockKind, DataComponentKind};
use pomme_registry::data::DamageKind;
use pomme_registry::{HolderSet, Identifier};

use super::Component;
use super::codec::{
    Sound, data_entry_value, entries_value, holder_set, holder_set_value, sound, sound_value,
    write_bool, write_f32, write_holder_set, write_list, write_optional, write_sound,
    write_var_i32,
};
use crate::hash::{Names, Record, Value};

/// Vanilla `Tool.Rule`: how fast some blocks mine and whether they drop.
#[derive(Clone, Debug, PartialEq)]
pub struct ToolRule {
    pub blocks: HolderSet<BlockKind, Identifier>,
    pub speed: Option<f32>,
    pub correct_for_drops: Option<bool>,
}

/// Vanilla `Tool`: what an item mines.
#[derive(Clone, Debug, PartialEq)]
pub struct Tool {
    pub rules: Vec<ToolRule>,
    pub default_mining_speed: f32,
    pub damage_per_block: i32,
    pub can_destroy_blocks_in_creative: bool,
}

impl Component for Tool {
    const KIND: DataComponentKind = DataComponentKind::Tool;

    fn read(r: &mut Reader) -> DecodeResult<Self> {
        Ok(Self {
            rules: r.list(usize::MAX, |r| {
                Ok(ToolRule {
                    blocks: holder_set(r)?,
                    speed: r.optional(Reader::f32)?,
                    correct_for_drops: r.optional(Reader::bool)?,
                })
            })?,
            default_mining_speed: r.f32()?,
            damage_per_block: r.var_i32()?,
            can_destroy_blocks_in_creative: r.bool()?,
        })
    }

    fn write(&self, buf: &mut Vec<u8>) {
        write_list(buf, &self.rules, |buf, rule| {
            write_holder_set(buf, &rule.blocks);
            write_optional(buf, &rule.speed, |buf, speed| write_f32(buf, *speed));
            write_optional(buf, &rule.correct_for_drops, |buf, correct| {
                write_bool(buf, *correct);
            });
        });
        write_f32(buf, self.default_mining_speed);
        write_var_i32(buf, self.damage_per_block);
        write_bool(buf, self.can_destroy_blocks_in_creative);
    }

    fn value(&self, _: &dyn Names) -> Option<Value> {
        let rules = self.rules.iter().map(|rule| {
            Record::new()
                .field("blocks", entries_value(&rule.blocks))
                .optional("speed", rule.speed.map(Value::Float))
                .optional("correct_for_drops", rule.correct_for_drops.map(Value::Bool))
                .build()
        });
        let speed = self.default_mining_speed;
        Some(
            Record::new()
                .field("rules", Value::list(rules))
                .float("default_mining_speed", speed, 1.0)
                .int("damage_per_block", self.damage_per_block, 1)
                .flag(
                    "can_destroy_blocks_in_creative",
                    self.can_destroy_blocks_in_creative,
                    true,
                )
                .build(),
        )
    }
}

/// Vanilla `DataComponents.MINIMUM_ATTACK_CHARGE`: how charged an attack
/// must be to land.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MinimumAttackCharge {
    pub value: f32,
}

impl Component for MinimumAttackCharge {
    const KIND: DataComponentKind = DataComponentKind::MinimumAttackCharge;

    fn read(r: &mut Reader) -> DecodeResult<Self> {
        let value = r.f32()?;
        Ok(Self { value })
    }

    fn write(&self, buf: &mut Vec<u8>) {
        write_f32(buf, self.value);
    }

    fn value(&self, _: &dyn Names) -> Option<Value> {
        Some(Value::Float(self.value))
    }
}

/// Vanilla `KineticWeapon.Condition`: how long and how fast a charge must
/// be for one of its effects.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct KineticWeaponCondition {
    pub max_duration_ticks: i32,
    pub min_speed: f32,
    pub min_relative_speed: f32,
}

impl KineticWeaponCondition {
    fn read(r: &mut Reader) -> DecodeResult<Self> {
        Ok(Self {
            max_duration_ticks: r.var_i32()?,
            min_speed: r.f32()?,
            min_relative_speed: r.f32()?,
        })
    }

    fn write(&self, buf: &mut Vec<u8>) {
        write_var_i32(buf, self.max_duration_ticks);
        write_f32(buf, self.min_speed);
        write_f32(buf, self.min_relative_speed);
    }

    fn value(&self) -> Value {
        let (speed, relative) = (self.min_speed, self.min_relative_speed);
        Record::new()
            .field("max_duration_ticks", Value::Int(self.max_duration_ticks))
            .float("min_speed", speed, 0.0)
            .float("min_relative_speed", relative, 0.0)
            .build()
    }
}

/// Vanilla `KineticWeapon`: a weapon that hits by being carried into its
/// target.
#[derive(Clone, Debug, PartialEq)]
pub struct KineticWeapon {
    pub contact_cooldown_ticks: i32,
    pub delay_ticks: i32,
    pub dismount_conditions: Option<KineticWeaponCondition>,
    pub knockback_conditions: Option<KineticWeaponCondition>,
    pub damage_conditions: Option<KineticWeaponCondition>,
    pub forward_movement: f32,
    pub damage_multiplier: f32,
    pub sound: Option<Sound>,
    pub hit_sound: Option<Sound>,
}

impl Component for KineticWeapon {
    const KIND: DataComponentKind = DataComponentKind::KineticWeapon;

    fn read(r: &mut Reader) -> DecodeResult<Self> {
        Ok(Self {
            contact_cooldown_ticks: r.var_i32()?,
            delay_ticks: r.var_i32()?,
            dismount_conditions: r.optional(KineticWeaponCondition::read)?,
            knockback_conditions: r.optional(KineticWeaponCondition::read)?,
            damage_conditions: r.optional(KineticWeaponCondition::read)?,
            forward_movement: r.f32()?,
            damage_multiplier: r.f32()?,
            sound: r.optional(sound)?,
            hit_sound: r.optional(sound)?,
        })
    }

    fn write(&self, buf: &mut Vec<u8>) {
        write_var_i32(buf, self.contact_cooldown_ticks);
        write_var_i32(buf, self.delay_ticks);
        for condition in [
            &self.dismount_conditions,
            &self.knockback_conditions,
            &self.damage_conditions,
        ] {
            write_optional(buf, condition, |buf, condition| condition.write(buf));
        }
        write_f32(buf, self.forward_movement);
        write_f32(buf, self.damage_multiplier);
        write_optional(buf, &self.sound, write_sound);
        write_optional(buf, &self.hit_sound, write_sound);
    }

    fn value(&self, _: &dyn Names) -> Option<Value> {
        let condition = |condition: &Option<KineticWeaponCondition>| condition.map(|c| c.value());
        let (forward, multiplier) = (self.forward_movement, self.damage_multiplier);
        Some(
            Record::new()
                .int("contact_cooldown_ticks", self.contact_cooldown_ticks, 10)
                .int("delay_ticks", self.delay_ticks, 0)
                .optional("dismount_conditions", condition(&self.dismount_conditions))
                .optional(
                    "knockback_conditions",
                    condition(&self.knockback_conditions),
                )
                .optional("damage_conditions", condition(&self.damage_conditions))
                .float("forward_movement", forward, 0.0)
                .float("damage_multiplier", multiplier, 1.0)
                .optional("sound", self.sound.as_ref().map(sound_value))
                .optional("hit_sound", self.hit_sound.as_ref().map(sound_value))
                .build(),
        )
    }
}

/// Vanilla `BlocksAttacks.DamageReduction`: how much of some damage a block
/// takes off.
#[derive(Clone, Debug, PartialEq)]
pub struct DamageReduction {
    pub horizontal_blocking_angle: f32,
    pub kind: Option<HolderSet<DamageKind, Identifier>>,
    pub base: f32,
    pub factor: f32,
}

/// Vanilla `BlocksAttacks.ItemDamageFunction`: the durability blocking
/// costs.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ItemDamageFunction {
    pub threshold: f32,
    pub base: f32,
    pub factor: f32,
}

/// Vanilla `BlocksAttacks`: an item that blocks, like a shield.
#[derive(Clone, Debug, PartialEq)]
pub struct BlocksAttacks {
    pub block_delay_seconds: f32,
    pub disable_cooldown_scale: f32,
    pub damage_reductions: Vec<DamageReduction>,
    pub item_damage: ItemDamageFunction,
    pub bypassed_by: Option<HolderSet<DamageKind, Identifier>>,
    pub block_sound: Option<Sound>,
    pub disabled_sound: Option<Sound>,
}

impl Component for BlocksAttacks {
    const KIND: DataComponentKind = DataComponentKind::BlocksAttacks;

    fn read(r: &mut Reader) -> DecodeResult<Self> {
        Ok(Self {
            block_delay_seconds: r.f32()?,
            disable_cooldown_scale: r.f32()?,
            damage_reductions: r.list(usize::MAX, |r| {
                Ok(DamageReduction {
                    horizontal_blocking_angle: r.f32()?,
                    kind: r.optional(holder_set)?,
                    base: r.f32()?,
                    factor: r.f32()?,
                })
            })?,
            item_damage: ItemDamageFunction {
                threshold: r.f32()?,
                base: r.f32()?,
                factor: r.f32()?,
            },
            bypassed_by: r.optional(holder_set)?,
            block_sound: r.optional(sound)?,
            disabled_sound: r.optional(sound)?,
        })
    }

    fn write(&self, buf: &mut Vec<u8>) {
        write_f32(buf, self.block_delay_seconds);
        write_f32(buf, self.disable_cooldown_scale);
        write_list(buf, &self.damage_reductions, |buf, reduction| {
            write_f32(buf, reduction.horizontal_blocking_angle);
            write_optional(buf, &reduction.kind, write_holder_set);
            write_f32(buf, reduction.base);
            write_f32(buf, reduction.factor);
        });
        write_f32(buf, self.item_damage.threshold);
        write_f32(buf, self.item_damage.base);
        write_f32(buf, self.item_damage.factor);
        write_optional(buf, &self.bypassed_by, write_holder_set);
        write_optional(buf, &self.block_sound, write_sound);
        write_optional(buf, &self.disabled_sound, write_sound);
    }

    fn value(&self, names: &dyn Names) -> Option<Value> {
        let kinds = |kinds: &Option<HolderSet<DamageKind, Identifier>>| match kinds {
            Some(kinds) => holder_set_value(kinds, |kind| data_entry_value(kind, names)).map(Some),
            None => Some(None),
        };
        // `List.of(new DamageReduction(90.0f, Optional.empty(), 0.0f, 1.0f))`.
        let default_reductions = matches!(
            self.damage_reductions.as_slice(),
            [DamageReduction {
                horizontal_blocking_angle: 90.0,
                kind: None,
                base: 0.0,
                factor: 1.0,
            }]
        );
        let reductions = self
            .damage_reductions
            .iter()
            .map(|reduction| {
                let angle = reduction.horizontal_blocking_angle;
                Some(
                    Record::new()
                        .float("horizontal_blocking_angle", angle, 90.0)
                        .optional("type", kinds(&reduction.kind)?)
                        .field("base", Value::Float(reduction.base))
                        .field("factor", Value::Float(reduction.factor))
                        .build(),
                )
            })
            .collect::<Option<Vec<_>>>()?;
        let damage = self.item_damage;
        // `ItemDamageFunction.DEFAULT`.
        let default_damage = (damage.threshold, damage.base, damage.factor) == (1.0, 0.0, 1.0);
        let (delay, scale) = (self.block_delay_seconds, self.disable_cooldown_scale);
        Some(
            Record::new()
                .float("block_delay_seconds", delay, 0.0)
                .float("disable_cooldown_scale", scale, 1.0)
                .unless("damage_reductions", default_reductions, || {
                    Value::list(reductions)
                })
                .unless("item_damage", default_damage, || {
                    Record::new()
                        .field("threshold", Value::Float(damage.threshold))
                        .field("base", Value::Float(damage.base))
                        .field("factor", Value::Float(damage.factor))
                        .build()
                })
                .optional("bypassed_by", kinds(&self.bypassed_by)?)
                .optional("block_sound", self.block_sound.as_ref().map(sound_value))
                .optional(
                    "disabled_sound",
                    self.disabled_sound.as_ref().map(sound_value),
                )
                .build(),
        )
    }
}
