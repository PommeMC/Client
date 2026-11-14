use std::ops::Range;
use std::rc::Rc;

use azalea_block::BlockState;
use glam::DVec3;

use super::super::{Block, BlockData};
use crate::particles::dust_particle_options::DustParticleOptions;
use crate::particles::types::ParticleType;
use crate::position::BlockPos;
use crate::world::level::LevelContext;

pub(in crate::world::block) struct RedstoneWireBlock {
    block_data: BlockData,
    dust: Rc<ParticleType<DustParticleOptions>>,
}

impl RedstoneWireBlock {
    pub(in crate::world::block) fn new(
        block_data: BlockData,
        dust: Rc<ParticleType<DustParticleOptions>>,
    ) -> Self {
        Self { block_data, dust }
    }
}

impl Block for RedstoneWireBlock {
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
        let state_data = level.block_state_table().block_state_data(state);
        let properties = state_data.properties();
        let power: usize = properties
            .get("power")
            .and_then(|value| value.parse().ok())
            .unwrap_or(0);
        if power == 0 {
            return;
        }

        let [red, green, blue] = state_data
            .redstone_wire_rgb()
            .map(|channel| (channel * 255.0).floor() as u32 & 0xff);
        let packed_color = 0xff000000 | red << 16 | green << 8 | blue;
        let options = DustParticleOptions::new(Rc::clone(&self.dust), packed_color, 1.0);

        for (property, horizontal) in [
            ("north", [0.0, 0.0, -1.0]),
            ("east", [1.0, 0.0, 0.0]),
            ("south", [0.0, 0.0, 1.0]),
            ("west", [-1.0, 0.0, 0.0]),
        ] {
            let connection = properties.get(property);
            if connection == Some("up") {
                spawn_particles_along_line(
                    level,
                    random,
                    pos,
                    &options,
                    horizontal,
                    [0.0, 1.0, 0.0],
                    -0.5..0.5,
                );
            }
            let end = match connection {
                Some("up" | "side") => 0.5,
                _ => 0.3,
            };
            spawn_particles_along_line(
                level,
                random,
                pos,
                &options,
                [0.0, -1.0, 0.0],
                horizontal,
                0.0..end,
            );
        }
    }
}

fn spawn_particles_along_line(
    level: &mut LevelContext<'_>,
    random: &mut fastrand::Rng,
    pos: BlockPos,
    options: &DustParticleOptions,
    side: [f32; 3],
    along: [f32; 3],
    range: Range<f32>,
) {
    let span = range.end - range.start;
    if random.f32() >= 0.2 * span {
        return;
    }
    let position = range.start + span * random.f32();
    let local = |axis| 0.5 + f64::from(0.4375 * side[axis]) + f64::from(position * along[axis]);
    level.add_particle(
        options,
        DVec3::new(
            pos.x as f64 + local(0),
            pos.y as f64 + local(1),
            pos.z as f64 + local(2),
        ),
        DVec3::ZERO,
    );
}
