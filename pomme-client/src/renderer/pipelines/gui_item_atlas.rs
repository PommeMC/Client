use std::collections::HashMap;
use std::path::Path;
use std::slice;
use std::sync::{Arc, Mutex};

use glam::{Mat4, Vec3};
use pomme_gpu_allocator::vulkan::{Allocation, Allocator};
use pyronyx::vk;

use crate::assets::AssetIndex;
use crate::renderer::camera::CameraUniform;
use crate::renderer::chunk::atlas::{AtlasUVMap, TextureAtlas};
use crate::renderer::chunk::mesher::ChunkVertex;
use crate::renderer::pipelines::item_entity::ItemEntityPipeline;
use crate::renderer::{shader, util};
use crate::world::block::registry::BlockRegistry;

pub const ATLAS_SIZE: u32 = 1024;
pub const TILE_SIZE: u32 = 32;
const GRID: u32 = ATLAS_SIZE / TILE_SIZE;
const COLOR_FORMAT: vk::Format = vk::Format::R8G8B8A8Srgb;
const DEPTH_FORMAT: vk::Format = vk::Format::D32Sfloat;
const MODEL_PARENT_LIMIT: u32 = 16;

#[derive(Debug, Clone, Copy)]
pub struct GuiItemRegion {
    pub u0: f32,
    pub v0: f32,
    pub u1: f32,
    pub v1: f32,
}

#[derive(Debug, Clone, Copy)]
struct DisplayTransform {
    rotation: Vec3,
    translation: Vec3,
    scale: Vec3,
}

impl DisplayTransform {
    const IDENTITY: Self = Self {
        rotation: Vec3::ZERO,
        translation: Vec3::ZERO,
        scale: Vec3::ONE,
    };

    fn to_matrix(self) -> Mat4 {
        let t = Mat4::from_translation(self.translation);
        let r = Mat4::from_rotation_z(self.rotation.z.to_radians())
            * Mat4::from_rotation_y(self.rotation.y.to_radians())
            * Mat4::from_rotation_x(self.rotation.x.to_radians());
        let s = Mat4::from_scale(self.scale);
        t * r * s
    }
}

pub struct GuiItemAtlas {
    pub image: vk::Image,
    pub view: vk::ImageView,
    pub sampler: vk::Sampler,
    pub regions: HashMap<String, GuiItemRegion>,

    image_alloc: Option<Allocation>,
    depth_image: vk::Image,
    depth_view: vk::ImageView,
    depth_alloc: Option<Allocation>,
    framebuffer: vk::Framebuffer,
    render_pass: vk::RenderPass,
    pipeline: vk::Pipeline,
    pipeline_layout: vk::PipelineLayout,
    camera_layout: vk::DescriptorSetLayout,
    atlas_layout: vk::DescriptorSetLayout,
    descriptor_pool: vk::DescriptorPool,
    camera_set: vk::DescriptorSet,
    world_atlas_set: vk::DescriptorSet,
    camera_buffer: vk::Buffer,
    camera_alloc: Option<Allocation>,
}

impl GuiItemAtlas {
    pub fn new(
        device: &vk::Device,
        allocator: &Arc<Mutex<Allocator>>,
        world_atlas: &TextureAtlas,
    ) -> Self {
        let (image, view, image_alloc) = util::create_color_attachment_image(
            device,
            allocator,
            ATLAS_SIZE,
            ATLAS_SIZE,
            COLOR_FORMAT,
            "gui_item_atlas_color",
        );
        let (depth_image, depth_view, depth_alloc) = util::create_depth_attachment_image(
            device,
            allocator,
            ATLAS_SIZE,
            ATLAS_SIZE,
            DEPTH_FORMAT,
            "gui_item_atlas_depth",
        );

        let sampler = unsafe { util::create_nearest_sampler(device) };

        let render_pass = create_render_pass(device);

        let attachments = [view, depth_view];
        let framebuffer_info = vk::FramebufferCreateInfo {
            render_pass,
            attachment_count: attachments.len() as u32,
            attachments: attachments.as_ptr(),
            width: ATLAS_SIZE,
            height: ATLAS_SIZE,
            layers: 1,
            ..Default::default()
        };
        let framebuffer = device
            .create_framebuffer(&framebuffer_info, None)
            .expect("failed to create gui_item_atlas framebuffer");

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
            .expect("failed to create gui_item_atlas pipeline layout");

        let pipeline = create_pipeline(device, render_pass, pipeline_layout);

        let pool_sizes = [
            vk::DescriptorPoolSize {
                ty: vk::DescriptorType::UniformBuffer,
                descriptor_count: 1,
            },
            vk::DescriptorPoolSize {
                ty: vk::DescriptorType::CombinedImageSampler,
                descriptor_count: 1,
            },
        ];
        let pool_info = vk::DescriptorPoolCreateInfo {
            max_sets: 2,
            pool_size_count: pool_sizes.len() as u32,
            pool_sizes: pool_sizes.as_ptr(),
            ..Default::default()
        };
        let descriptor_pool = device
            .create_descriptor_pool(&pool_info, None)
            .expect("failed to create gui_item_atlas descriptor pool");

        let mut camera_set = vk::DescriptorSet::null();
        let cam_alloc_info = vk::DescriptorSetAllocateInfo {
            descriptor_pool,
            descriptor_set_count: 1,
            set_layouts: &camera_layout,
            ..Default::default()
        };
        device
            .allocate_descriptor_sets(&cam_alloc_info, slice::from_mut(&mut camera_set))
            .expect("failed to allocate gui_item_atlas camera set");

        let mut world_atlas_set = vk::DescriptorSet::null();
        let atlas_alloc_info = vk::DescriptorSetAllocateInfo {
            descriptor_pool,
            descriptor_set_count: 1,
            set_layouts: &atlas_layout,
            ..Default::default()
        };
        device
            .allocate_descriptor_sets(&atlas_alloc_info, slice::from_mut(&mut world_atlas_set))
            .expect("failed to allocate gui_item_atlas world atlas set");

        let (camera_buffer, camera_alloc) = util::create_uniform_buffer(
            device,
            allocator,
            size_of::<CameraUniform>() as u64,
            "gui_item_atlas_camera",
        );
        let cam_buf_info = vk::DescriptorBufferInfo {
            buffer: camera_buffer,
            offset: 0,
            range: size_of::<CameraUniform>() as u64,
        };
        let cam_write = vk::WriteDescriptorSet {
            dst_set: camera_set,
            dst_binding: 0,
            descriptor_type: vk::DescriptorType::UniformBuffer,
            descriptor_count: 1,
            buffer_info: &cam_buf_info,
            ..Default::default()
        };

        let world_atlas_img_info = vk::DescriptorImageInfo {
            sampler: world_atlas.sampler,
            image_view: world_atlas.view,
            image_layout: vk::ImageLayout::ShaderReadOnlyOptimal,
        };
        let world_atlas_write = vk::WriteDescriptorSet {
            dst_set: world_atlas_set,
            dst_binding: 0,
            descriptor_type: vk::DescriptorType::CombinedImageSampler,
            descriptor_count: 1,
            image_info: &world_atlas_img_info,
            ..Default::default()
        };
        device.update_descriptor_sets(&[cam_write, world_atlas_write], &[]);

        Self {
            image,
            view,
            sampler,
            regions: HashMap::new(),
            image_alloc: Some(image_alloc),
            depth_image,
            depth_view,
            depth_alloc: Some(depth_alloc),
            framebuffer,
            render_pass,
            pipeline,
            pipeline_layout,
            camera_layout,
            atlas_layout,
            descriptor_pool,
            camera_set,
            world_atlas_set,
            camera_buffer,
            camera_alloc: Some(camera_alloc),
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn populate(
        &mut self,
        device: &vk::Device,
        queue: vk::Queue,
        command_pool: vk::CommandPool,
        allocator: &Arc<Mutex<Allocator>>,
        item_entity_pipeline: &mut ItemEntityPipeline,
        uv_map: &AtlasUVMap,
        registry: &BlockRegistry,
        jar_assets_dir: &Path,
        asset_index: &Option<AssetIndex>,
    ) {
        let mc_base = jar_assets_dir.join("minecraft");
        let items_dir = mc_base.join("items");
        let models_dir = mc_base.join("models");

        let mut item_names: Vec<String> = Vec::new();
        if let Ok(entries) = std::fs::read_dir(&items_dir) {
            for entry in entries.flatten() {
                let fname = entry.file_name().to_string_lossy().to_string();
                if let Some(name) = fname.strip_suffix(".json") {
                    item_names.push(name.to_string());
                }
            }
        }
        item_names.sort();

        let mut draws: Vec<(String, Mat4)> = Vec::new();
        let mut slot = 0u32;

        for name in &item_names {
            if slot >= GRID * GRID {
                tracing::warn!("GUI item atlas full, skipping remaining items");
                break;
            }

            let Some(model_path) = resolve_item_model_path(name, &items_dir) else {
                continue;
            };

            let is_block = if let Some(model) = registry.get_baked_model_by_name(name) {
                item_entity_pipeline.ensure_mesh(device, allocator, name, model, uv_map);
                true
            } else {
                item_entity_pipeline.ensure_flat_mesh(
                    device,
                    allocator,
                    name,
                    uv_map,
                    jar_assets_dir,
                    asset_index,
                );
                false
            };

            if !item_entity_pipeline.has_mesh(name) {
                continue;
            }

            let display = resolve_display_gui(&model_path, &models_dir, is_block);

            let gx = (slot % GRID) * TILE_SIZE;
            let gy = (slot / GRID) * TILE_SIZE;
            let inv = 1.0 / ATLAS_SIZE as f32;
            self.regions.insert(
                name.clone(),
                GuiItemRegion {
                    u0: gx as f32 * inv,
                    v0: gy as f32 * inv,
                    u1: (gx + TILE_SIZE) as f32 * inv,
                    v1: (gy + TILE_SIZE) as f32 * inv,
                },
            );

            let model_matrix = slot_model_matrix(gx, gy, display);
            draws.push((name.clone(), model_matrix));
            slot += 1;
        }

        tracing::info!(
            "GUI item atlas: rendered {} items into {}x{} atlas",
            self.regions.len(),
            ATLAS_SIZE,
            ATLAS_SIZE,
        );

        let view_proj = Mat4::orthographic_rh(
            0.0,
            ATLAS_SIZE as f32,
            0.0,
            ATLAS_SIZE as f32,
            -1000.0,
            1000.0,
        );
        let uniform = CameraUniform::with_view_proj(view_proj);
        let bytes = bytemuck::bytes_of(&uniform);
        if let Some(alloc) = self.camera_alloc.as_mut() {
            alloc.mapped_slice_mut().unwrap()[..bytes.len()].copy_from_slice(bytes);
        }

        self.record_and_submit(device, queue, command_pool, item_entity_pipeline, &draws);
    }

    fn record_and_submit(
        &self,
        device: &vk::Device,
        queue: vk::Queue,
        command_pool: vk::CommandPool,
        item_entity_pipeline: &ItemEntityPipeline,
        draws: &[(String, Mat4)],
    ) {
        let alloc_info = vk::CommandBufferAllocateInfo {
            command_pool,
            level: vk::CommandBufferLevel::Primary,
            command_buffer_count: 1,
            ..Default::default()
        };
        let mut cmd = vk::CommandBuffer::null();
        unsafe { device.allocate_command_buffers(&alloc_info, slice::from_mut(&mut cmd)) }
            .expect("failed to allocate gui_item_atlas command buffer");

        let begin_info = vk::CommandBufferBeginInfo {
            flags: vk::CommandBufferUsageFlags::OneTimeSubmit,
            ..Default::default()
        };
        cmd.begin(&begin_info)
            .expect("failed to begin gui_item_atlas command buffer");

        let clear_values = [
            vk::ClearValue {
                color: vk::ClearColorValue { float32: [0.0; 4] },
            },
            vk::ClearValue {
                depth_stencil: vk::ClearDepthStencilValue {
                    depth: 1.0,
                    stencil: 0,
                },
            },
        ];
        let render_pass_info = vk::RenderPassBeginInfo {
            render_pass: self.render_pass,
            framebuffer: self.framebuffer,
            render_area: vk::Rect2D {
                offset: vk::Offset2D { x: 0, y: 0 },
                extent: vk::Extent2D {
                    width: ATLAS_SIZE,
                    height: ATLAS_SIZE,
                },
            },
            clear_value_count: clear_values.len() as u32,
            clear_values: clear_values.as_ptr(),
            ..Default::default()
        };
        cmd.begin_render_pass(&render_pass_info, vk::SubpassContents::Inline);

        let viewport = vk::Viewport {
            x: 0.0,
            y: 0.0,
            width: ATLAS_SIZE as f32,
            height: ATLAS_SIZE as f32,
            min_depth: 0.0,
            max_depth: 1.0,
        };
        let scissor = vk::Rect2D {
            offset: vk::Offset2D { x: 0, y: 0 },
            extent: vk::Extent2D {
                width: ATLAS_SIZE,
                height: ATLAS_SIZE,
            },
        };
        cmd.set_viewport(0, &[viewport]);
        cmd.set_scissor(0, &[scissor]);

        cmd.bind_pipeline(vk::PipelineBindPoint::Graphics, self.pipeline);
        cmd.bind_descriptor_sets(
            vk::PipelineBindPoint::Graphics,
            self.pipeline_layout,
            0,
            &[self.camera_set, self.world_atlas_set],
            &[],
        );

        let world_light: f32 = 1.0;
        for (name, model) in draws {
            let Some((buffer, vertex_count)) = item_entity_pipeline.mesh_handle(name) else {
                continue;
            };
            let mvp_data = model.to_cols_array();
            let mvp_bytes = bytemuck::bytes_of(&mvp_data);
            let light_bytes = bytemuck::bytes_of(&world_light);

            cmd.bind_vertex_buffers(0, &[buffer], &[0]);
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
            cmd.draw(vertex_count, 1, 0, 0);
        }

        cmd.end_render_pass();
        cmd.end()
            .expect("failed to end gui_item_atlas command buffer");

        let submit_info = vk::SubmitInfo {
            command_buffer_count: 1,
            command_buffers: &cmd.handle(),
            ..Default::default()
        };
        queue
            .submit(&[submit_info], vk::Fence::null())
            .expect("failed to submit gui_item_atlas");
        queue
            .wait_idle()
            .expect("failed to wait for gui_item_atlas");
        device.free_command_buffers(command_pool, &[cmd.handle()]);
    }

    pub fn destroy(&mut self, device: &vk::Device, allocator: &Arc<Mutex<Allocator>>) {
        device.destroy_pipeline(self.pipeline, None);
        device.destroy_pipeline_layout(self.pipeline_layout, None);
        device.destroy_descriptor_pool(self.descriptor_pool, None);
        device.destroy_descriptor_set_layout(self.camera_layout, None);
        device.destroy_descriptor_set_layout(self.atlas_layout, None);
        device.destroy_framebuffer(self.framebuffer, None);
        device.destroy_render_pass(self.render_pass, None);

        device.destroy_sampler(self.sampler, None);
        device.destroy_image_view(self.view, None);
        device.destroy_image(self.image, None);
        device.destroy_image_view(self.depth_view, None);
        device.destroy_image(self.depth_image, None);
        device.destroy_buffer(self.camera_buffer, None);

        let mut alloc = allocator.lock().unwrap();
        if let Some(a) = self.image_alloc.take() {
            alloc.free(a).ok();
        }
        if let Some(a) = self.depth_alloc.take() {
            alloc.free(a).ok();
        }
        if let Some(a) = self.camera_alloc.take() {
            alloc.free(a).ok();
        }
    }
}

fn slot_model_matrix(gx: u32, gy: u32, display: DisplayTransform) -> Mat4 {
    let cx = gx as f32 + TILE_SIZE as f32 * 0.5;
    let cy = gy as f32 + TILE_SIZE as f32 * 0.5;
    let tile = TILE_SIZE as f32;
    Mat4::from_translation(Vec3::new(cx, cy, 0.0))
        * Mat4::from_scale(Vec3::new(tile, -tile, tile))
        * display.to_matrix()
}

fn create_render_pass(device: &vk::Device) -> vk::RenderPass {
    let attachments = [
        vk::AttachmentDescription {
            format: COLOR_FORMAT,
            samples: vk::SampleCountFlags::Type1,
            load_op: vk::AttachmentLoadOp::Clear,
            store_op: vk::AttachmentStoreOp::Store,
            stencil_load_op: vk::AttachmentLoadOp::DontCare,
            stencil_store_op: vk::AttachmentStoreOp::DontCare,
            initial_layout: vk::ImageLayout::Undefined,
            final_layout: vk::ImageLayout::ShaderReadOnlyOptimal,
            ..Default::default()
        },
        vk::AttachmentDescription {
            format: DEPTH_FORMAT,
            samples: vk::SampleCountFlags::Type1,
            load_op: vk::AttachmentLoadOp::Clear,
            store_op: vk::AttachmentStoreOp::DontCare,
            stencil_load_op: vk::AttachmentLoadOp::DontCare,
            stencil_store_op: vk::AttachmentStoreOp::DontCare,
            initial_layout: vk::ImageLayout::Undefined,
            final_layout: vk::ImageLayout::DepthStencilAttachmentOptimal,
            ..Default::default()
        },
    ];

    let color_ref = [vk::AttachmentReference {
        attachment: 0,
        layout: vk::ImageLayout::ColorAttachmentOptimal,
    }];
    let depth_ref = vk::AttachmentReference {
        attachment: 1,
        layout: vk::ImageLayout::DepthStencilAttachmentOptimal,
    };

    let subpass = [vk::SubpassDescription {
        pipeline_bind_point: vk::PipelineBindPoint::Graphics,
        color_attachment_count: color_ref.len() as u32,
        color_attachments: color_ref.as_ptr(),
        depth_stencil_attachment: &depth_ref,
        ..Default::default()
    }];

    let dependencies = [
        vk::SubpassDependency {
            src_subpass: vk::SUBPASS_EXTERNAL,
            dst_subpass: 0,
            src_stage_mask: vk::PipelineStageFlags::BottomOfPipe,
            src_access_mask: vk::AccessFlags::empty(),
            dst_stage_mask: vk::PipelineStageFlags::ColorAttachmentOutput
                | vk::PipelineStageFlags::EarlyFragmentTests,
            dst_access_mask: vk::AccessFlags::ColorAttachmentWrite
                | vk::AccessFlags::DepthStencilAttachmentWrite,
            ..Default::default()
        },
        vk::SubpassDependency {
            src_subpass: 0,
            dst_subpass: vk::SUBPASS_EXTERNAL,
            src_stage_mask: vk::PipelineStageFlags::ColorAttachmentOutput,
            src_access_mask: vk::AccessFlags::ColorAttachmentWrite,
            dst_stage_mask: vk::PipelineStageFlags::FragmentShader,
            dst_access_mask: vk::AccessFlags::ShaderRead,
            ..Default::default()
        },
    ];

    let info = vk::RenderPassCreateInfo {
        attachment_count: attachments.len() as u32,
        attachments: attachments.as_ptr(),
        subpass_count: subpass.len() as u32,
        subpasses: subpass.as_ptr(),
        dependency_count: dependencies.len() as u32,
        dependencies: dependencies.as_ptr(),
        ..Default::default()
    };
    device
        .create_render_pass(&info, None)
        .expect("failed to create gui_item_atlas render pass")
}

fn create_pipeline(
    device: &vk::Device,
    render_pass: vk::RenderPass,
    layout: vk::PipelineLayout,
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
        cull_mode: vk::CullModeFlags::None,
        front_face: vk::FrontFace::CounterClockwise,
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
        blend_enable: vk::FALSE,
        color_write_mask: vk::ColorComponentFlags::RGBA,
        ..Default::default()
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
        .expect("failed to create gui_item_atlas pipeline");

    device.destroy_shader_module(vert_mod, None);
    device.destroy_shader_module(frag_mod, None);

    pipeline
}

fn read_json(path: &Path) -> Option<serde_json::Value> {
    let s = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&s).ok()
}

fn find_first_model_string(json: &serde_json::Value) -> Option<String> {
    match json {
        serde_json::Value::Object(map) => {
            if let Some(serde_json::Value::String(s)) = map.get("model") {
                return Some(s.clone());
            }
            for v in map.values() {
                if let Some(r) = find_first_model_string(v) {
                    return Some(r);
                }
            }
            None
        }
        serde_json::Value::Array(arr) => arr.iter().find_map(find_first_model_string),
        _ => None,
    }
}

fn strip_mc_ns(s: &str) -> &str {
    s.strip_prefix("minecraft:").unwrap_or(s)
}

fn resolve_item_model_path(name: &str, items_dir: &Path) -> Option<String> {
    let item_json = read_json(&items_dir.join(format!("{name}.json")))?;
    let model_path = find_first_model_string(&item_json)?;
    Some(strip_mc_ns(&model_path).to_string())
}

fn parse_vec3(value: &serde_json::Value, default: Vec3) -> Vec3 {
    let Some(arr) = value.as_array() else {
        return default;
    };
    let get = |i: usize| arr.get(i).and_then(|v| v.as_f64()).map(|v| v as f32);
    Vec3::new(
        get(0).unwrap_or(default.x),
        get(1).unwrap_or(default.y),
        get(2).unwrap_or(default.z),
    )
}

fn parse_display_transform(json: &serde_json::Value) -> Option<DisplayTransform> {
    let obj = json.as_object()?;
    let rotation = obj
        .get("rotation")
        .map(|v| parse_vec3(v, Vec3::ZERO))
        .unwrap_or(Vec3::ZERO);
    let translation = obj
        .get("translation")
        .map(|v| parse_vec3(v, Vec3::ZERO))
        .unwrap_or(Vec3::ZERO);
    let scale = obj
        .get("scale")
        .map(|v| parse_vec3(v, Vec3::ONE))
        .unwrap_or(Vec3::ONE);
    Some(DisplayTransform {
        rotation,
        translation: translation * (1.0 / 16.0),
        scale,
    })
}

fn resolve_display_gui(start_path: &str, models_dir: &Path, is_block: bool) -> DisplayTransform {
    let mut current = Some(start_path.to_string());
    let mut depth = 0u32;
    while let Some(path) = current.take() {
        if depth >= MODEL_PARENT_LIMIT {
            break;
        }
        depth += 1;

        let file = models_dir.join(format!("{path}.json"));
        let Some(json) = read_json(&file) else { break };

        if let Some(gui) = json.get("display").and_then(|d| d.get("gui"))
            && let Some(t) = parse_display_transform(gui)
        {
            return t;
        }

        current = json
            .get("parent")
            .and_then(|p| p.as_str())
            .map(|p| strip_mc_ns(p).to_string());
    }

    if is_block {
        DisplayTransform {
            rotation: Vec3::new(30.0, 225.0, 0.0),
            translation: Vec3::ZERO,
            scale: Vec3::splat(0.625),
        }
    } else {
        DisplayTransform::IDENTITY
    }
}
