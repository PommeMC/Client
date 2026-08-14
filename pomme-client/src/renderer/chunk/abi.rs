use glam::DVec3;
use pyronyx::vk;

use super::mesher::ChunkAABB;

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
pub(crate) struct ChunkMeta {
    pub(crate) aabb_min: [f32; 4],
    pub(crate) aabb_max: [f32; 4],
    pub(crate) batch_word_offset: u32,
    pub(crate) solid_batch_count: u32,
    pub(crate) cutout_batch_count: u32,
    pub(crate) fluid_batch_count: u32,
    pub(crate) origin: [i32; 3],
    pub(crate) _pad: u32,
}

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
pub(crate) struct DrawCommand {
    pub(crate) vertex_count: u32,
    pub(crate) instance_count: u32,
    pub(crate) first_vertex: u32,
    pub(crate) first_instance: u32,
}

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
pub(crate) struct FrustumData {
    pub(crate) planes: [[f32; 4]; 6],
    pub(crate) prev_view_proj: [[f32; 4]; 4],
    pub(crate) chunk_count: u32,
    pub(crate) cam_block: [i32; 3],
    pub(crate) frac: [f32; 3],
    pub(crate) prev_cam_block: [i32; 3],
    pub(crate) prev_frac: [f32; 3],
    pub(crate) occlusion_valid: u32,
    pub(crate) player_chunk: [i32; 2],
    pub(crate) limit_rd: u32,
    pub(crate) _pad: [u32; 3],
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
