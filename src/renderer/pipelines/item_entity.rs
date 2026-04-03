use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use ash::vk;
use glam::Mat4;
use gpu_allocator::vulkan::{Allocation, Allocator};

use crate::renderer::camera::CameraUniform;
use std::path::Path;

use crate::assets::{AssetIndex, resolve_asset_path};
use crate::renderer::MAX_FRAMES_IN_FLIGHT;
use crate::renderer::chunk::atlas::{AtlasRegion, AtlasUVMap, TextureAtlas};
use crate::renderer::chunk::mesher::ChunkVertex;
use crate::renderer::shader;
use crate::renderer::util;
use crate::world::block::model::BakedModel;

const VERTEX_SIZE: usize = std::mem::size_of::<ChunkVertex>();

pub struct ItemRenderInfo {
    pub item_name: String,
    pub model_matrix: Mat4,
    pub light: f32,
}

struct MeshEntry {
    buffer: vk::Buffer,
    allocation: Allocation,
    vertex_count: u32,
}

pub struct ItemEntityPipeline {
    pipeline: vk::Pipeline,
    pipeline_layout: vk::PipelineLayout,
    camera_layout: vk::DescriptorSetLayout,
    atlas_layout: vk::DescriptorSetLayout,
    descriptor_pool: vk::DescriptorPool,
    camera_sets: Vec<vk::DescriptorSet>,
    atlas_set: vk::DescriptorSet,
    camera_buffers: Vec<vk::Buffer>,
    camera_allocations: Vec<Option<Allocation>>,
    meshes: HashMap<String, MeshEntry>,
}

impl ItemEntityPipeline {
    pub fn new(
        device: &ash::Device,
        render_pass: vk::RenderPass,
        allocator: &Arc<Mutex<Allocator>>,
        atlas: &TextureAtlas,
    ) -> Self {
        let camera_layout = util::create_descriptor_set_layout(
            device,
            vk::DescriptorType::UNIFORM_BUFFER,
            vk::ShaderStageFlags::VERTEX,
        );
        let atlas_layout = util::create_descriptor_set_layout(
            device,
            vk::DescriptorType::COMBINED_IMAGE_SAMPLER,
            vk::ShaderStageFlags::FRAGMENT,
        );

        let push_range = [vk::PushConstantRange {
            stage_flags: vk::ShaderStageFlags::VERTEX | vk::ShaderStageFlags::FRAGMENT,
            offset: 0,
            size: 68,
        }];
        let layouts = [camera_layout, atlas_layout];
        let layout_info = vk::PipelineLayoutCreateInfo::default()
            .set_layouts(&layouts)
            .push_constant_ranges(&push_range);
        let pipeline_layout = unsafe { device.create_pipeline_layout(&layout_info, None) }
            .expect("failed to create item entity pipeline layout");

        let pipeline = create_pipeline(device, render_pass, pipeline_layout);

        let pool_sizes = [
            vk::DescriptorPoolSize {
                ty: vk::DescriptorType::UNIFORM_BUFFER,
                descriptor_count: MAX_FRAMES_IN_FLIGHT as u32,
            },
            vk::DescriptorPoolSize {
                ty: vk::DescriptorType::COMBINED_IMAGE_SAMPLER,
                descriptor_count: 1,
            },
        ];
        let pool_info = vk::DescriptorPoolCreateInfo::default()
            .max_sets((MAX_FRAMES_IN_FLIGHT + 1) as u32)
            .pool_sizes(&pool_sizes);
        let descriptor_pool = unsafe { device.create_descriptor_pool(&pool_info, None) }
            .expect("failed to create item entity descriptor pool");

        let camera_layouts: Vec<_> = (0..MAX_FRAMES_IN_FLIGHT).map(|_| camera_layout).collect();
        let camera_alloc_info = vk::DescriptorSetAllocateInfo::default()
            .descriptor_pool(descriptor_pool)
            .set_layouts(&camera_layouts);
        let camera_sets = unsafe { device.allocate_descriptor_sets(&camera_alloc_info) }
            .expect("failed to allocate item entity camera sets");

        let atlas_layouts = [atlas_layout];
        let atlas_alloc_info = vk::DescriptorSetAllocateInfo::default()
            .descriptor_pool(descriptor_pool)
            .set_layouts(&atlas_layouts);
        let atlas_set = unsafe { device.allocate_descriptor_sets(&atlas_alloc_info) }
            .expect("failed to allocate item entity atlas set")[0];

        let mut camera_buffers = Vec::with_capacity(MAX_FRAMES_IN_FLIGHT);
        let mut camera_allocations: Vec<Option<Allocation>> =
            Vec::with_capacity(MAX_FRAMES_IN_FLIGHT);

        for &set in &camera_sets {
            let (buf, alloc) = util::create_uniform_buffer(
                device,
                allocator,
                std::mem::size_of::<CameraUniform>() as u64,
                "item_entity_camera",
            );
            let buffer_info = [vk::DescriptorBufferInfo {
                buffer: buf,
                offset: 0,
                range: std::mem::size_of::<CameraUniform>() as u64,
            }];
            let write = vk::WriteDescriptorSet::default()
                .dst_set(set)
                .dst_binding(0)
                .descriptor_type(vk::DescriptorType::UNIFORM_BUFFER)
                .buffer_info(&buffer_info);
            unsafe { device.update_descriptor_sets(&[write], &[]) };
            camera_buffers.push(buf);
            camera_allocations.push(Some(alloc));
        }

        let image_info = [vk::DescriptorImageInfo {
            sampler: atlas.sampler,
            image_view: atlas.view,
            image_layout: vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
        }];
        let atlas_write = vk::WriteDescriptorSet::default()
            .dst_set(atlas_set)
            .dst_binding(0)
            .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
            .image_info(&image_info);
        unsafe { device.update_descriptor_sets(&[atlas_write], &[]) };

        Self {
            pipeline,
            pipeline_layout,
            camera_layout,
            atlas_layout,
            descriptor_pool,
            camera_sets,
            atlas_set,
            camera_buffers,
            camera_allocations,
            meshes: HashMap::new(),
        }
    }

    pub fn update_camera(&mut self, frame: usize, uniform: &CameraUniform) {
        let bytes = bytemuck::bytes_of(uniform);
        if let Some(alloc) = self.camera_allocations[frame].as_mut() {
            alloc.mapped_slice_mut().unwrap()[..bytes.len()].copy_from_slice(bytes);
        }
    }

    fn insert_mesh(
        &mut self,
        device: &ash::Device,
        allocator: &Arc<Mutex<Allocator>>,
        name: &str,
        vertices: &[ChunkVertex],
    ) {
        let bytes = bytemuck::cast_slice(vertices);
        let (buffer, allocation) = util::create_mapped_buffer(
            device,
            allocator,
            bytes,
            vk::BufferUsageFlags::VERTEX_BUFFER,
            &format!("item_{name}"),
        );
        self.meshes.insert(
            name.to_string(),
            MeshEntry {
                buffer,
                allocation,
                vertex_count: vertices.len() as u32,
            },
        );
    }

    pub fn has_mesh(&self, name: &str) -> bool {
        self.meshes.contains_key(name)
    }

    pub fn ensure_mesh(
        &mut self,
        device: &ash::Device,
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
            self.insert_mesh(device, allocator, name, &vertices);
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn ensure_flat_mesh(
        &mut self,
        device: &ash::Device,
        allocator: &Arc<Mutex<Allocator>>,
        name: &str,
        uv_map: &AtlasUVMap,
        assets_dir: &Path,
        asset_index: &Option<AssetIndex>,
    ) {
        if self.meshes.contains_key(name) {
            return;
        }
        let tex_key = format!("minecraft:textures/item/{name}.png");
        if !uv_map.has_region(&tex_key) {
            return;
        }
        let region = uv_map.get_region(&tex_key);
        let asset_path = format!("minecraft/textures/item/{name}.png");
        let path = resolve_asset_path(assets_dir, asset_index, &asset_path);
        let vertices = match crate::assets::load_image(&path) {
            Ok(img) => {
                let rgba = img.to_rgba8();
                build_extruded_item(&rgba, region)
            }
            Err(_) => build_flat_quad(region),
        };
        if !vertices.is_empty() {
            self.insert_mesh(device, allocator, name, &vertices);
        }
    }

    pub fn draw(
        &self,
        device: &ash::Device,
        cmd: vk::CommandBuffer,
        frame: usize,
        items: &[ItemRenderInfo],
    ) {
        if items.is_empty() {
            return;
        }

        unsafe {
            device.cmd_bind_pipeline(cmd, vk::PipelineBindPoint::GRAPHICS, self.pipeline);
            device.cmd_bind_descriptor_sets(
                cmd,
                vk::PipelineBindPoint::GRAPHICS,
                self.pipeline_layout,
                0,
                &[self.camera_sets[frame], self.atlas_set],
                &[],
            );
        }

        for item in items {
            let mesh = match self.meshes.get(&item.item_name) {
                Some(m) => m,
                None => continue,
            };

            let mvp_data = item.model_matrix.to_cols_array();
            let mvp_bytes = bytemuck::bytes_of(&mvp_data);
            let light_bytes = bytemuck::bytes_of(&item.light);

            unsafe {
                device.cmd_bind_vertex_buffers(cmd, 0, &[mesh.buffer], &[0]);
                device.cmd_push_constants(
                    cmd,
                    self.pipeline_layout,
                    vk::ShaderStageFlags::VERTEX | vk::ShaderStageFlags::FRAGMENT,
                    0,
                    mvp_bytes,
                );
                device.cmd_push_constants(
                    cmd,
                    self.pipeline_layout,
                    vk::ShaderStageFlags::VERTEX | vk::ShaderStageFlags::FRAGMENT,
                    64,
                    light_bytes,
                );
                device.cmd_draw(cmd, mesh.vertex_count, 1, 0, 0);
            }
        }
    }

    pub fn recreate_pipeline(&mut self, device: &ash::Device, render_pass: vk::RenderPass) {
        unsafe { device.destroy_pipeline(self.pipeline, None) };
        self.pipeline = create_pipeline(device, render_pass, self.pipeline_layout);
    }

    pub fn destroy(&mut self, device: &ash::Device, allocator: &Arc<Mutex<Allocator>>) {
        for (_, entry) in self.meshes.drain() {
            unsafe { device.destroy_buffer(entry.buffer, None) };
            allocator.lock().unwrap().free(entry.allocation).ok();
        }
        for i in 0..MAX_FRAMES_IN_FLIGHT {
            unsafe { device.destroy_buffer(self.camera_buffers[i], None) };
            if let Some(alloc) = self.camera_allocations[i].take() {
                allocator.lock().unwrap().free(alloc).ok();
            }
        }
        unsafe {
            device.destroy_pipeline(self.pipeline, None);
            device.destroy_pipeline_layout(self.pipeline_layout, None);
            device.destroy_descriptor_pool(self.descriptor_pool, None);
            device.destroy_descriptor_set_layout(self.camera_layout, None);
            device.destroy_descriptor_set_layout(self.atlas_layout, None);
        }
    }
}

fn build_item_mesh(model: &BakedModel, uv_map: &AtlasUVMap) -> Vec<ChunkVertex> {
    let mut vertices = Vec::new();
    for quad in &model.quads {
        let region = uv_map.get_region(&quad.texture);
        let u_span = region.u_max - region.u_min;
        let v_span = region.v_max - region.v_min;
        let tint = if matches!(quad.tint, crate::world::block::registry::Tint::None) {
            [1.0, 1.0, 1.0]
        } else {
            [0.569, 0.741, 0.349]
        };

        for i in [0, 1, 2, 2, 3, 0] {
            let p = quad.positions[i];
            vertices.push(ChunkVertex {
                position: [p[0] - 0.5, p[1] - 0.5, p[2] - 0.5],
                tex_coords: [
                    region.u_min + quad.uvs[i][0] * u_span,
                    region.v_min + quad.uvs[i][1] * v_span,
                ],
                light: 1.0,
                tint,
            });
        }
    }
    vertices
}

fn build_extruded_item(img: &image::RgbaImage, region: AtlasRegion) -> Vec<ChunkVertex> {
    let w = img.width() as i32;
    let h = img.height() as i32;
    let mut vertices = Vec::new();

    let px = 1.0 / w as f32;
    let py = 1.0 / h as f32;
    let u_span = region.u_max - region.u_min;
    let v_span = region.v_max - region.v_min;
    let z_min = 7.5 / 16.0 - 0.5;
    let z_max = 8.5 / 16.0 - 0.5;

    let is_opaque = |x: i32, y: i32| -> bool {
        x >= 0 && y >= 0 && x < w && y < h && img.get_pixel(x as u32, y as u32)[3] > 0
    };

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
        vertices.push(ChunkVertex {
            position: front[i],
            tex_coords: front_uvs[i],
            light: 1.0,
            tint: [1.0, 1.0, 1.0],
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
    let back_uvs = [
        [region.u_min, region.v_max],
        [region.u_max, region.v_max],
        [region.u_max, region.v_min],
        [region.u_min, region.v_max],
        [region.u_max, region.v_min],
        [region.u_min, region.v_min],
    ];
    for i in 0..6 {
        vertices.push(ChunkVertex {
            position: back[i],
            tex_coords: back_uvs[i],
            light: 1.0,
            tint: [1.0, 1.0, 1.0],
        });
    }

    for y in 0..h {
        for x in 0..w {
            if !is_opaque(x, y) {
                continue;
            }
            let fx = x as f32 * px - 0.5;
            let fy = 0.5 - (y + 1) as f32 * py;
            let fx1 = fx + px;
            let fy1 = fy + py;
            let u0 = region.u_min + x as f32 * px * u_span;
            let u1 = region.u_min + (x + 1) as f32 * px * u_span;
            let v0 = region.v_min + y as f32 * py * v_span;
            let v1 = region.v_min + (y + 1) as f32 * py * v_span;
            let um = (u0 + u1) * 0.5;
            let vm = (v0 + v1) * 0.5;

            if !is_opaque(x, y - 1) {
                push_side_quad(&mut vertices, fx, fy1, fx1, fy1, z_min, z_max, um, vm, 0.8);
            }
            if !is_opaque(x, y + 1) {
                push_side_quad(&mut vertices, fx1, fy, fx, fy, z_min, z_max, um, vm, 0.8);
            }
            if !is_opaque(x - 1, y) {
                push_side_quad(&mut vertices, fx, fy, fx, fy1, z_min, z_max, um, vm, 0.8);
            }
            if !is_opaque(x + 1, y) {
                push_side_quad(&mut vertices, fx1, fy1, fx1, fy, z_min, z_max, um, vm, 0.8);
            }
        }
    }

    vertices
}

#[allow(clippy::too_many_arguments)]
fn push_side_quad(
    vertices: &mut Vec<ChunkVertex>,
    x0: f32,
    y0: f32,
    x1: f32,
    y1: f32,
    z0: f32,
    z1: f32,
    u: f32,
    v: f32,
    light: f32,
) {
    let positions = [
        [x0, y0, z0],
        [x1, y1, z0],
        [x1, y1, z1],
        [x0, y0, z0],
        [x1, y1, z1],
        [x0, y0, z1],
    ];
    for p in &positions {
        vertices.push(ChunkVertex {
            position: *p,
            tex_coords: [u, v],
            light,
            tint: [1.0, 1.0, 1.0],
        });
    }
}

fn build_flat_quad(region: AtlasRegion) -> Vec<ChunkVertex> {
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
        .map(|(p, uv)| ChunkVertex {
            position: *p,
            tex_coords: *uv,
            light: 1.0,
            tint: [1.0, 1.0, 1.0],
        })
        .collect()
}

fn create_pipeline(
    device: &ash::Device,
    render_pass: vk::RenderPass,
    layout: vk::PipelineLayout,
) -> vk::Pipeline {
    let vert_spv = shader::include_spirv!("item_entity.vert.spv");
    let frag_spv = shader::include_spirv!("item_entity.frag.spv");
    let vert_mod = shader::create_shader_module(device, vert_spv);
    let frag_mod = shader::create_shader_module(device, frag_spv);

    let stages = [
        vk::PipelineShaderStageCreateInfo::default()
            .stage(vk::ShaderStageFlags::VERTEX)
            .module(vert_mod)
            .name(c"main"),
        vk::PipelineShaderStageCreateInfo::default()
            .stage(vk::ShaderStageFlags::FRAGMENT)
            .module(frag_mod)
            .name(c"main"),
    ];

    let binding = [vk::VertexInputBindingDescription {
        binding: 0,
        stride: VERTEX_SIZE as u32,
        input_rate: vk::VertexInputRate::VERTEX,
    }];
    let attrs = [
        vk::VertexInputAttributeDescription {
            location: 0,
            binding: 0,
            format: vk::Format::R32G32B32_SFLOAT,
            offset: 0,
        },
        vk::VertexInputAttributeDescription {
            location: 1,
            binding: 0,
            format: vk::Format::R32G32_SFLOAT,
            offset: 12,
        },
        vk::VertexInputAttributeDescription {
            location: 2,
            binding: 0,
            format: vk::Format::R32_SFLOAT,
            offset: 20,
        },
        vk::VertexInputAttributeDescription {
            location: 3,
            binding: 0,
            format: vk::Format::R32G32B32_SFLOAT,
            offset: 24,
        },
    ];

    let vertex_input = vk::PipelineVertexInputStateCreateInfo::default()
        .vertex_binding_descriptions(&binding)
        .vertex_attribute_descriptions(&attrs);
    let input_assembly = vk::PipelineInputAssemblyStateCreateInfo::default()
        .topology(vk::PrimitiveTopology::TRIANGLE_LIST);
    let viewport_state = vk::PipelineViewportStateCreateInfo::default()
        .viewport_count(1)
        .scissor_count(1);
    let rasterizer = vk::PipelineRasterizationStateCreateInfo::default()
        .polygon_mode(vk::PolygonMode::FILL)
        .cull_mode(vk::CullModeFlags::NONE)
        .front_face(vk::FrontFace::COUNTER_CLOCKWISE)
        .line_width(1.0);
    let multisampling = vk::PipelineMultisampleStateCreateInfo::default()
        .rasterization_samples(vk::SampleCountFlags::TYPE_1);
    let depth_stencil = vk::PipelineDepthStencilStateCreateInfo::default()
        .depth_test_enable(true)
        .depth_write_enable(true)
        .depth_compare_op(vk::CompareOp::LESS);
    let blend_attachment = [vk::PipelineColorBlendAttachmentState {
        blend_enable: vk::TRUE,
        src_color_blend_factor: vk::BlendFactor::SRC_ALPHA,
        dst_color_blend_factor: vk::BlendFactor::ONE_MINUS_SRC_ALPHA,
        color_blend_op: vk::BlendOp::ADD,
        src_alpha_blend_factor: vk::BlendFactor::ONE,
        dst_alpha_blend_factor: vk::BlendFactor::ZERO,
        alpha_blend_op: vk::BlendOp::ADD,
        color_write_mask: vk::ColorComponentFlags::RGBA,
    }];
    let color_blending =
        vk::PipelineColorBlendStateCreateInfo::default().attachments(&blend_attachment);
    let dynamic_states = [vk::DynamicState::VIEWPORT, vk::DynamicState::SCISSOR];
    let dynamic_state =
        vk::PipelineDynamicStateCreateInfo::default().dynamic_states(&dynamic_states);

    let info = [vk::GraphicsPipelineCreateInfo::default()
        .stages(&stages)
        .vertex_input_state(&vertex_input)
        .input_assembly_state(&input_assembly)
        .viewport_state(&viewport_state)
        .rasterization_state(&rasterizer)
        .multisample_state(&multisampling)
        .depth_stencil_state(&depth_stencil)
        .color_blend_state(&color_blending)
        .dynamic_state(&dynamic_state)
        .layout(layout)
        .render_pass(render_pass)
        .subpass(0)];

    let pipeline =
        unsafe { device.create_graphics_pipelines(vk::PipelineCache::null(), &info, None) }
            .expect("failed to create item entity pipeline")[0];

    unsafe {
        device.destroy_shader_module(vert_mod, None);
        device.destroy_shader_module(frag_mod, None);
    }
    pipeline
}
