use std::sync::Arc;

use azalea_block::BlockState;
use parking_lot::RwLock;
use pomme_protocol::codec::{DecodeResult, Reader};
use pomme_protocol::section::{BitStorage, Section, ceil_log2};
use thiserror::Error;

use super::block_entity::StoredBlockEntity;
use crate::position::{BlockPos, ChunkPos};
use crate::world::block::BlockStateTable;
use crate::world::light::DataLayer;

pub(crate) const OVERWORLD_HEIGHT: u32 = 384;
pub(crate) const OVERWORLD_MIN_Y: i32 = -64;

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

/// `pos` and the eight columns around it: what vanilla requires to be loaded
/// and lit before a section in the middle one may first compile
/// (`SectionUpdateTracker.hasAllNeighbors`). The smaller set a mesh samples is
/// `mesh_neighborhood`.
pub(crate) fn column_neighborhood(pos: ChunkPos) -> impl Iterator<Item = ChunkPos> {
    (-1..=1).flat_map(move |dx| (-1..=1).map(move |dz| ChunkPos::new(pos.x + dx, pos.z + dz)))
}

#[derive(Error, Debug)]
pub enum ChunkError {
    #[error("failed to parse chunk data: {0}")]
    Parse(String),
}

/// A column's published light, written by the light engine and snapshotted
/// (via `Arc`) by the mesher. Sections are light sections: one padding
/// section below the world, `height/16` block sections, one above. Layers
/// share their bytes with the engine's until it writes them, and uniform
/// layers hold none.
#[derive(Clone)]
pub struct ChunkLightData {
    pub(crate) sky_sections: Vec<Option<DataLayer>>,
    pub(crate) block_sections: Vec<Option<DataLayer>>,
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
        let mut index = self.layer_index(y);
        if index >= top {
            return 15;
        }
        let mut local_y = (y - self.min_y).rem_euclid(16);
        loop {
            if let Some(layer) = Self::layer(&self.sky_sections, index) {
                return Self::read(layer, x, local_y, z);
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
        Self::layer(&self.block_sections, self.layer_index(y)).map_or(0, |layer| {
            Self::read(layer, x, (y - self.min_y).rem_euclid(16), z)
        })
    }

    /// The layer holding world `y`: index 0 is the section below the world.
    fn layer_index(&self, y: i32) -> i32 {
        (y - self.min_y).div_euclid(16) + 1
    }

    /// A layer's level at world `x`/`z` and layer-local `y`.
    fn read(layer: &DataLayer, x: i32, local_y: i32, z: i32) -> u8 {
        layer.get(x.rem_euclid(16), local_y, z.rem_euclid(16))
    }

    fn layer(sections: &[Option<DataLayer>], index: i32) -> Option<&DataLayer> {
        usize::try_from(index)
            .ok()
            .and_then(|i| sections.get(i))
            .and_then(Option::as_ref)
    }

    /// Stores a sky light level, allocating its layer if needed.
    #[cfg(feature = "cpu-bench")]
    pub(crate) fn set_sky_light(&mut self, x: i32, y: i32, z: i32, level: u8) {
        let local_y = (y - self.min_y).rem_euclid(16);
        let layer_index = self.layer_index(y) as usize;
        self.sky_sections[layer_index]
            .get_or_insert_with(DataLayer::new)
            .set(x.rem_euclid(16), local_y, z.rem_euclid(16), level);
    }
}

/// Vanilla `Heightmap.Types`, by network id (`Heightmap.java:144-149`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum HeightmapKind {
    WorldSurfaceWg,
    WorldSurface,
    OceanFloorWg,
    OceanFloor,
    MotionBlocking,
    MotionBlockingNoLeaves,
}

impl HeightmapKind {
    pub fn from_id(id: u32) -> Option<Self> {
        Some(match id {
            0 => Self::WorldSurfaceWg,
            1 => Self::WorldSurface,
            2 => Self::OceanFloorWg,
            3 => Self::OceanFloor,
            4 => Self::MotionBlocking,
            5 => Self::MotionBlockingNoLeaves,
            _ => return None,
        })
    }
}

/// Vanilla `Heightmap`: per column, one above the highest block its type
/// counts as opaque.
#[derive(Clone, Debug)]
struct Heightmap {
    kind: HeightmapKind,
    data: BitStorage,
    min_y: i32,
}

impl Heightmap {
    /// Vanilla `Heightmap.setRawData`. Data of the wrong length is dropped,
    /// where vanilla re-primes the map from the blocks.
    fn new(kind: HeightmapKind, height: u32, min_y: i32, raw: &[u64]) -> Self {
        let mut data = BitStorage::new(ceil_log2(height as usize + 1), 256);
        if data.raw().len() == raw.len() {
            data.raw_mut().copy_from_slice(raw);
        } else {
            tracing::warn!(
                "Ignoring heightmap data, size does not match; expected: {}, got: {}",
                data.raw().len(),
                raw.len()
            );
        }
        Self { kind, data, min_y }
    }

    fn index(x: u32, z: u32) -> usize {
        (x + z * 16) as usize
    }

    fn first_available(&self, x: u32, z: u32) -> i32 {
        self.data.get(Self::index(x, z)) as i32 + self.min_y
    }

    fn set_height(&mut self, x: u32, z: u32, height: i32) {
        self.data
            .set(Self::index(x, z), (height - self.min_y) as u32);
    }

    // TODO: vanilla's `Heightmap.Types` predicates (motion blocking also
    // counts fluids, the no-leaves type skips leaves); every type here counts
    // any non-air block.
    fn is_opaque(state: BlockState) -> bool {
        !state.is_air()
    }

    /// Vanilla `Heightmap.update`.
    fn update(&mut self, x: u32, y: i32, z: u32, state: BlockState, sections: &[Section]) {
        let first_available = self.first_available(x, z);
        if y <= first_available - 2 {
            return;
        }
        if Self::is_opaque(state) {
            if y >= first_available {
                self.set_height(x, z, y + 1);
            }
        } else if first_available - 1 == y {
            for below in (self.min_y..y).rev() {
                if Self::is_opaque(state_in(sections, x, below, z, self.min_y)) {
                    self.set_height(x, z, below + 1);
                    return;
                }
            }
            self.set_height(x, z, self.min_y);
        }
    }
}

/// A section's state ids as block states; ids that aren't one read as air.
pub trait SectionBlocks {
    fn get_block_state(&self, x: u32, y: u32, z: u32) -> BlockState;

    /// The state every block holds, when the section stores only one.
    fn single_block_state(&self) -> Option<BlockState>;
}

impl SectionBlocks for Section {
    #[inline]
    fn get_block_state(&self, x: u32, y: u32, z: u32) -> BlockState {
        block_state(self.states.get(x, y, z))
    }

    fn single_block_state(&self) -> Option<BlockState> {
        self.states.single_value().map(block_state)
    }
}

#[inline]
fn block_state(id: u32) -> BlockState {
    BlockState::try_from(id).unwrap_or_default()
}

/// Vanilla `LevelChunkSection.setBlockState`: the write, then the block
/// count, which wraps like vanilla's `short`. Returns the previous state.
// TODO: vanilla also tracks the fluid and ticking counts here.
fn set_section_state(
    section: &mut Section,
    x: u32,
    y: u32,
    z: u32,
    new: BlockState,
    blocks: BlockStateTable,
) -> BlockState {
    let is_air = |state| blocks.block_state_data(state).is_air();
    let id = u32::from(new.id());
    let previous = block_state(
        section
            .states
            .get_and_set(x, y, z, id, blocks.state_count()),
    );
    if !is_air(previous) {
        section.block_count = section.block_count.wrapping_sub(1);
    }
    if !is_air(new) {
        section.block_count = section.block_count.wrapping_add(1);
    }
    previous
}

/// The section index holding world `y`, or `None` below the column.
/// `div_euclid` so below-world y maps out of range instead of truncating into
/// section 0; vanilla `getSectionIndex` floors.
fn section_index(y: i32, min_y: i32) -> Option<usize> {
    usize::try_from((y - min_y).div_euclid(16)).ok()
}

/// The state at local `x`/`z` and world `y` in a column's sections; air
/// outside them.
fn state_in(sections: &[Section], x: u32, y: i32, z: u32, min_y: i32) -> BlockState {
    section_index(y, min_y)
        .and_then(|i| sections.get(i))
        .map_or(BlockState::AIR, |section| {
            section.get_block_state(x, (y - min_y).rem_euclid(16) as u32, z)
        })
}

/// A loaded column: what the client keeps of vanilla `LevelChunk`.
#[derive(Debug)]
pub struct Chunk {
    pub sections: Box<[Section]>,
    // TODO: only `MotionBlocking` is read (weather); the other kinds the
    // server sends cost ~0.7 KB per column.
    heightmaps: Vec<Heightmap>,
}

impl Chunk {
    /// An all-air column of `section_count` sections.
    #[cfg(any(test, feature = "cpu-bench"))]
    pub fn empty(section_count: usize) -> Self {
        Self {
            sections: vec![Section::default(); section_count].into(),
            heightmaps: Vec::new(),
        }
    }

    /// Vanilla `LevelChunk.replaceWithPacketData`: the section buffer, then
    /// the packet's heightmaps.
    fn read(
        data: &[u8],
        height: u32,
        min_y: i32,
        heightmaps: &[(HeightmapKind, Box<[u64]>)],
        biomes: usize,
        block_states: usize,
    ) -> DecodeResult<Self> {
        let mut r = Reader::new(data);
        let sections = (0..height / 16)
            .map(|_| Section::read(&mut r, block_states, biomes))
            .collect::<DecodeResult<Vec<_>>>()?;
        Ok(Self {
            sections: sections.into(),
            heightmaps: heightmaps
                .iter()
                .map(|(kind, raw)| Heightmap::new(*kind, height, min_y, raw))
                .collect(),
        })
    }

    /// The section holding world `y`, or `None` outside the column's height.
    pub fn section(&self, y: i32, min_y: i32) -> Option<&Section> {
        self.sections.get(section_index(y, min_y)?)
    }

    pub fn block_state(&self, x: i32, y: i32, z: i32, min_y: i32) -> BlockState {
        state_in(
            &self.sections,
            x.rem_euclid(16) as u32,
            y,
            z.rem_euclid(16) as u32,
            min_y,
        )
    }

    /// Vanilla `LevelChunk.setBlockState`'s storage half: the section write,
    /// then the heightmaps. Returns the previous state.
    fn get_and_set_block_state(
        &mut self,
        x: u32,
        y: i32,
        z: u32,
        state: BlockState,
        min_y: i32,
        blocks: BlockStateTable,
    ) -> BlockState {
        let Some(section) = section_index(y, min_y).and_then(|i| self.sections.get_mut(i)) else {
            return BlockState::AIR;
        };
        let local_y = (y - min_y).rem_euclid(16) as u32;
        let previous = set_section_state(section, x, local_y, z, state, blocks);
        for heightmap in &mut self.heightmaps {
            heightmap.update(x, y, z, state, &self.sections);
        }
        previous
    }

    /// The biome registry id at a block, or `None` outside the column.
    pub fn biome(&self, x: i32, y: i32, z: i32, min_y: i32) -> Option<u32> {
        let section = self.section(y, min_y)?;
        Some(section.biome(
            (x.rem_euclid(16) / 4) as u32,
            ((y - min_y).rem_euclid(16) / 4) as u32,
            (z.rem_euclid(16) / 4) as u32,
        ))
    }

    fn first_available(&self, kind: HeightmapKind, x: i32, z: i32) -> Option<i32> {
        self.heightmaps
            .iter()
            .find(|h| h.kind == kind)
            .map(|h| h.first_available(x.rem_euclid(16) as u32, z.rem_euclid(16) as u32))
    }
}

/// A grid slot: the column there and the position it was stored for.
type GridSlot = Option<(ChunkPos, Arc<RwLock<Chunk>>)>;

/// Vanilla `ClientChunkCache.Storage`: a square grid of columns around the
/// view center, indexed modulo its side so the center can move without
/// shuffling slots.
struct ChunkGrid {
    view_center: ChunkPos,
    chunk_radius: u32,
    view_range: u32,
    chunks: Box<[GridSlot]>,
}

impl ChunkGrid {
    fn new(chunk_radius: u32) -> Self {
        let view_range = chunk_radius * 2 + 1;
        Self {
            view_center: ChunkPos::new(0, 0),
            chunk_radius,
            view_range,
            chunks: vec![None; (view_range * view_range) as usize].into(),
        }
    }

    fn index(&self, pos: ChunkPos) -> usize {
        ring_index(pos, self.view_range as i32)
    }

    fn in_range(&self, pos: ChunkPos) -> bool {
        (pos.x - self.view_center.x).unsigned_abs() <= self.chunk_radius
            && (pos.z - self.view_center.z).unsigned_abs() <= self.chunk_radius
    }

    // TODO: vanilla's `ClientChunkCache.getChunk` also requires the column
    // to be in range of the current view center.
    fn get(&self, pos: ChunkPos) -> Option<&Arc<RwLock<Chunk>>> {
        match &self.chunks[self.index(pos)] {
            Some((at, chunk)) if *at == pos => Some(chunk),
            _ => None,
        }
    }

    /// Stores `chunk` at `pos`. A column already there is refilled in place
    /// so snapshots holding it see the new data, as vanilla replaces a valid
    /// chunk's contents. Out-of-range columns are ignored.
    fn replace(&mut self, pos: ChunkPos, chunk: Chunk) {
        if !self.in_range(pos) {
            tracing::warn!("Ignoring chunk since it's not in the view range: {pos:?}");
            return;
        }
        if let Some(existing) = self.get(pos) {
            *existing.write() = chunk;
            return;
        }
        let index = self.index(pos);
        self.chunks[index] = Some((pos, Arc::new(RwLock::new(chunk))));
    }

    /// Empties `pos`'s slot when it's in range.
    fn remove(&mut self, pos: ChunkPos) {
        if self.in_range(pos) {
            let index = self.index(pos);
            self.chunks[index] = None;
        }
    }
}

/// Per-column values in a fixed square indexed like [`ChunkGrid`] at
/// [`MAX_VIEW_DISTANCE`], so two columns the store holds at once never share
/// a slot. Callers compare a stored position to tell a slot's column apart
/// from an older one it has replaced.
pub struct ChunkRing<T> {
    slots: Box<[T]>,
}

const RING_SIDE: i32 = MAX_VIEW_DISTANCE as i32 * 2 + 1;

/// A column's slot in a `side`×`side` square that wraps around the world.
fn ring_index(pos: ChunkPos, side: i32) -> usize {
    (pos.z.rem_euclid(side) * side + pos.x.rem_euclid(side)) as usize
}

impl<T> ChunkRing<T> {
    pub fn new(mut init: impl FnMut() -> T) -> Self {
        Self {
            slots: (0..RING_SIDE * RING_SIDE).map(|_| init()).collect(),
        }
    }

    pub fn get(&self, pos: ChunkPos) -> &T {
        &self.slots[ring_index(pos, RING_SIDE)]
    }

    pub fn get_mut(&mut self, pos: ChunkPos) -> &mut T {
        &mut self.slots[ring_index(pos, RING_SIDE)]
    }
}

pub struct ChunkStore {
    grid: ChunkGrid,
    height: u32,
    min_y: i32,
    pub light_data: std::collections::HashMap<(i32, i32), Arc<ChunkLightData>>,
    pub block_entities: std::collections::HashMap<BlockPos, StoredBlockEntity>,
}

/// The 128-chunk max extended-view-distance servers allow; server
/// announcements clamp to it and the chunk grid is sized for it.
pub const MAX_VIEW_DISTANCE: u32 = 128;

impl ChunkStore {
    pub fn new(view_distance: u32) -> Self {
        Self::new_with_dimension(view_distance, OVERWORLD_HEIGHT, OVERWORLD_MIN_Y)
    }

    pub fn new_with_dimension(view_distance: u32, height: u32, min_y: i32) -> Self {
        Self {
            // The grid silently drops out-of-range chunks and is never resized,
            // so floor it at the max view distance (~1 MB of slots).
            // TODO: size to the view distance and rebuild when it changes, like
            // vanilla `ClientChunkCache.updateViewRadius` (~1 MB at the max).
            grid: ChunkGrid::new(view_distance.max(MAX_VIEW_DISTANCE)),
            height,
            min_y,
            light_data: std::collections::HashMap::new(),
            block_entities: std::collections::HashMap::new(),
        }
    }

    pub fn loaded_positions(&self) -> impl Iterator<Item = ChunkPos> + '_ {
        self.light_data.keys().map(|&(x, z)| ChunkPos::new(x, z))
    }

    /// Loads a chunk packet's column; `biomes` is the biome registry's size.
    pub fn load_chunk(
        &mut self,
        pos: ChunkPos,
        data: &[u8],
        heightmaps: &[(HeightmapKind, Box<[u64]>)],
        biomes: usize,
        blocks: BlockStateTable,
    ) -> Result<(), ChunkError> {
        let block_states = blocks.state_count();
        let chunk = Chunk::read(
            data,
            self.height,
            self.min_y,
            heightmaps,
            biomes,
            block_states,
        )
        .map_err(|e| ChunkError::Parse(e.to_string()))?;
        self.grid.replace(pos, chunk);
        Ok(())
    }

    /// Stores an already-built column, as a chunk packet would.
    #[cfg(any(test, feature = "cpu-bench"))]
    pub(crate) fn insert_chunk(&mut self, pos: ChunkPos, chunk: Chunk) {
        self.grid.replace(pos, chunk);
    }

    pub fn get_sky_light(&self, x: i32, y: i32, z: i32) -> u8 {
        let cx = x.div_euclid(16);
        let cz = z.div_euclid(16);
        if let Some(light) = self.light_data.get(&(cx, cz)) {
            light.get_sky_light(x.rem_euclid(16), y, z.rem_euclid(16))
        } else {
            15
        }
    }

    pub fn get_block_light(&self, x: i32, y: i32, z: i32) -> u8 {
        let cx = x.div_euclid(16);
        let cz = z.div_euclid(16);
        if let Some(light) = self.light_data.get(&(cx, cz)) {
            light.get_block_light(x.rem_euclid(16), y, z.rem_euclid(16))
        } else {
            0
        }
    }

    /// Vanilla `getMaxLocalRawBrightness`: block light, or sky light dimmed
    /// by `sky_darken`, whichever is brighter.
    pub fn max_local_raw_brightness(&self, x: i32, y: i32, z: i32, sky_darken: u8) -> u8 {
        self.get_block_light(x, y, z)
            .max(self.get_sky_light(x, y, z).saturating_sub(sky_darken))
    }

    pub fn unload_chunk(&mut self, pos: &ChunkPos) {
        self.light_data.remove(&(pos.x, pos.z));
        self.grid.remove(*pos);
        let cx = pos.x;
        let cz = pos.z;
        self.block_entities
            .retain(|bp, _| bp.x.div_euclid(16) != cx || bp.z.div_euclid(16) != cz);
    }

    pub fn set_center(&mut self, pos: ChunkPos) {
        self.grid.view_center = pos;
    }

    pub fn get_chunk(&self, pos: &ChunkPos) -> Option<Arc<RwLock<Chunk>>> {
        self.grid.get(*pos).cloned()
    }

    pub fn set_block_state(
        &self,
        x: i32,
        y: i32,
        z: i32,
        state: BlockState,
        blocks: BlockStateTable,
    ) {
        self.set_block_state_tracked(x, y, z, state, blocks);
    }

    /// Sets a block and reports what vanilla `LevelChunk.setBlockState` feeds
    /// the light engine: the previous state, plus whether the section flipped
    /// between empty and non-empty. No-op writes (missing chunk, out-of-range
    /// y) return the new state and no flip.
    pub fn set_block_state_tracked(
        &self,
        x: i32,
        y: i32,
        z: i32,
        state: BlockState,
        blocks: BlockStateTable,
    ) -> (BlockState, Option<bool>) {
        let chunk_pos = ChunkPos::new(x.div_euclid(16), z.div_euclid(16));
        let Some(chunk_lock) = self.get_chunk(&chunk_pos) else {
            return (state, None);
        };
        let mut chunk = chunk_lock.write();
        let Some(section) = section_index(y, self.min_y).filter(|&i| i < chunk.sections.len())
        else {
            return (state, None);
        };
        let was_empty = chunk.sections[section].block_count == 0;
        let old = chunk.get_and_set_block_state(
            x.rem_euclid(16) as u32,
            y,
            z.rem_euclid(16) as u32,
            state,
            self.min_y,
            blocks,
        );
        let is_empty = chunk.sections[section].block_count == 0;
        (old, (was_empty != is_empty).then_some(is_empty))
    }

    pub fn get_block_state(&self, x: i32, y: i32, z: i32) -> BlockState {
        let chunk_pos = ChunkPos::new(x.div_euclid(16), z.div_euclid(16));
        let Some(chunk_lock) = self.get_chunk(&chunk_pos) else {
            return BlockState::AIR;
        };
        let chunk = chunk_lock.read();
        chunk.block_state(x, y, z, self.min_y)
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    pub fn min_y(&self) -> i32 {
        self.min_y
    }

    /// Number of 16³ block sections in a column (zero-based section index range
    /// `0..section_count`).
    pub fn section_count(&self) -> i32 {
        (self.height() / 16) as i32
    }

    /// Whether the block section at world section-y has only air (vanilla
    /// `LevelChunkSection.hasOnlyAir`, from its block count). Missing chunks
    /// and out-of-range sections read as empty.
    pub fn section_is_empty(&self, pos: (i32, i32), section_y: i32) -> bool {
        let Some(chunk) = self.get_chunk(&ChunkPos::new(pos.0, pos.1)) else {
            return true;
        };
        let index = section_y - (self.min_y() >> 4);
        let chunk = chunk.read();
        match usize::try_from(index)
            .ok()
            .and_then(|i| chunk.sections.get(i))
        {
            Some(section) => section.block_count == 0,
            None => true,
        }
    }

    /// Top non-motion-blocking Y for the column (vanilla MOTION_BLOCKING
    /// surface, i.e. one above the highest solid block). Used to position
    /// weather columns. Returns `min_y` when the chunk or its heightmap is
    /// missing.
    pub fn motion_blocking_height(&self, x: i32, z: i32) -> i32 {
        let chunk_pos = ChunkPos::new(x.div_euclid(16), z.div_euclid(16));
        let Some(chunk_lock) = self.get_chunk(&chunk_pos) else {
            return self.min_y();
        };
        let chunk = chunk_lock.read();
        chunk
            .first_available(HeightmapKind::MotionBlocking, x, z)
            .unwrap_or(self.min_y())
    }

    /// The biome at a quart (4-block) cell, vanilla `getNoiseBiomeAtQuart`:
    /// its height clamped into the column, `None` while its chunk isn't
    /// loaded.
    pub fn noise_biome(&self, qx: i32, qy: i32, qz: i32) -> Option<u32> {
        let (x, z) = (qx * 4, qz * 4);
        let y = (qy * 4).clamp(self.min_y, self.min_y + self.height() as i32 - 1);
        let chunk = self.get_chunk(&ChunkPos::new(x.div_euclid(16), z.div_euclid(16)))?;
        chunk.read().biome(x, y, z, self.min_y)
    }

    /// Registry id of the biome at a block position (matches the mesher's biome
    /// lookup). Returns 0 when the chunk is missing.
    pub fn biome_id(&self, x: i32, y: i32, z: i32) -> u32 {
        let chunk_pos = ChunkPos::new(x.div_euclid(16), z.div_euclid(16));
        let Some(chunk_lock) = self.get_chunk(&chunk_pos) else {
            return 0;
        };
        let chunk = chunk_lock.read();
        chunk.biome(x, y, z, self.min_y).unwrap_or_default()
    }
}

#[cfg(test)]
impl ChunkStore {
    /// A 26.2 store with an empty chunk (0, 0) loaded.
    pub(crate) fn with_origin_chunk() -> Self {
        let mut chunks = Self::new(2);
        let sections = chunks.section_count() as usize;
        chunks.insert_chunk(ChunkPos::new(0, 0), Chunk::empty(sections));
        chunks
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn ring_slots_repeat_only_past_the_max_view_distance() {
        let mut ring = ChunkRing::new(|| 0);
        *ring.get_mut(ChunkPos::new(-1, -1)) = 1;
        *ring.get_mut(ChunkPos::new(MAX_VIEW_DISTANCE as i32, 0)) = 2;
        assert_eq!(*ring.get(ChunkPos::new(-1, -1)), 1);
        assert_eq!(*ring.get(ChunkPos::new(-(MAX_VIEW_DISTANCE as i32), 0)), 0);
        assert_eq!(*ring.get(ChunkPos::new(RING_SIDE - 1, RING_SIDE - 1)), 1);
    }

    use super::*;

    #[test]
    fn sky_light_tells_absent_layers_from_dark_ones() {
        let light = ChunkLightData {
            sky_sections: vec![
                None,
                Some(DataLayer::new()),
                None,
                Some(DataLayer::filled(7)),
            ],
            block_sections: vec![None; 4],
            min_y: 0,
            has_sky: true,
            sky_top_section: Some(4),
        };
        // A present all-dark layer reads 0.
        assert_eq!(light.get_sky_light(3, 5, 3), 0);
        // An absent layer defers up to the next stored layer's bottom plane.
        assert_eq!(light.get_sky_light(3, 20, 3), 7);
        // At or above the column's top section is open sky.
        assert_eq!(light.get_sky_light(3, 48, 3), 15);
        assert_eq!(light.get_block_light(3, 5, 3), 0);
    }

    #[test]
    fn column_neighborhood_is_the_full_three_by_three() {
        let columns: Vec<_> = column_neighborhood(ChunkPos::new(4, -2)).collect();
        assert_eq!(columns.len(), 9);
        for dx in -1..=1 {
            for dz in -1..=1 {
                assert!(
                    columns.contains(&ChunkPos::new(4 + dx, -2 + dz)),
                    "{dx},{dz}"
                );
            }
        }
    }
}
