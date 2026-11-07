mod lava_fluid;
mod water_fluid;

use lava_fluid::LavaFluid;
use water_fluid::WaterFluid;

use super::block::FluidKind;
use super::level::LevelContext;
use crate::particles::types::SimpleParticleType;
use crate::position::BlockPos;

pub(super) fn animate_tick(
    pos: BlockPos,
    level: &mut LevelContext<'_>,
    random: &mut fastrand::Rng,
    lava: &SimpleParticleType,
    underwater: &SimpleParticleType,
) {
    let fluid = level
        .block_state_table()
        .block_state_data(level.get_block_state(pos))
        .fluid();
    // COMPAT: Vanilla FluidState delegates animateTick to its registered Fluid.
    // This particle-only change uses FluidKind; an equivalent fluid registry is out
    // of scope.
    match fluid.kind {
        FluidKind::Lava => LavaFluid::animate_tick(pos, level, random, lava),
        FluidKind::Water => WaterFluid::animate_tick(pos, fluid, level, random, underwater),
        FluidKind::Empty => {}
    }
}
