use std::path::Path;
use std::time::Instant;

use crate::renderer::RenderTimings;

const DURATION_SECS: f32 = 10.0;
const WARMUP_FRAMES: u32 = 30;
const SPIKE_THRESHOLD_MS: f32 = 8.0;

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

        let now = std::time::SystemTime::now()
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
            .unwrap_or_default();

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
