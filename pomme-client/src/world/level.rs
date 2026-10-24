//! Conceptual equivalent to Vanilla ClientLevel/Level.  Provides access to
//! resources other game systems need, without exposing internals of GameState
//! generally.
use azalea_block::BlockState;
use glam::DVec3;

use super::block::block_registry::BlockRegistry;
use super::chunk::ChunkStore;
use crate::position::BlockPos;

/// World services for a block callback; never retains a chunk snapshot.
pub struct LevelContext<'a> {
    chunks: &'a ChunkStore,
}

impl<'a> LevelContext<'a> {
    pub(crate) fn new(chunks: &'a ChunkStore) -> Self {
        Self { chunks }
    }

    /// Missing chunks and positions outside the dimension height read as air.
    pub fn get_block_state(&self, pos: BlockPos) -> BlockState {
        self.chunks.get_block_state(pos.x, pos.y, pos.z)
    }
}

/// Owns persistent coordinate-sampling randomness for world animation.
/// Block rules, session protocol bindings and chunks remain externally owned.
pub struct LevelRuntime {
    // Preserve vanilla's distribution and draw order, not its seeded sequence.
    animation_rng: fastrand::Rng,
}

impl LevelRuntime {
    pub(crate) fn new() -> Self {
        Self {
            animation_rng: fastrand::Rng::new(),
        }
    }

    /// Vanilla `ClientLevel.animateTick`, called once per simulation tick.
    // TODO: vanilla gates this on `!pause && isLevelRunningNormally()`, as it
    // does the particle tick.
    pub(crate) fn animate_tick(
        &mut self,
        block_registry: &BlockRegistry,
        chunks: &ChunkStore,
        player_pos: DVec3,
    ) {
        animate_tick_with_draw(
            block_registry,
            player_pos,
            |radius| self.animation_rng.i32(0..radius),
            &mut LevelContext::new(chunks),
            fastrand::Rng::new(),
        );
    }
}

/// Own the effect stream for this pass; all sampled callbacks borrow it.
// TODO: the rest of `doAnimateTick`: the fluid's `animateTick` and drip
// particles, the creative marker particle and the ambient particles. The drip
// and ambient steps draw from the level random between samples, which
// `sample_animation_blocks` holds for the whole pass.
fn animate_tick_with_draw(
    block_registry: &BlockRegistry,
    player_pos: DVec3,
    next: impl FnMut(i32) -> i32,
    level: &mut LevelContext<'_>,
    mut random: fastrand::Rng,
) {
    sample_animation_blocks(player_pos, next, |pos| {
        block_registry.animate_tick(pos, level, &mut random);
    });
}

/// Each bounded draw is in `0..radius`; preserve vanilla's radius and XYZ
/// order.
fn sample_animation_blocks(
    player_pos: DVec3,
    mut next: impl FnMut(i32) -> i32,
    mut sample: impl FnMut(BlockPos),
) {
    let center = BlockPos::new(
        player_pos.x.floor() as i32,
        player_pos.y.floor() as i32,
        player_pos.z.floor() as i32,
    );
    for _ in 0..667 {
        for radius in [16, 32] {
            let x = center.x + next(radius) - next(radius);
            let y = center.y + next(radius) - next(radius);
            let z = center.z + next(radius) - next(radius);
            sample(BlockPos::new(x, y, z));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn samples_follow_vanilla_draw_order() {
        let mut radii = Vec::new();
        let mut samples = Vec::new();
        sample_animation_blocks(
            DVec3::new(10.5, 64.0, -3.5),
            |radius| {
                radii.push(radius);
                radii.len() as i32 % radius
            },
            |pos| samples.push(pos),
        );
        assert_eq!(samples.len(), 667 * 2);
        // Each sample draws x, y, then z, two draws apiece, at radius 16 then 32.
        assert_eq!(radii.len(), samples.len() * 6);
        for (i, sample) in radii.chunks(6).enumerate() {
            let radius = if i % 2 == 0 { 16 } else { 32 };
            assert!(
                sample.iter().all(|&r| r == radius),
                "sample {i}: {sample:?}"
            );
        }
        assert_eq!(
            samples[0],
            BlockPos::new(10 + 1 - 2, 64 + 3 - 4, -4 + 5 - 6)
        );
    }
}
