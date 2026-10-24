//! Blocks a piston is moving, drawn as terrain: meshed each frame by
//! `MeshDispatcher::mesh_moving_block` and drawn through the chunk pipelines
//! with a one-instance origin per block, like vanilla's `solidMovingBlock` and
//! `cutoutMovingBlock` render types.

use std::sync::{Arc, Mutex};

use pomme_gpu_allocator::vulkan::{Allocation, Allocator};
use pyronyx::vk;

use super::buffer::ChunkMeta;
use super::mesher::{MovingBlockMesh, PackedVertex};
use crate::renderer::{MAX_FRAMES_IN_FLIGHT, util};

/// One moving block's mesh and the cell its vertices are relative to.
pub struct MovingBlockRender {
    pub origin: [i32; 3],
    pub mesh: MovingBlockMesh,
}

/// Starting room for a large contraption: 12 blocks per piston and a few
/// pistons. A frame with more grows its buffers.
const INITIAL_BLOCKS: usize = 256;

#[derive(Clone, Copy)]
struct Draw {
    vertex_offset: i32,
    first_index: u32,
    solid_count: u32,
    cutout_count: u32,
}

/// A host-visible buffer that's replaced by a bigger one when a frame needs
/// more room.
struct Mapped {
    buffer: vk::Buffer,
    allocation: Allocation,
    capacity: usize,
    usage: vk::BufferUsageFlags,
    name: &'static str,
}

impl Mapped {
    fn new(
        device: &vk::Device,
        allocator: &Arc<Mutex<Allocator>>,
        capacity: usize,
        usage: vk::BufferUsageFlags,
        name: &'static str,
    ) -> Self {
        let (buffer, allocation) =
            util::create_host_buffer(device, allocator, capacity as u64, usage, name);
        Self {
            buffer,
            allocation,
            capacity,
            usage,
            name,
        }
    }

    /// Writes `bytes`, first growing the buffer if they don't fit. Only safe
    /// once this frame's fence has signalled.
    fn write(&mut self, device: &vk::Device, allocator: &Arc<Mutex<Allocator>>, bytes: &[u8]) {
        if bytes.len() > self.capacity {
            let capacity = bytes.len().max(self.capacity * 2);
            let old = std::mem::replace(
                self,
                Self::new(device, allocator, capacity, self.usage, self.name),
            );
            old.destroy(device, allocator);
        }
        if let Some(mapped) = self.allocation.mapped_slice_mut() {
            mapped[..bytes.len()].copy_from_slice(bytes);
        }
    }

    fn destroy(self, device: &vk::Device, allocator: &Arc<Mutex<Allocator>>) {
        device.destroy_buffer(self.buffer, None);
        allocator.lock().unwrap().free(self.allocation).ok();
    }
}

struct FrameBuffers {
    vertices: Mapped,
    indices: Mapped,
    meta: Mapped,
}

pub struct MovingBlockBuffers {
    frames: Vec<FrameBuffers>,
    draws: Vec<Draw>,
}

impl MovingBlockBuffers {
    pub fn new(device: &vk::Device, allocator: &Arc<Mutex<Allocator>>) -> Self {
        let frames = (0..MAX_FRAMES_IN_FLIGHT)
            .map(|_| FrameBuffers {
                vertices: Mapped::new(
                    device,
                    allocator,
                    INITIAL_BLOCKS * 24 * 4 * size_of::<PackedVertex>(),
                    vk::BufferUsageFlags::VertexBuffer,
                    "moving_block_vertices",
                ),
                indices: Mapped::new(
                    device,
                    allocator,
                    INITIAL_BLOCKS * 24 * 6 * size_of::<u32>(),
                    vk::BufferUsageFlags::IndexBuffer,
                    "moving_block_indices",
                ),
                meta: Mapped::new(
                    device,
                    allocator,
                    INITIAL_BLOCKS * size_of::<ChunkMeta>(),
                    vk::BufferUsageFlags::VertexBuffer,
                    "moving_block_meta",
                ),
            })
            .collect();
        Self {
            frames,
            draws: Vec::new(),
        }
    }

    /// Copies this frame's blocks into `frame`'s buffers, after its fence.
    pub fn upload(
        &mut self,
        device: &vk::Device,
        allocator: &Arc<Mutex<Allocator>>,
        frame: usize,
        blocks: &[MovingBlockRender],
    ) {
        self.draws.clear();
        let mut vertices: Vec<PackedVertex> = Vec::new();
        let mut indices: Vec<u32> = Vec::new();
        let mut meta: Vec<ChunkMeta> = Vec::new();
        for block in blocks {
            let mesh = &block.mesh;
            self.draws.push(Draw {
                vertex_offset: vertices.len() as i32,
                first_index: indices.len() as u32,
                solid_count: mesh.solid.len() as u32,
                cutout_count: mesh.cutout.len() as u32,
            });
            vertices.extend_from_slice(&mesh.vertices);
            indices.extend_from_slice(&mesh.solid);
            indices.extend_from_slice(&mesh.cutout);
            meta.push(ChunkMeta::instance(block.origin));
        }
        let buffers = &mut self.frames[frame];
        buffers
            .vertices
            .write(device, allocator, bytemuck::cast_slice(&vertices));
        buffers
            .indices
            .write(device, allocator, bytemuck::cast_slice(&indices));
        buffers
            .meta
            .write(device, allocator, bytemuck::cast_slice(&meta));
    }

    /// Draws the uploaded blocks' solid or cutout quads; the caller binds the
    /// matching chunk pipeline first.
    pub fn draw(&self, cmd: vk::CommandBuffer, frame: usize, cutout: bool) {
        if self.draws.is_empty() {
            return;
        }
        let buffers = &self.frames[frame];
        cmd.bind_vertex_buffers(0, &[buffers.vertices.buffer, buffers.meta.buffer], &[0, 0]);
        cmd.bind_index_buffer(buffers.indices.buffer, 0, vk::IndexType::Uint32);
        for (instance, draw) in self.draws.iter().enumerate() {
            let (first, count) = if cutout {
                (draw.first_index + draw.solid_count, draw.cutout_count)
            } else {
                (draw.first_index, draw.solid_count)
            };
            if count > 0 {
                cmd.draw_indexed(count, 1, first, draw.vertex_offset, instance as u32);
            }
        }
    }

    pub fn destroy(&mut self, device: &vk::Device, allocator: &Arc<Mutex<Allocator>>) {
        for frame in self.frames.drain(..) {
            for buffer in [frame.vertices, frame.indices, frame.meta] {
                buffer.destroy(device, allocator);
            }
        }
    }
}
