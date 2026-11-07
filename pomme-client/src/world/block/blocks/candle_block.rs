use azalea_block::BlockState;
use glam::DVec3;

use super::super::{Block, BlockData};
use crate::particles::types::SimpleParticleType;
use crate::position::BlockPos;
use crate::world::level::LevelContext;

// Ordered wick coordinates in sixteenths of a block, measured from its origin.
const CANDLE_OFFSETS: &[&[DVec3]] = &[
    &[DVec3::new(8.0, 8.0, 8.0)],
    &[DVec3::new(6.0, 7.0, 8.0), DVec3::new(10.0, 8.0, 7.0)],
    &[
        DVec3::new(8.0, 5.0, 10.0),
        DVec3::new(6.0, 7.0, 8.0),
        DVec3::new(9.0, 8.0, 7.0),
    ],
    &[
        DVec3::new(7.0, 5.0, 9.0),
        DVec3::new(10.0, 7.0, 9.0),
        DVec3::new(6.0, 7.0, 6.0),
        DVec3::new(9.0, 8.0, 6.0),
    ],
];

pub(in crate::world::block) struct CandleBlock {
    block_data: BlockData,
    smoke: SimpleParticleType,
    small_flame: SimpleParticleType,
}

impl CandleBlock {
    pub(in crate::world::block) fn new(
        block_data: BlockData,
        smoke: SimpleParticleType,
        small_flame: SimpleParticleType,
    ) -> Self {
        Self {
            block_data,
            smoke,
            small_flame,
        }
    }
}

impl Block for CandleBlock {
    fn block_data(&self) -> &BlockData {
        &self.block_data
    }

    fn animate_tick(
        &self,
        state: BlockState,
        pos: BlockPos,
        level: &mut LevelContext<'_>,
        random: &mut fastrand::Rng,
    ) {
        let properties = level
            .block_state_table()
            .block_state_data(state)
            .properties();
        if properties.get("lit") != Some("true") {
            return;
        }
        let Some(offsets) = properties
            .get("candles")
            .and_then(|count| count.parse::<usize>().ok())
            .and_then(|count| count.checked_sub(1))
            .and_then(|index| CANDLE_OFFSETS.get(index))
        else {
            return;
        };
        for &offset in *offsets {
            emit_wick(pos, offset, &self.smoke, &self.small_flame, level, random);
        }
    }
}

pub(in crate::world::block) struct CandleCakeBlock {
    block_data: BlockData,
    smoke: SimpleParticleType,
    small_flame: SimpleParticleType,
}

impl CandleCakeBlock {
    pub(in crate::world::block) fn new(
        block_data: BlockData,
        smoke: SimpleParticleType,
        small_flame: SimpleParticleType,
    ) -> Self {
        Self {
            block_data,
            smoke,
            small_flame,
        }
    }
}

impl Block for CandleCakeBlock {
    fn block_data(&self) -> &BlockData {
        &self.block_data
    }

    fn animate_tick(
        &self,
        state: BlockState,
        pos: BlockPos,
        level: &mut LevelContext<'_>,
        random: &mut fastrand::Rng,
    ) {
        let properties = level
            .block_state_table()
            .block_state_data(state)
            .properties();
        if properties.get("lit") != Some("true") {
            return;
        }
        emit_wick(
            pos,
            DVec3::new(8.0, 16.0, 8.0),
            &self.smoke,
            &self.small_flame,
            level,
            random,
        );
    }
}

fn emit_wick(
    pos: BlockPos,
    offset: DVec3,
    smoke: &SimpleParticleType,
    small_flame: &SimpleParticleType,
    level: &mut LevelContext<'_>,
    random: &mut fastrand::Rng,
) {
    let position = offset * 0.0625_f64 + DVec3::new(pos.x as f64, pos.y as f64, pos.z as f64);
    let chance = random.f32();
    if chance < 0.3_f32 {
        level.add_particle(smoke, position, DVec3::ZERO);
        if chance < 0.17_f32 {
            // COMPAT: Audio is excluded; preserve the volume then pitch RNG draws.
            let _ = 1.0_f32 + random.f32();
            let _ = random.f32() * 0.7_f32 + 0.3_f32;
        }
    }
    level.add_particle(small_flame, position, DVec3::ZERO);
}
