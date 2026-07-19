use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use azalea_block::BlockState;
use azalea_core::heightmap_kind::HeightmapKind;
use azalea_core::position::{BlockPos, ChunkPos};
use azalea_world::chunk::Chunk;
use crossbeam_epoch::{self as epoch, Atomic, Owned};
use thiserror::Error;

use super::block_entity::StoredBlockEntity;
use crate::util::{ChunkRing, MAX_RD, SIZE_Y};

const OVERWORLD_HEIGHT: u32 = 384;
const OVERWORLD_MIN_Y: i32 = -64;

/// `pos` and its four axis-neighbor chunks. This is both the neighborhood a
/// chunk's mesh samples (see `MeshDispatcher::enqueue`) and, by symmetry, the
/// set that must re-mesh when `pos` changes. Add the diagonals here when the
/// corner-sample TODO(chunk-light) lands, so the mesh snapshot and the re-mesh
/// set stay in sync.
pub(crate) fn mesh_neighborhood(pos: ChunkPos) -> [ChunkPos; 5] {
    [
        pos,
        ChunkPos::new(pos.x - 1, pos.z),
        ChunkPos::new(pos.x + 1, pos.z),
        ChunkPos::new(pos.x, pos.z - 1),
        ChunkPos::new(pos.x, pos.z + 1),
    ]
}

#[derive(Error, Debug)]
pub enum ChunkError {
    #[error("failed to parse chunk data: {0}")]
    Parse(String),
}

#[derive(Clone)]
pub struct ChunkLightData {
    pub sky_sections: Vec<Option<Box<[u8; 2048]>>>,
    pub block_sections: Vec<Option<Box<[u8; 2048]>>>,
    pub min_y: i32,
}

impl ChunkLightData {
    pub fn get_sky_light(&self, x: i32, y: i32, z: i32) -> u8 {
        self.get_nibble(&self.sky_sections, x, y, z)
    }

    pub fn get_block_light(&self, x: i32, y: i32, z: i32) -> u8 {
        self.get_nibble(&self.block_sections, x, y, z)
    }

    fn get_nibble(&self, sections: &[Option<Box<[u8; 2048]>>], x: i32, y: i32, z: i32) -> u8 {
        let section_idx = ((y - self.min_y + 16) / 16) as usize;
        if section_idx >= sections.len() {
            return 15;
        }
        let Some(data) = &sections[section_idx] else {
            return 15;
        };
        let lx = x.rem_euclid(16) as usize;
        let ly = y.rem_euclid(16) as usize;
        let lz = z.rem_euclid(16) as usize;
        let idx = ly * 256 + lz * 16 + lx;
        let byte = data[idx / 2];
        if idx.is_multiple_of(2) {
            byte & 0x0F
        } else {
            (byte >> 4) & 0x0F
        }
    }
}

pub const fn pack_chunk_pos(pos: ChunkPos) -> u64 {
    let x = (pos.x as u64) & 0x0000_0000_FFFF_FFFF;
    let z = (pos.z as u64) & 0x0000_0000_FFFF_FFFF;
    (x << 32) | z
}

pub const fn unpack_chunk_pos(val: u64) -> ChunkPos {
    let x = (val >> 32) as i32;
    let z = val as i32;
    ChunkPos { x, z }
}

/// Shared, lock-free chunk store accessible by main thread and worker threads
/// via `crossbeam-epoch`.
///
/// Concurrency contract: any thread may read, but all writes
/// (`load_chunk`, `set_block_state*`, `store_light`, `unload_chunk`) must stay
/// on the main thread. Mutation is clone-on-write with an unconditional
/// `swap`, so two concurrent writers to the same slot would silently lose one
/// update.
pub struct SharedChunkStore {
    view_center: AtomicU64,
    pub(crate) chunk_radius: u32,
    chunks: ChunkRing<Atomic<Chunk>>,
    pub light_data: ChunkRing<Atomic<ChunkLightData>>,
    height: u32,
    min_y: i32,
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
            view_center: AtomicU64::new(pack_chunk_pos(ChunkPos::new(0, 0))),
            chunk_radius: MAX_RD,
            height,
            min_y,
            chunks: ChunkRing::from_fn(|_, _| Atomic::null()),
            light_data: ChunkRing::from_fn(|_, _| Atomic::null()),
        }
    }

    pub fn update_view_center(&self, view_center: ChunkPos) {
        self.view_center
            .store(pack_chunk_pos(view_center), Ordering::Release);
    }

    pub fn view_center(&self) -> ChunkPos {
        unpack_chunk_pos(self.view_center.load(Ordering::Acquire))
    }

    pub fn loaded_positions(&self) -> Vec<ChunkPos> {
        let guard = epoch::pin();
        let center = self.view_center();
        let r = self.chunk_radius as i32;
        let mut list = Vec::new();
        for x in (center.x - r)..=(center.x + r) {
            for z in (center.z - r)..=(center.z + r) {
                let pos = ChunkPos::new(x, z);
                if !self
                    .chunks
                    .get(pos)
                    .load(Ordering::Acquire, &guard)
                    .is_null()
                {
                    list.push(pos);
                }
            }
        }
        list
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

    pub fn load_chunk(
        &self,
        pos: ChunkPos,
        data: &[u8],
        heightmaps: &[(HeightmapKind, Box<[u64]>)],
    ) -> Result<(), ChunkError> {
        let mut cursor = std::io::Cursor::new(data);
        let chunk =
            Chunk::read_with_dimension_height(&mut cursor, self.height, self.min_y, heightmaps)
                .map_err(|e| ChunkError::Parse(e.to_string()))?;
        let guard = epoch::pin();
        Self::publish(self.chunks.get(pos), chunk, &guard);
        Ok(())
    }

    pub fn store_light(
        &self,
        pos: ChunkPos,
        sky_updates: &[Box<[u8]>],
        block_updates: &[Box<[u8]>],
        sky_y_mask: &azalea_core::bitset::BitSet,
        block_y_mask: &azalea_core::bitset::BitSet,
    ) {
        let num_sections = (self.height / 16 + 2) as usize;
        let mut sky_sections = vec![None; num_sections];
        let mut block_sections = vec![None; num_sections];

        let mut sky_idx = 0usize;
        for (i, section) in sky_sections.iter_mut().enumerate().take(num_sections) {
            if i < sky_y_mask.len() && sky_y_mask.index(i) {
                if sky_idx < sky_updates.len() && sky_updates[sky_idx].len() == 2048 {
                    let mut arr = Box::new([0u8; 2048]);
                    arr.copy_from_slice(&sky_updates[sky_idx]);
                    *section = Some(arr);
                }
                sky_idx += 1;
            }
        }

        let mut block_idx = 0usize;
        for (i, section) in block_sections.iter_mut().enumerate().take(num_sections) {
            if i < block_y_mask.len() && block_y_mask.index(i) {
                if block_idx < block_updates.len() && block_updates[block_idx].len() == 2048 {
                    let mut arr = Box::new([0u8; 2048]);
                    arr.copy_from_slice(&block_updates[block_idx]);
                    *section = Some(arr);
                }
                block_idx += 1;
            }
        }

        let light = ChunkLightData {
            sky_sections,
            block_sections,
            min_y: self.min_y,
        };

        let guard = epoch::pin();
        Self::publish(self.light_data.get(pos), light, &guard);
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
        self.set_block_states(std::iter::once((BlockPos::new(x, y, z), state)));
    }

    /// Applies a run of block updates, cloning each affected chunk once per
    /// contiguous run instead of once per block (multi-block section updates
    /// arrive hundreds of blocks at a time).
    pub fn set_block_states(&self, updates: impl IntoIterator<Item = (BlockPos, BlockState)>) {
        let guard = epoch::pin();
        let mut pending: Option<(ChunkPos, Chunk)> = None;
        for (pos, state) in updates {
            let chunk_pos = ChunkPos::new(pos.x.div_euclid(16), pos.z.div_euclid(16));
            if pending.as_ref().is_none_or(|(p, _)| *p != chunk_pos) {
                if let Some((p, chunk)) = pending.take() {
                    Self::publish(self.chunks.get(p), chunk, &guard);
                }
                let Some(chunk_ref) = self.get_chunk_guard(chunk_pos, &guard) else {
                    continue;
                };
                pending = Some((chunk_pos, clone_chunk(chunk_ref)));
            }
            let (_, chunk) = pending.as_mut().unwrap();
            let block_pos = azalea_core::position::ChunkBlockPos {
                x: pos.x.rem_euclid(16) as u8,
                y: pos.y,
                z: pos.z.rem_euclid(16) as u8,
            };
            chunk.set_block_state(&block_pos, state, self.min_y);
        }
        if let Some((p, chunk)) = pending.take() {
            Self::publish(self.chunks.get(p), chunk, &guard);
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

/// Main-thread-only ChunkStore holding shared lock-free chunk store and
/// main-thread block entities map.
pub struct ChunkStore {
    pub shared: Arc<SharedChunkStore>,
    pub block_entities: std::collections::HashMap<BlockPos, StoredBlockEntity>,
}

impl ChunkStore {
    pub fn new(view_distance: u32) -> Self {
        Self {
            shared: Arc::new(SharedChunkStore::new(view_distance)),
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
            block_entities: std::collections::HashMap::new(),
        }
    }

    pub fn unload_chunk(&mut self, pos: &ChunkPos) {
        self.shared.unload_chunk(pos);
        let cx = pos.x;
        let cz = pos.z;
        self.block_entities
            .retain(|bp, _| bp.x.div_euclid(16) != cx || bp.z.div_euclid(16) != cz);
    }

    pub fn set_center(&mut self, pos: ChunkPos) {
        self.shared.update_view_center(pos);
    }

    #[inline]
    pub fn loaded_positions(&self) -> Vec<ChunkPos> {
        self.shared.loaded_positions()
    }

    #[inline]
    pub fn load_chunk(
        &self,
        pos: ChunkPos,
        data: &[u8],
        heightmaps: &[(HeightmapKind, Box<[u64]>)],
    ) -> Result<(), ChunkError> {
        self.shared.load_chunk(pos, data, heightmaps)
    }

    #[inline]
    pub fn store_light(
        &self,
        pos: ChunkPos,
        sky_updates: &[Box<[u8]>],
        block_updates: &[Box<[u8]>],
        sky_y_mask: &azalea_core::bitset::BitSet,
        block_y_mask: &azalea_core::bitset::BitSet,
    ) {
        self.shared
            .store_light(pos, sky_updates, block_updates, sky_y_mask, block_y_mask);
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
    pub fn set_block_states(&self, updates: impl IntoIterator<Item = (BlockPos, BlockState)>) {
        self.shared.set_block_states(updates);
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
    let section_idx = ((y - min_y) / 16) as usize;
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
