use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, Mutex};

use azalea_core::position::{ChunkPos, ChunkSectionPos};
use glam::DVec3;
use pomme_gpu_allocator::vulkan::Allocator;
use pyronyx::vk;

use super::abi::{ChunkMeta, DrawCommand, FrustumData};
use super::cull::{ChunkCulling, create_compute_pipeline, create_cull_desc_layout, desc_write};
use super::dispatcher::pack_section_pos;
use super::geometry::{ChunkAlloc, ChunkGeometry, SectionAlloc, TOMBSTONE_SLOT, slice_of};
use super::mesher::{FADE_DURATION_MS, PackedVertex, SectionMeshData};
use super::metadata::ChunkMetadata;
use super::resources::create_water_scaled_buffers;
use super::upload::{ChunkUploads, PendingCopy, write_verts};
use crate::renderer::buffer::Buffer;
use crate::renderer::hiz::OcclusionCamera;
use crate::renderer::{MAX_FRAMES_IN_FLIGHT, shader};

const BUCKET_VERTICES: u32 = 32768;
const VERTEX_SIZE: u64 = size_of::<PackedVertex>() as u64;
const BYTES_PER_BUCKET: u64 = BUCKET_VERTICES as u64 * VERTEX_SIZE;
/// Initial capacity (in quads) of the shared static quad index buffer; grown
/// by doubling if a single section's pass ever exceeds it.
const INITIAL_QUAD_INDEX_QUADS: u32 = 16384;
/// Manhattan-distance buckets (in sections) ordering the translucent water
/// draw list back-to-front on the GPU; sections farther than the last bucket
/// clamp into it.
const WATER_BUCKETS: u32 = 512;
const MIN_BUCKETS: u32 = 128;
// A full-world reload (dimension change, render-distance toggle) transiently
// holds both worlds: unloaded slices stay allocated until their frame deadline
// while the new world uploads. The cap leaves room for that 2x so the reload
// never hits the emergency GPU-wait reclaim; `VRAM_BUDGET_FRACTION` still
// bounds smaller cards.
const MAX_BUCKETS: u32 = 4096;
/// Integrated GPUs share system RAM (reported as device-local), so their pool
/// caps at ~512 MB instead of the discrete cards' ~1.75 GB.
const MAX_BUCKETS_INTEGRATED: u32 = 1024;
const VRAM_BUDGET_FRACTION: f64 = 0.25;
/// Sections whose center sits within this squared distance of the camera
/// render opaque immediately and never fade in.
const NEARBY_DIST_SQ: f32 = 768.0;

/// Whether a section's center is within the always-near radius of the eye
/// (vanilla `isNearby`: a 3D distance on the section center, so a section 30
/// blocks overhead still fades), rebased in f64 for precision at extreme
/// coordinates.
fn section_is_near(spos: ChunkSectionPos, eye: DVec3) -> bool {
    let dx = spos.x as f64 * 16.0 + 8.0 - eye.x;
    let dy = spos.y as f64 * 16.0 + 8.0 - eye.y;
    let dz = spos.z as f64 * 16.0 + 8.0 - eye.z;
    dx * dx + dy * dy + dz * dz < NEARBY_DIST_SQ as f64
}

fn compute_bucket_count(physical_device: vk::PhysicalDevice) -> u32 {
    let mem_props = physical_device.get_memory_properties();
    let props = physical_device.get_properties();
    let mut device_local_bytes: u64 = 0;
    for i in 0..mem_props.memory_type_count as usize {
        let mem_type = mem_props.memory_types[i];
        if mem_type
            .property_flags
            .contains(vk::MemoryPropertyFlags::DeviceLocal)
        {
            let heap = mem_props.memory_heaps[mem_type.heap_index as usize];
            if heap.size > device_local_bytes {
                device_local_bytes = heap.size;
            }
        }
    }
    let budget = (device_local_bytes as f64 * VRAM_BUDGET_FRACTION) as u64;
    // Integrated GPUs report system RAM as their device-local heap, so the
    // fraction would eagerly reserve gigabytes of host memory; cap them.
    let max_buckets = if props.device_type == vk::PhysicalDeviceType::DiscreteGpu {
        MAX_BUCKETS
    } else {
        MAX_BUCKETS_INTEGRATED
    };
    let buckets = (budget / BYTES_PER_BUCKET) as u32;
    let count = buckets.clamp(MIN_BUCKETS, max_buckets);
    tracing::info!(
        "GPU VRAM: {} MB, chunk budget: {} MB, buckets: {}",
        device_local_bytes / (1024 * 1024),
        (count as u64 * BYTES_PER_BUCKET) / (1024 * 1024),
        count
    );
    count
}

pub(super) type ChunkRendererCore = super::ChunkRendererState;

#[path = "state_culling.rs"]
mod state_culling;
#[path = "state_geometry.rs"]
mod state_geometry;
#[path = "state_resources.rs"]
mod state_resources;
#[path = "state_upload.rs"]
mod state_upload;
