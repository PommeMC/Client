use std::path::Path;
use std::time::Instant;

use crate::renderer::RenderTimings;

const DURATION_SECS: f32 = 10.0;
const WARMUP_FRAMES: u32 = 30;
const SPIKE_THRESHOLD_MS: f32 = 8.0;

/// A UTC timestamp (`YYYY-MM-DDTHH:MM:SSZ`) for stamping benchmark results that
/// get reported back.
fn iso8601_utc_now() -> String {
    let fmt = time::macros::format_description!("[year]-[month]-[day]T[hour]:[minute]:[second]Z");
    time::OffsetDateTime::now_utc()
        .format(&fmt)
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
/// Loading is done once the loaded-chunk count holds steady for this long —
/// long enough to ride out the server's inter-batch gaps at high render
/// distances, so a mid-stream pause isn't mistaken for completion.
const CHUNK_LOAD_STALL_SECS: f32 = 8.0;
/// ...or as soon as this fraction of the target radius's columns have loaded.
const CHUNK_LOAD_COMPLETE_FRAC: f32 = 0.98;
/// Safety cap so a stalled/capped load can't run forever.
const CHUNK_TIMEOUT_SECS: f32 = 90.0;
/// First run(s) are discarded as warmup (cold disk/network caches).
pub const CHUNK_LOAD_WARMUP_RUNS: u32 = 1;
/// Runs that actually count toward the averaged result.
pub const CHUNK_LOAD_MEASURED_RUNS: u32 = 3;
const CHUNK_LOAD_TOTAL_RUNS: u32 = CHUNK_LOAD_WARMUP_RUNS + CHUNK_LOAD_MEASURED_RUNS;
const MEASUREMENT_NOTE: &str =
    "frame_ms measured with entities, weather, and HUD hidden (top-down benchmark view)";

/// Debug builds run unoptimized, so their timings are far slower and not
/// comparable to release — results record and surface which one produced them.
pub fn is_debug_build() -> bool {
    cfg!(debug_assertions)
}

pub fn build_profile() -> &'static str {
    if is_debug_build() { "debug" } else { "release" }
}

/// Columns in a fully-loaded square of the given radius: (2r+1)².
fn expected_columns(rd: u32) -> u32 {
    let d = 2 * rd + 1;
    d * d
}

/// Infer the loaded radius from a (roughly square) loaded area: count ≈
/// (2r+1)². Servers often don't advertise their view distance (proxies, dynamic
/// VD), so this is what actually loaded — the honest number when the target is
/// unreachable.
fn radius_from_chunk_count(count: u32) -> u32 {
    if count == 0 {
        return 0;
    }
    (((count as f32).sqrt() - 1.0) / 2.0).round().max(0.0) as u32
}

/// `update_game`'s CPU phase timings — the per-frame work not covered by the
/// render timings. Set each frame and folded into [`FrameBreakdown`].
/// `update_ms` is the whole-`update_game` wall time (including the render
/// call); if it is far below `total_ms`, the hitch is outside `update_game`
/// (framerate limiter / OS scheduling / inter-frame gap) rather than in any CPU
/// phase.
#[derive(Clone, Copy, Default, serde::Serialize)]
pub struct UpdatePhases {
    pub update_ms: f32,
    pub net_decode_ms: f32,
    pub visibility_ms: f32,
    pub rescan_ms: f32,
    pub mesh_drain_ms: f32,
    pub upload_ms: f32,
}

/// Phase split of a run's single worst frame, to localize a hitch. `total_ms`
/// is the wall-clock frame (`raw_dt`); `render_ms` the `render_frame` portion
/// (which includes `fence_ms`, the GPU-bound wait); the `update` phases cover
/// the rest. All sub-timings reflect the same prior frame `raw_dt` measures, so
/// the split lines up; whatever `total_ms` exceeds the parts is time spent
/// outside `update_game` (limiter / OS scheduling / inter-frame gap).
#[derive(Clone, Default, serde::Serialize)]
pub struct FrameBreakdown {
    pub total_ms: f32,
    pub render_ms: f32,
    pub fence_ms: f32,
    pub acquire_ms: f32,
    pub cull_ms: f32,
    pub present_ms: f32,
    #[serde(flatten)]
    pub update: UpdatePhases,
}

/// One reset→load cycle's measurements.
#[derive(Clone, serde::Serialize)]
pub struct ChunkLoadRun {
    pub chunk_count: u32,
    pub load_secs: f32,
    pub chunks_per_sec: f32,
    pub time_to_first_secs: f32,
    pub avg_frame_ms: f32,
    pub worst_frame_ms: f32,
    pub worst_frame_breakdown: FrameBreakdown,
}

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
    /// Where the benchmark was taken — results vary a lot by terrain, so this
    /// is the context that makes two pastes comparable (or not).
    pub player_pos: [f64; 3],
    pub target_rd: u32,
    /// Server-advertised cap, if it sent one (else equals `target_rd`).
    pub effective_rd: u32,
    /// Radius actually loaded, inferred from `chunk_count` — the real distance
    /// when the server caps or never advertises its view distance.
    pub achieved_rd: u32,
    /// Number of measured (non-warmup) runs the scalar fields below average
    /// over.
    pub runs: u32,
    pub warmup_runs: u32,
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
    pub runs_detail: Vec<ChunkLoadRun>,
    /// Phase split of the worst frame across the measured runs — what the spike
    /// was actually spent on.
    pub worst_frame_breakdown: FrameBreakdown,
    /// "debug" or "release" — see [`build_profile`].
    pub profile: String,
    pub measurement_note: String,
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
    Done(Box<ChunkLoadResult>),
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
    player_pos: [f64; 3],
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
    /// Phase split of the current run's worst frame so far.
    worst_breakdown: FrameBreakdown,
    frame_samples: u32,
    /// How many reset→load cycles have finished (warmup + measured).
    runs_done: u32,
    completed: Vec<ChunkLoadRun>,
}

impl ChunkLoadBench {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        target_rd: u32,
        original_rd: u32,
        server_rd: u32,
        gpu_name: &str,
        vulkan: &str,
        width: u32,
        height: u32,
        player_pos: [f64; 3],
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
            player_pos,
            reset_start: now,
            start: now,
            last_count: 0,
            last_change: now,
            baseline_count: 0,
            first_load_at: None,
            frame_ms_sum: 0.0,
            frame_ms_max: 0.0,
            worst_breakdown: FrameBreakdown::default(),
            frame_samples: 0,
            runs_done: 0,
            completed: Vec::new(),
        }
    }

    pub fn update(
        &mut self,
        loaded_count: u32,
        frame_ms: f32,
        timings: &RenderTimings,
        phases: UpdatePhases,
    ) -> ChunkLoadStep {
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
                if frame_ms > self.frame_ms_max {
                    self.frame_ms_max = frame_ms;
                    self.worst_breakdown = FrameBreakdown {
                        total_ms: frame_ms,
                        render_ms: timings.frame_ms,
                        fence_ms: timings.fence_ms,
                        acquire_ms: timings.acquire_ms,
                        cull_ms: timings.cull_ms,
                        present_ms: timings.present_ms,
                        update: phases,
                    };
                }
                self.frame_samples += 1;

                if loaded_count != self.last_count {
                    self.last_count = loaded_count;
                    self.last_change = Instant::now();
                }
                if self.first_load_at.is_none() && loaded_count > self.baseline_count {
                    self.first_load_at = Some(Instant::now());
                }

                // Done when nearly the whole radius has loaded, or the stream has
                // genuinely stalled (a capped/slow server), or the safety timeout.
                let near_complete = loaded_count as f32
                    >= expected_columns(self.target_rd) as f32 * CHUNK_LOAD_COMPLETE_FRAC;
                let stalled = loaded_count > 0
                    && self.last_change.elapsed().as_secs_f32() >= CHUNK_LOAD_STALL_SECS;
                let timeout = self.start.elapsed().as_secs_f32() >= CHUNK_TIMEOUT_SECS;
                if near_complete || stalled || timeout {
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
                    self.completed.push(ChunkLoadRun {
                        chunk_count: loaded_count,
                        load_secs,
                        chunks_per_sec,
                        time_to_first_secs,
                        avg_frame_ms,
                        worst_frame_ms: self.frame_ms_max,
                        worst_frame_breakdown: self.worst_breakdown.clone(),
                    });
                    self.runs_done += 1;

                    if self.runs_done >= CHUNK_LOAD_TOTAL_RUNS {
                        return ChunkLoadStep::Done(Box::new(self.aggregate()));
                    }

                    // Next cycle: drop back to the minimum RD and re-enter the reset
                    // phase so the server unloads before the next timed load.
                    let now = Instant::now();
                    self.phase = ChunkPhase::Reset;
                    self.reset_start = now;
                    self.last_change = now;
                    self.last_count = loaded_count;
                    self.first_load_at = None;
                    self.frame_ms_sum = 0.0;
                    self.frame_ms_max = 0.0;
                    self.worst_breakdown = FrameBreakdown::default();
                    self.frame_samples = 0;
                    ChunkLoadStep::Load(CHUNK_LOAD_MIN_RD)
                } else {
                    ChunkLoadStep::Wait
                }
            }
        }
    }

    /// Average the measured (non-warmup) runs into the shareable result.
    fn aggregate(&self) -> ChunkLoadResult {
        let measured = &self.completed[CHUNK_LOAD_WARMUP_RUNS as usize..];
        let n = measured.len().max(1) as f32;
        let avg = |sel: fn(&ChunkLoadRun) -> f32| measured.iter().map(sel).sum::<f32>() / n;
        let chunk_count =
            (measured.iter().map(|r| r.chunk_count as f32).sum::<f32>() / n).round() as u32;
        ChunkLoadResult {
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
            player_pos: self.player_pos,
            target_rd: self.target_rd,
            effective_rd: self.effective_rd,
            achieved_rd: radius_from_chunk_count(chunk_count),
            runs: measured.len() as u32,
            warmup_runs: CHUNK_LOAD_WARMUP_RUNS,
            chunk_count,
            load_secs: avg(|r| r.load_secs),
            chunks_per_sec: avg(|r| r.chunks_per_sec),
            time_to_first_secs: avg(|r| r.time_to_first_secs),
            avg_frame_ms: avg(|r| r.avg_frame_ms),
            worst_frame_ms: measured
                .iter()
                .map(|r| r.worst_frame_ms)
                .fold(0.0, f32::max),
            worst_frame_breakdown: measured
                .iter()
                .max_by(|a, b| {
                    a.worst_frame_ms
                        .partial_cmp(&b.worst_frame_ms)
                        .unwrap_or(std::cmp::Ordering::Equal)
                })
                .map(|r| r.worst_frame_breakdown.clone())
                .unwrap_or_default(),
            runs_detail: measured.to_vec(),
            profile: build_profile().to_owned(),
            measurement_note: MEASUREMENT_NOTE.to_owned(),
        }
    }

    pub fn original_rd(&self) -> u32 {
        self.original_rd
    }

    pub fn target_rd(&self) -> u32 {
        self.target_rd
    }

    pub fn effective_rd(&self) -> u32 {
        self.effective_rd
    }

    /// 1-based index of the run currently in progress (warmup runs included).
    pub fn current_run(&self) -> u32 {
        (self.runs_done + 1).min(CHUNK_LOAD_TOTAL_RUNS)
    }

    pub fn total_runs(&self) -> u32 {
        CHUNK_LOAD_TOTAL_RUNS
    }

    pub fn loaded(&self) -> u32 {
        self.last_count
    }

    pub fn resetting(&self) -> bool {
        matches!(self.phase, ChunkPhase::Reset)
    }
}

const PASTE_URL: &str = "https://paste.marshall.dev/documents";

/// Progress of an in-flight (or finished) benchmark-result upload, shared
/// between the render thread and the spawned upload task.
#[derive(Clone)]
pub enum UploadStatus {
    Uploading,
    Done { url: String, copied: bool },
    Failed(String),
}

pub type UploadHandle = std::sync::Arc<std::sync::Mutex<UploadStatus>>;

#[derive(serde::Deserialize)]
struct DocResponse {
    key: String,
}

/// POST the result JSON to paste.marshall.dev and return the shareable link.
async fn post_paste(json: String) -> Result<String, String> {
    let resp = reqwest::Client::new()
        .post(PASTE_URL)
        .header(reqwest::header::CONTENT_TYPE, "text/plain")
        .body(json)
        .send()
        .await
        .map_err(|e| format!("Upload failed: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("Upload failed: HTTP {}", resp.status()));
    }
    let doc: DocResponse = resp
        .json()
        .await
        .map_err(|e| format!("Upload response parse failed: {e}"))?;
    Ok(format!("https://paste.marshall.dev/{}", doc.key))
}

/// Spawn a background upload of `json` and copy the resulting link to the
/// clipboard. Returns a handle the UI polls for status.
pub fn upload_result(rt: &tokio::runtime::Runtime, json: String) -> UploadHandle {
    let handle: UploadHandle = std::sync::Arc::new(std::sync::Mutex::new(UploadStatus::Uploading));
    let out = std::sync::Arc::clone(&handle);
    rt.spawn(async move {
        let status = match post_paste(json).await {
            Ok(url) => {
                let copied = crate::ui::common::set_clipboard(&url);
                UploadStatus::Done { url, copied }
            }
            Err(e) => UploadStatus::Failed(e),
        };
        *out.lock().unwrap() = status;
    });
    handle
}
