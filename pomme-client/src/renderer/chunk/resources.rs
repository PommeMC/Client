//! Grouped Vulkan resource construction for chunk rendering.
//!
//! Resource creation is kept as a separate boundary so descriptor updates and
//! capacity growth cannot be confused with pool or upload bookkeeping.

use std::sync::{Arc, Mutex};

use pomme_gpu_allocator::vulkan::Allocator;
use pyronyx::vk;

use crate::renderer::buffer::Buffer;

pub(crate) fn create_water_scaled_buffers(
    device: &vk::Device,
    allocator: &Arc<Mutex<Allocator>>,
    max_meta: usize,
    indirect_size: u64,
) -> (Vec<Buffer>, Vec<Buffer>) {
    let mut indirect_buffers = Vec::with_capacity(crate::renderer::MAX_FRAMES_IN_FLIGHT);
    let mut candidate_buffers = Vec::with_capacity(crate::renderer::MAX_FRAMES_IN_FLIGHT);
    for _ in 0..crate::renderer::MAX_FRAMES_IN_FLIGHT {
        indirect_buffers.push(Buffer::host(
            device,
            allocator,
            indirect_size,
            vk::BufferUsageFlags::StorageBuffer
                | vk::BufferUsageFlags::IndirectBuffer
                | vk::BufferUsageFlags::TransferDst,
            "water_indirect",
        ));
        candidate_buffers.push(Buffer::device(
            device,
            allocator,
            max_meta as u64 * 4,
            vk::BufferUsageFlags::StorageBuffer,
            "water_candidates",
        ));
    }
    (indirect_buffers, candidate_buffers)
}
