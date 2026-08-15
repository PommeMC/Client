use std::collections::{HashMap, VecDeque};

use azalea_core::position::ChunkSectionPos;

use super::abi::RegionMeta;
use super::metadata::PersistentMetadata;

pub(crate) const REGION_X: i32 = 8;
pub(crate) const REGION_Y: i32 = 4;
pub(crate) const REGION_Z: i32 = 8;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) struct RegionPos(pub(crate) i32, pub(crate) i32, pub(crate) i32);

impl RegionPos {
    pub(crate) fn of(section: ChunkSectionPos) -> Self {
        Self(
            section.x.div_euclid(REGION_X),
            section.y.div_euclid(REGION_Y),
            section.z.div_euclid(REGION_Z),
        )
    }

    fn local_index(self, section: ChunkSectionPos) -> usize {
        let x = section.x.rem_euclid(REGION_X) as usize;
        let y = section.y.rem_euclid(REGION_Y) as usize;
        let z = section.z.rem_euclid(REGION_Z) as usize;
        x | (z << 3) | (y << 6)
    }
}

pub(crate) struct RegionAlloc {
    pub(crate) slot: u32,
    occupied: [u64; 4],
}

impl RegionAlloc {
    fn set(&mut self, local: usize, value: bool) {
        let bit = 1u64 << (local & 63);
        if value {
            self.occupied[local >> 6] |= bit;
        } else {
            self.occupied[local >> 6] &= !bit;
        }
    }
    fn any(&self) -> bool {
        self.occupied.iter().any(|&v| v != 0)
    }
    fn contains(&self, local: usize) -> bool {
        self.occupied[local >> 6] & (1u64 << (local & 63)) != 0
    }
}

pub(crate) struct RegionStore {
    pub(crate) metadata: PersistentMetadata<RegionMeta>,
    pub(crate) by_pos: HashMap<RegionPos, RegionAlloc>,
    pub(crate) pending_free: VecDeque<(u64, u32)>,
}

impl RegionStore {
    pub(crate) fn new(capacity: usize) -> Self {
        Self {
            metadata: PersistentMetadata::new(capacity),
            by_pos: HashMap::new(),
            pending_free: VecDeque::new(),
        }
    }

    pub(crate) fn add(&mut self, section: ChunkSectionPos) -> Option<u32> {
        let pos = RegionPos::of(section);
        if let Some(region) = self.by_pos.get_mut(&pos) {
            region.set(pos.local_index(section), true);
            let slot = region.slot;
            self.rebuild(pos);
            return Some(slot);
        }
        let slot = self.metadata.slots.alloc(1)?;
        self.metadata.high_water = self.metadata.high_water.max(slot + 1);
        let mut alloc = RegionAlloc {
            slot,
            occupied: [0; 4],
        };
        alloc.set(pos.local_index(section), true);
        self.by_pos.insert(pos, alloc);
        self.rebuild(pos);
        Some(slot)
    }

    pub(crate) fn remove(&mut self, section: ChunkSectionPos, frame_seq: u64, frames: u64) {
        let pos = RegionPos::of(section);
        let Some(region) = self.by_pos.get_mut(&pos) else {
            return;
        };
        region.set(pos.local_index(section), false);
        if region.any() {
            self.rebuild(pos);
            return;
        }
        let region = self.by_pos.remove(&pos).unwrap();
        self.metadata
            .queue_write(region.slot, bytemuck::Zeroable::zeroed());
        self.pending_free
            .push_back((frame_seq + frames, region.slot));
    }

    pub(crate) fn reclaim(&mut self, frame_seq: u64) {
        while self
            .pending_free
            .front()
            .is_some_and(|&(at, _)| at <= frame_seq)
        {
            let (_, slot) = self.pending_free.pop_front().unwrap();
            self.metadata.slots.free(slot, 1);
        }
    }

    fn rebuild(&mut self, pos: RegionPos) {
        let region = &self.by_pos[&pos];
        let mut lo = [REGION_X, REGION_Y, REGION_Z];
        let mut hi = [0, 0, 0];
        for local in 0..256 {
            if !region.contains(local) {
                continue;
            }
            let x = (local & 7) as i32;
            let z = ((local >> 3) & 7) as i32;
            let y = ((local >> 6) & 3) as i32;
            lo[0] = lo[0].min(x);
            lo[1] = lo[1].min(y);
            lo[2] = lo[2].min(z);
            hi[0] = hi[0].max(x + 1);
            hi[1] = hi[1].max(y + 1);
            hi[2] = hi[2].max(z + 1);
        }
        self.metadata.queue_write(
            region.slot,
            RegionMeta {
                aabb_min: [
                    lo[0] as f32 * 16.0,
                    lo[1] as f32 * 16.0,
                    lo[2] as f32 * 16.0,
                ],
                _unused_slot: 0,
                aabb_max: [
                    hi[0] as f32 * 16.0,
                    hi[1] as f32 * 16.0,
                    hi[2] as f32 * 16.0,
                ],
                generation: 1,
                origin: [
                    pos.0 * REGION_X * 16,
                    pos.1 * REGION_Y * 16,
                    pos.2 * REGION_Z * 16,
                ],
                _pad: 0,
            },
        );
    }

    pub(crate) fn reset(&mut self) {
        self.by_pos.clear();
        self.pending_free.clear();
        self.metadata.reset();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn negative_sections_use_euclidean_regions() {
        assert_eq!(
            RegionPos::of(ChunkSectionPos::new(-1, -1, -1)),
            RegionPos(-1, -1, -1)
        );
        assert_eq!(
            RegionPos::of(ChunkSectionPos::new(-8, -4, -8)),
            RegionPos(-1, -1, -1)
        );
        assert_eq!(
            RegionPos::of(ChunkSectionPos::new(-9, -5, -9)),
            RegionPos(-2, -2, -2)
        );
    }
}
