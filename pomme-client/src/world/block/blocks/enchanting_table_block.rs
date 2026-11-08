use azalea_block::BlockState;
use glam::DVec3;

use super::super::{Block, BlockData};
use crate::particles::types::SimpleParticleType;
use crate::position::BlockPos;
use crate::world::level::LevelContext;

pub(in crate::world::block) struct EnchantingTableBlock {
    block_data: BlockData,
    enchant: SimpleParticleType,
}

impl EnchantingTableBlock {
    pub(in crate::world::block) fn new(block_data: BlockData, enchant: SimpleParticleType) -> Self {
        Self {
            block_data,
            enchant,
        }
    }
}

impl Block for EnchantingTableBlock {
    fn block_data(&self) -> &BlockData {
        &self.block_data
    }

    fn animate_tick(
        &self,
        _state: BlockState,
        pos: BlockPos,
        level: &mut LevelContext<'_>,
        random: &mut fastrand::Rng,
    ) {
        // BlockPos.betweenClosed visits x fastest, then y, then z.
        for z in -2_i32..=2 {
            for y in 0..=1 {
                for x in -2_i32..=2 {
                    if x.abs() != 2 && z.abs() != 2 {
                        continue;
                    }
                    if random.i32(0..16) == 0
                        && level.is_block_tag(
                            level.get_block_state(BlockPos::new(pos.x + x, pos.y + y, pos.z + z)),
                            "minecraft:enchantment_power_provider",
                        )
                        && level.is_block_tag(
                            level.get_block_state(BlockPos::new(
                                pos.x + x / 2,
                                pos.y + y,
                                pos.z + z / 2,
                            )),
                            "minecraft:enchantment_power_transmitter",
                        )
                    {
                        let xa = f64::from(x as f32 + random.f32()) - 0.5;
                        let ya = f64::from(y as f32 - random.f32() - 1.0_f32);
                        let za = f64::from(z as f32 + random.f32()) - 0.5;
                        level.add_particle(
                            &self.enchant,
                            DVec3::new(
                                f64::from(pos.x) + 0.5,
                                f64::from(pos.y) + 2.0,
                                f64::from(pos.z) + 0.5,
                            ),
                            DVec3::new(xa, ya, za),
                        );
                    }
                }
            }
        }
    }
}
