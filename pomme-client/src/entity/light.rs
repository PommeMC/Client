//! Vanilla `EntityRenderer.getPackedLightCoords`: an entity's light, read at
//! its light probe (the eye), with each renderer's `getBlockLightLevel`.

use azalea_registry::builtin::EntityKind;

use crate::entity::components::Position;
use crate::renderer::chunk::block_lighter::{LightLevels, LightSource};

/// The light an entity of `kind` renders with, its eye at `eye`. `on_fire` is
/// the shared fire flag; `dark_ticks` the glow squid's dark countdown.
// TODO: an armor stand marker probes its brightest block (`ArmorStand.
// getLightProbePosition`), a heart-bound creaking is fire immune, and a mob's
// eye height ignores its pose.
pub fn entity_light(
    world: &impl LightSource,
    kind: EntityKind,
    eye: Position,
    on_fire: bool,
    dark_ticks: i32,
) -> LightLevels {
    let levels = world.levels(eye.floor().as_ivec3().to_array());
    let block = if on_fire && !fire_immune(kind) {
        15
    } else {
        levels.block()
    };
    LightLevels::new(levels.sky(), block_light_level(kind, block, dark_ticks))
}

/// `EntityType.Builder.fireImmune` in `EntityTypes`: these never show as on
/// fire (`Entity.isOnFire`).
fn fire_immune(kind: EntityKind) -> bool {
    use EntityKind::*;
    matches!(
        kind,
        AreaEffectCloud
            | Blaze
            | EnderDragon
            | EndCrystal
            | Ghast
            | MagmaCube
            | Shulker
            | Strider
            | Tnt
            | Vex
            | Warden
            | Wither
            | WitherSkeleton
            | Zoglin
            | ZombifiedPiglin
    )
}

/// The renderers' `getBlockLightLevel` overrides of `block`.
fn block_light_level(kind: EntityKind, block: u8, dark_ticks: i32) -> u8 {
    use EntityKind::*;
    match kind {
        Allay | Blaze | DragonFireball | MagmaCube | ShulkerBullet | Vex | Wither | WitherSkull => {
            15
        }
        ExperienceOrb => (block + 7).min(15),
        // `Mth.clampedLerp` truncated to an int.
        GlowSquid => match ((1.0 - dark_ticks as f32 / 10.0).clamp(0.0, 1.0) * 15.0) as u8 {
            15 => 15,
            glow => glow.max(block),
        },
        _ => block,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::block::BlockStateData;

    /// Sky 12 and block 3 everywhere but y = 0, which is sky 0 and block 9.
    struct Levels;

    impl LightSource for Levels {
        fn state_data(&self, _: [i32; 3]) -> &BlockStateData {
            unreachable!()
        }

        fn levels(&self, [_, y, _]: [i32; 3]) -> LightLevels {
            if y == 0 {
                LightLevels::new(0, 9)
            } else {
                LightLevels::new(12, 3)
            }
        }
    }

    fn light(kind: EntityKind, eye_y: f64, on_fire: bool, dark_ticks: i32) -> LightLevels {
        entity_light(
            &Levels,
            kind,
            Position::new(0.5, eye_y, 0.5),
            on_fire,
            dark_ticks,
        )
    }

    #[test]
    fn probes_the_block_holding_the_eye() {
        assert_eq!(
            light(EntityKind::Cow, 0.99, false, 0),
            LightLevels::new(0, 9)
        );
        assert_eq!(
            light(EntityKind::Cow, 1.0, false, 0),
            LightLevels::new(12, 3)
        );
        assert_eq!(
            light(EntityKind::Cow, -0.01, false, 0),
            LightLevels::new(12, 3)
        );
    }

    #[test]
    fn fire_lights_all_but_the_fire_immune() {
        assert_eq!(
            light(EntityKind::Cow, 1.5, true, 0),
            LightLevels::new(12, 15)
        );
        assert_eq!(
            light(EntityKind::Ghast, 1.5, true, 0),
            LightLevels::new(12, 3)
        );
    }

    #[test]
    fn renderer_overrides() {
        assert_eq!(
            light(EntityKind::Blaze, 1.5, false, 0),
            LightLevels::new(12, 15)
        );
        assert_eq!(
            light(EntityKind::ExperienceOrb, 1.5, false, 0),
            LightLevels::new(12, 10)
        );
        assert_eq!(
            light(EntityKind::ExperienceOrb, 0.5, false, 0),
            LightLevels::new(0, 15)
        );
        assert_eq!(
            light(EntityKind::GlowSquid, 1.5, false, 0),
            LightLevels::new(12, 15)
        );
        // 5 dark ticks left: lerp 0.5 -> 7.
        assert_eq!(
            light(EntityKind::GlowSquid, 1.5, false, 5),
            LightLevels::new(12, 7)
        );
        assert_eq!(
            light(EntityKind::GlowSquid, 0.5, false, 10),
            LightLevels::new(0, 9)
        );
    }
}
