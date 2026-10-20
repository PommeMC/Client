//! Vanilla entity shadow sizes: each `EntityRenderer` subclass's
//! `shadowRadius`/`shadowStrength` (26.2), with the overrides that scale them.

use azalea_registry::builtin::EntityKind;

use super::LivingEntity;
use crate::attribute::{AttributeKind, AttributeMap};

/// `ItemEntityRenderer`: radius 0.15, strength 0.75.
pub const ITEM: (f32, f32) = (0.15, 0.75);

/// What a living entity's shadow size depends on.
pub struct ShadowInputs {
    /// The `scale` attribute.
    pub scale: f32,
    pub is_baby: bool,
    pub slime_size: u8,
    pub puff_state: u8,
}

/// A tracked mob's or player's shadow; none while it's invisible.
pub fn of_living(entity: &LivingEntity) -> (f32, f32) {
    if entity.is_invisible {
        return (0.0, 0.0);
    }
    let inputs = ShadowInputs {
        scale: scale(&entity.attributes),
        is_baby: entity.is_baby,
        slime_size: entity.slime_size,
        puff_state: entity.puff_state,
    };
    shadow(entity.entity_type, &inputs)
}

/// The local player's shadow, from its own attributes.
pub fn of_player(attributes: &AttributeMap) -> (f32, f32) {
    let inputs = ShadowInputs {
        scale: scale(attributes),
        is_baby: false,
        slime_size: 0,
        puff_state: 0,
    };
    shadow(EntityKind::Player, &inputs)
}

/// Vanilla `getScale`: the `scale` attribute.
fn scale(attributes: &AttributeMap) -> f32 {
    attributes.value(AttributeKind::Scale).unwrap_or(1.0) as f32
}

/// A living entity's shadow radius and strength.
/// TODO: boats (0.8), minecarts (0.7), primed TNT, falling blocks, end
/// crystals (0.5), experience orbs (0.15, strength 0.75) and display entities
/// (their own `shadow_radius`/`shadow_strength`) aren't tracked client-side.
pub fn shadow(kind: EntityKind, entity: &ShadowInputs) -> (f32, f32) {
    use EntityKind as K;
    // `LivingEntityRenderer` scales by the `scale` attribute, `MobRenderer`
    // also by the age scale.
    let living = |base: f32| base * entity.scale;
    let mob = |base: f32| living(base) * age_scale(kind, entity.is_baby);
    let baby = |radius: f32, factor: f32| {
        if entity.is_baby {
            radius * factor
        } else {
            radius
        }
    };
    let radius = match kind {
        // `AbstractCubeMobRenderer`, `PufferfishRenderer`: replace the radius.
        K::Slime | K::MagmaCube | K::SulfurCube => f32::from(entity.slime_size) * 0.25,
        K::Pufferfish => 0.1 + 0.1 * f32::from(entity.puff_state),
        K::Player | K::Mannequin => living(0.5),
        K::ArmorStand | K::Shulker => 0.0,
        K::Villager | K::Strider => baby(mob(0.5), 0.5),
        K::Turtle => baby(mob(0.7), 0.83),
        // Registered with the model scaled by 6.
        K::Giant => mob(3.0),
        K::HappyGhast => mob(2.0),
        K::Ghast => mob(1.5),
        K::ElderGuardian => mob(1.2),
        K::Ravager | K::Sniffer => mob(1.1),
        K::Wither => mob(1.0),
        K::Panda | K::PolarBear | K::Warden => mob(0.9),
        K::Spider => mob(0.8),
        K::Horse | K::Donkey | K::Mule | K::SkeletonHorse | K::ZombieHorse | K::Phantom => {
            mob(0.75)
        }
        K::Cow
        | K::Mooshroom
        | K::Pig
        | K::Sheep
        | K::Goat
        | K::Llama
        | K::TraderLlama
        | K::Camel
        | K::CamelHusk
        | K::Dolphin
        | K::Hoglin
        | K::Zoglin
        | K::IronGolem
        | K::Squid
        | K::GlowSquid
        | K::Nautilus
        | K::ZombieNautilus => mob(0.7),
        K::Creaking => mob(0.6),
        K::CaveSpider => mob(0.56),
        K::Zombie
        | K::Husk
        | K::Drowned
        | K::ZombieVillager
        | K::Piglin
        | K::PiglinBrute
        | K::ZombifiedPiglin
        | K::Skeleton
        | K::Stray
        | K::WitherSkeleton
        | K::Bogged
        | K::Parched
        | K::Guardian
        | K::Axolotl
        | K::Blaze
        | K::Breeze
        | K::CopperGolem
        | K::Creeper
        | K::Enderman
        | K::Evoker
        | K::Illusioner
        | K::Pillager
        | K::Vindicator
        | K::SnowGolem
        | K::WanderingTrader
        | K::Witch
        | K::Wolf => mob(0.5),
        K::Allay | K::Armadillo | K::Bee | K::Cat | K::Fox | K::Ocelot | K::Salmon => mob(0.4),
        K::Chicken
        | K::Cod
        | K::Endermite
        | K::Frog
        | K::Parrot
        | K::Rabbit
        | K::Silverfish
        | K::Vex => mob(0.3),
        K::Bat => mob(0.25),
        K::TropicalFish => mob(0.15),
        K::Tadpole => mob(0.14),
        // `EnderDragonRenderer` isn't a `LivingEntityRenderer`.
        K::EnderDragon => 0.5,
        _ => 0.0,
    };
    (radius, 1.0)
}

/// Vanilla `getAgeScale`.
fn age_scale(kind: EntityKind, is_baby: bool) -> f32 {
    if !is_baby {
        return 1.0;
    }
    match kind {
        EntityKind::Dolphin => 0.65,
        EntityKind::Camel | EntityKind::CamelHusk | EntityKind::Armadillo => 0.6,
        EntityKind::Goat => 0.55,
        EntityKind::Turtle => 0.3,
        EntityKind::HappyGhast => 0.2375,
        _ => 0.5,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn radius(kind: EntityKind, scale: f32, is_baby: bool) -> f32 {
        let inputs = ShadowInputs {
            scale,
            is_baby,
            slime_size: 2,
            puff_state: 2,
        };
        shadow(kind, &inputs).0
    }

    #[test]
    fn radii_match_the_vanilla_renderers() {
        for (kind, scale, is_baby, expected) in [
            (EntityKind::Cow, 1.0, false, 0.7),
            (EntityKind::Cow, 2.0, false, 1.4),
            (EntityKind::Cow, 1.0, true, 0.35),
            (EntityKind::CaveSpider, 1.0, false, 0.56),
            (EntityKind::HappyGhast, 1.0, false, 2.0),
            (EntityKind::Slime, 3.0, true, 0.5),
            (EntityKind::Pufferfish, 1.0, false, 0.3),
            (EntityKind::Villager, 1.0, true, 0.125),
            (EntityKind::Turtle, 1.0, true, 0.7 * 0.3 * 0.83),
            (EntityKind::Player, 1.0, true, 0.5),
            (EntityKind::Shulker, 1.0, false, 0.0),
            (EntityKind::Arrow, 1.0, false, 0.0),
        ] {
            assert!(
                (radius(kind, scale, is_baby) - expected).abs() < 1e-6,
                "{kind:?} scale {scale} baby {is_baby}"
            );
        }
    }
}
