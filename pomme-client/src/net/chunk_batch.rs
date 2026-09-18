//! Port of vanilla `client/multiplayer/ChunkBatchSizeCalculator.java` (26.2).
//!
//! The server sends chunks in batches and asks, after each one, how many
//! chunks per tick the client wants next (`ServerboundChunkBatchReceived`).
//! Vanilla answers from how long the batch took per chunk, starting
//! deliberately low and ramping, which is what paces the world in on join.

use std::time::Instant;

const MAX_OLD_SAMPLES_WEIGHT: u32 = 49;
/// Vanilla `CLAMP_COEFFICIENT`: one batch can move the average by at most 3x
/// in either direction.
const CLAMP_COEFFICIENT: f64 = 3.0;
/// Vanilla's starting estimate, 2ms per chunk, i.e. an opening ask of 3.5.
const INITIAL_NANOS_PER_CHUNK: f64 = 2_000_000.0;
/// Vanilla's per-tick chunk budget: 7ms of the 50ms tick.
const NANOS_PER_TICK_BUDGET: f64 = 7_000_000.0;

pub struct ChunkBatchSizeCalculator {
    aggregated_nanos_per_chunk: f64,
    old_samples_weight: u32,
    batch_start: Instant,
}

impl Default for ChunkBatchSizeCalculator {
    fn default() -> Self {
        Self {
            aggregated_nanos_per_chunk: INITIAL_NANOS_PER_CHUNK,
            old_samples_weight: 1,
            batch_start: Instant::now(),
        }
    }
}

impl ChunkBatchSizeCalculator {
    pub fn on_batch_start(&mut self) {
        self.batch_start = Instant::now();
    }

    pub fn on_batch_finished(&mut self, batch_size: u32) {
        if batch_size == 0 {
            return;
        }
        let nanos_per_chunk = self.batch_start.elapsed().as_nanos() as f64 / f64::from(batch_size);
        let clamped = nanos_per_chunk.clamp(
            self.aggregated_nanos_per_chunk / CLAMP_COEFFICIENT,
            self.aggregated_nanos_per_chunk * CLAMP_COEFFICIENT,
        );
        let weight = f64::from(self.old_samples_weight);
        self.aggregated_nanos_per_chunk =
            (self.aggregated_nanos_per_chunk * weight + clamped) / (weight + 1.0);
        self.old_samples_weight = (self.old_samples_weight + 1).min(MAX_OLD_SAMPLES_WEIGHT);
    }

    pub fn desired_chunks_per_tick(&self) -> f32 {
        (NANOS_PER_TICK_BUDGET / self.aggregated_nanos_per_chunk) as f32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opens_at_vanillas_three_and_a_half_chunks_per_tick() {
        assert_eq!(
            ChunkBatchSizeCalculator::default().desired_chunks_per_tick(),
            3.5
        );
    }

    #[test]
    fn a_slow_batch_is_clamped_to_three_times_the_average() {
        // A batch that took far longer per chunk than the running average only
        // counts as 3x it, so one stall can't collapse the ask.
        let mut calc = ChunkBatchSizeCalculator {
            batch_start: Instant::now() - std::time::Duration::from_secs(1),
            ..Default::default()
        };
        calc.on_batch_finished(1);

        let clamped = INITIAL_NANOS_PER_CHUNK * CLAMP_COEFFICIENT;
        let expected = (INITIAL_NANOS_PER_CHUNK + clamped) / 2.0;
        assert_eq!(
            calc.desired_chunks_per_tick(),
            (7_000_000.0 / expected) as f32
        );
    }

    #[test]
    fn an_empty_batch_changes_nothing() {
        let mut calc = ChunkBatchSizeCalculator {
            batch_start: Instant::now() - std::time::Duration::from_secs(1),
            ..Default::default()
        };
        calc.on_batch_finished(0);
        assert_eq!(calc.desired_chunks_per_tick(), 3.5);
    }

    #[test]
    fn fast_batches_ramp_the_ask_up() {
        let mut calc = ChunkBatchSizeCalculator::default();
        let mut previous = calc.desired_chunks_per_tick();
        for _ in 0..5 {
            calc.on_batch_start();
            calc.on_batch_finished(10);
            let now = calc.desired_chunks_per_tick();
            assert!(now > previous, "{now} should exceed {previous}");
            previous = now;
        }
    }
}
