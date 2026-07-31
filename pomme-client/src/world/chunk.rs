use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use azalea_block::BlockState;
use azalea_core::heightmap_kind::HeightmapKind;
use azalea_core::position::{BlockPos, ChunkBlockPos, ChunkPos, ChunkSectionBlockPos};
use azalea_world::Section;
use azalea_world::chunk::{Chunk, section_index};
use azalea_world::heightmap::{Heightmap, is_heightmap_opaque};
use crossbeam_epoch::{self as epoch, Atomic, Owned};

use super::block_entity::StoredBlockEntity;
use crate::util::{ChunkRing, MAX_RD, SIZE_Y};

const OVERWORLD_HEIGHT: u32 = 384;
const OVERWORLD_MIN_Y: i32 = -64;

/// `pos` and its eight neighbor chunks. This is both the neighborhood a
/// chunk's mesh samples (corner AO reads diagonal cells at section corners)
/// and, by symmetry, the set that must re-mesh when `pos` changes (vanilla's
/// `enableChunkLight` dirties the full 3x3 via `setSectionRangeDirty`).
pub(crate) fn mesh_neighborhood(pos: ChunkPos) -> [ChunkPos; 9] {
    [
        pos,
        ChunkPos::new(pos.x - 1, pos.z),
        ChunkPos::new(pos.x + 1, pos.z),
        ChunkPos::new(pos.x, pos.z - 1),
        ChunkPos::new(pos.x, pos.z + 1),
        ChunkPos::new(pos.x - 1, pos.z - 1),
        ChunkPos::new(pos.x - 1, pos.z + 1),
        ChunkPos::new(pos.x + 1, pos.z - 1),
        ChunkPos::new(pos.x + 1, pos.z + 1),
    ]
}

/// A column's published light, written by the light engine and snapshotted
/// (via `Arc`) by the mesher. Sections are light sections: one padding
/// section below the world, `height/16` block sections, one above.
/// Layers are `Arc`'d so the per-publish clone-on-write copy of a column is
/// refcount bumps, not up to 52 * 2 KB of memcpy; a publish replaces whole
/// layers rather than mutating them.
#[derive(Clone)]
pub struct ChunkLightData {
    pub sky_sections: Vec<Option<Arc<[u8; 2048]>>>,
    pub block_sections: Vec<Option<Arc<[u8; 2048]>>>,
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

/// Pomme-owned column: azalea's parsed sections wrapped in `Arc` so the
/// clone-on-write edit path copies the 24-pointer spine plus exactly one
/// section (`Arc::make_mut`), instead of deep-cloning every palette in the
/// column per block write.
pub struct PommeChunk {
    pub sections: Box<[Arc<Section>]>,
    pub heightmaps: HashMap<HeightmapKind, Heightmap>,
}

impl PommeChunk {
    /// Wraps a freshly parsed azalea chunk (net thread); sections move into
    /// their `Arc`s without copying.
    pub fn from_azalea(chunk: Chunk) -> Self {
        Self {
            sections: chunk
                .sections
                .into_vec()
                .into_iter()
                .map(Arc::new)
                .collect(),
            heightmaps: chunk.heightmaps,
        }
    }

    /// Cheap spine + heightmap copy for clone-on-write publication; the
    /// sections themselves stay shared until one is edited.
    fn clone_spine(&self) -> Self {
        Self {
            sections: self.sections.clone(),
            heightmaps: self.heightmaps.clone(),
        }
    }

    /// Port of azalea `Chunk::get_and_set_block_state` onto the Arc'd
    /// sections: palette set + `block_count` upkeep on a `make_mut` copy of
    /// the one target section, then the heightmap update.
    pub fn get_and_set_block_state(
        &mut self,
        pos: &ChunkBlockPos,
        state: BlockState,
        min_y: i32,
    ) -> BlockState {
        let Some(section) = self.sections.get_mut(section_index(pos.y, min_y) as usize) else {
            return BlockState::AIR;
        };
        let previous_state =
            Arc::make_mut(section).get_and_set_block_state(ChunkSectionBlockPos::from(pos), state);

        for heightmap in self.heightmaps.values_mut() {
            update_heightmap(heightmap, pos, state, &self.sections);
        }

        previous_state
    }
}

/// Port of azalea `Heightmap::update` against the Arc'd section spine (the
/// upstream signature needs a contiguous `&[Section]`); all the pieces it
/// composes are public.
fn update_heightmap(
    heightmap: &mut Heightmap,
    pos: &ChunkBlockPos,
    block_state: BlockState,
    sections: &[Arc<Section>],
) -> bool {
    let first_available_y = heightmap.get_first_available(pos.x, pos.z);
    if pos.y <= first_available_y - 2 {
        return false;
    }
    if is_heightmap_opaque(heightmap.kind, block_state) {
        // increase y
        if pos.y >= first_available_y {
            heightmap.set_height(pos.x, pos.z, pos.y + 1);
            return true;
        }
    } else if first_available_y - 1 == pos.y {
        // decrease y: scan down for the next opaque block
        for y in (heightmap.min_y..pos.y).rev() {
            let state = sections
                .get(section_index(y, heightmap.min_y) as usize)
                .map(|s| {
                    s.get_block_state(ChunkSectionBlockPos::from(&ChunkBlockPos::new(
                        pos.x, y, pos.z,
                    )))
                })
                .unwrap_or_default();
            if is_heightmap_opaque(heightmap.kind, state) {
                heightmap.set_height(pos.x, pos.z, y + 1);
                return true;
            }
        }

        heightmap.set_height(pos.x, pos.z, heightmap.min_y);
        return true;
    }

    false
}

/// Shared, lock-free chunk store accessible by main thread and worker threads
/// via `crossbeam-epoch`.
///
/// Concurrency contract: any thread may read, but all writes
/// (`insert_chunk`, `set_block_state*`, `set_light_data`, `update_light_data`,
/// `unload_chunk`) must stay on the main thread. Mutation is clone-on-write
/// with an unconditional `swap`, so two concurrent writers to the same slot
/// would silently lose one update.
///
/// Ring slots alias every `MAX_SIZE` chunks, so each slot carries an occupant
/// tag (vanilla `ClientChunkCache.Storage` keeps the same invariant via
/// `isValidChunk`): reads return `None` on a tag mismatch, and an unload for
/// a position the slot no longer holds is a no-op instead of destroying the
/// current occupant (e.g. a server forget arriving after a long-teleport
/// alias already replaced the column).
pub struct SharedChunkStore {
    chunks: ChunkRing<Atomic<PommeChunk>>,
    pub light_data: ChunkRing<Atomic<ChunkLightData>>,
    /// Packed occupant position per slot (`pack_chunk_pos`), `TAG_EMPTY` when
    /// vacant. One ring serves both data rings: light only ever exists for a
    /// loaded chunk column.
    tags: ChunkRing<AtomicU64>,
    height: u32,
    min_y: i32,
}

/// Sentinel for a vacant slot: chunk (`i32::MIN`, `i32::MIN`) is outside any
/// reachable world (the border caps at ~±1.9M chunks).
const TAG_EMPTY: u64 = pack_chunk_pos(ChunkPos {
    x: i32::MIN,
    z: i32::MIN,
});

const fn pack_chunk_pos(pos: ChunkPos) -> u64 {
    ((pos.z as u32 as u64) << 32) | (pos.x as u32 as u64)
}

const fn unpack_chunk_pos(tag: u64) -> ChunkPos {
    ChunkPos {
        x: tag as u32 as i32,
        z: (tag >> 32) as u32 as i32,
    }
}

/// Swaps `slot` to null and retires the previous occupant, if any.
fn clear_slot<T: Send + Sync + 'static>(slot: &Atomic<T>, guard: &epoch::Guard) {
    let old = slot.swap(epoch::Shared::null(), Ordering::Release, guard);
    if !old.is_null() {
        // SAFETY: unlinked from the ring; pinned readers are waited out.
        unsafe { guard.defer_destroy(old) };
    }
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
        // The rings are fixed at MAX_SIZE (= 2 * MAX_RD + 1) slots per axis;
        // the occupant tags catch aliasing, but a view distance past MAX_RD
        // would evict live columns, so it is still clamped upstream.
        if view_distance > MAX_RD {
            tracing::warn!("view distance {view_distance} exceeds ring capacity {MAX_RD}");
        }
        if height / 16 > SIZE_Y as u32 {
            tracing::warn!(
                "dimension height {height} exceeds the {SIZE_Y}-section masks; sections above won't mesh or draw"
            );
        }
        Self {
            height,
            min_y,
            chunks: ChunkRing::from_fn(|_, _| Atomic::null()),
            light_data: ChunkRing::from_fn(|_, _| Atomic::null()),
            tags: ChunkRing::from_fn(|_, _| AtomicU64::new(TAG_EMPTY)),
        }
    }

    /// Whether `pos`'s ring slot currently holds `pos`. Cross-thread readers
    /// pair this with a re-check after the pointer load (see
    /// [`Self::get_chunk_guard`]); the main-thread writers use it alone.
    #[inline]
    fn tag_matches(&self, pos: ChunkPos) -> bool {
        self.tags.get(pos).load(Ordering::Acquire) == pack_chunk_pos(pos)
    }

    /// The position currently occupying `pos`'s ring slot, if any.
    pub fn slot_occupant(&self, pos: ChunkPos) -> Option<ChunkPos> {
        let tag = self.tags.get(pos).load(Ordering::Acquire);
        (tag != TAG_EMPTY).then(|| unpack_chunk_pos(tag))
    }

    pub fn has_chunk(&self, pos: ChunkPos) -> bool {
        let guard = epoch::pin();
        self.get_chunk_guard(pos, &guard).is_some()
    }

    pub fn get_chunk_guard<'g>(
        &self,
        pos: ChunkPos,
        guard: &'g epoch::Guard,
    ) -> Option<&'g PommeChunk> {
        // Tag check, pointer load, tag re-check: writers store the tag with
        // Release strictly before (evict/unload) or after (insert) the slot
        // swap, so a pointer read that observes a swap for a *different*
        // position also observes a tag that fails one of the two checks. A
        // single check would race the evict path (old tag read, new pointer
        // loaded).
        if !self.tag_matches(pos) {
            return None;
        }
        let shared = self.chunks.get(pos).load(Ordering::Acquire, guard);
        if !self.tag_matches(pos) {
            return None;
        }
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

    /// Publishes a chunk parsed on the net thread (main-thread write; see the
    /// struct-level contract).
    pub fn insert_chunk(&self, pos: ChunkPos, chunk: PommeChunk) {
        let guard = epoch::pin();
        let tag = self.tags.get(pos);
        // Taking the slot over from an alias: blank the tag first so readers
        // of the old position reject the window where the pointer already
        // belongs to `pos`, and drop the alias's light so readers of `pos`
        // never see it before this column's own light publishes.
        if tag.load(Ordering::Relaxed) != pack_chunk_pos(pos) {
            tag.store(TAG_EMPTY, Ordering::Release);
            clear_slot(self.light_data.get(pos), &guard);
        }
        Self::publish(self.chunks.get(pos), chunk, &guard);
        tag.store(pack_chunk_pos(pos), Ordering::Release);
    }

    pub fn get_light_guard<'g>(
        &self,
        pos: ChunkPos,
        guard: &'g epoch::Guard,
    ) -> Option<&'g ChunkLightData> {
        // Same check/load/re-check protocol as `get_chunk_guard`.
        if !self.tag_matches(pos) {
            return None;
        }
        let shared = self.light_data.get(pos).load(Ordering::Acquire, guard);
        if !self.tag_matches(pos) {
            return None;
        }
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
        // A forget for a position the slot no longer holds (evicted by an
        // aliasing insert) must not destroy the current occupant.
        if !self.tag_matches(*pos) {
            return;
        }
        // Tag goes first (readers of `pos` start missing before the pointers
        // die), then the data.
        self.tags.get(*pos).store(TAG_EMPTY, Ordering::Release);
        let guard = epoch::pin();
        clear_slot(self.chunks.get(*pos), &guard);
        clear_slot(self.light_data.get(*pos), &guard);
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
        let mut chunk = chunk_ref.clone_spine();
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
        // Same lookup azalea `Chunk::get_biome` performs (section by block y,
        // cell y = block y & 3), inlined against the Arc'd sections.
        if y < self.min_y {
            return 0;
        }
        let Some(section) = chunk_ref
            .sections
            .get(section_index(y, self.min_y) as usize)
        else {
            return 0;
        };
        let biome = section.get_biome(azalea_core::position::ChunkSectionBiomePos {
            x: (x.rem_euclid(16) / 4) as u8,
            y: (y & 0b11) as u8,
            z: (z.rem_euclid(16) / 4) as u8,
        });
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

    /// Returns whether the column was actually resident. False means the ring
    /// slot belongs to someone else (or nothing): the caller must skip its
    /// teardown, since slots carry no position tag and alias every MAX_SIZE
    /// chunks — a late forget for an evicted column must not destroy the
    /// current occupant.
    pub fn unload_chunk(&mut self, pos: &ChunkPos) -> bool {
        if !self.loaded.remove(pos) {
            return false;
        }
        self.shared.unload_chunk(pos);
        let cx = pos.x;
        let cz = pos.z;
        self.block_entities
            .retain(|bp, _| bp.x.div_euclid(16) != cx || bp.z.div_euclid(16) != cz);
        true
    }

    pub fn loaded_set(&self) -> &std::collections::HashSet<ChunkPos> {
        &self.loaded
    }

    #[inline]
    pub fn insert_chunk(&mut self, pos: ChunkPos, chunk: PommeChunk) {
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

pub fn block_state_from_section(
    chunk: &PommeChunk,
    x: i32,
    y: i32,
    z: i32,
    min_y: i32,
) -> BlockState {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::MAX_SIZE;

    #[test]
    fn chunk_pos_tag_roundtrips() {
        for pos in [
            ChunkPos::new(0, 0),
            ChunkPos::new(-1, -1),
            ChunkPos::new(1_874_999, -1_874_999),
        ] {
            let tag = pack_chunk_pos(pos);
            assert_ne!(tag, TAG_EMPTY);
            assert_eq!(unpack_chunk_pos(tag), pos);
        }
    }

    #[test]
    fn stale_unload_spares_the_alias_occupant() {
        let store = SharedChunkStore::new(8);
        let a = ChunkPos::new(0, 0);
        let b = ChunkPos::new(MAX_SIZE as i32, 0);
        store.insert_chunk(a, PommeChunk::from_azalea(Chunk::default()));
        assert!(store.has_chunk(a));
        assert_eq!(store.slot_occupant(b), Some(a));

        // B takes A's slot (long teleport); A stops resolving immediately.
        store.insert_chunk(b, PommeChunk::from_azalea(Chunk::default()));
        assert!(store.has_chunk(b));
        assert!(!store.has_chunk(a));

        // The server's late forget for A must not destroy B.
        store.unload_chunk(&a);
        assert!(store.has_chunk(b));

        store.unload_chunk(&b);
        assert!(!store.has_chunk(b));
        assert_eq!(store.slot_occupant(b), None);
    }
}
