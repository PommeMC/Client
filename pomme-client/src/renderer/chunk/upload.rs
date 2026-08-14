//! Upload responsibilities for the chunk renderer.
//!
//! The concrete upload state currently lives beside the façade while the
//! extraction is staged; this module is the boundary for the staging ring,
//! pending copies, and transfer barriers.

use crate::renderer::buffer::Buffer;

pub(crate) struct PendingCopy {
    pub(crate) bytes: Vec<u8>,
    pub(crate) pool_off: u32,
}

pub(crate) struct ChunkUploads {
    pub(crate) staging_buffers: Vec<Buffer>,
    pub(crate) staging_size: u64,
    pub(crate) use_staging: bool,
    pub(crate) pending_copies: Vec<PendingCopy>,
    pub(crate) pending_mesh_bytes: usize,
}

impl ChunkUploads {
    pub(crate) fn new(staging_buffers: Vec<Buffer>, staging_size: u64, use_staging: bool) -> Self {
        Self {
            staging_buffers,
            staging_size,
            use_staging,
            pending_copies: Vec::new(),
            pending_mesh_bytes: 0,
        }
    }

    pub(crate) fn clear_pending(&mut self) {
        self.pending_copies.clear();
        self.pending_mesh_bytes = 0;
    }

    pub(crate) fn clear_pending_for(&mut self, freed: &[(u32, u32, u32)]) {
        if self.pending_copies.is_empty() || freed.is_empty() {
            return;
        }
        let mut dropped_bytes = 0usize;
        self.pending_copies.retain(|copy| {
            if freed.iter().any(|(offset, ..)| *offset == copy.pool_off) {
                dropped_bytes += copy.bytes.len();
                false
            } else {
                true
            }
        });
        self.pending_mesh_bytes -= dropped_bytes;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clearing_a_section_drops_only_its_single_pending_allocation() {
        let mut uploads = ChunkUploads::new(Vec::new(), 0, true);
        uploads.pending_copies = vec![
            PendingCopy {
                bytes: vec![0; 24],
                pool_off: 10,
            },
            PendingCopy {
                bytes: vec![0; 40],
                pool_off: 20,
            },
        ];
        uploads.pending_mesh_bytes = 64;
        uploads.clear_pending_for(&[(10, 3, 7)]);
        assert_eq!(uploads.pending_copies.len(), 1);
        assert_eq!(uploads.pending_copies[0].pool_off, 20);
        assert_eq!(uploads.pending_mesh_bytes, 40);
    }
}
