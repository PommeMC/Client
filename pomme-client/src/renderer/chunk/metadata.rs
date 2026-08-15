use super::abi::ChunkMeta;
use super::pool::FreeList;
use crate::renderer::buffer::Buffer;

/// Persistent CPU-side metadata bookkeeping. GPU buffer ownership remains in
/// the culling component; this type owns slots, mirrors, and write ordering.
pub(crate) struct PersistentMetadata<T: bytemuck::Pod + bytemuck::Zeroable + Copy> {
    pub(crate) slots: FreeList,
    pub(crate) mirror: Vec<T>,
    pub(crate) high_water: u32,
    pub(crate) writes: Vec<(u32, T)>,
    pub(crate) applied: [usize; crate::renderer::MAX_FRAMES_IN_FLIGHT],
}

impl<T: bytemuck::Pod + bytemuck::Zeroable + Copy> PersistentMetadata<T> {
    pub(crate) fn new(capacity: usize) -> Self {
        Self {
            slots: FreeList::new(capacity as u32),
            mirror: vec![bytemuck::Zeroable::zeroed(); capacity],
            high_water: 0,
            writes: Vec::new(),
            applied: [0; crate::renderer::MAX_FRAMES_IN_FLIGHT],
        }
    }

    pub(crate) fn queue_write(&mut self, slot: u32, entry: T) {
        self.mirror[slot as usize] = entry;
        self.writes.push((slot, entry));
    }

    pub(crate) fn apply_writes(&mut self, frame: usize, buffer: &mut Buffer, max_meta: usize) {
        let dst = buffer.mapped_slice_mut();
        let pending = &self.writes[self.applied[frame]..];
        if pending.len() > max_meta / 2 {
            let bytes: &[u8] = bytemuck::cast_slice(&self.mirror);
            dst[..bytes.len()].copy_from_slice(bytes);
        } else {
            for &(slot, entry) in pending {
                let offset = slot as usize * size_of::<T>();
                dst[offset..offset + size_of::<T>()].copy_from_slice(bytemuck::bytes_of(&entry));
            }
        }
        self.applied[frame] = self.writes.len();
        if self
            .applied
            .iter()
            .all(|&cursor| cursor == self.writes.len())
        {
            self.writes.clear();
            self.applied = [0; crate::renderer::MAX_FRAMES_IN_FLIGHT];
        }
    }

    pub(crate) fn reset(&mut self) {
        self.slots.reset();
        self.high_water = 0;
        self.mirror.fill(bytemuck::Zeroable::zeroed());
        self.writes.clear();
        self.applied = [0; crate::renderer::MAX_FRAMES_IN_FLIGHT];
    }
}

pub(crate) type ChunkMetadata = PersistentMetadata<ChunkMeta>;
