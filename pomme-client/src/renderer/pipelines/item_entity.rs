use std::collections::HashMap;
use std::path::Path;
use std::slice;
use std::sync::{Arc, Mutex};

use glam::Mat4;
use pomme_gpu_allocator::vulkan::{Allocation, Allocator};
use pyronyx::vk;

use crate::assets::{AssetIndex, resolve_asset_path};
use crate::renderer::camera::CameraUniform;
use crate::renderer::chunk::atlas::{AtlasRegion, AtlasUVMap, TextureAtlas, atlas_asset_path};
use crate::renderer::chunk::mesher::ChunkVertex;
use crate::renderer::{MAX_FRAMES_IN_FLIGHT, shader, util};
use crate::world::block::model::BakedModel;

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
        device: &vk::Device,
        render_pass: vk::RenderPass,
        allocator: &Arc<Mutex<Allocator>>,
        atlas: &TextureAtlas,
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
            size: 68,
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
            .expect("failed to create item entity pipeline layout");

        let pipeline = create_pipeline(device, render_pass, pipeline_layout);

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
            max_sets: (MAX_FRAMES_IN_FLIGHT + 1) as u32,
            pool_size_count: pool_sizes.len() as u32,
            pool_sizes: pool_sizes.as_ptr(),
            ..Default::default()
        };
        let descriptor_pool = device
            .create_descriptor_pool(&pool_info, None)
            .expect("failed to create item entity descriptor pool");

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
            .expect("failed to allocate item entity camera sets");

        let atlas_alloc_info = vk::DescriptorSetAllocateInfo {
            descriptor_pool,
            descriptor_set_count: 1,
            set_layouts: &atlas_layout,
            ..Default::default()
        };
        let mut atlas_set = vk::DescriptorSet::null();
        device
            .allocate_descriptor_sets(&atlas_alloc_info, slice::from_mut(&mut atlas_set))
            .expect("failed to allocate item entity atlas set");

        let mut camera_buffers = Vec::with_capacity(MAX_FRAMES_IN_FLIGHT);
        let mut camera_allocations: Vec<Option<Allocation>> =
            Vec::with_capacity(MAX_FRAMES_IN_FLIGHT);

        for &set in &camera_sets {
            let (buf, alloc) = util::create_uniform_buffer(
                device,
                allocator,
                size_of::<CameraUniform>() as u64,
                "item_entity_camera",
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

        let image_info = vk::DescriptorImageInfo {
            sampler: atlas.sampler,
            image_view: atlas.view,
            image_layout: vk::ImageLayout::ShaderReadOnlyOptimal,
        };
        let atlas_write = vk::WriteDescriptorSet {
            dst_set: atlas_set,
            dst_binding: 0,
            descriptor_type: vk::DescriptorType::CombinedImageSampler,
            descriptor_count: 1,
            image_info: &image_info,
            ..Default::default()
        };
        device.update_descriptor_sets(&[atlas_write], &[]);

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
        device: &vk::Device,
        allocator: &Arc<Mutex<Allocator>>,
        name: &str,
        vertices: &[ChunkVertex],
    ) {
        let bytes = bytemuck::cast_slice(vertices);
        let (buffer, allocation) = util::create_mapped_buffer(
            device,
            allocator,
            bytes,
            vk::BufferUsageFlags::VertexBuffer,
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
            self.insert_mesh(device, allocator, name, &vertices);
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn ensure_flat_mesh(
        &mut self,
        device: &vk::Device,
        allocator: &Arc<Mutex<Allocator>>,
        name: &str,
        texture_key: &str,
        uv_map: &AtlasUVMap,
        assets_dir: &Path,
        asset_index: &Option<AssetIndex>,
    ) {
        if self.meshes.contains_key(name) {
            return;
        }
        if !uv_map.has_region(texture_key) {
            return;
        }
        let region = uv_map.get_region(texture_key);
        let asset_path = atlas_asset_path(texture_key);
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

    pub fn draw(&self, cmd: vk::CommandBuffer, frame: usize, items: &[ItemRenderInfo]) {
        if items.is_empty() {
            return;
        }

        cmd.bind_pipeline(vk::PipelineBindPoint::Graphics, self.pipeline);
        cmd.bind_descriptor_sets(
            vk::PipelineBindPoint::Graphics,
            self.pipeline_layout,
            0,
            &[self.camera_sets[frame], self.atlas_set],
            &[],
        );

        for item in items {
            let mesh = match self.meshes.get(&item.item_name) {
                Some(m) => m,
                None => continue,
            };

            let mvp_data = item.model_matrix.to_cols_array();
            let mvp_bytes = bytemuck::bytes_of(&mvp_data);
            let light_bytes = bytemuck::bytes_of(&item.light);

            cmd.bind_vertex_buffers(0, &[mesh.buffer], &[0]);
            cmd.push_constants(
                self.pipeline_layout,
                vk::ShaderStageFlags::Vertex | vk::ShaderStageFlags::Fragment,
                0,
                mvp_bytes,
            );
            cmd.push_constants(
                self.pipeline_layout,
                vk::ShaderStageFlags::Vertex | vk::ShaderStageFlags::Fragment,
                64,
                light_bytes,
            );
            cmd.draw(mesh.vertex_count, 1, 0, 0);
        }
    }

    pub fn recreate_pipeline(&mut self, device: &vk::Device, render_pass: vk::RenderPass) {
        device.destroy_pipeline(self.pipeline, None);
        self.pipeline = create_pipeline(device, render_pass, self.pipeline_layout);
    }

    pub fn destroy(&mut self, device: &vk::Device, allocator: &Arc<Mutex<Allocator>>) {
        for (_, entry) in self.meshes.drain() {
            device.destroy_buffer(entry.buffer, None);
            allocator.lock().unwrap().free(entry.allocation).ok();
        }
        for i in 0..MAX_FRAMES_IN_FLIGHT {
            device.destroy_buffer(self.camera_buffers[i], None);
            if let Some(alloc) = self.camera_allocations[i].take() {
                allocator.lock().unwrap().free(alloc).ok();
            }
        }

        device.destroy_pipeline(self.pipeline, None);
        device.destroy_pipeline_layout(self.pipeline_layout, None);
        device.destroy_descriptor_pool(self.descriptor_pool, None);
        device.destroy_descriptor_set_layout(self.camera_layout, None);
        device.destroy_descriptor_set_layout(self.atlas_layout, None);
    }
}

fn build_item_mesh(model: &BakedModel, uv_map: &AtlasUVMap) -> Vec<ChunkVertex> {
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

        for i in [0, 1, 2, 2, 3, 0] {
            let p = quad.positions[i];
            vertices.push(ChunkVertex {
                position: [p[0] - 0.5, p[1] - 0.5, p[2] - 0.5],
                tex_coords: crate::renderer::chunk::mesher::pack_uv(
                    region.u_min + quad.uvs[i][0] * u_span,
                    region.v_min + quad.uvs[i][1] * v_span,
                ),
                light_tint: crate::renderer::chunk::mesher::pack_light_tint(quad.shade_light, tint),
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
            tex_coords: crate::renderer::chunk::mesher::pack_uv(front_uvs[i][0], front_uvs[i][1]),
            light_tint: crate::renderer::chunk::mesher::pack_light_tint(
                1.0,
                crate::renderer::chunk::mesher::PACKED_WHITE_SHIFTED,
            ),
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
            tex_coords: crate::renderer::chunk::mesher::pack_uv(back_uvs[i][0], back_uvs[i][1]),
            light_tint: crate::renderer::chunk::mesher::pack_light_tint(
                1.0,
                crate::renderer::chunk::mesher::PACKED_WHITE_SHIFTED,
            ),
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
            tex_coords: crate::renderer::chunk::mesher::pack_uv(u, v),
            light_tint: crate::renderer::chunk::mesher::pack_light_tint(
                light,
                crate::renderer::chunk::mesher::PACKED_WHITE_SHIFTED,
            ),
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
            tex_coords: crate::renderer::chunk::mesher::pack_uv(uv[0], uv[1]),
            light_tint: crate::renderer::chunk::mesher::pack_light_tint(
                1.0,
                crate::renderer::chunk::mesher::PACKED_WHITE_SHIFTED,
            ),
        })
        .collect()
}

pub(super) fn create_pipeline(
    device: &vk::Device,
    render_pass: vk::RenderPass,
    layout: vk::PipelineLayout,
) -> vk::Pipeline {
    create_pipeline_with_front_face(device, render_pass, layout, vk::FrontFace::CounterClockwise)
}

pub(super) fn create_pipeline_with_front_face(
    device: &vk::Device,
    render_pass: vk::RenderPass,
    layout: vk::PipelineLayout,
    front_face: vk::FrontFace,
) -> vk::Pipeline {
    let vert_spv = shader::include_spirv!("item_entity.vert.spv");
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

    let binding = ChunkVertex::binding_description();
    let attrs = ChunkVertex::attribute_descriptions();

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
        depth_compare_op: vk::CompareOp::Less,
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
