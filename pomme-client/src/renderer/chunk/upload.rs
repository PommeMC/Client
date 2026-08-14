//! Upload responsibilities for the chunk renderer.
//!
//! The concrete upload state currently lives beside the façade while the
//! extraction is staged; this module is the boundary for the staging ring,
//! pending copies, and transfer barriers.

use super::mesher::PackedVertex;
use crate::renderer::buffer::Buffer;

pub(crate) struct PendingCopy {
    pub(crate) vertices: Vec<PackedVertex>,
    pub(crate) vtx_off: u32,
}

pub(crate) struct ChunkUploads {
    pub(crate) quad_index_buffer: Buffer,
    pub(crate) quad_index_quads: u32,
    pub(crate) quad_index_src: Option<Buffer>,
    pub(crate) quad_index_copy_pending: bool,
    pub(crate) staging_buffers: Vec<Buffer>,
    pub(crate) staging_size: u64,
    pub(crate) use_staging: bool,
    pub(crate) pending_copies: Vec<PendingCopy>,
    pub(crate) pending_v_bytes: usize,
}

impl ChunkUploads {
    pub(crate) fn new(staging_buffers: Vec<Buffer>, staging_size: u64, use_staging: bool) -> Self {
        Self {
            quad_index_buffer: Buffer::default(),
            quad_index_quads: 0,
            quad_index_src: None,
            quad_index_copy_pending: false,
            staging_buffers,
            staging_size,
            use_staging,
            pending_copies: Vec::new(),
            pending_v_bytes: 0,
        }
    }

    pub(crate) fn clear_pending(&mut self) {
        self.pending_copies.clear();
        self.pending_v_bytes = 0;
    }

    pub(crate) fn clear_pending_for(&mut self, freed: &[(u32, u32, u32)]) {
        if self.pending_copies.is_empty() || freed.is_empty() {
            return;
        }
        let mut dropped_bytes = 0usize;
        self.pending_copies.retain(|copy| {
            if freed.iter().any(|&(offset, ..)| offset == copy.vtx_off) {
                dropped_bytes += copy.vertices.len() * size_of::<PackedVertex>();
                false
            } else {
                true
            }
        });
        self.pending_v_bytes -= dropped_bytes;
    }
}

pub(crate) fn write_verts(dst: &mut [u8], off: usize, verts: &[PackedVertex]) {
    let bytes: &[u8] = bytemuck::cast_slice(verts);
    dst[off..off + bytes.len()].copy_from_slice(bytes);
}
