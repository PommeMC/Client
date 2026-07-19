use std::sync::Arc;
use std::sync::atomic::Ordering;

use azalea_block::BlockState;
use azalea_core::heightmap_kind::HeightmapKind;
use azalea_core::position::{BlockPos, ChunkPos};
use azalea_world::chunk::Chunk;
use crossbeam_epoch::{self as epoch, Atomic, Owned};

use super::block_entity::StoredBlockEntity;
use crate::util::{ChunkRing, MAX_RD, SIZE_Y};

const OVERWORLD_HEIGHT: u32 = 384;
const OVERWORLD_MIN_Y: i32 = -64;

/// `pos` and its four axis-neighbor chunks. This is both the neighborhood a
/// chunk's mesh samples (see `MeshDispatcher::enqueue`) and, by symmetry, the
/// set that must re-mesh when `pos` changes. Add the diagonals here when the
/// corner-sample TODO(chunk-light) lands, so the mesh snapshot and the re-mesh
/// set stay in sync (vanilla's `enableChunkLight` dirties the full 3x3 via
/// `setSectionRangeDirty`).
pub(crate) fn mesh_neighborhood(pos: ChunkPos) -> [ChunkPos; 5] {
    [
        pos,
        ChunkPos::new(pos.x - 1, pos.z),
        ChunkPos::new(pos.x + 1, pos.z),
        ChunkPos::new(pos.x, pos.z - 1),
        ChunkPos::new(pos.x, pos.z + 1),
    ]
}

/// A column's published light, written by the light engine and snapshotted
/// (via `Arc`) by the mesher. Sections are light sections: one padding
/// section below the world, `height/16` block sections, one above.
#[derive(Clone)]
pub struct ChunkLightData {
    pub sky_sections: Vec<Option<Box<[u8; 2048]>>>,
    pub block_sections: Vec<Option<Box<[u8; 2048]>>>,
    pub min_y: i32,
    /// Whether the dimension has skylight; without it sky reads 0 (vanilla's
    /// dummy sky listener).
    pub has_sky: bool,
    /// One above the column's highest sky section holding data, as an index
    /// into `sky_sections`; `None` means no sky data is tracked and the whole
    /// column reads as open sky.
    pub sky_top_section: Option<i32>,
}

impl ChunkLightData {
    /// Vanilla `SkyLightSectionStorage.getLightValue` on the visible buffer:
    /// at/above the column's top section is implicit 15, below it missing
    /// layers defer upward to the nearest stored layer's bottom plane.
    pub fn get_sky_light(&self, x: i32, y: i32, z: i32) -> u8 {
        if !self.has_sky {
            return 0;
        }
        let Some(top) = self.sky_top_section else {
            return 15;
        };
        let mut index = (y - self.min_y).div_euclid(16) + 1;
        if index >= top {
            return 15;
        }
        let mut local_y = (y - self.min_y).rem_euclid(16);
        loop {
            if let Some(data) = usize::try_from(index)
                .ok()
                .and_then(|i| self.sky_sections.get(i))
                .and_then(Option::as_deref)
            {
                return Self::nibble(data, x, local_y, z);
            }
            index += 1;
            if index >= top {
                return 15;
            }
            // Walking up reads the found layer's bottom plane (vanilla
            // flattens the block position's Y).
            local_y = 0;
        }
    }

    pub fn get_block_light(&self, x: i32, y: i32, z: i32) -> u8 {
        let index = (y - self.min_y).div_euclid(16) + 1;
        match usize::try_from(index)
            .ok()
            .and_then(|i| self.block_sections.get(i))
            .and_then(Option::as_deref)
        {
            Some(data) => Self::nibble(data, x, (y - self.min_y).rem_euclid(16), z),
            None => 0,
        }
    }

    fn nibble(data: &[u8; 2048], x: i32, local_y: i32, z: i32) -> u8 {
        let lx = x.rem_euclid(16) as usize;
        let lz = z.rem_euclid(16) as usize;
        let idx = local_y as usize * 256 + lz * 16 + lx;
        let byte = data[idx / 2];
        if idx.is_multiple_of(2) {
            byte & 0x0F
        } else {
            (byte >> 4) & 0x0F
        }
    }
}

/// Shared, lock-free chunk store accessible by main thread and worker threads
/// via `crossbeam-epoch`.
///
/// Concurrency contract: any thread may read, but all writes
/// (`insert_chunk`, `set_block_state*`, `set_light_data`, `update_light_data`,
/// `unload_chunk`) must stay on the main thread. Mutation is clone-on-write
/// with an unconditional `swap`, so two concurrent writers to the same slot
/// would silently lose one update.
pub struct SharedChunkStore {
    chunks: ChunkRing<Atomic<Chunk>>,
    pub light_data: ChunkRing<Atomic<ChunkLightData>>,
    height: u32,
    min_y: i32,
}

/// Drains a ring's slots, dropping every published value. Only sound with
/// exclusive access to the ring.
fn drop_ring<T>(ring: &mut ChunkRing<Atomic<T>>) {
    for slot in ring.buf.iter_mut() {
        let atomic = std::mem::replace(slot, Atomic::null());
        // SAFETY: exclusive access (see `Drop` below); nothing can observe
        // the pointer, so it is reclaimed directly.
        unsafe {
            if !atomic
                .load(Ordering::Relaxed, epoch::unprotected())
                .is_null()
            {
                drop(atomic.into_owned());
            }
        }
    }
}

impl Drop for SharedChunkStore {
    /// `Atomic`'s own drop is a no-op, so without this every loaded chunk and
    /// light column leaks when the store is replaced (dimension change,
    /// reconnect). Runs at the last Arc owner: `ChunkMeshing`'s drop joins
    /// the workers before its store Arc dies, so access here is exclusive.
    fn drop(&mut self) {
        drop_ring(&mut self.chunks);
        drop_ring(&mut self.light_data);
    }
}

impl SharedChunkStore {
    pub fn new(view_distance: u32) -> Self {
        Self::new_with_dimension(view_distance, OVERWORLD_HEIGHT, OVERWORLD_MIN_Y)
    }

    pub fn new_with_dimension(view_distance: u32, height: u32, min_y: i32) -> Self {
        // The rings are fixed at MAX_SIZE (= 2 * MAX_RD + 1) slots per axis and
        // slots carry no position tag, so positions MAX_SIZE apart alias. The
        // radius is pinned to MAX_RD: anything wider would silently read the
        // wrong chunk.
        if view_distance > MAX_RD {
            tracing::warn!("view distance {view_distance} exceeds ring capacity {MAX_RD}");
        }
        Self {
            height,
            min_y,
            chunks: ChunkRing::from_fn(|_, _| Atomic::null()),
            light_data: ChunkRing::from_fn(|_, _| Atomic::null()),
        }
    }

    pub fn has_chunk(&self, pos: ChunkPos) -> bool {
        let guard = epoch::pin();
        !self
            .chunks
            .get(pos)
            .load(Ordering::Acquire, &guard)
            .is_null()
    }

    pub fn get_chunk_guard<'g>(&self, pos: ChunkPos, guard: &'g epoch::Guard) -> Option<&'g Chunk> {
        let shared = self.chunks.get(pos).load(Ordering::Acquire, guard);
        // SAFETY: loaded under `guard`, which the returned reference borrows,
        // so a concurrent swap's defer_destroy can't run while it lives.
        unsafe { shared.as_ref() }
    }

    /// Publishes a new value into `slot`, retiring the previous occupant.
    fn publish<T>(slot: &Atomic<T>, value: T, guard: &epoch::Guard)
    where
        T: Send + Sync + 'static,
    {
        let old_ptr = slot.swap(Owned::new(value), Ordering::Release, guard);
        if !old_ptr.is_null() {
            // SAFETY: the old pointer is unlinked from the slot; readers that
            // still hold it are pinned, which defer_destroy waits out.
            unsafe {
                guard.defer_destroy(old_ptr);
            }
        }
    }

    /// Publishes a chunk parsed on the net thread.
    pub fn insert_chunk(&self, pos: ChunkPos, chunk: Chunk) {
        let guard = epoch::pin();
        Self::publish(self.chunks.get(pos), chunk, &guard);
    }

    pub fn get_light_guard<'g>(
        &self,
        pos: ChunkPos,
        guard: &'g epoch::Guard,
    ) -> Option<&'g ChunkLightData> {
        let shared = self.light_data.get(pos).load(Ordering::Acquire, guard);
        // SAFETY: loaded under `guard`, which the returned reference borrows.
        unsafe { shared.as_ref() }
    }

    /// Publishes a column's light wholesale (the light engine's
    /// `on_chunk_loaded` path).
    pub fn set_light_data(&self, pos: ChunkPos, light: ChunkLightData) {
        let guard = epoch::pin();
        Self::publish(self.light_data.get(pos), light, &guard);
    }

    /// Clone-on-write update of a column's existing light (the light engine's
    /// publish path). Returns false when the column has no light yet.
    pub fn update_light_data(
        &self,
        pos: ChunkPos,
        mutate: impl FnOnce(&mut ChunkLightData),
    ) -> bool {
        let guard = epoch::pin();
        let Some(current) = self.get_light_guard(pos, &guard) else {
            return false;
        };
        let mut light = current.clone();
        mutate(&mut light);
        Self::publish(self.light_data.get(pos), light, &guard);
        true
    }

    pub fn get_sky_light(&self, x: i32, y: i32, z: i32) -> u8 {
        let pos = ChunkPos::new(x.div_euclid(16), z.div_euclid(16));
        let guard = epoch::pin();
        if let Some(light) = self.get_light_guard(pos, &guard) {
            light.get_sky_light(x.rem_euclid(16), y, z.rem_euclid(16))
        } else {
            15
        }
    }

    pub fn get_block_light(&self, x: i32, y: i32, z: i32) -> u8 {
        let pos = ChunkPos::new(x.div_euclid(16), z.div_euclid(16));
        let guard = epoch::pin();
        if let Some(light) = self.get_light_guard(pos, &guard) {
            light.get_block_light(x.rem_euclid(16), y, z.rem_euclid(16))
        } else {
            0
        }
    }

    pub fn unload_chunk(&self, pos: &ChunkPos) {
        let guard = epoch::pin();
        let old_chunk =
            self.chunks
                .get(*pos)
                .swap(epoch::Shared::null(), Ordering::Release, &guard);
        if !old_chunk.is_null() {
            // SAFETY: unlinked from the ring; pinned readers are waited out.
            unsafe {
                guard.defer_destroy(old_chunk);
            }
        }
        let old_light =
            self.light_data
                .get(*pos)
                .swap(epoch::Shared::null(), Ordering::Release, &guard);
        if !old_light.is_null() {
            // SAFETY: unlinked from the ring; pinned readers are waited out.
            unsafe {
                guard.defer_destroy(old_light);
            }
        }
    }

    pub fn set_block_state(&self, x: i32, y: i32, z: i32, state: BlockState) {
        self.set_block_state_tracked(x, y, z, state);
    }

    /// Sets a block and reports what vanilla `LevelChunk.setBlockState` feeds
    /// the light engine: the previous state, plus whether the section flipped
    /// between empty and non-empty. No-op writes (missing chunk, out-of-range
    /// y) return the new state and no flip.
    // TODO: multi-block updates clone the whole chunk once per block; batch
    // them per column while still reporting per-block old states.
    pub fn set_block_state_tracked(
        &self,
        x: i32,
        y: i32,
        z: i32,
        state: BlockState,
    ) -> (BlockState, Option<bool>) {
        let chunk_pos = ChunkPos::new(x.div_euclid(16), z.div_euclid(16));
        let guard = epoch::pin();
        let Some(chunk_ref) = self.get_chunk_guard(chunk_pos, &guard) else {
            return (state, None);
        };
        let section_index = (y - self.min_y).div_euclid(16);
        let Some(section) = usize::try_from(section_index)
            .ok()
            .filter(|&i| i < chunk_ref.sections.len())
        else {
            return (state, None);
        };
        let mut chunk = clone_chunk(chunk_ref);
        let was_empty = chunk.sections[section].block_count == 0;
        let block_pos = azalea_core::position::ChunkBlockPos {
            x: x.rem_euclid(16) as u8,
            y,
            z: z.rem_euclid(16) as u8,
        };
        let old = chunk.get_and_set_block_state(&block_pos, state, self.min_y);
        let is_empty = chunk.sections[section].block_count == 0;
        Self::publish(self.chunks.get(chunk_pos), chunk, &guard);
        (old, (was_empty != is_empty).then_some(is_empty))
    }

    /// Whether the block section at world section-y has only air (vanilla
    /// `LevelChunkSection.hasOnlyAir`; azalea tracks per-section block
    /// counts). Missing chunks and out-of-range sections read as empty.
    pub fn section_is_empty(&self, pos: (i32, i32), section_y: i32) -> bool {
        let guard = epoch::pin();
        let Some(chunk) = self.get_chunk_guard(ChunkPos::new(pos.0, pos.1), &guard) else {
            return true;
        };
        let index = section_y - self.min_section_y();
        match usize::try_from(index)
            .ok()
            .and_then(|i| chunk.sections.get(i))
        {
            Some(section) => section.block_count == 0,
            None => true,
        }
    }

    pub fn get_block_state(&self, x: i32, y: i32, z: i32) -> BlockState {
        let chunk_pos = ChunkPos::new(x.div_euclid(16), z.div_euclid(16));
        let guard = epoch::pin();
        let Some(chunk_ref) = self.get_chunk_guard(chunk_pos, &guard) else {
            return BlockState::AIR;
        };
        block_state_from_section(chunk_ref, x, y, z, self.min_y)
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    pub fn min_y(&self) -> i32 {
        self.min_y
    }

    pub fn section_count(&self) -> i32 {
        (self.height / 16) as i32
    }

    /// Y of the world's lowest section, in section coordinates.
    pub fn min_section_y(&self) -> i32 {
        self.min_y.div_euclid(16)
    }

    /// A section Y's bit index within a column's 32-bit section masks.
    pub fn section_y_index(&self, section_y: i32) -> u32 {
        (section_y - self.min_section_y()).clamp(0, SIZE_Y as i32 - 1) as u32
    }

    pub fn motion_blocking_height(&self, x: i32, z: i32) -> i32 {
        let chunk_pos = ChunkPos::new(x.div_euclid(16), z.div_euclid(16));
        let guard = epoch::pin();
        let Some(chunk_ref) = self.get_chunk_guard(chunk_pos, &guard) else {
            return self.min_y;
        };
        chunk_ref
            .heightmaps
            .get(&HeightmapKind::MotionBlocking)
            .map(|h| h.get_first_available(x.rem_euclid(16) as u8, z.rem_euclid(16) as u8))
            .unwrap_or(self.min_y)
    }

    pub fn biome_id(&self, x: i32, y: i32, z: i32) -> u32 {
        let chunk_pos = ChunkPos::new(x.div_euclid(16), z.div_euclid(16));
        let guard = epoch::pin();
        let Some(chunk_ref) = self.get_chunk_guard(chunk_pos, &guard) else {
            return 0;
        };
        let biome_pos = azalea_core::position::ChunkBiomePos {
            x: (x.rem_euclid(16) / 4) as u8,
            y,
            z: (z.rem_euclid(16) / 4) as u8,
        };
        let biome = chunk_ref
            .get_biome(biome_pos, self.min_y)
            .unwrap_or_default();
        u32::from(biome)
    }
}

/// Main-thread-only ChunkStore holding shared lock-free chunk store, the
/// loaded-column set, and the block entities map.
pub struct ChunkStore {
    pub shared: Arc<SharedChunkStore>,
    /// Columns currently published in the ring, so per-frame consumers
    /// (rescan, HUD) iterate the live set instead of scanning every slot.
    loaded: std::collections::HashSet<ChunkPos>,
    pub block_entities: std::collections::HashMap<BlockPos, StoredBlockEntity>,
}

impl ChunkStore {
    pub fn new(view_distance: u32) -> Self {
        Self {
            shared: Arc::new(SharedChunkStore::new(view_distance)),
            loaded: std::collections::HashSet::new(),
            block_entities: std::collections::HashMap::new(),
        }
    }

    pub fn new_with_dimension(view_distance: u32, height: u32, min_y: i32) -> Self {
        Self {
            shared: Arc::new(SharedChunkStore::new_with_dimension(
                view_distance,
                height,
                min_y,
            )),
            loaded: std::collections::HashSet::new(),
            block_entities: std::collections::HashMap::new(),
        }
    }

    pub fn unload_chunk(&mut self, pos: &ChunkPos) {
        self.shared.unload_chunk(pos);
        self.loaded.remove(pos);
        let cx = pos.x;
        let cz = pos.z;
        self.block_entities
            .retain(|bp, _| bp.x.div_euclid(16) != cx || bp.z.div_euclid(16) != cz);
    }

    pub fn loaded_set(&self) -> &std::collections::HashSet<ChunkPos> {
        &self.loaded
    }

    #[inline]
    pub fn insert_chunk(&mut self, pos: ChunkPos, chunk: Chunk) {
        self.shared.insert_chunk(pos, chunk);
        self.loaded.insert(pos);
    }

    #[inline]
    pub fn get_sky_light(&self, x: i32, y: i32, z: i32) -> u8 {
        self.shared.get_sky_light(x, y, z)
    }

    #[inline]
    pub fn get_block_light(&self, x: i32, y: i32, z: i32) -> u8 {
        self.shared.get_block_light(x, y, z)
    }

    #[inline]
    pub fn set_block_state(&self, x: i32, y: i32, z: i32, state: BlockState) {
        self.shared.set_block_state(x, y, z, state);
    }

    #[inline]
    pub fn set_block_state_tracked(
        &self,
        x: i32,
        y: i32,
        z: i32,
        state: BlockState,
    ) -> (BlockState, Option<bool>) {
        self.shared.set_block_state_tracked(x, y, z, state)
    }

    #[inline]
    pub fn get_block_state(&self, x: i32, y: i32, z: i32) -> BlockState {
        self.shared.get_block_state(x, y, z)
    }

    #[inline]
    pub fn height(&self) -> u32 {
        self.shared.height()
    }

    #[inline]
    pub fn min_y(&self) -> i32 {
        self.shared.min_y()
    }

    #[inline]
    pub fn section_count(&self) -> i32 {
        self.shared.section_count()
    }

    #[inline]
    pub fn min_section_y(&self) -> i32 {
        self.shared.min_section_y()
    }

    #[inline]
    pub fn section_is_empty(&self, pos: (i32, i32), section_y: i32) -> bool {
        self.shared.section_is_empty(pos, section_y)
    }

    #[inline]
    pub fn motion_blocking_height(&self, x: i32, z: i32) -> i32 {
        self.shared.motion_blocking_height(x, z)
    }

    #[inline]
    pub fn biome_id(&self, x: i32, y: i32, z: i32) -> u32 {
        self.shared.biome_id(x, y, z)
    }
}

/// azalea's `Chunk` doesn't derive `Clone`; both of its fields do, so the
/// clone-on-write path builds the copy field-wise.
fn clone_chunk(chunk: &Chunk) -> Chunk {
    Chunk {
        sections: chunk.sections.clone(),
        heightmaps: chunk.heightmaps.clone(),
    }
}

pub fn block_state_from_section(chunk: &Chunk, x: i32, y: i32, z: i32, min_y: i32) -> BlockState {
    // div_euclid so below-world y maps out of range (-> AIR) instead of
    // truncating into section 0; vanilla getSectionIndex floors.
    let section_idx = (y - min_y).div_euclid(16) as usize;
    if section_idx >= chunk.sections.len() {
        return BlockState::AIR;
    }
    let local_x = x.rem_euclid(16) as u8;
    let local_y = (y - min_y).rem_euclid(16) as u8;
    let local_z = z.rem_euclid(16) as u8;
    chunk.sections[section_idx].get_block_state(azalea_core::position::ChunkSectionBlockPos {
        x: local_x,
        y: local_y,
        z: local_z,
    })
}
