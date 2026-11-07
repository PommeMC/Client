use glam::DVec3;

use crate::particles::types::SimpleParticleType;
use crate::position::BlockPos;
use crate::world::level::LevelContext;

// COMPAT: Only Vanilla LavaFluid's client ambient animation is implemented
// here. Its drip selection, lava-specific flow/spread/replacement rules, random
// fire ignition, and entityInside contact-effect dispatch belong here as added.
// Most are not yet implemented in Pomme. Entity travel/buoyancy and rendering
// remain with their entity and renderer implementations.
pub(super) struct LavaFluid;

impl LavaFluid {
    pub(super) fn animate_tick(
        pos: BlockPos,
        level: &mut LevelContext<'_>,
        random: &mut fastrand::Rng,
        lava: &SimpleParticleType,
    ) {
        let above = BlockPos::new(pos.x, pos.y + 1, pos.z);
        // All three supported air blocks are non-solid-render, so is_air suffices.
        if !level
            .block_state_table()
            .block_state_data(level.get_block_state(above))
            .is_air()
        {
            return;
        }

        if random.usize(..100) == 0 {
            let position = DVec3::new(
                f64::from(pos.x) + random.f64(),
                f64::from(pos.y) + 1.0,
                f64::from(pos.z) + random.f64(),
            );
            level.add_particle(lava, position, DVec3::ZERO);
            // POP audio is excluded; retain volume then pitch draws after admission.
            let _ = 0.2_f32 + random.f32() * 0.2_f32;
            let _ = 0.9_f32 + random.f32() * 0.15_f32;
        }

        if random.usize(..200) == 0 {
            // Ambient audio is excluded; retain volume then pitch draws.
            let _ = 0.2_f32 + random.f32() * 0.2_f32;
            let _ = 0.9_f32 + random.f32() * 0.15_f32;
        }
    }
}
