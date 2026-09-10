use std::collections::HashMap;
use std::slice;
use std::sync::{Arc, Mutex};

use glam::Mat4;
use pomme_gpu_allocator::vulkan::{Allocation, Allocator};
use pyronyx::vk;

use crate::renderer::camera::CameraUniform;
use crate::renderer::chunk::atlas::{AtlasRegion, AtlasUVMap, SpriteAlphaMask, TextureAtlas};
use crate::renderer::{MAX_FRAMES_IN_FLIGHT, shader, util};
use crate::world::block::model::BakedModel;

/// Item-only vertex format. Vanilla's ENTITY item format keeps UV0 as floats
/// and carries a baked face normal; both matter here because generated sprite
/// texel boundaries must line up with extrusion geometry and dropped items use
/// the normal for two-direction diffuse lighting.
#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct ItemVertex {
    position: [f32; 3],
    tex_coords: [f32; 2],
    light_tint: u32,
    normal: [i8; 4],
}

impl ItemVertex {
    const STRIDE: u32 = size_of::<Self>() as u32;

    fn binding_description() -> vk::VertexInputBindingDescription {
        vk::VertexInputBindingDescription {
            binding: 0,
            stride: Self::STRIDE,
            input_rate: vk::VertexInputRate::Vertex,
        }
    }

    fn attribute_descriptions() -> [vk::VertexInputAttributeDescription; 4] {
        [
            vk::VertexInputAttributeDescription {
                location: 0,
                binding: 0,
                format: vk::Format::R32G32B32Sfloat,
                offset: 0,
            },
            vk::VertexInputAttributeDescription {
                location: 1,
                binding: 0,
                format: vk::Format::R32G32Sfloat,
                offset: 12,
            },
            vk::VertexInputAttributeDescription {
                location: 2,
                binding: 0,
                format: vk::Format::R8G8B8A8Unorm,
                offset: 20,
            },
            vk::VertexInputAttributeDescription {
                location: 3,
                binding: 0,
                format: vk::Format::R8G8B8A8Snorm,
                offset: 24,
            },
        ]
    }
}

fn pack_normal(normal: glam::Vec3) -> [i8; 4] {
    let normal = normal.normalize_or_zero();
    [
        (normal.x.clamp(-1.0, 1.0) * 127.0).round() as i8,
        (normal.y.clamp(-1.0, 1.0) * 127.0).round() as i8,
        (normal.z.clamp(-1.0, 1.0) * 127.0).round() as i8,
        0,
    ]
}

pub struct ItemRenderInfo {
    pub item_name: String,
    pub model_matrix: Mat4,
    pub light: f32,
    pub nether_lighting: bool,
}

/// What the item-entity renderer needs to place a mesh: whether it baked from a
/// 3D (block) model, plus its local-space bounding box used for the hover
/// height and the 3D-vs-flat copy layout (vanilla reads these off
/// `getModelBoundingBox`).
#[derive(Clone, Copy)]
pub struct ItemMeshInfo {
    pub is_block_model: bool,
    pub bounds_min: glam::Vec3,
    pub bounds_max: glam::Vec3,
}

struct MeshEntry {
    buffer: vk::Buffer,
    allocation: Allocation,
    vertex_count: u32,
    is_3d_model: bool,
    bounds_min: glam::Vec3,
    bounds_max: glam::Vec3,
}

/// Descriptor layouts, per-frame camera UBOs, and atlas set shared by the
/// pipelines that draw item meshes with the item_entity shaders.
pub(super) struct ItemPipelineShared {
    pub pipeline_layout: vk::PipelineLayout,
    camera_layout: vk::DescriptorSetLayout,
    atlas_layout: vk::DescriptorSetLayout,
    descriptor_pool: vk::DescriptorPool,
    camera_sets: Vec<vk::DescriptorSet>,
    atlas_set: vk::DescriptorSet,
    atlas_sampler: vk::Sampler,
    camera_buffers: Vec<vk::Buffer>,
    camera_allocations: Vec<Option<Allocation>>,
}

fn write_atlas_descriptor(
    device: &vk::Device,
    set: vk::DescriptorSet,
    atlas: &TextureAtlas,
    sampler: vk::Sampler,
) {
    let image_info = vk::DescriptorImageInfo {
        sampler,
        image_view: atlas.view,
        image_layout: vk::ImageLayout::ShaderReadOnlyOptimal,
    };
    let write = vk::WriteDescriptorSet {
        dst_set: set,
        dst_binding: 0,
        descriptor_type: vk::DescriptorType::CombinedImageSampler,
        descriptor_count: 1,
        image_info: &image_info,
        ..Default::default()
    };
    device.update_descriptor_sets(&[write], &[]);
}

impl ItemPipelineShared {
    pub(super) fn new(
        device: &vk::Device,
        allocator: &Arc<Mutex<Allocator>>,
        atlas: &TextureAtlas,
        label: &str,
    ) -> Self {
        let camera_layout = util::create_descriptor_set_layout(
            device,
            vk::DescriptorType::UniformBuffer,
            vk::ShaderStageFlags::Vertex,
        );
        let atlas_layout = util::create_descriptor_set_layout(
            device,
            vk::DescriptorType::CombinedImageSampler,
            vk::ShaderStageFlags::Fragment,
        );

        let push_range = vk::PushConstantRange {
            stage_flags: vk::ShaderStageFlags::Vertex | vk::ShaderStageFlags::Fragment,
            offset: 0,
            // 64-byte model + fragment light + world-light selector + padded
            // mat3 normal matrix. Vulkan guarantees at least 128 push bytes.
            size: 128,
        };
        let layouts = [camera_layout, atlas_layout];
        let layout_info = vk::PipelineLayoutCreateInfo {
            set_layout_count: layouts.len() as u32,
            set_layouts: layouts.as_ptr(),
            push_constant_range_count: 1,
            push_constant_ranges: &push_range,
            ..Default::default()
        };
        let pipeline_layout = device
            .create_pipeline_layout(&layout_info, None)
            .unwrap_or_else(|_| panic!("failed to create {label} pipeline layout"));

        let pool_sizes = [
            vk::DescriptorPoolSize {
                ty: vk::DescriptorType::UniformBuffer,
                descriptor_count: MAX_FRAMES_IN_FLIGHT as u32,
            },
            vk::DescriptorPoolSize {
                ty: vk::DescriptorType::CombinedImageSampler,
                descriptor_count: 1,
            },
        ];
        let pool_info = vk::DescriptorPoolCreateInfo {
            max_sets: MAX_FRAMES_IN_FLIGHT as u32 + 1,
            pool_size_count: pool_sizes.len() as u32,
            pool_sizes: pool_sizes.as_ptr(),
            ..Default::default()
        };
        let descriptor_pool = device
            .create_descriptor_pool(&pool_info, None)
            .unwrap_or_else(|_| panic!("failed to create {label} descriptor pool"));

        let camera_layouts: Vec<_> = (0..MAX_FRAMES_IN_FLIGHT).map(|_| camera_layout).collect();
        let camera_alloc_info = vk::DescriptorSetAllocateInfo {
            descriptor_pool,
            descriptor_set_count: camera_layouts.len() as u32,
            set_layouts: camera_layouts.as_ptr(),
            ..Default::default()
        };
        let mut camera_sets = vec![vk::DescriptorSet::null(); camera_layouts.len()];
        device
            .allocate_descriptor_sets(&camera_alloc_info, &mut camera_sets)
            .unwrap_or_else(|_| panic!("failed to allocate {label} camera sets"));

        let atlas_alloc_info = vk::DescriptorSetAllocateInfo {
            descriptor_pool,
            descriptor_set_count: 1,
            set_layouts: &atlas_layout,
            ..Default::default()
        };
        let mut atlas_set = vk::DescriptorSet::null();
        device
            .allocate_descriptor_sets(&atlas_alloc_info, slice::from_mut(&mut atlas_set))
            .unwrap_or_else(|_| panic!("failed to allocate {label} atlas set"));
        // Vanilla ordinary item rendering samples atlas level 0 even when a
        // model's sprite physically belongs to the mipmapped blocks atlas.
        let atlas_sampler = unsafe { util::create_nearest_sampler(device) };

        let mut camera_buffers = Vec::with_capacity(MAX_FRAMES_IN_FLIGHT);
        let mut camera_allocations: Vec<Option<Allocation>> =
            Vec::with_capacity(MAX_FRAMES_IN_FLIGHT);

        for &set in &camera_sets {
            let (buf, alloc) = util::create_uniform_buffer(
                device,
                allocator,
                size_of::<CameraUniform>() as u64,
                &format!("{label}_camera"),
            );
            let buffer_info = vk::DescriptorBufferInfo {
                buffer: buf,
                offset: 0,
                range: size_of::<CameraUniform>() as u64,
            };
            let write = vk::WriteDescriptorSet {
                dst_set: set,
                dst_binding: 0,
                descriptor_type: vk::DescriptorType::UniformBuffer,
                descriptor_count: 1,
                buffer_info: &buffer_info,
                ..Default::default()
            };
            device.update_descriptor_sets(&[write], &[]);
            camera_buffers.push(buf);
            camera_allocations.push(Some(alloc));
        }

        let this = Self {
            pipeline_layout,
            camera_layout,
            atlas_layout,
            descriptor_pool,
            camera_sets,
            atlas_set,
            atlas_sampler,
            camera_buffers,
            camera_allocations,
        };
        this.rebind_atlas(device, atlas);
        this
    }

    pub(super) fn rebind_atlas(&self, device: &vk::Device, atlas: &TextureAtlas) {
        write_atlas_descriptor(device, self.atlas_set, atlas, self.atlas_sampler);
    }

    pub(super) fn update_camera(&mut self, frame: usize, uniform: &CameraUniform) {
        let bytes = bytemuck::bytes_of(uniform);
        if let Some(alloc) = self.camera_allocations[frame].as_mut() {
            alloc.mapped_slice_mut().unwrap()[..bytes.len()].copy_from_slice(bytes);
        }
    }

    pub(super) fn bind(&self, cmd: vk::CommandBuffer, frame: usize, pipeline: vk::Pipeline) {
        cmd.bind_pipeline(vk::PipelineBindPoint::Graphics, pipeline);
        cmd.bind_descriptor_sets(
            vk::PipelineBindPoint::Graphics,
            self.pipeline_layout,
            0,
            &[self.camera_sets[frame], self.atlas_set],
            &[],
        );
    }

    pub(super) fn destroy(&mut self, device: &vk::Device, allocator: &Arc<Mutex<Allocator>>) {
        for i in 0..MAX_FRAMES_IN_FLIGHT {
            device.destroy_buffer(self.camera_buffers[i], None);
            if let Some(alloc) = self.camera_allocations[i].take() {
                allocator.lock().unwrap().free(alloc).ok();
            }
        }

        device.destroy_sampler(self.atlas_sampler, None);
        device.destroy_pipeline_layout(self.pipeline_layout, None);
        device.destroy_descriptor_pool(self.descriptor_pool, None);
        device.destroy_descriptor_set_layout(self.camera_layout, None);
        device.destroy_descriptor_set_layout(self.atlas_layout, None);
    }
}

pub(super) fn push_model_light(
    cmd: vk::CommandBuffer,
    layout: vk::PipelineLayout,
    model: &Mat4,
    light: f32,
) {
    let mvp_data = model.to_cols_array();
    cmd.push_constants(
        layout,
        vk::ShaderStageFlags::Vertex | vk::ShaderStageFlags::Fragment,
        0,
        bytemuck::bytes_of(&mvp_data),
    );
    cmd.push_constants(
        layout,
        vk::ShaderStageFlags::Vertex | vk::ShaderStageFlags::Fragment,
        64,
        bytemuck::bytes_of(&light),
    );
}

fn push_world_lighting(
    cmd: vk::CommandBuffer,
    layout: vk::PipelineLayout,
    model: &Mat4,
    nether: bool,
) {
    let nether = if nether { 1.0_f32 } else { 0.0_f32 };
    cmd.push_constants(
        layout,
        vk::ShaderStageFlags::Vertex,
        68,
        bytemuck::bytes_of(&nether),
    );

    let normal = glam::Mat3::from_mat4(*model).inverse().transpose();
    let cols = normal.to_cols_array();
    let padded_cols = [
        cols[0], cols[1], cols[2], 0.0, cols[3], cols[4], cols[5], 0.0, cols[6], cols[7], cols[8],
        0.0,
    ];
    cmd.push_constants(
        layout,
        vk::ShaderStageFlags::Vertex,
        80,
        bytemuck::bytes_of(&padded_cols),
    );
}

pub struct ItemEntityPipeline {
    pipeline: vk::Pipeline,
    shared: ItemPipelineShared,
    meshes: HashMap<String, MeshEntry>,
}

impl ItemEntityPipeline {
    pub fn new(
        device: &vk::Device,
        render_pass: vk::RenderPass,
        allocator: &Arc<Mutex<Allocator>>,
        atlas: &TextureAtlas,
    ) -> Self {
        let shared = ItemPipelineShared::new(device, allocator, atlas, "item_entity");
        let pipeline = create_world_pipeline(device, render_pass, shared.pipeline_layout);

        Self {
            pipeline,
            shared,
            meshes: HashMap::new(),
        }
    }

    pub fn update_camera(&mut self, frame: usize, uniform: &CameraUniform) {
        self.shared.update_camera(frame, uniform);
    }

    fn insert_mesh(
        &mut self,
        device: &vk::Device,
        allocator: &Arc<Mutex<Allocator>>,
        name: &str,
        vertices: &[ItemVertex],
        is_3d_model: bool,
    ) {
        let bytes = bytemuck::cast_slice(vertices);
        let (buffer, allocation) = util::create_mapped_buffer(
            device,
            allocator,
            bytes,
            vk::BufferUsageFlags::VertexBuffer,
            &format!("item_{name}"),
        );
        let (bounds_min, bounds_max) = mesh_bounds(vertices);
        self.meshes.insert(
            name.to_string(),
            MeshEntry {
                buffer,
                allocation,
                vertex_count: vertices.len() as u32,
                is_3d_model,
                bounds_min,
                bounds_max,
            },
        );
    }

    /// `None` if no mesh is built yet, else its 3D-model flag and local bounds.
    pub fn mesh_info(&self, name: &str) -> Option<ItemMeshInfo> {
        self.meshes.get(name).map(|m| ItemMeshInfo {
            is_block_model: m.is_3d_model,
            bounds_min: m.bounds_min,
            bounds_max: m.bounds_max,
        })
    }

    pub fn mesh_handle(&self, name: &str) -> Option<(vk::Buffer, u32)> {
        self.meshes.get(name).map(|m| (m.buffer, m.vertex_count))
    }

    pub fn ensure_mesh(
        &mut self,
        device: &vk::Device,
        allocator: &Arc<Mutex<Allocator>>,
        name: &str,
        model: &BakedModel,
        uv_map: &AtlasUVMap,
    ) {
        if self.meshes.contains_key(name) {
            return;
        }
        let vertices = build_item_mesh(model, uv_map);
        if !vertices.is_empty() {
            self.insert_mesh(device, allocator, name, &vertices, true);
        }
    }

    pub fn ensure_flat_mesh(
        &mut self,
        device: &vk::Device,
        allocator: &Arc<Mutex<Allocator>>,
        name: &str,
        texture_key: &str,
        uv_map: &AtlasUVMap,
    ) {
        if self.meshes.contains_key(name) {
            return;
        }
        if !uv_map.has_region(texture_key) {
            return;
        }
        let region = uv_map.get_region(texture_key);
        let vertices = uv_map
            .sprite_alpha_mask(texture_key)
            .map(|mask| build_extruded_item_mask(mask, region))
            .unwrap_or_else(|| build_flat_quad(region));
        if !vertices.is_empty() {
            self.insert_mesh(device, allocator, name, &vertices, false);
        }
    }

    pub fn draw(&self, cmd: vk::CommandBuffer, frame: usize, items: &[ItemRenderInfo]) {
        if items.is_empty() {
            return;
        }

        self.shared.bind(cmd, frame, self.pipeline);

        for item in items {
            let mesh = match self.meshes.get(&item.item_name) {
                Some(m) => m,
                None => continue,
            };
            cmd.bind_vertex_buffers(0, &[mesh.buffer], &[0]);
            push_model_light(
                cmd,
                self.shared.pipeline_layout,
                &item.model_matrix,
                item.light,
            );
            push_world_lighting(
                cmd,
                self.shared.pipeline_layout,
                &item.model_matrix,
                item.nether_lighting,
            );
            cmd.draw(mesh.vertex_count, 1, 0, 0);
        }
    }

    pub fn recreate_pipeline(&mut self, device: &vk::Device, render_pass: vk::RenderPass) {
        device.destroy_pipeline(self.pipeline, None);
        self.pipeline = create_world_pipeline(device, render_pass, self.shared.pipeline_layout);
    }

    pub fn rebind_atlas(&self, device: &vk::Device, atlas: &TextureAtlas) {
        self.shared.rebind_atlas(device, atlas);
    }

    pub fn clear_meshes(&mut self, device: &vk::Device, allocator: &Arc<Mutex<Allocator>>) {
        for (_, entry) in self.meshes.drain() {
            device.destroy_buffer(entry.buffer, None);
            allocator.lock().unwrap().free(entry.allocation).ok();
        }
    }

    pub fn destroy(&mut self, device: &vk::Device, allocator: &Arc<Mutex<Allocator>>) {
        self.clear_meshes(device, allocator);
        device.destroy_pipeline(self.pipeline, None);
        self.shared.destroy(device, allocator);
    }
}

/// Local-space bounds of a baked item mesh before its display transform.
/// Empty meshes report a degenerate box at the origin.
fn mesh_bounds(vertices: &[ItemVertex]) -> (glam::Vec3, glam::Vec3) {
    if vertices.is_empty() {
        return (glam::Vec3::ZERO, glam::Vec3::ZERO);
    }

    let mut min = glam::Vec3::splat(f32::INFINITY);
    let mut max = glam::Vec3::splat(f32::NEG_INFINITY);
    for vertex in vertices {
        let position = glam::Vec3::from_array(vertex.position);
        min = min.min(position);
        max = max.max(position);
    }
    (min, max)
}

fn quad_normal(positions: &[[f32; 3]; 4]) -> glam::Vec3 {
    let p0 = glam::Vec3::from_array(positions[0]);
    let p1 = glam::Vec3::from_array(positions[1]);
    let p2 = glam::Vec3::from_array(positions[2]);
    (p1 - p0).cross(p2 - p0).normalize_or_zero()
}

/// Vanilla stores a cardinal `BakedQuad.direction()`, even when an element
/// rotation makes the geometric face normal non-cardinal. `FaceBakery` picks
/// the cardinal direction with the largest positive dot product, in
/// Direction.values() order: DOWN, UP, NORTH, SOUTH, WEST, EAST.
fn baked_quad_normal(positions: &[[f32; 3]; 4]) -> glam::Vec3 {
    let geometric = quad_normal(positions);
    let directions = [
        glam::Vec3::NEG_Y,
        glam::Vec3::Y,
        glam::Vec3::NEG_Z,
        glam::Vec3::Z,
        glam::Vec3::NEG_X,
        glam::Vec3::X,
    ];
    let mut best = glam::Vec3::Y;
    let mut best_dot = 0.0_f32;
    for direction in directions {
        let dot = geometric.dot(direction);
        if dot >= 0.0 && dot > best_dot {
            best_dot = dot;
            best = direction;
        }
    }
    best
}

fn build_item_mesh(model: &BakedModel, uv_map: &AtlasUVMap) -> Vec<ItemVertex> {
    let mut vertices = Vec::new();
    for quad in &model.quads {
        let region = uv_map.get_region(&quad.texture);
        let u_span = region.u_max - region.u_min;
        let v_span = region.v_max - region.v_min;
        let tint = if matches!(quad.tint, crate::world::block::registry::Tint::None) {
            crate::renderer::chunk::mesher::PACKED_WHITE_SHIFTED
        } else {
            crate::renderer::chunk::mesher::pack_tint_shifted([0.569, 0.741, 0.349])
        };
        let normal = pack_normal(baked_quad_normal(&quad.positions));

        for i in [0, 1, 2, 2, 3, 0] {
            let p = quad.positions[i];
            vertices.push(ItemVertex {
                position: [p[0] - 0.5, p[1] - 0.5, p[2] - 0.5],
                tex_coords: [
                    region.u_min + quad.uvs[i][0] * u_span,
                    region.v_min + quad.uvs[i][1] * v_span,
                ],
                // Held/GUI item rendering still consumes the baked shade byte.
                // The dropped-item world shader ignores it and instead uses
                // the baked cardinal normal, matching vanilla's item shader.
                light_tint: crate::renderer::chunk::mesher::pack_light_tint(quad.shade_light, tint),
                normal,
            });
        }
    }
    vertices
}

#[cfg(test)]
fn build_extruded_item(img: &image::RgbaImage, region: AtlasRegion) -> Vec<ItemVertex> {
    let w = img.width();
    let h = img.height();
    let mask = SpriteAlphaMask {
        width: w,
        height: h,
        frames: vec![img.pixels().map(|pixel| pixel[3] != 0).collect()],
    };
    build_extruded_item_mask(&mask, region)
}

fn build_extruded_item_mask(mask: &SpriteAlphaMask, region: AtlasRegion) -> Vec<ItemVertex> {
    let w = mask.width as i32;
    let h = mask.height as i32;
    let mut vertices = Vec::new();

    let px = 1.0 / w as f32;
    let py = 1.0 / h as f32;
    let u_span = region.u_max - region.u_min;
    let v_span = region.v_max - region.v_min;
    let z_min = 7.5 / 16.0 - 0.5;
    let z_max = 8.5 / 16.0 - 0.5;

    let front = [
        [-0.5, -0.5, z_max],
        [0.5, -0.5, z_max],
        [0.5, 0.5, z_max],
        [-0.5, -0.5, z_max],
        [0.5, 0.5, z_max],
        [-0.5, 0.5, z_max],
    ];
    let front_uvs = [
        [region.u_min, region.v_max],
        [region.u_max, region.v_max],
        [region.u_max, region.v_min],
        [region.u_min, region.v_max],
        [region.u_max, region.v_min],
        [region.u_min, region.v_min],
    ];
    for i in 0..6 {
        vertices.push(ItemVertex {
            position: front[i],
            tex_coords: front_uvs[i],
            light_tint: crate::renderer::chunk::mesher::pack_light_tint(
                1.0,
                crate::renderer::chunk::mesher::PACKED_WHITE_SHIFTED,
            ),
            normal: pack_normal(glam::Vec3::Z),
        });
    }

    let back = [
        [0.5, -0.5, z_min],
        [-0.5, -0.5, z_min],
        [-0.5, 0.5, z_min],
        [0.5, -0.5, z_min],
        [-0.5, 0.5, z_min],
        [0.5, 0.5, z_min],
    ];
    // Vanilla's generated-item NORTH face uses UVs [16, 0, 0, 16].
    // With the north-face winding below that preserves the sprite's model-space
    // orientation: x=+0.5 samples the right side of the sprite, just like the
    // SOUTH face. Mirroring these UVs makes an asymmetric back silhouette no
    // longer line up with the generated side faces.
    let back_uvs = [
        [region.u_max, region.v_max],
        [region.u_min, region.v_max],
        [region.u_min, region.v_min],
        [region.u_max, region.v_max],
        [region.u_min, region.v_min],
        [region.u_max, region.v_min],
    ];
    for i in 0..6 {
        vertices.push(ItemVertex {
            position: back[i],
            tex_coords: back_uvs[i],
            light_tint: crate::renderer::chunk::mesher::pack_light_tint(
                1.0,
                crate::renderer::chunk::mesher::PACKED_WHITE_SHIFTED,
            ),
            normal: pack_normal(glam::Vec3::NEG_Z),
        });
    }

    for y in 0..h {
        for x in 0..w {
            let top_exposed = mask.has_exposed_edge(x, y, x, y - 1);
            let bottom_exposed = mask.has_exposed_edge(x, y, x, y + 1);
            let left_exposed = mask.has_exposed_edge(x, y, x - 1, y);
            let right_exposed = mask.has_exposed_edge(x, y, x + 1, y);
            if !(top_exposed || bottom_exposed || left_exposed || right_exposed) {
                continue;
            }
            let fx = x as f32 * px - 0.5;
            let fy = 0.5 - (y + 1) as f32 * py;
            let fx1 = fx + px;
            let fy1 = fy + py;
            // Vanilla ItemModelGenerator maps each side face from 0.1 to 0.9
            // inside its source pixel, measured across the exact sprite bounds.
            let u0 = region.u_min + (x as f32 + 0.1) * px * u_span;
            let u1 = region.u_min + (x as f32 + 0.9) * px * u_span;
            let v0 = region.v_min + (y as f32 + 0.1) * py * v_span;
            let v1 = region.v_min + (y as f32 + 0.9) * py * v_span;

            if top_exposed {
                push_side_quad(
                    &mut vertices,
                    fx,
                    fy1,
                    fx1,
                    fy1,
                    z_min,
                    z_max,
                    [[u0, v0], [u0, v1], [u1, v1], [u1, v0]],
                    0.8,
                );
            }
            if bottom_exposed {
                push_side_quad(
                    &mut vertices,
                    fx1,
                    fy,
                    fx,
                    fy,
                    z_min,
                    z_max,
                    [[u1, v1], [u1, v0], [u0, v0], [u0, v1]],
                    0.8,
                );
            }
            if left_exposed {
                push_side_quad(
                    &mut vertices,
                    fx,
                    fy,
                    fx,
                    fy1,
                    z_min,
                    z_max,
                    [[u1, v1], [u0, v1], [u0, v0], [u1, v0]],
                    0.8,
                );
            }
            if right_exposed {
                push_side_quad(
                    &mut vertices,
                    fx1,
                    fy1,
                    fx1,
                    fy,
                    z_min,
                    z_max,
                    [[u0, v0], [u1, v0], [u1, v1], [u0, v1]],
                    0.8,
                );
            }
        }
    }

    vertices
}

#[allow(clippy::too_many_arguments)]
fn push_side_quad(
    vertices: &mut Vec<ItemVertex>,
    x0: f32,
    y0: f32,
    x1: f32,
    y1: f32,
    z0: f32,
    z1: f32,
    uvs: [[f32; 2]; 4],
    light: f32,
) {
    let positions = [[x0, y0, z0], [x0, y0, z1], [x1, y1, z1], [x1, y1, z0]];
    let normal = pack_normal(quad_normal(&positions));
    for i in [0, 1, 2, 0, 2, 3] {
        vertices.push(ItemVertex {
            position: positions[i],
            tex_coords: uvs[i],
            light_tint: crate::renderer::chunk::mesher::pack_light_tint(
                light,
                crate::renderer::chunk::mesher::PACKED_WHITE_SHIFTED,
            ),
            normal,
        });
    }
}

fn build_flat_quad(region: AtlasRegion) -> Vec<ItemVertex> {
    let h = 0.5;
    let positions = [
        [-h, -h, 0.0],
        [h, -h, 0.0],
        [h, h, 0.0],
        [-h, -h, 0.0],
        [h, h, 0.0],
        [-h, h, 0.0],
    ];
    let uvs = [
        [region.u_min, region.v_max],
        [region.u_max, region.v_max],
        [region.u_max, region.v_min],
        [region.u_min, region.v_max],
        [region.u_max, region.v_min],
        [region.u_min, region.v_min],
    ];
    positions
        .iter()
        .zip(uvs.iter())
        .map(|(p, uv)| ItemVertex {
            position: *p,
            tex_coords: *uv,
            light_tint: crate::renderer::chunk::mesher::pack_light_tint(
                1.0,
                crate::renderer::chunk::mesher::PACKED_WHITE_SHIFTED,
            ),
            normal: pack_normal(glam::Vec3::Z),
        })
        .collect()
}

pub(super) fn create_pipeline(
    device: &vk::Device,
    render_pass: vk::RenderPass,
    layout: vk::PipelineLayout,
) -> vk::Pipeline {
    create_pipeline_impl(
        device,
        render_pass,
        layout,
        vk::FrontFace::CounterClockwise,
        false,
    )
}

fn create_world_pipeline(
    device: &vk::Device,
    render_pass: vk::RenderPass,
    layout: vk::PipelineLayout,
) -> vk::Pipeline {
    create_pipeline_impl(
        device,
        render_pass,
        layout,
        vk::FrontFace::CounterClockwise,
        true,
    )
}

pub(super) fn create_pipeline_with_front_face(
    device: &vk::Device,
    render_pass: vk::RenderPass,
    layout: vk::PipelineLayout,
    front_face: vk::FrontFace,
) -> vk::Pipeline {
    create_pipeline_impl(device, render_pass, layout, front_face, false)
}

fn create_pipeline_impl(
    device: &vk::Device,
    render_pass: vk::RenderPass,
    layout: vk::PipelineLayout,
    front_face: vk::FrontFace,
    world_lighting: bool,
) -> vk::Pipeline {
    let vert_spv: &[u8] = if world_lighting {
        &shader::include_spirv!("item_entity_world.vert.spv")[..]
    } else {
        &shader::include_spirv!("item_entity.vert.spv")[..]
    };
    let frag_spv = shader::include_spirv!("item_entity.frag.spv");
    let vert_mod = shader::create_shader_module(device, vert_spv);
    let frag_mod = shader::create_shader_module(device, frag_spv);

    let stages = [
        vk::PipelineShaderStageCreateInfo {
            stage: vk::ShaderStageFlags::Vertex,
            module: vert_mod,
            name: c"main".as_ptr(),
            ..Default::default()
        },
        vk::PipelineShaderStageCreateInfo {
            stage: vk::ShaderStageFlags::Fragment,
            module: frag_mod,
            name: c"main".as_ptr(),
            ..Default::default()
        },
    ];

    let binding = ItemVertex::binding_description();
    let attrs = ItemVertex::attribute_descriptions();

    let vertex_input = vk::PipelineVertexInputStateCreateInfo {
        vertex_binding_description_count: 1,
        vertex_binding_descriptions: &binding,
        vertex_attribute_description_count: attrs.len() as u32,
        vertex_attribute_descriptions: attrs.as_ptr(),
        ..Default::default()
    };
    let input_assembly = vk::PipelineInputAssemblyStateCreateInfo {
        topology: vk::PrimitiveTopology::TriangleList,
        ..Default::default()
    };
    let viewport_state = vk::PipelineViewportStateCreateInfo {
        viewport_count: 1,
        scissor_count: 1,
        ..Default::default()
    };
    let rasterizer = vk::PipelineRasterizationStateCreateInfo {
        polygon_mode: vk::PolygonMode::Fill,
        cull_mode: vk::CullModeFlags::Back,
        front_face,
        line_width: 1.0,
        ..Default::default()
    };
    let multisampling = vk::PipelineMultisampleStateCreateInfo {
        rasterization_samples: vk::SampleCountFlags::Type1,
        ..Default::default()
    };
    let depth_stencil = vk::PipelineDepthStencilStateCreateInfo {
        depth_test_enable: vk::TRUE,
        depth_write_enable: vk::TRUE,
        // Vanilla 26.2's DepthStencilState.DEFAULT is inclusive
        // GREATER_THAN_OR_EQUAL under reversed-Z. Pomme uses conventional
        // depth here, so the equivalent comparison is LESS_OR_EQUAL. The
        // equality case matters where generated front/back planes meet their
        // per-pixel extrusion walls at the exact same depth.
        depth_compare_op: vk::CompareOp::LessOrEqual,
        ..Default::default()
    };
    let blend_attachment = vk::PipelineColorBlendAttachmentState {
        blend_enable: vk::TRUE,
        src_color_blend_factor: vk::BlendFactor::SrcAlpha,
        dst_color_blend_factor: vk::BlendFactor::OneMinusSrcAlpha,
        color_blend_op: vk::BlendOp::Add,
        src_alpha_blend_factor: vk::BlendFactor::One,
        dst_alpha_blend_factor: vk::BlendFactor::Zero,
        alpha_blend_op: vk::BlendOp::Add,
        color_write_mask: vk::ColorComponentFlags::RGBA,
    };
    let color_blending = vk::PipelineColorBlendStateCreateInfo {
        attachment_count: 1,
        attachments: &blend_attachment,
        ..Default::default()
    };
    let dynamic_states = [vk::DynamicState::Viewport, vk::DynamicState::Scissor];
    let dynamic_state = vk::PipelineDynamicStateCreateInfo {
        dynamic_state_count: dynamic_states.len() as u32,
        dynamic_states: dynamic_states.as_ptr(),
        ..Default::default()
    };

    let info = [vk::GraphicsPipelineCreateInfo {
        stage_count: stages.len() as u32,
        stages: stages.as_ptr(),
        vertex_input_state: &vertex_input,
        input_assembly_state: &input_assembly,
        viewport_state: &viewport_state,
        rasterization_state: &rasterizer,
        multisample_state: &multisampling,
        depth_stencil_state: &depth_stencil,
        color_blend_state: &color_blending,
        dynamic_state: &dynamic_state,
        layout,
        render_pass,
        subpass: 0,
        ..Default::default()
    }];

    let mut pipeline = vk::Pipeline::null();
    device
        .create_graphics_pipelines(
            vk::PipelineCache::null(),
            &info,
            None,
            slice::from_mut(&mut pipeline),
        )
        .expect("failed to create item entity pipeline");

    device.destroy_shader_module(vert_mod, None);
    device.destroy_shader_module(frag_mod, None);

    pipeline
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unit_region() -> AtlasRegion {
        AtlasRegion {
            u_min: 0.0,
            v_min: 0.0,
            u_max: 1.0,
            v_max: 1.0,
            pixel_rect: [0, 0, 1, 1],
            opaque: true,
        }
    }

    fn unpack_uv(vertex: &ItemVertex) -> [f32; 2] {
        vertex.tex_coords
    }

    fn assert_uvs_near(vertices: &[ItemVertex], expected: &[[f32; 2]]) {
        assert_eq!(vertices.len(), expected.len());
        for (vertex, expected) in vertices.iter().zip(expected) {
            let actual = unpack_uv(vertex);
            assert!(
                (actual[0] - expected[0]).abs() <= f32::EPSILON
                    && (actual[1] - expected[1]).abs() <= f32::EPSILON,
                "UV {actual:?} differs from expected {expected:?}"
            );
        }
    }

    fn unpack_normal(vertex: &ItemVertex) -> glam::Vec3 {
        glam::Vec3::new(
            vertex.normal[0] as f32 / 127.0,
            vertex.normal[1] as f32 / 127.0,
            vertex.normal[2] as f32 / 127.0,
        )
    }

    #[test]
    fn extruded_item_side_faces_wind_outward() {
        let image = image::RgbaImage::from_pixel(1, 1, image::Rgba([255, 255, 255, 255]));
        let vertices = build_extruded_item(&image, unit_region());
        let expected_normals = [
            glam::Vec3::Y,
            glam::Vec3::NEG_Y,
            glam::Vec3::NEG_X,
            glam::Vec3::X,
        ];

        assert_eq!(vertices.len(), 36);
        for (face, expected) in vertices[12..].chunks_exact(6).zip(expected_normals) {
            let p0 = glam::Vec3::from_array(face[0].position);
            let p1 = glam::Vec3::from_array(face[1].position);
            let p2 = glam::Vec3::from_array(face[2].position);
            let normal = (p1 - p0).cross(p2 - p0);
            assert!(
                normal.dot(expected) > 0.0,
                "side face normal {normal:?} points away from {expected:?}"
            );
        }
    }

    #[test]
    fn extruded_item_carries_vanilla_cardinal_normals() {
        let image = image::RgbaImage::from_pixel(1, 1, image::Rgba([255, 255, 255, 255]));
        let vertices = build_extruded_item(&image, unit_region());
        let expected = [
            glam::Vec3::Z,
            glam::Vec3::NEG_Z,
            glam::Vec3::Y,
            glam::Vec3::NEG_Y,
            glam::Vec3::NEG_X,
            glam::Vec3::X,
        ];

        for (face, expected) in vertices.chunks_exact(6).zip(expected) {
            for vertex in face {
                assert_eq!(unpack_normal(vertex), expected);
            }
        }
    }

    #[test]
    fn baked_quad_normal_matches_vanilla_closest_cardinal_direction() {
        let angle = 22.5_f32.to_radians();
        let normal = glam::Vec3::new(angle.sin(), 0.0, angle.cos());
        let tangent = glam::Vec3::X * angle.cos() - glam::Vec3::Z * angle.sin();
        let up = glam::Vec3::Y;
        let p0 = -tangent * 0.5 - up * 0.5;
        let p1 = tangent * 0.5 - up * 0.5;
        let p2 = tangent * 0.5 + up * 0.5;
        let p3 = -tangent * 0.5 + up * 0.5;
        let mut positions = [p0.to_array(), p1.to_array(), p2.to_array(), p3.to_array()];

        // Ensure the synthetic winding points along the intended oblique normal.
        if quad_normal(&positions).dot(normal) < 0.0 {
            positions.reverse();
        }
        assert_eq!(baked_quad_normal(&positions), glam::Vec3::Z);
    }

    #[test]
    fn animated_extrusion_unions_exposed_edges_per_frame() {
        let mask = SpriteAlphaMask {
            width: 2,
            height: 1,
            frames: vec![vec![true, false], vec![false, true]],
        };
        let vertices = build_extruded_item_mask(&mask, unit_region());

        // Each frame exposes the shared edge from one side, so vanilla emits
        // eight side quads total. Collapsing the frames into one opacity bitmap
        // would incorrectly remove both shared-edge quads and yield 48 vertices.
        assert_eq!(vertices.len(), 12 + 8 * 6);
    }

    #[test]
    fn extruded_item_back_face_preserves_sprite_orientation() {
        let image = image::RgbaImage::from_pixel(1, 1, image::Rgba([255, 255, 255, 255]));
        let vertices = build_extruded_item(&image, unit_region());

        assert_uvs_near(
            &vertices[6..12],
            &[
                [1.0, 1.0],
                [0.0, 1.0],
                [0.0, 0.0],
                [1.0, 1.0],
                [0.0, 0.0],
                [1.0, 0.0],
            ],
        );
    }

    #[test]
    fn extruded_item_front_and_back_use_true_sprite_bounds() {
        let image = image::RgbaImage::from_pixel(1, 1, image::Rgba([255, 255, 255, 255]));
        let region = AtlasRegion {
            u_min: 16.0 / 64.0,
            v_min: 8.0 / 64.0,
            u_max: 32.0 / 64.0,
            v_max: 24.0 / 64.0,
            pixel_rect: [16, 8, 16, 16],
            opaque: true,
        };
        let vertices = build_extruded_item(&image, region);

        assert_uvs_near(
            &vertices[..12],
            &[
                [region.u_min, region.v_max],
                [region.u_max, region.v_max],
                [region.u_max, region.v_min],
                [region.u_min, region.v_max],
                [region.u_max, region.v_min],
                [region.u_min, region.v_min],
                [region.u_max, region.v_max],
                [region.u_min, region.v_max],
                [region.u_min, region.v_min],
                [region.u_max, region.v_max],
                [region.u_min, region.v_min],
                [region.u_max, region.v_min],
            ],
        );
    }

    #[test]
    fn extruded_item_side_faces_match_vanilla_pixel_inset_uvs() {
        let image = image::RgbaImage::from_pixel(1, 1, image::Rgba([255, 255, 255, 255]));
        let vertices = build_extruded_item(&image, unit_region());
        let expected = [
            // UP
            [0.1, 0.1],
            [0.1, 0.9],
            [0.9, 0.9],
            [0.1, 0.1],
            [0.9, 0.9],
            [0.9, 0.1],
            // DOWN
            [0.9, 0.9],
            [0.9, 0.1],
            [0.1, 0.1],
            [0.9, 0.9],
            [0.1, 0.1],
            [0.1, 0.9],
            // LEFT / west-facing geometry
            [0.9, 0.9],
            [0.1, 0.9],
            [0.1, 0.1],
            [0.9, 0.9],
            [0.1, 0.1],
            [0.9, 0.1],
            // RIGHT / east-facing geometry
            [0.1, 0.1],
            [0.9, 0.1],
            [0.9, 0.9],
            [0.1, 0.1],
            [0.9, 0.9],
            [0.1, 0.9],
        ];

        assert_uvs_near(&vertices[12..], &expected);
    }
}
