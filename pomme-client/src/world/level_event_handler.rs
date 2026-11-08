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

pub(crate) fn composter_fill_particles(
    level: &mut LevelContext<'_>,
    pos: BlockPos,
    composter: &SimpleParticleType,
) {
    let state = level.get_block_state(pos);
    let random = level.get_random().clone();
    // COMPAT: sound playback is deferred, but its LEVEL seed draw is unconditional,
    // even when muted or distant. Success data only selects the omitted sound.
    let _ = random.next_long();
    let data = level.block_state_table().block_state_data(state);
    let origin = DVec3::new(f64::from(pos.x), 0.0, f64::from(pos.z));
    let delta = data.outline_shape_position(pos.x, 0, pos.z) - origin;
    let center_height = crate::physics::block_shape::outline_shape(data)
        .iter()
        .filter(|bounds| {
            bounds[0] + delta.x <= 0.5
                && 0.5 < bounds[3] + delta.x
                && bounds[2] + delta.z <= 0.5
                && 0.5 < bounds[5] + delta.z
        })
        .map(|bounds| bounds[4])
        .fold(f64::NEG_INFINITY, f64::max)
        + delta.y
        + 0.03125;

    for _ in 0..10 {
        let vx = next_gaussian(&random) * 0.02;
        let vy = next_gaussian(&random) * 0.02;
        let vz = next_gaussian(&random) * 0.02;
        let x = f64::from(pos.x) + 0.1875 + 0.625 * f64::from(random.next_float());
        let y = f64::from(pos.y)
            + center_height
            + f64::from(random.next_float()) * (1.0 - center_height);
        let z = f64::from(pos.z) + 0.1875 + 0.625 * f64::from(random.next_float());
        level.add_particle(composter, DVec3::new(x, y, z), DVec3::new(vx, vy, vz));
    }
}

pub(crate) fn spawn_particles_on_block_faces(
    level: &mut LevelContext<'_>,
    pos: BlockPos,
    particle: &SimpleParticleType,
) {
    let random = level.get_random().clone();
    let center = DVec3::new(
        f64::from(pos.x) + 0.5,
        f64::from(pos.y) + 0.5,
        f64::from(pos.z) + 0.5,
    );
    for direction in Direction::VALUES {
        let count = random.next_int(3) + 3;
        let step = direction.offset();
        for _ in 0..count {
            let speed = DVec3::new(
                random.next_double() * (0.5 - (-0.5)) + (-0.5),
                random.next_double() * (0.5 - (-0.5)) + (-0.5),
                random.next_double() * (0.5 - (-0.5)) + (-0.5),
            );
            let mut position = center;
            let mut velocity = speed;
            for axis in 0..3 {
                position[axis] += if step[axis] == 0 {
                    random.next_double() * (0.5 - (-0.5)) + (-0.5)
                } else {
                    f64::from(step[axis]) * 0.55
                };
                if step[axis] != 0 {
                    velocity[axis] = 0.0;
                }
            }
            level.add_particle(particle, position, velocity);
        }
    }
}
