use std::io::Cursor;
use std::sync::Arc;

use azalea_block::BlockState;
use azalea_core::heightmap_kind::HeightmapKind;
use azalea_core::position::{BlockPos, ChunkPos};
use azalea_world::chunk::Chunk;
use azalea_world::chunk::partial::PartialChunkStorage;
use azalea_world::chunk::storage::ChunkStorage;
use parking_lot::RwLock;
use thiserror::Error;

use super::block_entity::StoredBlockEntity;

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

pub struct ChunkStore {
    pub chunk_storage: ChunkStorage,
    pub partial_storage: PartialChunkStorage,
    pub light_data: std::collections::HashMap<(i32, i32), Arc<ChunkLightData>>,
    pub block_entities: std::collections::HashMap<BlockPos, StoredBlockEntity>,
}

impl ChunkStore {
    pub fn new(view_distance: u32) -> Self {
        Self::new_with_dimension(view_distance, OVERWORLD_HEIGHT, OVERWORLD_MIN_Y)
    }

    pub fn new_with_dimension(view_distance: u32, height: u32, min_y: i32) -> Self {
        Self {
            chunk_storage: ChunkStorage::new(height, min_y),
            partial_storage: PartialChunkStorage::new(view_distance.max(64)),
            light_data: std::collections::HashMap::new(),
            block_entities: std::collections::HashMap::new(),
        }
    }

    pub fn loaded_positions(&self) -> impl Iterator<Item = ChunkPos> + '_ {
        self.light_data.keys().map(|&(x, z)| ChunkPos::new(x, z))
    }

    pub fn load_chunk(
        &mut self,
        pos: ChunkPos,
        data: &[u8],
        heightmaps: &[(HeightmapKind, Box<[u64]>)],
    ) -> Result<(), ChunkError> {
        let mut cursor = Cursor::new(data);
        self.partial_storage
            .replace_with_packet_data(&pos, &mut cursor, heightmaps, &mut self.chunk_storage)
            .map_err(|e| ChunkError::Parse(e.to_string()))
    }

    pub fn store_light(
        &mut self,
        pos: ChunkPos,
        sky_updates: &[Box<[u8]>],
        block_updates: &[Box<[u8]>],
        sky_y_mask: &azalea_core::bitset::BitSet,
        block_y_mask: &azalea_core::bitset::BitSet,
    ) {
        let num_sections = (self.chunk_storage.height() / 16 + 2) as usize;
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

        self.light_data.insert(
            (pos.x, pos.z),
            Arc::new(ChunkLightData {
                sky_sections,
                block_sections,
                min_y: self.chunk_storage.min_y(),
            }),
        );
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

    pub fn unload_chunk(&mut self, pos: &ChunkPos) {
        self.light_data.remove(&(pos.x, pos.z));
        self.partial_storage.limited_set(pos, None);
        let cx = pos.x;
        let cz = pos.z;
        self.block_entities
            .retain(|bp, _| bp.x.div_euclid(16) != cx || bp.z.div_euclid(16) != cz);
    }

    pub fn set_center(&mut self, pos: ChunkPos) {
        self.partial_storage.update_view_center(pos);
    }

    pub fn get_chunk(&self, pos: &ChunkPos) -> Option<Arc<RwLock<Chunk>>> {
        self.chunk_storage.get(pos).map(|c| Arc::clone(&c))
    }

    pub fn set_block_state(&self, x: i32, y: i32, z: i32, state: BlockState) {
        let chunk_pos = ChunkPos::new(x.div_euclid(16), z.div_euclid(16));
        let Some(chunk_lock) = self.get_chunk(&chunk_pos) else {
            return;
        };
        let mut chunk = chunk_lock.write();
        let block_pos = azalea_core::position::ChunkBlockPos {
            x: x.rem_euclid(16) as u8,
            y,
            z: z.rem_euclid(16) as u8,
        };
        chunk.set_block_state(&block_pos, state, self.chunk_storage.min_y());
    }

    pub fn get_block_state(&self, x: i32, y: i32, z: i32) -> BlockState {
        let chunk_pos = ChunkPos::new(x.div_euclid(16), z.div_euclid(16));
        let Some(chunk_lock) = self.get_chunk(&chunk_pos) else {
            return BlockState::AIR;
        };
        let chunk = chunk_lock.read();
        block_state_from_section(&chunk, x, y, z, self.chunk_storage.min_y())
    }

    pub fn height(&self) -> u32 {
        self.chunk_storage.height()
    }

    pub fn min_y(&self) -> i32 {
        self.chunk_storage.min_y()
    }

    /// Number of 16³ block sections in a column (zero-based section index range
    /// `0..section_count`).
    pub fn section_count(&self) -> i32 {
        (self.height() / 16) as i32
    }

    /// Whether the block section at world section-y has only air (vanilla
    /// `LevelChunkSection.hasOnlyAir`; azalea tracks per-section block
    /// counts). Missing chunks and out-of-range sections read as empty.
    // TODO: drop the allow once the light engine is wired into the game.
    #[allow(dead_code)]
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
            .heightmaps
            .get(&HeightmapKind::MotionBlocking)
            .map(|h| h.get_first_available(x.rem_euclid(16) as u8, z.rem_euclid(16) as u8))
            .unwrap_or(self.min_y())
    }

    /// Registry id of the biome at a block position (matches the mesher's biome
    /// lookup). Returns 0 when the chunk is missing.
    pub fn biome_id(&self, x: i32, y: i32, z: i32) -> u32 {
        let chunk_pos = ChunkPos::new(x.div_euclid(16), z.div_euclid(16));
        let Some(chunk_lock) = self.get_chunk(&chunk_pos) else {
            return 0;
        };
        let chunk = chunk_lock.read();
        let biome_pos = azalea_core::position::ChunkBiomePos {
            x: (x.rem_euclid(16) / 4) as u8,
            y,
            z: (z.rem_euclid(16) / 4) as u8,
        };
        let biome = chunk
            .get_biome(biome_pos, self.chunk_storage.min_y())
            .unwrap_or_default();
        u32::from(biome)
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
