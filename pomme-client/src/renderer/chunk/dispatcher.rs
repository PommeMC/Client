use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, AtomicU64, Ordering};
use std::thread;

use azalea_core::position::{ChunkPos, ChunkSectionPos};
use crossbeam_epoch as epoch;
use crossbeam_epoch::Atomic;
use glam::DVec3;

use super::mesher::{BiomeClimate, Colormap, SectionMeshData, SectionStoreSnapshot, mesh_section};
use super::section::LocalSection;
use crate::renderer::Renderer;
use crate::renderer::chunk::atlas::AtlasUVMap;
use crate::resource_pack::ResourcePackManager;
use crate::util::{ChunkRing, SectionRing};
use crate::world::block::registry::BlockRegistry;
use crate::world::chunk::SharedChunkStore;
/// Per-section lock-ordered metadata, aligned to 64 bytes to eliminate false
/// sharing across adjacent sections in the cache line.
#[repr(align(64))]
pub struct SectionMeta {
    pub ver: AtomicU64,       // Monotonic, bumped on any mutation to this slot
    pub pos: AtomicU64,       // Packed ChunkSectionPos identity of current slot occupant
    pub target_lod: AtomicU8, // Target LOD level assigned to this section slot
}
impl Default for SectionMeta {
    fn default() -> Self {
        Self {
            ver: AtomicU64::new(0),
            pos: AtomicU64::new(u64::MAX),
            target_lod: AtomicU8::new(0),
        }
    }
}
pub const fn pack_section_pos(pos: ChunkSectionPos) -> u64 {
    let x = (pos.x as u64) & 0x00FF_FFFF;
    let z = (pos.z as u64) & 0x00FF_FFFF;
    let y = (pos.y as u64) & 0x0000_FFFF;
    x | (z << 24) | (y << 48)
}
pub const fn unpack_section_pos(val: u64) -> ChunkSectionPos {
    let x_raw = (val & 0x00FF_FFFF) as u32;
    let z_raw = ((val >> 24) & 0x00FF_FFFF) as u32;
    let y_raw = ((val >> 48) & 0x0000_FFFF) as u32;
    let x = if x_raw & 0x0080_0000 != 0 {
        (x_raw | 0xFF00_0000) as i32
    } else {
        x_raw as i32
    };
    let z = if z_raw & 0x0080_0000 != 0 {
        (z_raw | 0xFF00_0000) as i32
    } else {
        z_raw as i32
    };
    let y = if y_raw & 0x0000_8000 != 0 {
        (y_raw | 0xFFFF_0000) as i32
    } else {
        y_raw as i32
    };
    ChunkSectionPos::new(x, y, z)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn section_pos_roundtrips_negative_coords() {
        for pos in [
            ChunkSectionPos::new(0, 0, 0),
            ChunkSectionPos::new(-1, -4, -1),
            ChunkSectionPos::new(123_456, -4, -123_456),
            ChunkSectionPos::new(-8_388_608, 19, 8_388_607),
            ChunkSectionPos::new(7, -32_768, -7),
        ] {
            assert_eq!(unpack_section_pos(pack_section_pos(pos)), pos);
        }
    }
}
pub struct ChunkMeshing {
    /// Section metadata ring: single 64-byte cache line per section slot
    pub section_meta: Arc<SectionRing<SectionMeta>>,
    /// Shared: workers clear/re-set bits, edits set bits
    pub update_set: Arc<ChunkRing<AtomicU32>>,
    store: Arc<SharedChunkStore>,
    result_rx: crossbeam_channel::Receiver<SectionMeshData>,
    queue: Arc<MeshQueue>,
    workers: Vec<std::thread::JoinHandle<()>>,
    next_epoch: AtomicU64,
    grass_colormap: Arc<Colormap>,
    foliage_colormap: Arc<Colormap>,
    dry_foliage_colormap: Arc<Colormap>,
    biome_climate: Arc<HashMap<u32, BiomeClimate>>,
}
impl ChunkMeshing {
    pub fn new(
        renderer: &Renderer,
        shared_chunk_store: Arc<SharedChunkStore>,
        biome_climate: Arc<HashMap<u32, BiomeClimate>>,
        resource_packs: Option<&ResourcePackManager>,
    ) -> Self {
        renderer.create_chunk_meshing(shared_chunk_store, biome_climate, resource_packs)
    }
    pub fn notify(&self) {
        for worker in &self.workers {
            worker.thread().unpark();
        }
    }
    pub fn create(
        shared_chunk_store: Arc<SharedChunkStore>,
        registry: BlockRegistry,
        uv_map: AtlasUVMap,
        grass_colormap: Colormap,
        foliage_colormap: Colormap,
        dry_foliage_colormap: Colormap,
        biome_climate: Arc<HashMap<u32, BiomeClimate>>,
    ) -> Self {
        let (result_tx, result_rx) = crossbeam_channel::unbounded();
        let section_meta = Arc::new(SectionRing::from_fn(|_, _, _| SectionMeta::default()));
        let update_set = Arc::new(ChunkRing::from_fn(|_, _| AtomicU32::new(0)));
        let queue = Arc::new(MeshQueue::new());
        let worker_count = std::thread::available_parallelism()
            .map(|n| n.get().saturating_sub(1).max(1))
            .unwrap_or(1);
        let registry = Arc::new(registry);
        let uv_map = Arc::new(uv_map);
        let grass_colormap = Arc::new(grass_colormap);
        let foliage_colormap = Arc::new(foliage_colormap);
        let dry_foliage_colormap = Arc::new(dry_foliage_colormap);
        let mut workers = Vec::with_capacity(worker_count);
        for _ in 0..worker_count {
            let queue = Arc::clone(&queue);
            let shared_chunk_store = Arc::clone(&shared_chunk_store);
            let section_meta = Arc::clone(&section_meta);
            let update_set = Arc::clone(&update_set);
            let registry = Arc::clone(&registry);
            let uv_map = Arc::clone(&uv_map);
            let grass_colormap = Arc::clone(&grass_colormap);
            let foliage_colormap = Arc::clone(&foliage_colormap);
            let dry_foliage_colormap = Arc::clone(&dry_foliage_colormap);
            let biome_climate = Arc::clone(&biome_climate);
            let tx = result_tx.clone();
            workers.push(
                std::thread::Builder::new()
                    .name("chunk-mesher".into())
                    .spawn(move || {
                        queue.run_worker(
                            shared_chunk_store,
                            section_meta,
                            update_set,
                            registry,
                            uv_map,
                            grass_colormap,
                            foliage_colormap,
                            dry_foliage_colormap,
                            biome_climate,
                            tx,
                        )
                    })
                    .expect("spawn chunk-mesher thread"),
            );
        }
        Self {
            section_meta,
            update_set,
            store: shared_chunk_store,
            result_rx,
            queue,
            workers,
            next_epoch: AtomicU64::new(1),
            grass_colormap,
            foliage_colormap,
            dry_foliage_colormap,
            biome_climate,
        }
    }

    /// The grass/foliage/dry-foliage colormaps, shared with the particle
    /// tinting.
    pub fn colormaps(&self) -> (Arc<Colormap>, Arc<Colormap>, Arc<Colormap>) {
        (
            Arc::clone(&self.grass_colormap),
            Arc::clone(&self.foliage_colormap),
            Arc::clone(&self.dry_foliage_colormap),
        )
    }
    pub fn clear(&mut self) {
        for cell in &self.update_set.buf {
            cell.store(0xFFFF_FFFF, Ordering::Release);
        }
    }
    pub fn on_chunk_unload(&mut self, pos: ChunkPos) {
        let min_y_sec = self.store.min_section_y();
        for si in 0..self.store.section_count() {
            let spos = ChunkSectionPos::new(pos.x, min_y_sec + si, pos.z);
            let meta = self.section_meta.get(spos);
            meta.pos.store(u64::MAX, Ordering::Release);
            meta.ver.fetch_add(1, Ordering::Release);
        }
        self.update_set.get(pos).store(0, Ordering::Release);
    }
    pub fn recreate_dispatcher(
        &mut self,
        renderer: &Renderer,
        shared_chunk_store: Arc<SharedChunkStore>,
        biome_climate: Arc<HashMap<u32, BiomeClimate>>,
    ) {
        *self = renderer.create_chunk_meshing(shared_chunk_store, biome_climate, None);
    }
    pub fn set_biome_climate(&mut self, climate: Arc<HashMap<u32, BiomeClimate>>) {
        self.biome_climate = climate;
    }
    pub fn bump_content_gen(&mut self, pos: ChunkSectionPos) -> u64 {
        let meta = self.section_meta.get(pos);
        let chunk_pos = ChunkPos::new(pos.x, pos.z);
        let rel_y = self.store.section_y_index(pos.y);
        meta.pos.store(pack_section_pos(pos), Ordering::Release);
        let new_ver = meta.ver.fetch_add(1, Ordering::Release) + 1;
        self.update_set
            .get(chunk_pos)
            .fetch_or(1 << rel_y, Ordering::Release);
        new_ver
    }
    pub fn enqueue_section_edit(&mut self, pos: ChunkSectionPos, lod: u32) {
        let meta = self.section_meta.get(pos);
        let chunk_pos = ChunkPos::new(pos.x, pos.z);
        let rel_y = self.store.section_y_index(pos.y);
        meta.pos.store(pack_section_pos(pos), Ordering::Release);
        meta.target_lod.store(lod as u8, Ordering::Release);
        meta.ver.fetch_add(1, Ordering::Release);
        self.update_set
            .get(chunk_pos)
            .fetch_or(1 << rel_y, Ordering::Release);
    }
    /// Per-frame rescan: Bitwise AND of `vis_mask` and `update_set`,
    /// distance-sorted nearest to camera.
    pub fn rescan_mesh_jobs(
        &mut self,
        loaded: &std::collections::HashSet<ChunkPos>,
        player_chunk: ChunkPos,
        visibility: &ChunkRing<u32>,
        visibility_center: ChunkPos,
        camera_pos: &DVec3,
    ) {
        let min_y_section = self.store.min_section_y();
        let section_count = self.store.section_count();
        let mut candidate_jobs = Vec::new();
        for &pos in loaded {
            let vis_mask = visibility
                .get_in_range(pos, visibility_center)
                .copied()
                .unwrap_or(u32::MAX);
            let update_mask = self.update_set.get(pos).load(Ordering::Acquire);
            let active_mask = vis_mask & update_mask;
            let lod = crate::app::core::chunk_lod(pos, player_chunk);
            for si in 0..section_count {
                let spos = ChunkSectionPos::new(pos.x, min_y_section + si, pos.z);
                let meta = self.section_meta.get(spos);
                let current_lod = meta.target_lod.load(Ordering::Acquire) as u32;
                let lod_changed = current_lod != lod;
                let is_dirty = (active_mask & (1 << si)) != 0;
                let packed_pos = meta.pos.load(Ordering::Acquire);
                let identity_changed = packed_pos != pack_section_pos(spos);
                if is_dirty || lod_changed || identity_changed {
                    if lod_changed {
                        meta.target_lod.store(lod as u8, Ordering::Release);
                        meta.ver.fetch_add(1, Ordering::Release);
                    }
                    meta.pos.store(pack_section_pos(spos), Ordering::Release);
                    let upload_epoch = self.next_epoch.fetch_add(1, Ordering::Relaxed);
                    candidate_jobs.push(PendingJob {
                        pos: spos,
                        upload_epoch,
                    });
                }
            }
        }
        candidate_jobs.sort_by(
            |PendingJob { pos: pos_a, .. }, PendingJob { pos: pos_b, .. }| {
                let da = (pos_a.x as f64 * 16.0 + 8.0 - camera_pos.x).powi(2)
                    + (pos_a.y as f64 * 16.0 + 8.0 - camera_pos.y).powi(2)
                    + (pos_a.z as f64 * 16.0 + 8.0 - camera_pos.z).powi(2);
                let db = (pos_b.x as f64 * 16.0 + 8.0 - camera_pos.x).powi(2)
                    + (pos_b.y as f64 * 16.0 + 8.0 - camera_pos.y).powi(2)
                    + (pos_b.z as f64 * 16.0 + 8.0 - camera_pos.z).powi(2);
                da.partial_cmp(&db).unwrap()
            },
        );
        self.queue.send(candidate_jobs);
        self.notify();
    }
    pub fn drain_results(&self) -> impl Iterator<Item = SectionMeshData> + '_ {
        self.result_rx.try_iter()
    }
    pub fn pending_jobs(&self) -> usize {
        self.queue.pending_jobs()
    }
}
impl Drop for ChunkMeshing {
    fn drop(&mut self) {
        self.queue.close();
        self.notify();
        for handle in self.workers.drain(..) {
            let _ = handle.join();
        }
    }
}
#[derive(Clone)]
struct PendingJob {
    pos: ChunkSectionPos,
    upload_epoch: u64,
}
struct ReMarkGuard<'a> {
    update_set: &'a ChunkRing<AtomicU32>,
    pos: ChunkPos,
    rel_y: u32,
    defused: bool,
}
impl<'a> Drop for ReMarkGuard<'a> {
    fn drop(&mut self) {
        if !self.defused {
            self.update_set
                .get(self.pos)
                .fetch_or(1 << self.rel_y, Ordering::Release);
        }
    }
}
impl PendingJob {
    #[allow(clippy::too_many_arguments)]
    fn run(
        self,
        shared_chunk_store: &SharedChunkStore,
        section_meta: &SectionRing<SectionMeta>,
        update_set: &ChunkRing<AtomicU32>,
        registry: &BlockRegistry,
        uv_map: &AtlasUVMap,
        grass_colormap: &Arc<Colormap>,
        foliage_colormap: &Arc<Colormap>,
        dry_foliage_colormap: &Arc<Colormap>,
        biome_climate: &Arc<HashMap<u32, BiomeClimate>>,
        tx: &crossbeam_channel::Sender<SectionMeshData>,
        queue_ms: f32,
    ) {
        let chunk_pos = ChunkPos::new(self.pos.x, self.pos.z);
        let rel_y = shared_chunk_store.section_y_index(self.pos.y);
        // Claim: clear the dirty bit before snapshotting below, so an edit
        // landing mid-mesh re-marks the section (ReMarkGuard) instead of being
        // dropped.
        update_set
            .get(chunk_pos)
            .fetch_and(!(1 << rel_y), Ordering::Acquire);
        let meta = section_meta.get(self.pos);
        let claim_ver = meta.ver.load(Ordering::Acquire);
        let claim_pos_packed = meta.pos.load(Ordering::Acquire);
        let claim_pos = if claim_pos_packed != u64::MAX {
            unpack_section_pos(claim_pos_packed)
        } else {
            self.pos
        };
        let claim_lod = meta.target_lod.load(Ordering::Acquire) as u32;
        let mut guard = ReMarkGuard {
            update_set,
            pos: chunk_pos,
            rel_y,
            defused: false,
        };
        let snapshot = SectionStoreSnapshot {
            section: LocalSection::new_boxed(shared_chunk_store, claim_pos),
            grass_colormap: Arc::clone(grass_colormap),
            foliage_colormap: Arc::clone(foliage_colormap),
            dry_foliage_colormap: Arc::clone(dry_foliage_colormap),
            biome_climate: Arc::clone(biome_climate),
            min_y: shared_chunk_store.min_y(),
            spos: claim_pos,
        };
        let meshed_at = std::time::Instant::now();
        let mut mesh = mesh_section(
            &snapshot,
            claim_pos,
            registry,
            uv_map,
            claim_lod,
            claim_ver,
            self.upload_epoch,
        );
        mesh.queue_ms = queue_ms;
        mesh.mesh_ms = meshed_at.elapsed().as_secs_f32() * 1000.0;
        // Meshing finished: keep the guard from re-marking, then hand off.
        guard.defused = true;
        let _ = tx.send(mesh);
    }
}
struct QueueState {
    tasks: Box<[PendingJob]>,
    head: AtomicU32,
    /// When this batch was enqueued; a claimed job's queue wait is measured
    /// from here (batches are rebuilt per frame, so it is per-job accurate).
    created_at: std::time::Instant,
}
struct MeshQueue {
    state: Atomic<QueueState>,
    closed: AtomicBool,
}
impl Drop for MeshQueue {
    fn drop(&mut self) {
        let state = std::mem::replace(&mut self.state, Atomic::null());
        // SAFETY: dropping implies exclusive access (workers are joined before
        // the last Arc goes away), and `state` is never null.
        unsafe { drop(state.into_owned()) };
    }
}
impl MeshQueue {
    fn new() -> Self {
        Self {
            state: Atomic::init(QueueState {
                tasks: vec![].into_boxed_slice(),
                head: AtomicU32::new(0),
                created_at: std::time::Instant::now(),
            }),
            closed: AtomicBool::new(false),
        }
    }
    fn send(&self, jobs: Vec<PendingJob>) {
        let new_state = QueueState {
            tasks: jobs.into_boxed_slice(),
            head: AtomicU32::new(0),
            created_at: std::time::Instant::now(),
        };
        let guard = epoch::pin();
        let old = self
            .state
            .swap(epoch::Owned::new(new_state), Ordering::AcqRel, &guard);
        if !old.is_null() {
            // SAFETY: `old` was the queue's owned state and is now unreachable
            // to new loads; workers still holding it are pinned, which is
            // exactly what defer_destroy waits out.
            unsafe { guard.defer_destroy(old) };
        }
    }
    fn pending_jobs(&self) -> usize {
        let guard = epoch::pin();
        // SAFETY: loaded under the pinned guard; never null (initialized at
        // construction, swapped but never nulled).
        let state = unsafe { self.state.load(Ordering::Acquire, &guard).as_ref().unwrap() };
        let taken = state.head.load(Ordering::Acquire) as usize;
        state.tasks.len().saturating_sub(taken)
    }
    fn close(&self) {
        self.closed.store(true, Ordering::Relaxed);
    }
    #[allow(clippy::too_many_arguments)]
    fn run_worker(
        &self,
        shared_chunk_store: Arc<SharedChunkStore>,
        section_meta: Arc<SectionRing<SectionMeta>>,
        update_set: Arc<ChunkRing<AtomicU32>>,
        registry: Arc<BlockRegistry>,
        uv_map: Arc<AtlasUVMap>,
        grass_colormap: Arc<Colormap>,
        foliage_colormap: Arc<Colormap>,
        dry_foliage_colormap: Arc<Colormap>,
        biome_climate: Arc<HashMap<u32, BiomeClimate>>,
        tx: crossbeam_channel::Sender<SectionMeshData>,
    ) {
        loop {
            let (job, queue_ms) = loop {
                if self.closed.load(Ordering::Relaxed) {
                    return;
                }
                let guard = epoch::pin();
                let state = unsafe { self.state.load(Ordering::Acquire, &guard).as_ref().unwrap() };
                let idx = state.head.fetch_add(1, Ordering::AcqRel);
                if let Some(job) = state.tasks.get(idx as usize) {
                    break (
                        job.clone(),
                        state.created_at.elapsed().as_secs_f32() * 1000.0,
                    );
                }
                drop(guard);
                thread::park();
            };
            if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                job.run(
                    &shared_chunk_store,
                    &section_meta,
                    &update_set,
                    &registry,
                    &uv_map,
                    &grass_colormap,
                    &foliage_colormap,
                    &dry_foliage_colormap,
                    &biome_climate,
                    &tx,
                    queue_ms,
                )
            }))
            .is_err()
            {
                tracing::error!("chunk mesh job panicked; worker continuing");
            }
        }
    }
}
