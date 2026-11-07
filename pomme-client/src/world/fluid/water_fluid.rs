use glam::DVec3;

use crate::particles::types::SimpleParticleType;
use crate::position::BlockPos;
use crate::world::block::Fluid;
use crate::world::level::LevelContext;

// COMPAT: Only WaterFluid's client ambient callback is implemented here.
// Drips, source conversion, flow/spread/replacement, block destruction and
// extinguishing contact semantics are outside this checkpoint. Entity travel/
// buoyancy and rendering remain with their existing implementations.
pub(super) struct WaterFluid;

impl WaterFluid {
    pub(super) fn animate_tick(
        pos: BlockPos,
        fluid: Fluid,
        level: &mut LevelContext<'_>,
        random: &mut fastrand::Rng,
        underwater: &SimpleParticleType,
    ) {
        let is_source = fluid.amount == 8 && !fluid.falling;
        if !is_source && !fluid.falling {
            if random.usize(..64) == 0 {
                // Ambient audio is excluded; retain volume then pitch draws.
                let _ = random.f32() * 0.25_f32 + 0.75_f32;
                let _ = random.f32() + 0.5_f32;
            }
        } else if random.usize(..10) == 0 {
            let position = DVec3::new(
                f64::from(pos.x) + random.f64(),
                f64::from(pos.y) + random.f64(),
                f64::from(pos.z) + random.f64(),
            );
            level.add_particle(underwater, position, DVec3::ZERO);
        }
    }
}
