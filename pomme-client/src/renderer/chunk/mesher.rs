use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};

use azalea_block::BlockState;
use azalea_core::position::ChunkPos;
use pyronyx::vk;

use super::greedy;
use super::occlusion_graph::{VisibilitySet, compute_visibility};
use crate::renderer::chunk::atlas::{AtlasRegion, AtlasUVMap};
use crate::world::block::model::{BakedModel, Direction};
use crate::world::block::registry::{BlockRegistry, FaceTextures, Tint};
use crate::world::chunk;
use crate::world::chunk::ChunkStore;

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
pub struct ChunkVertex {
    pub position: [f32; 3],
    pub tex_coords: [u16; 2],
    pub light_tint: u32,
}

impl ChunkVertex {
    pub const STRIDE: u32 = size_of::<Self>() as u32;

    pub fn binding_description() -> vk::VertexInputBindingDescription {
        vk::VertexInputBindingDescription {
            binding: 0,
            stride: Self::STRIDE,
            input_rate: vk::VertexInputRate::Vertex,
        }
    }

    pub fn attribute_descriptions() -> [vk::VertexInputAttributeDescription; 3] {
        [
            vk::VertexInputAttributeDescription {
                location: 0,
                binding: 0,
                format: vk::Format::R32G32B32Sfloat,
                offset: 0,
            },
            vk::VertexInputAttributeDescription {
                location: 1,
                binding: 0,
                format: vk::Format::R16G16Unorm,
                offset: 12,
            },
            vk::VertexInputAttributeDescription {
                location: 2,
                binding: 0,
                format: vk::Format::R8G8B8A8Unorm,
                offset: 16,
            },
        ]
    }
}

pub fn pack_uv(u: f32, v: f32) -> [u16; 2] {
    [
        (u.clamp(0.0, 1.0) * 65535.0 + 0.5) as u16,
        (v.clamp(0.0, 1.0) * 65535.0 + 0.5) as u16,
    ]
}

pub fn pack_light_tint(light: f32, tint: u32) -> u32 {
    let l = (light.clamp(0.0, 1.0) * 255.0 + 0.5) as u32;
    l | (tint & 0xFFFFFF00)
}

pub const fn pack_tint_shifted(rgb: [f32; 3]) -> u32 {
    const fn channel(v: f32) -> u32 {
        let c = (v * 255.0 + 0.5) as i32;
        if c < 0 {
            0
        } else if c > 255 {
            255
        } else {
            c as u32
        }
    }
    (channel(rgb[0]) << 8) | (channel(rgb[1]) << 16) | (channel(rgb[2]) << 24)
}

pub const PACKED_WHITE_SHIFTED: u32 = pack_tint_shifted([1.0, 1.0, 1.0]);

/// One 16³ section's geometry. Indices are section-local (0-based into
/// `vertices`) so each section can be uploaded as a self-contained draw with
/// its own tight AABB, giving per-section cull granularity instead of
/// per-column.
pub struct SectionMesh {
    /// 0-based section index from the column's min_y; stable identity for
    /// per-section upload/replace.
    pub section_index: i32,
    pub vertices: Vec<ChunkVertex>,
    pub indices: Vec<u32>,
    /// Translucent (water) indices into the same `vertices`, drawn in a
    /// separate blended pass after opaque geometry.
    pub water_indices: Vec<u32>,
}

pub struct ChunkMeshData {
    pub pos: ChunkPos,
    /// Non-empty meshed sections (each tagged with its `section_index`).
    pub sections: Vec<SectionMesh>,
    /// The section-index range this job (re)meshed. Upload replaces exactly
    /// these indices: any index in the range with no `SectionMesh` is now
    /// empty and its slice is freed. `0..section_count` for a whole-column
    /// (re)mesh.
    pub replaced: std::ops::Range<i32>,
    /// Content generation this mesh was built from (see
    /// `GameState::content_gen`). Lets the drain drop a stale result whose
    /// column has since been edited.
    pub content_gen: u64,
    /// Globally monotonic stamp assigned at enqueue. The buffer keeps the
    /// highest epoch uploaded per section and rejects any older upload, so an
    /// in-flight bulk mesh can never clobber a section a newer edit already
    /// uploaded (the edit always enqueues a higher epoch after its write).
    pub upload_epoch: u64,
    /// Per-section cave-cull visibility, one entry per index in `replaced`
    /// (including now-empty sections, which connect all faces).
    pub visibility: Vec<(i32, VisibilitySet)>,
    /// Latency stamps for edit remeshes (diagnostic); `None` for bulk loads.
    pub timing: Option<RemeshTiming>,
}

pub struct RemeshTiming {
    pub enqueued_at: std::time::Instant,
    pub started_at: std::time::Instant,
    pub meshed_at: std::time::Instant,
}

#[derive(Clone, Copy, Debug, Default)]
pub enum GrassColorModifier {
    #[default]
    None,
    DarkForest,
    Swamp,
}

#[derive(Clone, Copy, Debug)]
pub struct BiomeClimate {
    pub temperature: f32,
    pub downfall: f32,
    pub grass_color_override: Option<[f32; 3]>,
    pub grass_color_modifier: GrassColorModifier,
    pub foliage_color_override: Option<[f32; 3]>,
    pub dry_foliage_color_override: Option<[f32; 3]>,
}

impl Default for BiomeClimate {
    fn default() -> Self {
        Self {
            temperature: 0.8,
            downfall: 0.4,
            grass_color_override: None,
            grass_color_modifier: GrassColorModifier::None,
            foliage_color_override: None,
            dry_foliage_color_override: None,
        }
    }
}

fn tint_color(tint: Tint, grass: [f32; 3], foliage: [f32; 3], dry_foliage: [f32; 3]) -> u32 {
    match tint {
        Tint::None => PACKED_WHITE_SHIFTED,
        Tint::Grass => pack_tint_shifted(grass),
        Tint::Foliage => pack_tint_shifted(foliage),
        Tint::DryFoliage => pack_tint_shifted(dry_foliage),
    }
}

const MAX_MESH_UPLOADS_PER_FRAME: usize = 16;

pub struct Colormap {
    pixels: Vec<[u8; 3]>,
}

impl Colormap {
    pub fn load(
        jar_assets_dir: &std::path::Path,
        asset_index: &Option<crate::assets::AssetIndex>,
        colormap_path: &str,
        packs: Option<&crate::resource_pack::ResourcePackManager>,
    ) -> Self {
        let path = crate::assets::resolve_asset_path_with_packs(
            jar_assets_dir,
            asset_index,
            colormap_path,
            packs,
        );
        let pixels = crate::renderer::util::load_png(&path)
            .map(|(data, _w, _h)| {
                data.chunks(4)
                    .take(256 * 256)
                    .map(|c| [c[0], c[1], c[2]])
                    .collect()
            })
            .unwrap_or_else(|| vec![[145, 189, 89]; 256 * 256]);
        Self { pixels }
    }

    fn lookup(&self, temperature: f32, downfall: f32) -> [f32; 3] {
        let t = temperature.clamp(0.0, 1.0);
        let d = (downfall.clamp(0.0, 1.0)) * t;
        let x = ((1.0 - t) * 255.0) as usize;
        let y = ((1.0 - d) * 255.0) as usize;
        let idx = (y * 256 + x).min(256 * 256 - 1);
        let [r, g, b] = self.pixels[idx];
        [r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0]
    }
}

pub fn grass_color(climate: &BiomeClimate, colormap: &Colormap, x: i32, z: i32) -> [f32; 3] {
    let base = climate
        .grass_color_override
        .unwrap_or_else(|| colormap.lookup(climate.temperature, climate.downfall));
    apply_grass_modifier(climate.grass_color_modifier, base, x, z)
}

pub fn foliage_color(climate: &BiomeClimate, colormap: &Colormap) -> [f32; 3] {
    climate
        .foliage_color_override
        .unwrap_or_else(|| colormap.lookup(climate.temperature, climate.downfall))
}

pub fn dry_foliage_color(climate: &BiomeClimate, colormap: &Colormap) -> [f32; 3] {
    climate
        .dry_foliage_color_override
        .unwrap_or_else(|| colormap.lookup(climate.temperature, climate.downfall))
}

/// Average a biome color over the vanilla 5x5 horizontal blend
/// (`BiomeColors` with the default blend radius of 2).
pub fn blend_color(x: i32, z: i32, mut color_at: impl FnMut(i32, i32) -> [f32; 3]) -> [f32; 3] {
    const RADIUS: i32 = 2;
    const COUNT: f32 = ((RADIUS * 2 + 1) * (RADIUS * 2 + 1)) as f32;
    let mut sum = [0.0f32; 3];
    for dz in -RADIUS..=RADIUS {
        for dx in -RADIUS..=RADIUS {
            let c = color_at(x + dx, z + dz);
            for (s, v) in sum.iter_mut().zip(c) {
                *s += v;
            }
        }
    }
    sum.map(|s| s / COUNT)
}

fn apply_grass_modifier(modifier: GrassColorModifier, base: [f32; 3], x: i32, z: i32) -> [f32; 3] {
    match modifier {
        GrassColorModifier::None => base,
        GrassColorModifier::DarkForest => {
            let r = ((to_u8(base[0]) & 0xFE) as u32 + 0x28) >> 1;
            let g = ((to_u8(base[1]) & 0xFE) as u32 + 0x34) >> 1;
            let b = ((to_u8(base[2]) & 0xFE) as u32 + 0x0A) >> 1;
            [
                r.min(255) as f32 / 255.0,
                g.min(255) as f32 / 255.0,
                b.min(255) as f32 / 255.0,
            ]
        }
        GrassColorModifier::Swamp => {
            use std::sync::LazyLock;
            static BIOME_NOISE: LazyLock<SimplexNoise> =
                LazyLock::new(SimplexNoise::new_biome_info);
            let noise = BIOME_NOISE.value_2d(x as f64 * 0.0225, z as f64 * 0.0225);
            if noise < -0.1 {
                [
                    0x4C as f32 / 255.0,
                    0x76 as f32 / 255.0,
                    0x3C as f32 / 255.0,
                ]
            } else {
                [
                    0x6A as f32 / 255.0,
                    0x70 as f32 / 255.0,
                    0x39 as f32 / 255.0,
                ]
            }
        }
    }
}

fn to_u8(f: f32) -> u8 {
    (f * 255.0).round() as u8
}

struct SimplexNoise {
    perm: [u8; 256],
    #[allow(dead_code)]
    xo: f64,
    #[allow(dead_code)]
    yo: f64,
}

const GRADIENT: [[i32; 3]; 16] = [
    [1, 1, 0],
    [-1, 1, 0],
    [1, -1, 0],
    [-1, -1, 0],
    [1, 0, 1],
    [-1, 0, 1],
    [1, 0, -1],
    [-1, 0, -1],
    [0, 1, 1],
    [0, -1, 1],
    [0, 1, -1],
    [0, -1, -1],
    [1, 1, 0],
    [0, -1, 1],
    [-1, 1, 0],
    [0, -1, -1],
];

impl SimplexNoise {
    fn new_biome_info() -> Self {
        let mut rng = JavaRng::new(2345);
        let xo = rng.next_double() * 256.0;
        let yo = rng.next_double() * 256.0;
        let _zo = rng.next_double() * 256.0;
        let mut perm = [0u8; 256];
        for (i, p) in perm.iter_mut().enumerate() {
            *p = i as u8;
        }
        for i in 0..256 {
            let j = rng.next_int((256 - i) as i32) as usize + i;
            perm.swap(i, j);
        }
        Self { perm, xo, yo }
    }

    fn p(&self, i: i32) -> i32 {
        self.perm[(i & 0xFF) as usize] as i32
    }

    fn value_2d(&self, x: f64, y: f64) -> f64 {
        let sqrt3: f64 = 3.0_f64.sqrt();
        let f2 = 0.5 * (sqrt3 - 1.0);
        let g2 = (3.0 - sqrt3) / 6.0;

        let s = (x + y) * f2;
        let i = (x + s).floor() as i32;
        let j = (y + s).floor() as i32;
        let t = (i + j) as f64 * g2;
        let x0 = x - (i as f64 - t);
        let y0 = y - (j as f64 - t);

        let (i1, j1) = if x0 > y0 { (1, 0) } else { (0, 1) };

        let x1 = x0 - i1 as f64 + g2;
        let y1 = y0 - j1 as f64 + g2;
        let x2 = x0 - 1.0 + 2.0 * g2;
        let y2 = y0 - 1.0 + 2.0 * g2;

        let gi0 = (self.p(i + self.p(j)) % 12) as usize;
        let gi1 = (self.p(i + i1 + self.p(j + j1)) % 12) as usize;
        let gi2 = (self.p(i + 1 + self.p(j + 1)) % 12) as usize;

        let n0 = corner_noise(gi0, x0, y0, 0.0, 0.5);
        let n1 = corner_noise(gi1, x1, y1, 0.0, 0.5);
        let n2 = corner_noise(gi2, x2, y2, 0.0, 0.5);

        70.0 * (n0 + n1 + n2)
    }
}

fn corner_noise(gi: usize, x: f64, y: f64, z: f64, falloff: f64) -> f64 {
    let t = falloff - x * x - y * y - z * z;
    if t < 0.0 {
        0.0
    } else {
        let t2 = t * t;
        let g = &GRADIENT[gi];
        t2 * t2 * (g[0] as f64 * x + g[1] as f64 * y + g[2] as f64 * z)
    }
}

struct JavaRng {
    seed: i64,
}

impl JavaRng {
    fn new(seed: i64) -> Self {
        Self {
            seed: (seed ^ 0x5DEECE66D) & ((1i64 << 48) - 1),
        }
    }

    fn next(&mut self, bits: u32) -> i32 {
        self.seed = (self.seed.wrapping_mul(0x5DEECE66D).wrapping_add(0xB)) & ((1i64 << 48) - 1);
        (self.seed >> (48 - bits)) as i32
    }

    fn next_int(&mut self, bound: i32) -> i32 {
        if bound & (bound - 1) == 0 {
            return ((bound as i64 * self.next(31) as i64) >> 31) as i32;
        }
        loop {
            let bits = self.next(31);
            let val = bits % bound;
            if bits - val + (bound - 1) >= 0 {
                return val;
            }
        }
    }

    fn next_double(&mut self) -> f64 {
        let hi = self.next(26) as i64;
        let lo = self.next(27) as i64;
        ((hi << 27) + lo) as f64 / ((1i64 << 53) as f64)
    }
}

pub fn int_to_rgb(color: i32) -> [f32; 3] {
    let r = ((color >> 16) & 0xFF) as f32 / 255.0;
    let g = ((color >> 8) & 0xFF) as f32 / 255.0;
    let b = (color & 0xFF) as f32 / 255.0;
    [r, g, b]
}

pub struct MeshDispatcher {
    result_rx: crossbeam_channel::Receiver<ChunkMeshData>,
    result_tx: crossbeam_channel::Sender<ChunkMeshData>,
    // Edits drain ahead of and uncapped by the bulk load lane (see drain_results).
    priority_rx: crossbeam_channel::Receiver<ChunkMeshData>,
    priority_tx: crossbeam_channel::Sender<ChunkMeshData>,
    queue: Arc<MeshQueue>,
    workers: Vec<std::thread::JoinHandle<()>>,
    // Monotonic per-enqueue stamp; see `ChunkMeshData::upload_epoch`. Starts at 1
    // so 0 means "never uploaded" on the buffer side.
    next_epoch: AtomicU64,
    registry: Arc<BlockRegistry>,
    uv_map: Arc<AtlasUVMap>,
    grass_colormap: Arc<Colormap>,
    foliage_colormap: Arc<Colormap>,
    dry_foliage_colormap: Arc<Colormap>,
    biome_climate: Arc<HashMap<u32, BiomeClimate>>,
}

impl MeshDispatcher {
    pub fn new(
        registry: BlockRegistry,
        uv_map: AtlasUVMap,
        grass_colormap: Colormap,
        foliage_colormap: Colormap,
        dry_foliage_colormap: Colormap,
        biome_climate: Arc<HashMap<u32, BiomeClimate>>,
    ) -> Self {
        let (result_tx, result_rx) = crossbeam_channel::unbounded();
        let (priority_tx, priority_rx) = crossbeam_channel::unbounded();

        let queue = Arc::new(MeshQueue::new());
        // One worker per core minus one, leaving a core for the main/render thread.
        let worker_count = std::thread::available_parallelism()
            .map(|n| n.get().saturating_sub(1).max(1))
            .unwrap_or(1);
        let mut workers = Vec::with_capacity(worker_count);
        for _ in 0..worker_count {
            let queue = Arc::clone(&queue);
            workers.push(
                std::thread::Builder::new()
                    .name("chunk-mesher".into())
                    .spawn(move || queue.run_worker())
                    .expect("spawn chunk-mesher thread"),
            );
        }

        Self {
            result_rx,
            result_tx,
            priority_rx,
            priority_tx,
            queue,
            workers,
            next_epoch: AtomicU64::new(1),
            registry: Arc::new(registry),
            uv_map: Arc::new(uv_map),
            grass_colormap: Arc::new(grass_colormap),
            foliage_colormap: Arc::new(foliage_colormap),
            dry_foliage_colormap: Arc::new(dry_foliage_colormap),
            biome_climate,
        }
    }

    pub fn set_biome_climate(&mut self, climate: Arc<HashMap<u32, BiomeClimate>>) {
        self.biome_climate = climate;
    }

    /// (grass, foliage, dry foliage) colormaps, shared with the particle
    /// store for break-particle tinting.
    pub fn colormaps(&self) -> (Arc<Colormap>, Arc<Colormap>, Arc<Colormap>) {
        (
            Arc::clone(&self.grass_colormap),
            Arc::clone(&self.foliage_colormap),
            Arc::clone(&self.dry_foliage_colormap),
        )
    }

    // Async worker path, vanilla's default `prioritizeChunkUpdates = NONE`.
    // Player edits use `mesh_section_now` instead.
    pub fn enqueue(
        &self,
        chunk_store: &ChunkStore,
        pos: ChunkPos,
        lod: u32,
        priority: bool,
        content_gen: u64,
        sections: std::ops::Range<i32>,
    ) {
        let tx = if priority {
            self.priority_tx.clone()
        } else {
            self.result_tx.clone()
        };
        let enqueued_at = priority.then(std::time::Instant::now);
        let upload_epoch = self.next_epoch.fetch_add(1, Ordering::Relaxed);

        self.queue.push(PendingJob {
            pos,
            lod,
            content_gen,
            upload_epoch,
            sections,
            // An edit re-meshes an already-shown chunk (vanilla's "recompile").
            is_recompile: priority,
            enqueued_at,
            snapshot: self.build_snapshot(chunk_store, pos),
            registry: Arc::clone(&self.registry),
            uv_map: Arc::clone(&self.uv_map),
            tx,
        });
    }

    /// Vanilla `compileSync` (`PrioritizeChunkUpdates.PLAYER_AFFECTED`): mesh
    /// a column's edited sections on the calling thread so a player edit is
    /// renderable the same frame, skipping the worker round-trip. One
    /// snapshot serves the whole span.
    pub fn mesh_sections_now(
        &self,
        chunk_store: &ChunkStore,
        pos: ChunkPos,
        sections: std::ops::Range<i32>,
        content_gen: u64,
    ) -> ChunkMeshData {
        let snapshot = self.build_snapshot(chunk_store, pos);
        let mut mesh =
            mesh_chunk_snapshot(&snapshot, pos, &self.registry, &self.uv_map, 0, sections);
        mesh.content_gen = content_gen;
        mesh.upload_epoch = self.next_epoch.fetch_add(1, Ordering::Relaxed);
        mesh
    }

    /// Point-in-time snapshot of `pos`'s mesh neighbourhood: chunk arcs plus
    /// a deep copy of their light data.
    fn build_snapshot(&self, chunk_store: &ChunkStore, pos: ChunkPos) -> ChunkStoreSnapshot {
        let chunks_needed = chunk::mesh_neighborhood(pos);
        ChunkStoreSnapshot {
            chunks: chunks_needed
                .iter()
                .map(|p| (*p, chunk_store.get_chunk(p)))
                .collect(),
            light: chunks_needed
                .iter()
                .filter_map(|p| {
                    chunk_store
                        .light_data
                        .get(&(p.x, p.z))
                        .map(|ld| ((p.x, p.z), ld.clone()))
                })
                .collect(),
            grass_colormap: Arc::clone(&self.grass_colormap),
            foliage_colormap: Arc::clone(&self.foliage_colormap),
            dry_foliage_colormap: Arc::clone(&self.dry_foliage_colormap),
            biome_climate: Arc::clone(&self.biome_climate),
            min_y: chunk_store.min_y(),
            height: chunk_store.height(),
        }
    }

    /// Latest camera position, used to mesh the nearest pending chunk first.
    pub fn set_camera_position(&self, pos: glam::DVec3) {
        self.queue.set_camera(pos);
    }

    pub fn drain_results(&self) -> impl Iterator<Item = ChunkMeshData> + '_ {
        // Edits drain fully and first; bulk chunk loads stay capped per frame.
        self.priority_rx
            .try_iter()
            .chain(self.result_rx.try_iter().take(MAX_MESH_UPLOADS_PER_FRAME))
    }
}

impl Drop for MeshDispatcher {
    fn drop(&mut self) {
        self.queue.close();
        for handle in self.workers.drain(..) {
            let _ = handle.join();
        }
    }
}

const MAX_RECOMPILE_QUOTA: i32 = 2;

/// A pending chunk-mesh job: a point-in-time snapshot of the neighbourhood plus
/// everything `mesh_chunk_snapshot` needs. Gathered on the calling thread
/// (chunk data isn't shareable across threads), then meshed by a worker.
struct PendingJob {
    pos: ChunkPos,
    lod: u32,
    content_gen: u64,
    upload_epoch: u64,
    sections: std::ops::Range<i32>,
    is_recompile: bool,
    enqueued_at: Option<std::time::Instant>,
    snapshot: ChunkStoreSnapshot,
    registry: Arc<BlockRegistry>,
    uv_map: Arc<AtlasUVMap>,
    tx: crossbeam_channel::Sender<ChunkMeshData>,
}

impl PendingJob {
    fn run(self) {
        let started_at = self.enqueued_at.map(|_| std::time::Instant::now());
        let mut mesh = mesh_chunk_snapshot(
            &self.snapshot,
            self.pos,
            &self.registry,
            &self.uv_map,
            self.lod,
            self.sections,
        );
        mesh.content_gen = self.content_gen;
        mesh.upload_epoch = self.upload_epoch;
        if let (Some(enqueued_at), Some(started_at)) = (self.enqueued_at, started_at) {
            mesh.timing = Some(RemeshTiming {
                enqueued_at,
                started_at,
                meshed_at: std::time::Instant::now(),
            });
        }
        let _ = self.tx.send(mesh);
    }
}

struct QueueState {
    tasks: Vec<PendingJob>,
    // Consecutive edits served ahead of an initial load before one is forced, so
    // streaming never starves (vanilla SectionTaskDynamicQueue.MAX_RECOMPILE_QUOTA).
    recompile_quota: i32,
    camera: glam::DVec3,
}

/// Re-orderable mesh queue, a port of vanilla `SectionTaskDynamicQueue`. The
/// best task is chosen at poll time rather than fixed at submission, so a
/// freshly enqueued edit is taken before the already-queued chunk-load backlog.
struct MeshQueue {
    state: Mutex<QueueState>,
    available: Condvar,
    closed: AtomicBool,
}

impl MeshQueue {
    fn new() -> Self {
        Self {
            state: Mutex::new(QueueState {
                tasks: Vec::new(),
                recompile_quota: MAX_RECOMPILE_QUOTA,
                camera: glam::DVec3::ZERO,
            }),
            available: Condvar::new(),
            closed: AtomicBool::new(false),
        }
    }

    fn push(&self, job: PendingJob) {
        let mut state = self.state.lock().unwrap();
        // A re-edit of a still-queued section replaces the queued job in place
        // instead of duplicating it. Bulk loads can't duplicate (`meshed` gates
        // them), so only edits need this.
        if job.is_recompile
            && let Some(existing) = state
                .tasks
                .iter_mut()
                .find(|t| t.is_recompile && t.pos == job.pos && t.sections == job.sections)
        {
            *existing = job;
        } else {
            state.tasks.push(job);
        }
        drop(state);
        self.available.notify_one();
    }

    fn set_camera(&self, camera: glam::DVec3) {
        self.state.lock().unwrap().camera = camera;
    }

    fn close(&self) {
        self.closed.store(true, Ordering::Relaxed);
        self.available.notify_all();
    }

    fn run_worker(&self) {
        loop {
            let mut state = self.state.lock().unwrap();
            let job = loop {
                if self.closed.load(Ordering::Relaxed) {
                    return;
                }
                if let Some(job) = poll(&mut state) {
                    break job;
                }
                state = self.available.wait(state).unwrap();
            };
            drop(state);
            // A panicking job must not kill the worker thread; its column stays
            // unmeshed (its `meshed` bit is set), but meshing continues.
            if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| job.run())).is_err() {
                tracing::error!("chunk mesh job panicked; worker continuing");
            }
        }
    }
}

/// Pick the next task: nearest to the camera, preferring edits (recompiles)
/// over initial loads when the edit is closer, bounded by the recompile quota.
/// Mirrors vanilla `SectionTaskDynamicQueue.poll`.
fn poll(state: &mut QueueState) -> Option<PendingJob> {
    let camera = state.camera;
    let dist_sq = |task: &PendingJob| {
        let dx = (task.pos.x as f64 * 16.0 + 8.0) - camera.x;
        let dz = (task.pos.z as f64 * 16.0 + 8.0) - camera.z;
        dx * dx + dz * dz
    };

    // Both lanes mesh nearest-first; edits (recompiles) are preferred over initial
    // loads when closer, bounded by the recompile quota. Occlusion gates drawing,
    // not meshing, so meshing order is purely distance-based.
    let mut best_initial: Option<(usize, f64)> = None;
    let mut best_recompile: Option<(usize, f64)> = None;
    for (i, task) in state.tasks.iter().enumerate() {
        let dist = dist_sq(task);
        if task.is_recompile {
            if best_recompile.is_none_or(|(_, d)| dist < d) {
                best_recompile = Some((i, dist));
            }
        } else if best_initial.is_none_or(|(_, d)| dist < d) {
            best_initial = Some((i, dist));
        }
    }

    if let Some((ri, rd)) = best_recompile {
        let take_recompile = match best_initial {
            None => true,
            Some((_, id)) => state.recompile_quota > 0 && rd < id,
        };
        if take_recompile {
            state.recompile_quota -= 1;
            return Some(state.tasks.swap_remove(ri));
        }
    }
    state.recompile_quota = MAX_RECOMPILE_QUOTA;
    best_initial.map(|(ii, _)| state.tasks.swap_remove(ii))
}

struct ChunkStoreSnapshot {
    chunks: Vec<(
        ChunkPos,
        Option<Arc<parking_lot::RwLock<azalea_world::Chunk>>>,
    )>,
    light: std::collections::HashMap<(i32, i32), crate::world::chunk::ChunkLightData>,
    grass_colormap: Arc<Colormap>,
    foliage_colormap: Arc<Colormap>,
    dry_foliage_colormap: Arc<Colormap>,
    biome_climate: Arc<HashMap<u32, BiomeClimate>>,
    min_y: i32,
    height: u32,
}

impl ChunkStoreSnapshot {
    fn get_block_state(&self, x: i32, y: i32, z: i32) -> azalea_block::BlockState {
        let chunk_pos = ChunkPos::new(x.div_euclid(16), z.div_euclid(16));
        let chunk_lock = self
            .chunks
            .iter()
            .find(|(p, _)| *p == chunk_pos)
            .and_then(|(_, c): &(ChunkPos, _)| c.as_ref());

        let Some(chunk_lock) = chunk_lock else {
            return azalea_block::BlockState::AIR;
        };

        let c: parking_lot::RwLockReadGuard<'_, azalea_world::Chunk> = chunk_lock.read();
        chunk::block_state_from_section(&c, x, y, z, self.min_y)
    }

    fn min_y(&self) -> i32 {
        self.min_y
    }

    fn height(&self) -> u32 {
        self.height
    }

    fn get_biome(&self, x: i32, y: i32, z: i32) -> azalea_registry::data::Biome {
        let chunk_pos = ChunkPos::new(x.div_euclid(16), z.div_euclid(16));
        let chunk_lock = self
            .chunks
            .iter()
            .find(|(p, _)| *p == chunk_pos)
            .and_then(|(_, c)| c.as_ref());
        let Some(chunk_lock) = chunk_lock else {
            return azalea_registry::data::Biome::default();
        };
        let c = chunk_lock.read();
        let biome_pos = azalea_core::position::ChunkBiomePos {
            x: (x.rem_euclid(16) / 4) as u8,
            y,
            z: (z.rem_euclid(16) / 4) as u8,
        };
        c.get_biome(biome_pos, self.min_y).unwrap_or_default()
    }

    fn climate_at(&self, x: i32, y: i32, z: i32) -> BiomeClimate {
        let biome = self.get_biome(x, y, z);
        self.biome_climate
            .get(&u32::from(biome))
            .copied()
            .unwrap_or_default()
    }

    fn grass_color_at(&self, x: i32, y: i32, z: i32) -> [f32; 3] {
        grass_color(&self.climate_at(x, y, z), &self.grass_colormap, x, z)
    }

    fn foliage_color_at(&self, x: i32, y: i32, z: i32) -> [f32; 3] {
        foliage_color(&self.climate_at(x, y, z), &self.foliage_colormap)
    }

    fn dry_foliage_color_at(&self, x: i32, y: i32, z: i32) -> [f32; 3] {
        dry_foliage_color(&self.climate_at(x, y, z), &self.dry_foliage_colormap)
    }

    fn grass_tint(&self, x: i32, y: i32, z: i32) -> [f32; 3] {
        blend_color(x, z, |bx, bz| self.grass_color_at(bx, y, bz))
    }

    fn foliage_tint(&self, x: i32, y: i32, z: i32) -> [f32; 3] {
        blend_color(x, z, |bx, bz| self.foliage_color_at(bx, y, bz))
    }

    fn dry_foliage_tint(&self, x: i32, y: i32, z: i32) -> [f32; 3] {
        blend_color(x, z, |bx, bz| self.dry_foliage_color_at(bx, y, bz))
    }

    fn get_light(&self, x: i32, y: i32, z: i32) -> f32 {
        let cx = x.div_euclid(16);
        let cz = z.div_euclid(16);
        let lx = x.rem_euclid(16);
        let lz = z.rem_euclid(16);
        let level = if let Some(light) = self.light.get(&(cx, cz)) {
            light
                .get_sky_light(lx, y, lz)
                .max(light.get_block_light(lx, y, lz))
        } else {
            15
        };
        LIGHT_TABLE[level as usize]
    }
}

pub const LIGHT_TABLE: [f32; 16] = [
    0.05, 0.067, 0.085, 0.106, 0.129, 0.156, 0.188, 0.227, 0.272, 0.328, 0.393, 0.472, 0.566,
    0.679, 0.815, 1.0,
];

/// Brightness at a block position from the chunk store's light data:
/// `LIGHT_TABLE[max(sky, block)]`.
pub fn world_brightness(chunks: &ChunkStore, x: i32, y: i32, z: i32) -> f32 {
    let level = chunks
        .get_sky_light(x, y, z)
        .max(chunks.get_block_light(x, y, z));
    LIGHT_TABLE[level as usize]
}

struct GreedyBlockInfo {
    textures: FaceTextures,
}

struct BlockTypeMap {
    state_to_id: HashMap<BlockState, u16>,
    id_to_info: Vec<GreedyBlockInfo>,
}

impl BlockTypeMap {
    fn build(
        snapshot: &ChunkStoreSnapshot,
        registry: &BlockRegistry,
        world_x: i32,
        world_z: i32,
        min_y: i32,
        max_y: i32,
    ) -> Self {
        let mut state_to_id = HashMap::new();
        let mut id_to_info: Vec<GreedyBlockInfo> = Vec::new();
        let mut next_id = 1u16;

        for lz in -1..17i32 {
            for lx in -1..17i32 {
                let bx = world_x + lx;
                let bz = world_z + lz;
                for by in (min_y - 1)..=(max_y) {
                    let state = snapshot.get_block_state(bx, by, bz);
                    if state.is_air() || state_to_id.contains_key(&state) {
                        continue;
                    }
                    let has_baked = registry.get_baked_model(state).is_some();
                    let has_multipart = registry.get_multipart_quads(state).is_some();
                    if has_baked || has_multipart {
                        state_to_id.insert(state, 0);
                        continue;
                    }
                    if let Some(textures) = registry.get_textures(state) {
                        if textures.side_overlay.is_some() || !registry.is_opaque_full_cube(state) {
                            state_to_id.insert(state, 0);
                            continue;
                        }
                        state_to_id.insert(state, next_id);
                        id_to_info.push(GreedyBlockInfo {
                            textures: textures.clone(),
                        });
                        next_id += 1;
                    } else {
                        state_to_id.insert(state, 0);
                    }
                }
            }
        }

        Self {
            state_to_id,
            id_to_info,
        }
    }

    fn get_id(&self, state: BlockState) -> u16 {
        if state.is_air() {
            return 0;
        }
        self.state_to_id.get(&state).copied().unwrap_or(0)
    }

    fn get_info(&self, id: u16) -> Option<&GreedyBlockInfo> {
        if id == 0 {
            return None;
        }
        self.id_to_info.get((id - 1) as usize)
    }
}

const SECTION_SIZE: usize = 16;

fn face_texture_name(textures: &FaceTextures, face: greedy::Face) -> &str {
    match face {
        greedy::Face::Up => &textures.top,
        greedy::Face::Down => &textures.bottom,
        greedy::Face::Right => &textures.east,
        greedy::Face::Left => &textures.west,
        greedy::Face::Front => &textures.south,
        greedy::Face::Back => &textures.north,
    }
}

use super::block_ao::AO_BRIGHTNESS;

#[allow(clippy::too_many_arguments)]
fn greedy_mesh_section(
    vertices: &mut Vec<ChunkVertex>,
    indices: &mut Vec<u32>,
    snapshot: &ChunkStoreSnapshot,
    registry: &BlockRegistry,
    type_map: &BlockTypeMap,
    uv_map: &AtlasUVMap,
    world_x: i32,
    section_y: i32,
    world_z: i32,
) -> VisibilitySet {
    type M = greedy::GreedyMesher<SECTION_SIZE>;
    let mut mesher = M::new();
    let mut voxels = vec![0u16; M::CS_P3];
    let mut occluders = vec![false; M::CS_P3];
    let mut light = vec![0.0f32; M::CS_P3];

    for ly in 0..18 {
        for lx in 0..18 {
            for lz in 0..18 {
                let bx = world_x + lx as i32 - 1;
                let by = section_y + ly as i32 - 1;
                let bz = world_z + lz as i32 - 1;
                let state = snapshot.get_block_state(bx, by, bz);
                let idx = greedy::pad_linearize::<SECTION_SIZE>(lx, ly, lz);
                voxels[idx] = type_map.get_id(state);
                occluders[idx] = registry.is_opaque_full_cube(state);
                light[idx] = snapshot.get_light(bx, by, bz);
            }
        }
    }

    let transparent_set = std::collections::BTreeSet::new();
    mesher.mesh(&voxels, &occluders, &light, &transparent_set);

    for face_idx in 0..6 {
        let face = greedy::Face::from(face_idx);
        let dir_shade = face.shade_light();

        for quad in &mesher.quads[face_idx] {
            let block_id = quad.voxel_id();
            let info = match type_map.get_info(block_id) {
                Some(i) => i,
                None => continue,
            };

            let tex_name = face_texture_name(&info.textures, face);
            let region = uv_map.get_region(tex_name);
            let verts_uvs = face.vertices(quad);

            let [x0, _, z0] = verts_uvs[0].0;
            let block_x = x0 as i32 + world_x;
            let block_z = z0 as i32 + world_z;
            let tint = tint_color(
                info.textures.tint,
                snapshot.grass_tint(block_x, section_y, block_z),
                snapshot.foliage_tint(block_x, section_y, block_z),
                snapshot.dry_foliage_tint(block_x, section_y, block_z),
            );

            let ao = quad.ao_levels();
            // Per-vertex smooth light (averaged across chunk borders in the mesher); `i`
            // matches `ao`.
            let lights: [f32; 4] = core::array::from_fn(|i| {
                AO_BRIGHTNESS[ao[i] as usize] * (quad.light[i] as f32 / 255.0) * dir_shade
            });

            let base = vertices.len() as u32;
            let u_span = region.u_max - region.u_min;
            let v_span = region.v_max - region.v_min;

            for (i, (pos, uv)) in verts_uvs.iter().enumerate() {
                vertices.push(ChunkVertex {
                    position: [
                        pos[0] + world_x as f32,
                        pos[1] + section_y as f32,
                        pos[2] + world_z as f32,
                    ],
                    tex_coords: pack_uv(
                        region.u_min + uv[0] * u_span,
                        region.v_min + uv[1] * v_span,
                    ),
                    light_tint: pack_light_tint(lights[i], tint),
                });
            }

            if lights[0] + lights[2] > lights[1] + lights[3] {
                indices.extend_from_slice(&[
                    base + 1,
                    base + 2,
                    base + 3,
                    base + 3,
                    base,
                    base + 1,
                ]);
            } else {
                indices.extend_from_slice(&[base, base + 1, base + 2, base + 2, base + 3, base]);
            }
        }
    }

    // Section visibility (cave culling) shares the opacity grid the mesher just
    // built: the section's 16³ cells sit at padded coords +1.
    compute_visibility(|x, y, z| {
        occluders[greedy::pad_linearize::<SECTION_SIZE>(x + 1, y + 1, z + 1)]
    })
}

fn mesh_chunk_snapshot(
    snapshot: &ChunkStoreSnapshot,
    pos: ChunkPos,
    registry: &BlockRegistry,
    uv_map: &AtlasUVMap,
    lod: u32,
    sections_to_mesh: std::ops::Range<i32>,
) -> ChunkMeshData {
    let mut logged_missing: std::collections::HashSet<String> = std::collections::HashSet::new();

    let step = 1i32 << lod;

    let min_y = snapshot.min_y();
    let max_y = min_y + snapshot.height() as i32;
    let world_x = pos.x * 16;
    let world_z = pos.z * 16;

    let section_count = ((max_y - min_y) / 16).max(0);
    // Clamp the request; only these sections are meshed. The Vec still spans the
    // whole column so blocks route by absolute section index.
    let range = sections_to_mesh.start.max(0)..sections_to_mesh.end.min(section_count);
    let by_start = min_y + range.start * 16;
    let by_end = min_y + range.end * 16;

    let mut sections: Vec<SectionMesh> = (0..section_count)
        .map(|i| SectionMesh {
            section_index: i,
            vertices: Vec::new(),
            indices: Vec::new(),
            water_indices: Vec::new(),
        })
        .collect();

    // The type map is a state->id map, so it only needs the meshed span (+1-block
    // border for face culling); states outside it are never queried.
    let type_map = if lod == 0 {
        Some(BlockTypeMap::build(
            snapshot, registry, world_x, world_z, by_start, by_end,
        ))
    } else {
        None
    };
    let mut visibility: Vec<(i32, VisibilitySet)> = Vec::new();
    if let Some(ref tm) = type_map {
        for si in range.clone() {
            let sec = &mut sections[si as usize];
            let section_y = min_y + si * 16;
            let vis = greedy_mesh_section(
                &mut sec.vertices,
                &mut sec.indices,
                snapshot,
                registry,
                tm,
                uv_map,
                world_x,
                section_y,
                world_z,
            );
            visibility.push((si, vis));
        }
    } else {
        // LOD > 0 (distant): treat as fully see-through. Cave culling is a
        // near-field win; the long-range pass is deferred.
        for si in range.clone() {
            visibility.push((si, VisibilitySet::all()));
        }
    }

    let mut local_z = 0i32;
    while local_z < 16 {
        let mut local_x = 0i32;
        while local_x < 16 {
            let bx = world_x + local_x;
            let bz = world_z + local_z;

            let mut by = by_start;
            while by < by_end {
                let mut state = snapshot.get_block_state(bx, by, bz);
                let mut kind = classify_block(state);
                // Checks for non air block in the cube region to represent the area if the
                // picked block is air
                if lod > 0 && matches!(kind, BlockKind::Air) {
                    let end_y = (by + step).min(by_end);
                    for try_y in (by + 1)..end_y {
                        let s = snapshot.get_block_state(bx, try_y, bz);
                        let k = classify_block(s);
                        if !matches!(k, BlockKind::Air) {
                            state = s;
                            kind = k;
                            break;
                        }
                    }
                }

                if matches!(kind, BlockKind::Air) {
                    by += step;
                    continue;
                }

                if lod == 0
                    && let Some(ref tm) = type_map
                    && tm.get_id(state) != 0
                {
                    by += step;
                    continue;
                }

                let block_pos = [bx as f32, by as f32, bz as f32];

                // Route this block's geometry to its 16-tall section. Clamped so
                // a non-16-aligned world height can't index past the last section.
                let s =
                    (((by - min_y) / 16) as usize).min((section_count as usize).saturating_sub(1));
                let sec = &mut sections[s];

                if lod > 0 {
                    emit_lod_cube(
                        &mut sec.vertices,
                        &mut sec.indices,
                        block_pos,
                        state,
                        snapshot,
                        registry,
                        uv_map,
                        bx,
                        by,
                        bz,
                        step,
                    );
                } else if let BlockKind::Water | BlockKind::Lava = kind {
                    // Water is translucent (separate blended pass); lava is opaque.
                    let fluid_indices = if matches!(kind, BlockKind::Water) {
                        &mut sec.water_indices
                    } else {
                        &mut sec.indices
                    };
                    emit_fluid(
                        &mut sec.vertices,
                        fluid_indices,
                        block_pos,
                        state,
                        snapshot,
                        registry,
                        uv_map,
                        bx,
                        by,
                        bz,
                    );
                } else if let Some(baked) = registry.get_baked_model(state) {
                    emit_baked_model(
                        &mut sec.vertices,
                        &mut sec.indices,
                        block_pos,
                        baked,
                        snapshot,
                        registry,
                        uv_map,
                        bx,
                        by,
                        bz,
                    );
                } else if let Some(quads) = registry.get_multipart_quads(state) {
                    emit_multipart(
                        &mut sec.vertices,
                        &mut sec.indices,
                        block_pos,
                        &quads,
                        snapshot,
                        registry,
                        uv_map,
                        bx,
                        by,
                        bz,
                    );
                } else if let Some(textures) = registry.get_textures(state) {
                    emit_cube_faces(
                        &mut sec.vertices,
                        &mut sec.indices,
                        block_pos,
                        textures,
                        snapshot,
                        registry,
                        uv_map,
                        bx,
                        by,
                        bz,
                    );
                } else {
                    let block: Box<dyn azalea_block::BlockTrait> = state.into();
                    let id = block.id().to_string();
                    if logged_missing.insert(id.clone()) {
                        tracing::warn!("Missing model: {id}");
                    }
                    emit_missing_cube(
                        &mut sec.vertices,
                        &mut sec.indices,
                        block_pos,
                        snapshot,
                        registry,
                        bx,
                        by,
                        bz,
                    );
                }
                by += step;
            }
            local_x += step;
        }
        local_z += step;
    }

    // Keep only non-empty sections (untouched out-of-range ones stay empty);
    // empty indices within `range` are freed by the per-section upload.
    sections.retain(|s| {
        !s.vertices.is_empty() && (!s.indices.is_empty() || !s.water_indices.is_empty())
    });

    ChunkMeshData {
        pos,
        sections,
        replaced: range,
        content_gen: 0,
        upload_epoch: 0,
        visibility,
        timing: None,
    }
}

#[allow(clippy::too_many_arguments)]
fn emit_baked_model(
    vertices: &mut Vec<ChunkVertex>,
    indices: &mut Vec<u32>,
    block_pos: [f32; 3],
    model: &BakedModel,
    snapshot: &ChunkStoreSnapshot,
    registry: &BlockRegistry,
    uv_map: &AtlasUVMap,
    bx: i32,
    by: i32,
    bz: i32,
) {
    for quad in &model.quads {
        if let Some(cullface) = quad.cullface {
            let offset = cullface.offset();
            let neighbor = snapshot.get_block_state(bx + offset[0], by + offset[1], bz + offset[2]);
            if registry.occludes_neighbor(neighbor) {
                continue;
            }
        }

        let region = uv_map.get_region(&quad.texture);
        let tint = tint_color(
            quad.tint,
            snapshot.grass_tint(bx, by, bz),
            snapshot.foliage_tint(bx, by, bz),
            snapshot.dry_foliage_tint(bx, by, bz),
        );
        let lights = if let Some(dir) = quad.cullface {
            compute_face_ao(snapshot, registry, bx, by, bz, dir)
        } else {
            [quad.shade_light; 4]
        };
        emit_face(
            vertices,
            indices,
            block_pos,
            &quad.positions,
            &quad.uvs,
            lights,
            region,
            tint,
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn emit_cube_faces(
    vertices: &mut Vec<ChunkVertex>,
    indices: &mut Vec<u32>,
    block_pos: [f32; 3],
    textures: &crate::world::block::registry::FaceTextures,
    snapshot: &ChunkStoreSnapshot,
    registry: &BlockRegistry,
    uv_map: &AtlasUVMap,
    bx: i32,
    by: i32,
    bz: i32,
) {
    let tint = tint_color(
        textures.tint,
        snapshot.grass_tint(bx, by, bz),
        snapshot.foliage_tint(bx, by, bz),
        snapshot.dry_foliage_tint(bx, by, bz),
    );

    for (i, dir) in CUBE_FACE_DIRS.iter().enumerate() {
        let offset = dir.offset();
        let neighbor = snapshot.get_block_state(bx + offset[0], by + offset[1], bz + offset[2]);
        if registry.occludes_neighbor(neighbor) {
            continue;
        }

        let face_tex = match i {
            0 => &textures.top,
            1 => &textures.bottom,
            2 => &textures.north,
            3 => &textures.south,
            4 => &textures.east,
            _ => &textures.west,
        };
        let region = uv_map.get_region(face_tex);
        let (positions, uvs, _) = cube_face_geometry(*dir);
        let lights = compute_face_ao(snapshot, registry, bx, by, bz, *dir);

        let is_side = i >= 2;
        if let Some(overlay) = textures.side_overlay.as_deref().filter(|_| is_side) {
            emit_face(
                vertices,
                indices,
                block_pos,
                &positions,
                &uvs,
                lights,
                region,
                PACKED_WHITE_SHIFTED,
            );
            let overlay_region = uv_map.get_region(overlay);
            emit_face(
                vertices,
                indices,
                block_pos,
                &positions,
                &uvs,
                lights,
                overlay_region,
                tint,
            );
        } else {
            let is_tinted =
                !matches!(textures.tint, Tint::None) && (textures.side_overlay.is_none() || i == 0);
            let face_tint = if is_tinted {
                tint
            } else {
                PACKED_WHITE_SHIFTED
            };
            emit_face(
                vertices, indices, block_pos, &positions, &uvs, lights, region, face_tint,
            );
        }
    }
}

enum BlockKind {
    Air,
    Water,
    Lava,
    Solid,
}

fn classify_block(state: azalea_block::BlockState) -> BlockKind {
    if state.is_air() {
        return BlockKind::Air;
    }
    let block: Box<dyn azalea_block::BlockTrait> = state.into();
    match block.id() {
        "cave_air" | "void_air" | "light" | "barrier" | "structure_void" | "moving_piston" => {
            BlockKind::Air
        }
        "water" | "bubble_column" => BlockKind::Water,
        "lava" => BlockKind::Lava,
        _ => BlockKind::Solid,
    }
}

// TODO: biome-based water color
// TODO: per-corner height averaging for smooth water surfaces
// TODO: flowing water texture (water_flow) with direction-based rotation
// TODO: per-level height for flowing water (level / 9.0 per corner)

const FLUID_MAX_HEIGHT: f32 = 8.0 / 9.0;

#[allow(clippy::too_many_arguments)]
fn block_face_tex_tint(
    state: azalea_block::BlockState,
    dir: Direction,
    uv_map: &AtlasUVMap,
    snapshot: &ChunkStoreSnapshot,
    registry: &BlockRegistry,
    bx: i32,
    by: i32,
    bz: i32,
) -> (AtlasRegion, u32) {
    match classify_block(state) {
        BlockKind::Water => (
            uv_map.get_region("water_still"),
            pack_tint_shifted([0.247, 0.463, 0.894]),
        ),
        BlockKind::Lava => (uv_map.get_region("lava_still"), PACKED_WHITE_SHIFTED),
        _ => {
            if let Some(textures) = registry.get_textures(state) {
                let tint = tint_color(
                    textures.tint,
                    snapshot.grass_tint(bx, by, bz),
                    snapshot.foliage_tint(bx, by, bz),
                    snapshot.dry_foliage_tint(bx, by, bz),
                );
                let tex_name = match dir {
                    Direction::Up => &textures.top,
                    Direction::Down => &textures.bottom,
                    Direction::North => &textures.north,
                    Direction::South => &textures.south,
                    Direction::East => &textures.east,
                    Direction::West => &textures.west,
                };
                (uv_map.get_region(tex_name), tint)
            } else {
                (uv_map.get_region(""), MISSING_TINT)
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn emit_fluid(
    vertices: &mut Vec<ChunkVertex>,
    indices: &mut Vec<u32>,
    block_pos: [f32; 3],
    state: azalea_block::BlockState,
    snapshot: &ChunkStoreSnapshot,
    registry: &BlockRegistry,
    uv_map: &AtlasUVMap,
    bx: i32,
    by: i32,
    bz: i32,
) {
    let (region, tint) =
        block_face_tex_tint(state, Direction::Up, uv_map, snapshot, registry, bx, by, bz);

    for dir in &CUBE_FACE_DIRS {
        let offset = dir.offset();
        let neighbor = snapshot.get_block_state(bx + offset[0], by + offset[1], bz + offset[2]);

        if matches!(classify_block(neighbor), BlockKind::Water | BlockKind::Lava)
            || registry.occludes_neighbor(neighbor)
        {
            continue;
        }

        let (mut positions, uvs, light) = cube_face_geometry(*dir);

        if matches!(dir, Direction::Up) {
            // A water/lava block above would have culled this face already, so
            // the surface always sits at the lowered fluid height.
            for p in &mut positions {
                p[1] = FLUID_MAX_HEIGHT;
            }

            emit_face(
                vertices, indices, block_pos, &positions, &uvs, [light; 4], region, tint,
            );

            // Vanilla's backward up-face: the surface seen from below (underwater
            // looking up). Reversed winding so it survives back-face culling.
            let rev_positions = [positions[0], positions[3], positions[2], positions[1]];
            let rev_uvs = [uvs[0], uvs[3], uvs[2], uvs[1]];
            emit_face(
                vertices,
                indices,
                block_pos,
                &rev_positions,
                &rev_uvs,
                [light; 4],
                region,
                tint,
            );
            continue;
        }

        emit_face(
            vertices, indices, block_pos, &positions, &uvs, [light; 4], region, tint,
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn emit_multipart(
    vertices: &mut Vec<ChunkVertex>,
    indices: &mut Vec<u32>,
    block_pos: [f32; 3],
    quads: &[&crate::world::block::model::BakedQuad],
    snapshot: &ChunkStoreSnapshot,
    registry: &BlockRegistry,
    uv_map: &AtlasUVMap,
    bx: i32,
    by: i32,
    bz: i32,
) {
    for quad in quads {
        if let Some(cullface) = quad.cullface {
            let offset = cullface.offset();
            let neighbor = snapshot.get_block_state(bx + offset[0], by + offset[1], bz + offset[2]);
            if registry.occludes_neighbor(neighbor) {
                continue;
            }
        }

        let region = uv_map.get_region(&quad.texture);
        let tint = tint_color(
            quad.tint,
            snapshot.grass_tint(bx, by, bz),
            snapshot.foliage_tint(bx, by, bz),
            snapshot.dry_foliage_tint(bx, by, bz),
        );
        emit_face(
            vertices,
            indices,
            block_pos,
            &quad.positions,
            &quad.uvs,
            [quad.shade_light; 4],
            region,
            tint,
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn emit_lod_cube(
    vertices: &mut Vec<ChunkVertex>,
    indices: &mut Vec<u32>,
    block_pos: [f32; 3],
    state: azalea_block::BlockState,
    snapshot: &ChunkStoreSnapshot,
    registry: &BlockRegistry,
    uv_map: &AtlasUVMap,
    bx: i32,
    by: i32,
    bz: i32,
    step: i32,
) {
    let is_fluid = matches!(classify_block(state), BlockKind::Water | BlockKind::Lava);
    // We have to do this otherwise there becomes a visible seam at the LOD border
    let fluid_top = if is_fluid {
        let above = snapshot.get_block_state(bx, by + 1, bz);
        if matches!(classify_block(above), BlockKind::Water | BlockKind::Lava) {
            1.0
        } else {
            FLUID_MAX_HEIGHT
        }
    } else {
        1.0
    };

    for dir in &CUBE_FACE_DIRS {
        let offset = dir.offset();
        let nx = bx + offset[0] * step;
        let ny = by + offset[1] * step;
        let nz = bz + offset[2] * step;
        let neighbor = snapshot.get_block_state(nx, ny, nz);
        if registry.occludes_neighbor(neighbor) {
            continue;
        }
        if is_fluid && matches!(classify_block(neighbor), BlockKind::Water | BlockKind::Lava) {
            continue;
        }

        let (region, tint) =
            block_face_tex_tint(state, *dir, uv_map, snapshot, registry, bx, by, bz);

        let (positions, uvs, light) = cube_face_geometry(*dir);
        let s = step as f32;
        let sy = if is_fluid { fluid_top } else { s };
        let base = vertices.len() as u32;
        for i in 0..4 {
            vertices.push(ChunkVertex {
                position: [
                    block_pos[0] + positions[i][0] * s,
                    block_pos[1] + positions[i][1] * sy,
                    block_pos[2] + positions[i][2] * s,
                ],
                tex_coords: pack_uv(
                    region.u_min + uvs[i][0] * (region.u_max - region.u_min),
                    region.v_min + uvs[i][1] * (region.v_max - region.v_min),
                ),
                light_tint: pack_light_tint(light, tint),
            });
        }
        indices.extend_from_slice(&[base, base + 1, base + 2, base + 2, base + 3, base]);
    }
}

const MISSING_TINT: u32 = pack_tint_shifted([1.0, 0.0, 1.0]);

#[allow(clippy::too_many_arguments)]
fn emit_missing_cube(
    vertices: &mut Vec<ChunkVertex>,
    indices: &mut Vec<u32>,
    block_pos: [f32; 3],
    snapshot: &ChunkStoreSnapshot,
    registry: &BlockRegistry,
    bx: i32,
    by: i32,
    bz: i32,
) {
    for dir in &CUBE_FACE_DIRS {
        let offset = dir.offset();
        let neighbor = snapshot.get_block_state(bx + offset[0], by + offset[1], bz + offset[2]);
        if registry.occludes_neighbor(neighbor) {
            continue;
        }

        let (positions, _, light) = cube_face_geometry(*dir);
        let base = vertices.len() as u32;
        for pos in &positions {
            vertices.push(ChunkVertex {
                position: [
                    block_pos[0] + pos[0],
                    block_pos[1] + pos[1],
                    block_pos[2] + pos[2],
                ],
                tex_coords: pack_uv(0.0, 0.0),
                light_tint: pack_light_tint(light, MISSING_TINT),
            });
        }
        indices.extend_from_slice(&[base, base + 1, base + 2, base + 2, base + 3, base]);
    }
}

pub(crate) const CUBE_FACE_DIRS: [Direction; 6] = [
    Direction::Up,
    Direction::Down,
    Direction::North,
    Direction::South,
    Direction::East,
    Direction::West,
];

#[allow(clippy::too_many_arguments)]
fn emit_face(
    vertices: &mut Vec<ChunkVertex>,
    indices: &mut Vec<u32>,
    block_pos: [f32; 3],
    positions: &[[f32; 3]; 4],
    uvs: &[[f32; 2]; 4],
    lights: [f32; 4],
    region: AtlasRegion,
    tint: u32,
) {
    let base = vertices.len() as u32;
    let u_span = region.u_max - region.u_min;
    let v_span = region.v_max - region.v_min;

    for i in 0..4 {
        vertices.push(ChunkVertex {
            position: [
                block_pos[0] + positions[i][0],
                block_pos[1] + positions[i][1],
                block_pos[2] + positions[i][2],
            ],
            tex_coords: pack_uv(
                region.u_min + uvs[i][0] * u_span,
                region.v_min + uvs[i][1] * v_span,
            ),
            light_tint: pack_light_tint(lights[i], tint),
        });
    }

    if lights[0] + lights[2] > lights[1] + lights[3] {
        indices.extend_from_slice(&[base + 1, base + 2, base + 3, base + 3, base, base + 1]);
    } else {
        indices.extend_from_slice(&[base, base + 1, base + 2, base + 2, base + 3, base]);
    }
}

fn shade_brightness(state: azalea_block::BlockState, registry: &BlockRegistry) -> f32 {
    // TODO: non-occluding full cubes (leaves, glass, ice) still darken adjacent
    // faces here. Vanilla's are `isViewBlocking=never` and don't contribute AO.
    if registry.is_opaque_full_cube(state) {
        0.2
    } else {
        1.0
    }
}

/// Centre-relative offset of vanilla's `AdjacencyInfo.corners[0]` neighbour
/// (`centre + dir + corners[0]`), the `shade0` occlusion fallback.
fn corners0_offset(dir: Direction) -> [i32; 3] {
    match dir {
        // corners[0] = EAST(+x)
        Direction::Up => [1, 1, 0],
        // corners[0] = WEST(-x)
        Direction::Down => [-1, -1, 0],
        // corners[0] = UP(+y)
        Direction::North => [0, 1, -1],
        // corners[0] = WEST(-x)
        Direction::South => [-1, 0, 1],
        // corners[0] = UP(+y)
        Direction::West => [-1, 1, 0],
        // corners[0] = DOWN(-y)
        Direction::East => [1, -1, 0],
    }
}

fn compute_face_ao(
    snapshot: &ChunkStoreSnapshot,
    registry: &BlockRegistry,
    bx: i32,
    by: i32,
    bz: i32,
    dir: Direction,
) -> [f32; 4] {
    let s = |dx: i32, dy: i32, dz: i32| -> f32 {
        shade_brightness(
            snapshot.get_block_state(bx + dx, by + dy, bz + dz),
            registry,
        )
    };
    let l = |dx: i32, dy: i32, dz: i32| -> f32 { snapshot.get_light(bx + dx, by + dy, bz + dz) };
    let dir_shade = match dir {
        Direction::Up => 1.0,
        Direction::Down => 0.5,
        Direction::North | Direction::South => 0.8,
        Direction::East | Direction::West => 0.6,
    };

    let c0 = corners0_offset(dir);
    let shade0 = s(c0[0], c0[1], c0[2]);
    let vertex_ao = |side1: f32, side2: f32, corner: f32| -> f32 {
        super::block_ao::vertex_brightness(side1, side2, corner, shade0)
    };

    let (ao, lights) = match dir {
        Direction::Up => {
            let n = [0, 1, 0];
            (
                [
                    vertex_ao(s(0, 1, 1), s(-1, 1, 0), s(-1, 1, 1)),
                    vertex_ao(s(0, 1, 1), s(1, 1, 0), s(1, 1, 1)),
                    vertex_ao(s(0, 1, -1), s(1, 1, 0), s(1, 1, -1)),
                    vertex_ao(s(0, 1, -1), s(-1, 1, 0), s(-1, 1, -1)),
                ],
                [
                    avg4(l(n[0], n[1], n[2]), l(0, 1, 1), l(-1, 1, 0), l(-1, 1, 1)),
                    avg4(l(n[0], n[1], n[2]), l(0, 1, 1), l(1, 1, 0), l(1, 1, 1)),
                    avg4(l(n[0], n[1], n[2]), l(0, 1, -1), l(1, 1, 0), l(1, 1, -1)),
                    avg4(l(n[0], n[1], n[2]), l(0, 1, -1), l(-1, 1, 0), l(-1, 1, -1)),
                ],
            )
        }
        Direction::Down => {
            let n = [0, -1, 0];
            (
                [
                    vertex_ao(s(0, -1, -1), s(-1, -1, 0), s(-1, -1, -1)),
                    vertex_ao(s(0, -1, -1), s(1, -1, 0), s(1, -1, -1)),
                    vertex_ao(s(0, -1, 1), s(1, -1, 0), s(1, -1, 1)),
                    vertex_ao(s(0, -1, 1), s(-1, -1, 0), s(-1, -1, 1)),
                ],
                [
                    avg4(
                        l(n[0], n[1], n[2]),
                        l(0, -1, -1),
                        l(-1, -1, 0),
                        l(-1, -1, -1),
                    ),
                    avg4(l(n[0], n[1], n[2]), l(0, -1, -1), l(1, -1, 0), l(1, -1, -1)),
                    avg4(l(n[0], n[1], n[2]), l(0, -1, 1), l(1, -1, 0), l(1, -1, 1)),
                    avg4(l(n[0], n[1], n[2]), l(0, -1, 1), l(-1, -1, 0), l(-1, -1, 1)),
                ],
            )
        }
        Direction::North => {
            let n = [0, 0, -1];
            (
                [
                    vertex_ao(s(-1, 0, -1), s(0, -1, -1), s(-1, -1, -1)),
                    vertex_ao(s(-1, 0, -1), s(0, 1, -1), s(-1, 1, -1)),
                    vertex_ao(s(1, 0, -1), s(0, 1, -1), s(1, 1, -1)),
                    vertex_ao(s(1, 0, -1), s(0, -1, -1), s(1, -1, -1)),
                ],
                [
                    avg4(
                        l(n[0], n[1], n[2]),
                        l(-1, 0, -1),
                        l(0, -1, -1),
                        l(-1, -1, -1),
                    ),
                    avg4(l(n[0], n[1], n[2]), l(-1, 0, -1), l(0, 1, -1), l(-1, 1, -1)),
                    avg4(l(n[0], n[1], n[2]), l(1, 0, -1), l(0, 1, -1), l(1, 1, -1)),
                    avg4(l(n[0], n[1], n[2]), l(1, 0, -1), l(0, -1, -1), l(1, -1, -1)),
                ],
            )
        }
        Direction::South => {
            let n = [0, 0, 1];
            (
                [
                    vertex_ao(s(1, 0, 1), s(0, -1, 1), s(1, -1, 1)),
                    vertex_ao(s(1, 0, 1), s(0, 1, 1), s(1, 1, 1)),
                    vertex_ao(s(-1, 0, 1), s(0, 1, 1), s(-1, 1, 1)),
                    vertex_ao(s(-1, 0, 1), s(0, -1, 1), s(-1, -1, 1)),
                ],
                [
                    avg4(l(n[0], n[1], n[2]), l(1, 0, 1), l(0, -1, 1), l(1, -1, 1)),
                    avg4(l(n[0], n[1], n[2]), l(1, 0, 1), l(0, 1, 1), l(1, 1, 1)),
                    avg4(l(n[0], n[1], n[2]), l(-1, 0, 1), l(0, 1, 1), l(-1, 1, 1)),
                    avg4(l(n[0], n[1], n[2]), l(-1, 0, 1), l(0, -1, 1), l(-1, -1, 1)),
                ],
            )
        }
        Direction::East => {
            let n = [1, 0, 0];
            (
                [
                    vertex_ao(s(1, 0, -1), s(1, -1, 0), s(1, -1, -1)),
                    vertex_ao(s(1, 0, -1), s(1, 1, 0), s(1, 1, -1)),
                    vertex_ao(s(1, 0, 1), s(1, 1, 0), s(1, 1, 1)),
                    vertex_ao(s(1, 0, 1), s(1, -1, 0), s(1, -1, 1)),
                ],
                [
                    avg4(l(n[0], n[1], n[2]), l(1, 0, -1), l(1, -1, 0), l(1, -1, -1)),
                    avg4(l(n[0], n[1], n[2]), l(1, 0, -1), l(1, 1, 0), l(1, 1, -1)),
                    avg4(l(n[0], n[1], n[2]), l(1, 0, 1), l(1, 1, 0), l(1, 1, 1)),
                    avg4(l(n[0], n[1], n[2]), l(1, 0, 1), l(1, -1, 0), l(1, -1, 1)),
                ],
            )
        }
        Direction::West => {
            let n = [-1, 0, 0];
            (
                [
                    vertex_ao(s(-1, 0, 1), s(-1, -1, 0), s(-1, -1, 1)),
                    vertex_ao(s(-1, 0, 1), s(-1, 1, 0), s(-1, 1, 1)),
                    vertex_ao(s(-1, 0, -1), s(-1, 1, 0), s(-1, 1, -1)),
                    vertex_ao(s(-1, 0, -1), s(-1, -1, 0), s(-1, -1, -1)),
                ],
                [
                    avg4(l(n[0], n[1], n[2]), l(-1, 0, 1), l(-1, -1, 0), l(-1, -1, 1)),
                    avg4(l(n[0], n[1], n[2]), l(-1, 0, 1), l(-1, 1, 0), l(-1, 1, 1)),
                    avg4(l(n[0], n[1], n[2]), l(-1, 0, -1), l(-1, 1, 0), l(-1, 1, -1)),
                    avg4(
                        l(n[0], n[1], n[2]),
                        l(-1, 0, -1),
                        l(-1, -1, 0),
                        l(-1, -1, -1),
                    ),
                ],
            )
        }
    };
    [
        ao[0] * lights[0] * dir_shade,
        ao[1] * lights[1] * dir_shade,
        ao[2] * lights[2] * dir_shade,
        ao[3] * lights[3] * dir_shade,
    ]
}

fn avg4(a: f32, b: f32, c: f32, d: f32) -> f32 {
    (a + b + c + d) * 0.25
}

pub(crate) fn cube_face_geometry(dir: Direction) -> ([[f32; 3]; 4], [[f32; 2]; 4], f32) {
    match dir {
        Direction::Up => (
            [
                [0.0, 1.0, 1.0],
                [1.0, 1.0, 1.0],
                [1.0, 1.0, 0.0],
                [0.0, 1.0, 0.0],
            ],
            [[0.0, 1.0], [1.0, 1.0], [1.0, 0.0], [0.0, 0.0]],
            1.0,
        ),
        Direction::Down => (
            [
                [0.0, 0.0, 0.0],
                [1.0, 0.0, 0.0],
                [1.0, 0.0, 1.0],
                [0.0, 0.0, 1.0],
            ],
            [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]],
            0.5,
        ),
        Direction::North => (
            [
                [0.0, 0.0, 0.0],
                [0.0, 1.0, 0.0],
                [1.0, 1.0, 0.0],
                [1.0, 0.0, 0.0],
            ],
            [[0.0, 1.0], [0.0, 0.0], [1.0, 0.0], [1.0, 1.0]],
            0.8,
        ),
        Direction::South => (
            [
                [1.0, 0.0, 1.0],
                [1.0, 1.0, 1.0],
                [0.0, 1.0, 1.0],
                [0.0, 0.0, 1.0],
            ],
            [[1.0, 1.0], [1.0, 0.0], [0.0, 0.0], [0.0, 1.0]],
            0.8,
        ),
        Direction::East => (
            [
                [1.0, 0.0, 0.0],
                [1.0, 1.0, 0.0],
                [1.0, 1.0, 1.0],
                [1.0, 0.0, 1.0],
            ],
            [[1.0, 1.0], [1.0, 0.0], [0.0, 0.0], [0.0, 1.0]],
            0.6,
        ),
        Direction::West => (
            [
                [0.0, 0.0, 1.0],
                [0.0, 1.0, 1.0],
                [0.0, 1.0, 0.0],
                [0.0, 0.0, 0.0],
            ],
            [[1.0, 1.0], [1.0, 0.0], [0.0, 0.0], [0.0, 1.0]],
            0.6,
        ),
    }
}
