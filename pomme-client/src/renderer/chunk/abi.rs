use glam::DVec3;
use pyronyx::vk;

use super::mesher::ChunkAABB;

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
pub(crate) struct ChunkMeta {
    pub(crate) aabb_min: [f32; 3],
    pub(crate) region_slot: u32,
    pub(crate) aabb_max: [f32; 3],
    pub(crate) visibility_generation: u32,
    pub(crate) origin: [i32; 3],
    pub(crate) _pad: u32,
    pub(crate) batch_word_offset: u32,
    pub(crate) solid_batch_count: u32,
    pub(crate) cutout_batch_count: u32,
    pub(crate) fluid_batch_count: u32,
}

/// Conservative occupied bounds for one 8x4x8-section region.  The prefix is
/// intentionally layout-compatible with the bounds prefix of `ChunkMeta` so
/// the same raster shaders can consume either buffer.
#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
pub(crate) struct RegionMeta {
    pub(crate) aabb_min: [f32; 3],
    pub(crate) _unused_slot: u32,
    pub(crate) aabb_max: [f32; 3],
    pub(crate) generation: u32,
    pub(crate) origin: [i32; 3],
    pub(crate) _pad: u32,
}

const _: () = assert!(size_of::<ChunkMeta>() == 64);
const _: () = assert!(size_of::<RegionMeta>() == 48);

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
pub(crate) struct DrawCommand {
    pub(crate) vertex_count: u32,
    pub(crate) instance_count: u32,
    pub(crate) first_vertex: u32,
    pub(crate) first_instance: u32,
}

const _: () = assert!(
    std::mem::size_of::<DrawCommand>()
        == std::mem::size_of::<vk::DrawMeshTasksIndirectCommandEXT>() + 4
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn indirect_command_has_mesh_dispatch_prefix_and_batch_word() {
        assert_eq!(size_of::<DrawCommand>(), 16);
        assert_eq!(size_of::<vk::DrawMeshTasksIndirectCommandEXT>(), 12);
        assert_eq!(std::mem::offset_of!(DrawCommand, first_instance), 12);
    }

    #[test]
    fn metadata_layouts_keep_the_shared_bounds_prefix() {
        assert_eq!(size_of::<ChunkMeta>(), 64);
        assert_eq!(size_of::<RegionMeta>(), 48);
        assert_eq!(std::mem::offset_of!(ChunkMeta, origin), 32);
        assert_eq!(std::mem::offset_of!(RegionMeta, origin), 32);
    }
}

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
pub(crate) struct FrustumData {
    pub(crate) planes: [[f32; 4]; 6],
    pub(crate) chunk_count: u32,
    pub(crate) region_count: u32,
    pub(crate) cam_block: [i32; 3],
    pub(crate) frac: [f32; 3],
    pub(crate) player_chunk: [i32; 2],
    pub(crate) limit_rd: u32,
    pub(crate) draw_capacity: u32,
    pub(crate) occlusion_enabled: u32,
    pub(crate) _pad: [u32; 2],
}

pub(crate) fn vertex_bindings() -> [vk::VertexInputBindingDescription; 0] {
    []
}

pub(crate) fn vertex_attributes() -> [vk::VertexInputAttributeDescription; 0] {
    []
}

pub(crate) fn aabb_in_frustum(
    aabb: &ChunkAABB,
    origin: [i32; 3],
    planes: &[[f32; 4]; 6],
    eye: DVec3,
) -> bool {
    let base = (DVec3::new(origin[0] as f64, origin[1] as f64, origin[2] as f64) - eye).as_vec3();
    let mn = [
        base.x + aabb.min[0],
        base.y + aabb.min[1],
        base.z + aabb.min[2],
    ];
    let mx = [
        base.x + aabb.max[0],
        base.y + aabb.max[1],
        base.z + aabb.max[2],
    ];
    planes.iter().all(|p| {
        let d = p[0] * if p[0] >= 0.0 { mx[0] } else { mn[0] }
            + p[1] * if p[1] >= 0.0 { mx[1] } else { mn[1] }
            + p[2] * if p[2] >= 0.0 { mx[2] } else { mn[2] }
            + p[3];
        d >= 0.0
    })
}
