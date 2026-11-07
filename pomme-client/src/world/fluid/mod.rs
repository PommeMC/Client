mod lava_fluid;
mod water_fluid;

use lava_fluid::LavaFluid;
use water_fluid::WaterFluid;

use super::block::{Fluid, FluidKind};
use super::level::LevelContext;
use crate::particles::types::SimpleParticleType;
use crate::position::BlockPos;

pub(super) fn animate_tick(
    pos: BlockPos,
    fluid: Fluid,
    level: &mut LevelContext<'_>,
    random: &mut fastrand::Rng,
    lava: &SimpleParticleType,
    underwater: &SimpleParticleType,
) {
    // COMPAT: Vanilla FluidState delegates animateTick to its registered Fluid.
    // This particle-only change uses FluidKind; an equivalent fluid registry is out
    // of scope.
    match fluid.kind {
        FluidKind::Lava => LavaFluid::animate_tick(pos, level, random, lava),
        FluidKind::Water => WaterFluid::animate_tick(pos, fluid, level, random, underwater),
        FluidKind::Empty => {}
    }
}

pub(super) fn get_drip_particle<'a>(
    fluid: Fluid,
    dripping_water: &'a SimpleParticleType,
    dripping_lava: &'a SimpleParticleType,
) -> Option<&'a SimpleParticleType> {
    match fluid.kind {
        FluidKind::Water => Some(WaterFluid::get_drip_particle(dripping_water)),
        FluidKind::Lava => Some(LavaFluid::get_drip_particle(dripping_lava)),
        FluidKind::Empty => None,
    }
}
