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

/// Room for a large contraption: 12 blocks per piston and a few pistons.
const MAX_BLOCKS: usize = 256;
const MAX_VERTICES: usize = MAX_BLOCKS * 24 * 4;
const MAX_INDICES: usize = MAX_BLOCKS * 24 * 6;

#[derive(Clone, Copy)]
struct Draw {
    vertex_offset: i32,
    first_index: u32,
    solid_count: u32,
    cutout_count: u32,
}

struct FrameBuffers {
    vertices: (vk::Buffer, Allocation),
    indices: (vk::Buffer, Allocation),
    meta: (vk::Buffer, Allocation),
}

pub struct MovingBlockBuffers {
    frames: Vec<FrameBuffers>,
    draws: Vec<Draw>,
}

impl MovingBlockBuffers {
    pub fn new(device: &vk::Device, allocator: &Arc<Mutex<Allocator>>) -> Self {
        let buffer = |bytes: usize, usage, name| {
            util::create_mapped_buffer(device, allocator, &vec![0u8; bytes], usage, name)
        };
        let frames = (0..MAX_FRAMES_IN_FLIGHT)
            .map(|_| FrameBuffers {
                vertices: buffer(
                    MAX_VERTICES * size_of::<PackedVertex>(),
                    vk::BufferUsageFlags::VertexBuffer,
                    "moving_block_vertices",
                ),
                indices: buffer(
                    MAX_INDICES * size_of::<u32>(),
                    vk::BufferUsageFlags::IndexBuffer,
                    "moving_block_indices",
                ),
                meta: buffer(
                    MAX_BLOCKS * size_of::<ChunkMeta>(),
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

    /// Copies this frame's blocks into `frame`'s buffers; blocks past the
    /// capacity are dropped.
    pub fn upload(&mut self, frame: usize, blocks: &[MovingBlockRender]) {
        self.draws.clear();
        let mut vertices: Vec<PackedVertex> = Vec::new();
        let mut indices: Vec<u32> = Vec::new();
        let mut meta: Vec<ChunkMeta> = Vec::new();
        for block in blocks.iter().take(MAX_BLOCKS) {
            let mesh = &block.mesh;
            let index_count = mesh.solid.len() + mesh.cutout.len();
            if vertices.len() + mesh.vertices.len() > MAX_VERTICES
                || indices.len() + index_count > MAX_INDICES
            {
                break;
            }
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
        write(&mut buffers.vertices.1, bytemuck::cast_slice(&vertices));
        write(&mut buffers.indices.1, bytemuck::cast_slice(&indices));
        write(&mut buffers.meta.1, bytemuck::cast_slice(&meta));
    }

    /// Draws the uploaded blocks' solid or cutout quads; the caller binds the
    /// matching chunk pipeline first.
    pub fn draw(&self, cmd: vk::CommandBuffer, frame: usize, cutout: bool) {
        if self.draws.is_empty() {
            return;
        }
        let buffers = &self.frames[frame];
        cmd.bind_vertex_buffers(0, &[buffers.vertices.0, buffers.meta.0], &[0, 0]);
        cmd.bind_index_buffer(buffers.indices.0, 0, vk::IndexType::Uint32);
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
        let mut alloc = allocator.lock().unwrap();
        for frame in self.frames.drain(..) {
            for (buffer, allocation) in [frame.vertices, frame.indices, frame.meta] {
                device.destroy_buffer(buffer, None);
                alloc.free(allocation).ok();
            }
        }
    }
}

fn write(allocation: &mut Allocation, bytes: &[u8]) {
    if let Some(mapped) = allocation.mapped_slice_mut() {
        mapped[..bytes.len()].copy_from_slice(bytes);
    }
}
