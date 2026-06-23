use std::path::Path;
use std::time::Instant;

use crate::renderer::RenderTimings;

const DURATION_SECS: f32 = 10.0;
const WARMUP_FRAMES: u32 = 30;
const SPIKE_THRESHOLD_MS: f32 = 8.0;

/// A rough UTC timestamp (`YYYY-MM-DDTHH:MM:SSZ`) for stamping benchmark
/// results that get reported back.
fn iso8601_utc_now() -> String {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| {
            let secs = d.as_secs();
            let h = (secs / 3600) % 24;
            let m = (secs / 60) % 60;
            let s = secs % 60;
            format!(
                "{:04}-{:02}-{:02}T{h:02}:{m:02}:{s:02}Z",
                1970 + secs / 31557600,
                (secs % 31557600) / 2629800 + 1,
                (secs % 2629800) / 86400 + 1,
            )
        })
        .unwrap_or_default()
}

#[derive(Clone, serde::Serialize)]
pub struct FrameSample {
    pub frame_ms: f32,
    pub fence_ms: f32,
    pub cull_ms: f32,
    pub draw_ms: f32,
    pub chunk_count: u32,
    pub entity_count: u32,
}

#[derive(Clone, serde::Serialize)]
pub struct SpikeSample {
    pub frame_index: u32,
    pub frame_ms: f32,
    pub fence_ms: f32,
    pub cull_ms: f32,
    pub draw_ms: f32,
    pub chunk_count: u32,
    pub entity_count: u32,
}

pub struct Benchmark {
    start: Instant,
    samples: Vec<FrameSample>,
    spikes: Vec<SpikeSample>,
    warmup_remaining: u32,
    gpu_name: String,
    resolution: (u32, u32),
    render_distance: u32,
}

#[derive(serde::Serialize)]
pub struct BenchmarkResult {
    pub version: String,
    pub os: String,
    pub arch: String,
    pub gpu: String,
    pub resolution: [u32; 2],
    pub render_distance: u32,
    pub timestamp: String,
    pub total_frames: u32,
    pub duration_secs: f32,
    pub avg_fps: f32,
    pub min_fps: f32,
    pub max_fps: f32,
    pub avg_frame_ms: f32,
    pub p1_frame_ms: f32,
    pub p99_frame_ms: f32,
    pub avg_fence_ms: f32,
    pub avg_cull_ms: f32,
    pub avg_draw_ms: f32,
    pub peak_chunk_count: u32,
    pub peak_entity_count: u32,
    pub spike_count: u32,
    pub spikes: Vec<SpikeSample>,
}

impl Benchmark {
    pub fn new(gpu_name: &str, width: u32, height: u32, render_distance: u32) -> Self {
        Self {
            start: Instant::now(),
            samples: Vec::with_capacity(6000),
            spikes: Vec::new(),
            warmup_remaining: WARMUP_FRAMES,
            gpu_name: gpu_name.to_owned(),
            resolution: (width, height),
            render_distance,
        }
    }

    pub fn record_frame(
        &mut self,
        frame_ms: f32,
        timings: &RenderTimings,
        chunk_count: u32,
        entity_count: u32,
    ) -> bool {
        if self.warmup_remaining > 0 {
            self.warmup_remaining -= 1;
            if self.warmup_remaining == 0 {
                self.start = Instant::now();
            }
            return false;
        }

        let sample = FrameSample {
            frame_ms,
            fence_ms: timings.fence_ms,
            cull_ms: timings.cull_ms,
            draw_ms: timings.draw_ms,
            chunk_count,
            entity_count,
        };

        if frame_ms > SPIKE_THRESHOLD_MS {
            self.spikes.push(SpikeSample {
                frame_index: self.samples.len() as u32,
                frame_ms: sample.frame_ms,
                fence_ms: sample.fence_ms,
                cull_ms: sample.cull_ms,
                draw_ms: sample.draw_ms,
                chunk_count: sample.chunk_count,
                entity_count: sample.entity_count,
            });
        }

        self.samples.push(sample);
        self.start.elapsed().as_secs_f32() >= DURATION_SECS
    }

    pub fn finish(self, game_dir: &Path) -> BenchmarkResult {
        let count = self.samples.len().max(1);
        let mut frame_times: Vec<f32> = self.samples.iter().map(|s| s.frame_ms).collect();
        frame_times.sort_by(|a, b| a.partial_cmp(b).unwrap());

        let sum: f32 = frame_times.iter().sum();
        let avg_ms = sum / count as f32;
        let p1_idx = ((count as f32 * 0.99) as usize).min(count - 1);
        let p99_idx = (count as f32 * 0.01) as usize;

        let fence_sum: f32 = self.samples.iter().map(|s| s.fence_ms).sum();
        let cull_sum: f32 = self.samples.iter().map(|s| s.cull_ms).sum();
        let draw_sum: f32 = self.samples.iter().map(|s| s.draw_ms).sum();
        let peak_chunks = self
            .samples
            .iter()
            .map(|s| s.chunk_count)
            .max()
            .unwrap_or(0);
        let peak_entities = self
            .samples
            .iter()
            .map(|s| s.entity_count)
            .max()
            .unwrap_or(0);

        let now = iso8601_utc_now();

        let result = BenchmarkResult {
            version: env!("CARGO_PKG_VERSION").to_owned(),
            os: std::env::consts::OS.to_owned(),
            arch: std::env::consts::ARCH.to_owned(),
            gpu: self.gpu_name,
            resolution: [self.resolution.0, self.resolution.1],
            render_distance: self.render_distance,
            timestamp: now,
            total_frames: count as u32,
            duration_secs: DURATION_SECS,
            avg_fps: 1000.0 / avg_ms,
            min_fps: 1000.0 / frame_times[p1_idx],
            max_fps: 1000.0 / frame_times[p99_idx].max(0.001),
            avg_frame_ms: avg_ms,
            p1_frame_ms: frame_times[p1_idx],
            p99_frame_ms: frame_times[p99_idx],
            avg_fence_ms: fence_sum / count as f32,
            avg_cull_ms: cull_sum / count as f32,
            avg_draw_ms: draw_sum / count as f32,
            peak_chunk_count: peak_chunks,
            peak_entity_count: peak_entities,
            spike_count: self.spikes.len() as u32,
            spikes: self.spikes,
        };

        let path = game_dir.join("benchmark.json");
        if let Ok(json) = serde_json::to_string_pretty(&result) {
            let _ = std::fs::write(&path, json);
            tracing::info!("Benchmark saved to {}", path.display());
        }

        result
    }

    pub fn progress(&self) -> f32 {
        if self.warmup_remaining > 0 {
            return 0.0;
        }
        (self.start.elapsed().as_secs_f32() / DURATION_SECS).min(1.0)
    }
}

/// Lowest render distance to drop to during the chunk-load reset phase.
pub const CHUNK_LOAD_MIN_RD: u32 = 2;
/// Minimum time to hold the minimum render distance before the timed load can
/// start, so the server has a chance to begin unloading the far chunks.
const CHUNK_RESET_MIN_SECS: f32 = 0.75;
/// The reset is done once the loaded-chunk count has stopped dropping for this
/// long — i.e. the server has finished unloading — regardless of latency.
const CHUNK_RESET_STABLE_SECS: f32 = 0.5;
/// Loading is considered finished once the loaded-chunk count holds steady for
/// this long.
const CHUNK_STABLE_SECS: f32 = 1.5;
/// Safety cap so a stalled/capped load can't run forever.
const CHUNK_TIMEOUT_SECS: f32 = 90.0;

#[derive(Clone, serde::Serialize)]
pub struct ChunkLoadResult {
    pub version: String,
    pub os: String,
    pub arch: String,
    pub gpu: String,
    pub vulkan: String,
    pub cpu_threads: u32,
    pub resolution: [u32; 2],
    pub timestamp: String,
    pub target_rd: u32,
    pub effective_rd: u32,
    pub chunk_count: u32,
    /// Wall-clock from raising the render distance to the last chunk landing.
    pub load_secs: f32,
    pub chunks_per_sec: f32,
    /// Time from the raise to the first new chunk landing — server/network
    /// response latency before throughput kicks in.
    pub time_to_first_secs: f32,
    /// Average and worst frame time observed while loading — the hitching you
    /// feel as chunks mesh and upload.
    pub avg_frame_ms: f32,
    pub worst_frame_ms: f32,
}

impl ChunkLoadResult {
    pub fn save(&self, game_dir: &Path) {
        let path = game_dir.join("chunk_load.json");
        if let Ok(json) = serde_json::to_string_pretty(self) {
            let _ = std::fs::write(&path, json);
            tracing::info!("Chunk load result saved to {}", path.display());
        }
    }
}

enum ChunkPhase {
    Reset,
    Load,
}

/// What the per-frame driver should do with the render distance this frame.
pub enum ChunkLoadStep {
    /// Nothing to apply; keep waiting/measuring.
    Wait,
    /// Apply this render distance and sync it to the server — the timed load
    /// starts now.
    Load(u32),
    /// Loading finished; the driver should restore the original render
    /// distance.
    Done(ChunkLoadResult),
}

/// Measures how long it takes to load every chunk in a chosen render-distance
/// radius. First drops to [`CHUNK_LOAD_MIN_RD`] so the server unloads the far
/// chunks, then raises to the target and times the fresh load until the
/// loaded-chunk count stops rising.
pub struct ChunkLoadBench {
    phase: ChunkPhase,
    target_rd: u32,
    effective_rd: u32,
    original_rd: u32,
    gpu_name: String,
    vulkan: String,
    resolution: (u32, u32),
    reset_start: Instant,
    start: Instant,
    last_count: u32,
    last_change: Instant,
    /// Loaded count when the timed load began (the reset baseline).
    baseline_count: u32,
    /// When the first chunk past the baseline landed.
    first_load_at: Option<Instant>,
    frame_ms_sum: f32,
    frame_ms_max: f32,
    frame_samples: u32,
}

impl ChunkLoadBench {
    pub fn new(
        target_rd: u32,
        original_rd: u32,
        server_rd: u32,
        gpu_name: &str,
        vulkan: &str,
        width: u32,
        height: u32,
    ) -> Self {
        let effective_rd = if server_rd > 0 {
            target_rd.min(server_rd)
        } else {
            target_rd
        };
        let now = Instant::now();
        Self {
            phase: ChunkPhase::Reset,
            target_rd,
            effective_rd,
            original_rd,
            gpu_name: gpu_name.to_owned(),
            vulkan: vulkan.to_owned(),
            resolution: (width, height),
            reset_start: now,
            start: now,
            last_count: 0,
            last_change: now,
            baseline_count: 0,
            first_load_at: None,
            frame_ms_sum: 0.0,
            frame_ms_max: 0.0,
            frame_samples: 0,
        }
    }

    pub fn update(&mut self, loaded_count: u32, frame_ms: f32) -> ChunkLoadStep {
        match self.phase {
            ChunkPhase::Reset => {
                // Wait for the unload to settle (count stops dropping) so the
                // timed load always starts from a clean low baseline, even on a
                // laggy connection.
                if loaded_count != self.last_count {
                    self.last_count = loaded_count;
                    self.last_change = Instant::now();
                }
                let min_elapsed = self.reset_start.elapsed().as_secs_f32() >= CHUNK_RESET_MIN_SECS;
                let settled = self.last_change.elapsed().as_secs_f32() >= CHUNK_RESET_STABLE_SECS;
                if min_elapsed && settled {
                    let now = Instant::now();
                    self.phase = ChunkPhase::Load;
                    self.start = now;
                    self.last_change = now;
                    self.last_count = loaded_count;
                    self.baseline_count = loaded_count;
                    ChunkLoadStep::Load(self.target_rd)
                } else {
                    ChunkLoadStep::Wait
                }
            }
            ChunkPhase::Load => {
                self.frame_ms_sum += frame_ms;
                self.frame_ms_max = self.frame_ms_max.max(frame_ms);
                self.frame_samples += 1;

                if loaded_count != self.last_count {
                    self.last_count = loaded_count;
                    self.last_change = Instant::now();
                }
                if self.first_load_at.is_none() && loaded_count > self.baseline_count {
                    self.first_load_at = Some(Instant::now());
                }

                let stable = loaded_count > 0
                    && self.last_change.elapsed().as_secs_f32() >= CHUNK_STABLE_SECS;
                let timeout = self.start.elapsed().as_secs_f32() >= CHUNK_TIMEOUT_SECS;
                if stable || timeout {
                    let load_secs = self
                        .last_change
                        .saturating_duration_since(self.start)
                        .as_secs_f32();
                    let chunks_per_sec = if load_secs > 0.0 {
                        loaded_count as f32 / load_secs
                    } else {
                        0.0
                    };
                    let time_to_first_secs = self
                        .first_load_at
                        .map(|t| t.saturating_duration_since(self.start).as_secs_f32())
                        .unwrap_or(0.0);
                    let avg_frame_ms = if self.frame_samples > 0 {
                        self.frame_ms_sum / self.frame_samples as f32
                    } else {
                        0.0
                    };
                    ChunkLoadStep::Done(ChunkLoadResult {
                        version: env!("CARGO_PKG_VERSION").to_owned(),
                        os: std::env::consts::OS.to_owned(),
                        arch: std::env::consts::ARCH.to_owned(),
                        gpu: self.gpu_name.clone(),
                        vulkan: self.vulkan.clone(),
                        cpu_threads: std::thread::available_parallelism()
                            .map(|n| n.get() as u32)
                            .unwrap_or(0),
                        resolution: [self.resolution.0, self.resolution.1],
                        timestamp: iso8601_utc_now(),
                        target_rd: self.target_rd,
                        effective_rd: self.effective_rd,
                        chunk_count: loaded_count,
                        load_secs,
                        chunks_per_sec,
                        time_to_first_secs,
                        avg_frame_ms,
                        worst_frame_ms: self.frame_ms_max,
                    })
                } else {
                    ChunkLoadStep::Wait
                }
            }
        }
    }

    pub fn original_rd(&self) -> u32 {
        self.original_rd
    }

    pub fn target_rd(&self) -> u32 {
        self.target_rd
    }

    pub fn loaded(&self) -> u32 {
        self.last_count
    }

    pub fn resetting(&self) -> bool {
        matches!(self.phase, ChunkPhase::Reset)
    }
}
