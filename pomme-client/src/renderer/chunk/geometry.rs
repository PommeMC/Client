use std::collections::{HashMap, VecDeque};

use azalea_core::position::ChunkPos;

use super::pool::FreeList;

pub(crate) const TOMBSTONE_SLOT: u32 = u32::MAX;

pub(crate) struct SectionAlloc {
    pub(crate) section_index: i32,
    pub(crate) meta_slot: u32,
    pub(crate) vertex_offset: i32,
    pub(crate) vtx_len: u32,
    pub(crate) content_gen: u64,
    pub(crate) epoch: u64,
}

impl SectionAlloc {
    pub(crate) fn is_tombstone(&self) -> bool {
        self.meta_slot == TOMBSTONE_SLOT
    }
    pub(crate) fn order_key(&self) -> (u64, u64) {
        (self.content_gen, self.epoch)
    }
}

pub(crate) struct ChunkAlloc {
    pub(crate) sections: Vec<SectionAlloc>,
}

pub(crate) fn slice_of(section: &SectionAlloc) -> (u32, u32, u32) {
    (
        section.vertex_offset as u32,
        section.vtx_len,
        section.meta_slot,
    )
}

/// CPU ownership of section allocations and their reusable ranges.
pub(crate) struct ChunkGeometry {
    pub(crate) vertex_free: FreeList,
    pub(crate) chunks: HashMap<ChunkPos, ChunkAlloc>,
    pub(crate) last_reclaim_ms: f32,
    pub(crate) frame_seq: u64,
    pub(crate) pending_free: VecDeque<(u64, (u32, u32, u32))>,
}

impl ChunkGeometry {
    pub(crate) fn new(capacity: u32) -> Self {
        Self {
            vertex_free: FreeList::new(capacity),
            chunks: HashMap::new(),
            last_reclaim_ms: 0.0,
            frame_seq: 0,
            pending_free: VecDeque::new(),
        }
    }
}
