//! COMPAT: Vanilla's client.renderer.LevelEventHandler owns dispatch, sounds,
//! particles, and other client effects. This module currently contains only
//! particle emission helpers; dispatch remains in app/core.rs. Revisit its
//! placement when porting the broader Vanilla handler.

use glam::DVec3;

use super::block::model::Direction;
use super::level::LevelContext;
use crate::particles::types::SimpleParticleType;
use crate::position::BlockPos;
use crate::util::random_source::RandomSource;

pub(crate) fn shoot_particles(
    level: &mut LevelContext<'_>,
    pos: BlockPos,
    data: u32,
    smoke: &SimpleParticleType,
) {
    // Vanilla takes the signed remainder before abs, including for i32::MIN.
    let direction = Direction::VALUES[((data as i32) % 6).abs() as usize];
    let [nx, ny, nz] = direction.offset().map(f64::from);
    let random = level.get_random().clone();

    for _ in 0..10 {
        let pow = random.next_double() * 0.2 + 0.01;
        // Preserve Vanilla's vertical Y scatter and horizontal cross-axis scatter.
        let x =
            f64::from(pos.x) + nx * 0.6 + 0.5 + nx * 0.01 + (random.next_double() - 0.5) * nz * 0.5;
        let y =
            f64::from(pos.y) + ny * 0.6 + 0.5 + ny * 0.01 + (random.next_double() - 0.5) * ny * 0.5;
        let z =
            f64::from(pos.z) + nz * 0.6 + 0.5 + nz * 0.01 + (random.next_double() - 0.5) * nx * 0.5;
        let vx = nx * pow + next_gaussian(&random) * 0.01;
        let vy = ny * pow + next_gaussian(&random) * 0.01;
        let vz = nz * pow + next_gaussian(&random) * 0.01;
        level.add_particle(smoke, DVec3::new(x, y, z), DVec3::new(vx, vy, vz));
    }
}

pub(crate) fn spawner_particles(
    level: &mut LevelContext<'_>,
    pos: BlockPos,
    smoke: &SimpleParticleType,
    flame: &SimpleParticleType,
) {
    let random = level.get_random().clone();
    for _ in 0..20 {
        let x = f64::from(pos.x) + 0.5 + (random.next_double() - 0.5) * 2.0;
        let y = f64::from(pos.y) + 0.5 + (random.next_double() - 0.5) * 2.0;
        let z = f64::from(pos.z) + 0.5 + (random.next_double() - 0.5) * 2.0;
        let position = DVec3::new(x, y, z);
        level.add_particle(smoke, position, DVec3::ZERO);
        level.add_particle(flame, position, DVec3::ZERO);
    }
}

// COMPAT: uncached polar sampling on the shared LEVEL stream preserves the
// distribution and draw purposes, not Java's cached Gaussian/raw seeded
// sequence.
fn next_gaussian(random: &RandomSource) -> f64 {
    loop {
        let v1 = 2.0 * random.next_double() - 1.0;
        let v2 = 2.0 * random.next_double() - 1.0;
        let s = v1 * v1 + v2 * v2;
        if s > 0.0 && s < 1.0 {
            return v1 * (-2.0 * s.ln() / s).sqrt();
        }
    }
}

pub(crate) fn spawn_particle_in_block(
    level: &mut LevelContext<'_>,
    pos: BlockPos,
    count: i32,
    particle: &SimpleParticleType,
) {
    let state = level.get_block_state(pos);
    let data = level.block_state_table().block_state_data(state);
    let spread_height = if data.is_air() {
        1.0
    } else {
        let max_y = crate::physics::block_shape::outline_shape(data)
            .iter()
            .map(|bounds| bounds[4])
            .fold(f64::NEG_INFINITY, f64::max);
        let delta_y = data.outline_shape_position(pos.x, 0, pos.z).y;
        max_y + delta_y
    };
    let random = level.get_random().clone();

    for _ in 0..count {
        let vx = next_gaussian(&random) * 0.02;
        let vy = next_gaussian(&random) * 0.02;
        let vz = next_gaussian(&random) * 0.02;
        let x = f64::from(pos.x) + 0.0 + random.next_double() * 0.5 * 2.0;
        let y = f64::from(pos.y) + random.next_double() * spread_height;
        let z = f64::from(pos.z) + 0.0 + random.next_double() * 0.5 * 2.0;
        level.add_particle(particle, DVec3::new(x, y, z), DVec3::new(vx, vy, vz));
    }
}
