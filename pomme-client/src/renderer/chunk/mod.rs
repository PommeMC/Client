pub(crate) mod abi;
pub mod atlas;
pub mod block_ao;
pub(crate) mod cull;
pub mod dispatcher;
pub(crate) mod geometry;
pub mod mesher;
pub(crate) mod metadata;
pub(crate) mod pool;
pub(crate) mod region;
pub(crate) mod resources;
pub mod section;
mod state;
pub(crate) mod upload;

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use azalea_core::position::ChunkPos;
use glam::DVec3;
use pomme_gpu_allocator::vulkan::Allocator;
use pyronyx::vk;

use self::cull::ChunkCulling;
use self::geometry::ChunkGeometry;
use self::mesher::SectionMeshData;
use self::metadata::ChunkMetadata;
use self::state::ChunkRendererCore;
use self::upload::ChunkUploads;
use crate::renderer::ChunkDrawBackend;
use crate::renderer::buffer::Buffer;
use crate::renderer::context::TaskShaderLimits;

pub(super) struct ChunkRendererState {
    last_pool_warn: Option<std::time::Instant>,
    mesh_buffer: Buffer,
    global_cuboid_buffer: Buffer,
    uploads: ChunkUploads,
    geometry: ChunkGeometry,
    metadata: ChunkMetadata,
    regions: region::RegionStore,
    next_visibility_generation: u32,
    culling: ChunkCulling,
}

/// Public chunk-rendering façade. Resource lifetime and rendering phases are
/// implemented by private components; callers do not depend on their layout.
pub struct ChunkRenderer {
    core: ChunkRendererCore,
}

impl ChunkRenderer {
    pub fn new(
        device: &vk::Device,
        physical_device: vk::PhysicalDevice,
        allocator: &Arc<Mutex<Allocator>>,
        global_cuboids: &[mesher::CuboidData],
        render_distance: u32,
        backend: ChunkDrawBackend,
        task_limits: TaskShaderLimits,
    ) -> Self {
        Self {
            core: ChunkRendererCore::new(
                device,
                physical_device,
                allocator,
                global_cuboids,
                render_distance,
                backend,
                task_limits,
            ),
        }
    }

    pub fn sections_drawn(&self) -> u32 {
        self.core.sections_drawn()
    }

    pub fn has_sections(&self) -> bool {
        self.core.metadata.high_water != 0
    }
    pub fn meta_rebuild_ms(&self) -> f32 {
        self.core.meta_rebuild_ms()
    }
    pub fn last_reclaim_ms(&self) -> f32 {
        self.core.last_reclaim_ms()
    }
    pub fn chunk_count(&self) -> u32 {
        self.core.chunk_count()
    }

    pub fn record_copies(&mut self, cmd: vk::CommandBuffer, frame: usize) {
        self.core.record_copies(cmd, frame);
    }

    pub fn stage_mesh_batch(
        &mut self,
        device: &vk::Device,
        allocator: &Arc<Mutex<Allocator>>,
        mesh_queue: &mut VecDeque<SectionMeshData>,
        eye: DVec3,
    ) {
        self.core
            .stage_mesh_batch(device, allocator, mesh_queue, eye);
    }

    pub fn begin_frame(&mut self) {
        self.core.begin_frame();
    }
    pub fn frame_submitted(&mut self) {
        self.core.frame_submitted();
    }
    pub fn remove(&mut self, pos: &ChunkPos) {
        self.core.remove(pos);
    }
    pub fn clear(&mut self) {
        self.core.clear();
    }

    #[allow(clippy::too_many_arguments)]
    pub fn dispatch_cull(
        &mut self,
        cmd: vk::CommandBuffer,
        frame: usize,
        frustum: &[[f32; 4]; 6],
        anchor: DVec3,
        eye: DVec3,
        player_chunk: ChunkPos,
        limit_rd: Option<u32>,
        occlusion_enabled: bool,
    ) {
        self.core.dispatch_cull(
            cmd,
            frame,
            frustum,
            anchor,
            eye,
            player_chunk,
            limit_rd,
            occlusion_enabled,
        );
    }

    pub fn draw_indirect(&mut self, cmd: vk::CommandBuffer, frame: usize, cutout: bool) {
        self.core.draw_indirect(cmd, frame, cutout);
    }
    pub fn draw_set(&self, frame: usize) -> vk::DescriptorSet {
        self.core.culling.compute_sets[frame]
    }
    pub fn expand_sections(&self, cmd: vk::CommandBuffer, frame: usize) {
        self.core.expand_sections(cmd, frame);
    }
    pub fn finalize_occlusion(&self, cmd: vk::CommandBuffer, frame: usize) {
        self.core.finalize_occlusion(cmd, frame);
    }
    pub fn aabb_resources(
        &self,
        frame: usize,
        sections: bool,
    ) -> (vk::Buffer, vk::Buffer, vk::Buffer, vk::Buffer) {
        self.core.aabb_resources(frame, sections)
    }
    pub fn draw_water(&mut self, cmd: vk::CommandBuffer, frame: usize) {
        self.core.draw_water(cmd, frame);
    }
    pub fn destroy(&mut self, device: &vk::Device, allocator: &Arc<Mutex<Allocator>>) {
        self.core.destroy(device, allocator);
    }

    pub fn geometry_buffers(&self) -> (vk::Buffer, vk::Buffer) {
        (
            self.core.mesh_buffer.buffer,
            self.core.global_cuboid_buffer.buffer,
        )
    }

    pub fn replace_global_cuboids(
        &mut self,
        device: &vk::Device,
        allocator: &Arc<Mutex<Allocator>>,
        cuboids: &[mesher::CuboidData],
    ) {
        self.core.replace_global_cuboids(device, allocator, cuboids);
    }
}

pub(crate) use abi::{
    vertex_attributes as chunk_vertex_attributes, vertex_bindings as chunk_vertex_bindings,
};
