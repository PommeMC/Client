// ChunkRendererState geometry responsibilities.

use super::*;

impl ChunkRendererCore {
    pub(crate) fn last_reclaim_ms(&self) -> f32 {
        self.geometry.last_reclaim_ms
    }

    pub(super) fn alloc_mesh(&mut self, device: &vk::Device, count: u32) -> Option<u32> {
        if let Some(off) = self.geometry.mesh_free.alloc(count) {
            return Some(off);
        }
        self.reclaim_retired(device)
            .then(|| self.geometry.mesh_free.alloc(count))
            .flatten()
    }

    pub(super) fn reclaim_retired(&mut self, device: &vk::Device) -> bool {
        const MIN_RECLAIM_SLICES: usize = 64;
        if self.geometry.pending_free.len() < MIN_RECLAIM_SLICES {
            return false;
        }
        let start = std::time::Instant::now();
        device.wait_idle().ok();
        while let Some((_, slice)) = self.geometry.pending_free.pop_front() {
            self.free_slice(slice);
        }
        self.geometry.last_reclaim_ms += start.elapsed().as_secs_f32() * 1000.0;
        true
    }

    pub(super) fn free_slice(&mut self, (vo, vl, slot): (u32, u32, u32)) {
        self.geometry.mesh_free.free_region(vo, vl);
        self.metadata.slots.free_region(slot, 1);
    }

    pub(super) fn take_section(&mut self, col_pos: ChunkPos, si: i32) -> bool {
        let mut was_present = false;
        let mut freed = Vec::new();
        let mut removed_positions = Vec::new();
        if let Some(entry) = self.geometry.chunks.get_mut(&col_pos) {
            entry.sections.retain(|s| {
                if s.section_index == si {
                    was_present = true;
                    if !s.is_tombstone() {
                        freed.push(slice_of(s));
                        removed_positions.push(s.section_pos);
                    }
                    false
                } else {
                    true
                }
            });
        }
        for spos in removed_positions {
            self.regions
                .remove(spos, self.geometry.frame_seq, MAX_FRAMES_IN_FLIGHT as u64);
        }
        self.retire_freed(freed);
        was_present
    }

    pub(super) fn retire_freed(&mut self, freed: Vec<(u32, u32, u32)>) {
        for &(_, _, slot) in &freed {
            self.queue_meta_write(slot, bytemuck::Zeroable::zeroed());
        }
        self.drop_pending_copies_for(&freed);
        self.retire_slices(freed);
    }

    pub(super) fn retire_slices(&mut self, slices: impl IntoIterator<Item = (u32, u32, u32)>) {
        let retire_at = self.geometry.frame_seq + MAX_FRAMES_IN_FLIGHT as u64;
        for slice in slices {
            self.geometry.pending_free.push_back((retire_at, slice));
        }
    }

    pub fn begin_frame(&mut self) {
        while self
            .geometry
            .pending_free
            .front()
            .is_some_and(|&(retire_at, _)| retire_at <= self.geometry.frame_seq)
        {
            let (_, slice) = self.geometry.pending_free.pop_front().unwrap();
            self.free_slice(slice);
        }
        self.regions.reclaim(self.geometry.frame_seq);
    }

    pub fn frame_submitted(&mut self) {
        self.geometry.frame_seq += 1;
    }

    pub fn remove(&mut self, pos: &ChunkPos) {
        if let Some(alloc) = self.geometry.chunks.remove(pos) {
            for section in alloc.sections.iter().filter(|s| !s.is_tombstone()) {
                self.regions.remove(
                    section.section_pos,
                    self.geometry.frame_seq,
                    MAX_FRAMES_IN_FLIGHT as u64,
                );
            }
            let freed = alloc
                .sections
                .iter()
                .filter(|s| !s.is_tombstone())
                .map(slice_of)
                .collect();
            self.retire_freed(freed);
        }
    }

    pub fn clear(&mut self) {
        self.geometry.chunks.clear();
        self.geometry.mesh_free.reset();
        self.geometry.pending_free.clear();
        // Staged copies target pool offsets that just died with the pools.
        self.drop_pending_copies();
        // Dropping the high-water mark to 0 makes every stale GPU meta entry
        // unreachable; no buffer scrub needed.
        self.metadata.reset();
        self.regions.reset();
        self.culling.history_reset_pending = true;
        self.culling.fade_enabled = false;
    }

    pub fn chunk_count(&self) -> u32 {
        self.geometry.chunks.len() as u32
    }

    pub fn mesh_pool_bytes(&self) -> (u64, u64) {
        let used_units = self.geometry.mesh_free.used() as u64;
        let capacity_units = self.geometry.mesh_free.capacity() as u64;
        (used_units * POOL_UNIT, capacity_units * POOL_UNIT)
    }
}
