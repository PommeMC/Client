use std::io::Cursor;
use std::sync::Arc;

use azalea_block::BlockState;
use azalea_core::heightmap_kind::HeightmapKind;
use azalea_core::position::ChunkPos;
use azalea_world::chunk::Chunk;
use azalea_world::chunk::partial::PartialChunkStorage;
use azalea_world::chunk::storage::ChunkStorage;
use parking_lot::RwLock;
use thiserror::Error;

const OVERWORLD_HEIGHT: u32 = 384;
const OVERWORLD_MIN_Y: i32 = -64;

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
    pub light_data: std::collections::HashMap<(i32, i32), ChunkLightData>,
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
        }
    }

    pub fn loaded_positions(&self) -> Vec<ChunkPos> {
        self.light_data
            .keys()
            .map(|&(x, z)| ChunkPos::new(x, z))
            .collect()
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
            ChunkLightData {
                sky_sections,
                block_sections,
                min_y: self.chunk_storage.min_y(),
            },
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
