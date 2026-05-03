use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};

use ash::vk;
use azalea_registry::builtin::EntityKind;
use gpu_allocator::vulkan::{Allocation, Allocator};

use crate::assets::{AssetIndex, resolve_asset_path};
use crate::renderer::MAX_FRAMES_IN_FLIGHT;
use crate::renderer::camera::CameraUniform;
use crate::renderer::chunk::mesher::ChunkVertex;
use crate::renderer::entity_model::{self, BakedEntityModel};
use crate::renderer::shader;
use crate::renderer::util;

pub struct EntityRenderInfo {
    pub x: f64,
    pub y: f64,
    pub z: f64,
    pub yaw: f32,
    pub pitch: f32,
    pub head_yaw: f32,
    pub is_baby: bool,
    pub walk_anim_pos: f32,
    pub walk_anim_speed: f32,
    pub entity_kind: EntityKind,
}

struct MobVariant {
    model: BakedEntityModel,
    vertex_buffer: vk::Buffer,
    vertex_allocation: Allocation,
    texture_image: vk::Image,
    texture_view: vk::ImageView,
    texture_allocation: Allocation,
    texture_set: vk::DescriptorSet,
}

struct MobEntry {
    adult: MobVariant,
    baby: Option<MobVariant>,
    anim: AnimationType,
}

impl MobEntry {
    fn variant(&self, is_baby: bool) -> &MobVariant {
        if is_baby {
            self.baby.as_ref().unwrap_or(&self.adult)
        } else {
            &self.adult
        }
    }
}

pub struct EntityRenderer {
    pipeline: vk::Pipeline,
    pipeline_layout: vk::PipelineLayout,
    camera_layout: vk::DescriptorSetLayout,
    texture_layout: vk::DescriptorSetLayout,
    descriptor_pool: vk::DescriptorPool,
    camera_sets: Vec<vk::DescriptorSet>,
    camera_buffers: Vec<vk::Buffer>,
    camera_allocations: Vec<Allocation>,
    texture_sampler: vk::Sampler,
    mobs: HashMap<EntityKind, MobEntry>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum AnimationType {
    Quadruped,
    Humanoid,
}

struct MobDef {
    kind: EntityKind,
    anim: AnimationType,
    adult_model: BakedEntityModel,
    adult_tex_keys: &'static [&'static str],
    adult_tex_size: u32,
    baby_model: Option<BakedEntityModel>,
    baby_tex_keys: Option<&'static [&'static str]>,
    baby_tex_size: u32,
}

fn mob_definitions() -> Vec<MobDef> {
    vec![
        MobDef {
            kind: EntityKind::Pig,
            anim: AnimationType::Quadruped,
            adult_model: entity_model::bake_pig_model(),
            adult_tex_keys: &[
                "minecraft/textures/entity/pig/pig_temperate.png",
                "minecraft/textures/entity/pig/temperate_pig.png",
            ],
            adult_tex_size: 64,
            baby_model: Some(entity_model::bake_baby_pig_model()),
            baby_tex_keys: Some(&["minecraft/textures/entity/pig/pig_temperate_baby.png"]),
            baby_tex_size: 32,
        },
        MobDef {
            kind: EntityKind::Player,
            anim: AnimationType::Humanoid,
            adult_model: entity_model::bake_player_model(),
            adult_tex_keys: &["minecraft/textures/entity/player/wide/steve.png"],
            adult_tex_size: 64,
            baby_model: None,
            baby_tex_keys: None,
            baby_tex_size: 64,
        },
    ]
}

impl EntityRenderer {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        device: &ash::Device,
        queue: vk::Queue,
        command_pool: vk::CommandPool,
        render_pass: vk::RenderPass,
        allocator: &Arc<Mutex<Allocator>>,
        jar_assets_dir: &Path,
        asset_index: &Option<AssetIndex>,
    ) -> Self {
        let camera_layout = util::create_descriptor_set_layout(
            device,
            vk::DescriptorType::UNIFORM_BUFFER,
            vk::ShaderStageFlags::VERTEX,
        );
        let texture_layout = util::create_descriptor_set_layout(
            device,
            vk::DescriptorType::COMBINED_IMAGE_SAMPLER,
            vk::ShaderStageFlags::FRAGMENT,
        );

        let push_constant_range = vk::PushConstantRange {
            stage_flags: vk::ShaderStageFlags::VERTEX,
            offset: 0,
            size: 64,
        };

        let layouts = [camera_layout, texture_layout];
        let push_ranges = [push_constant_range];
        let layout_info = vk::PipelineLayoutCreateInfo::default()
            .set_layouts(&layouts)
            .push_constant_ranges(&push_ranges);
        let pipeline_layout = unsafe { device.create_pipeline_layout(&layout_info, None) }
            .expect("failed to create entity pipeline layout");

        let pipeline = create_pipeline(device, render_pass, pipeline_layout);

        let defs = mob_definitions();
        let tex_count: u32 = defs
            .iter()
            .map(|d| if d.baby_model.is_some() { 2 } else { 1 })
            .sum();

        let pool_sizes = [
            vk::DescriptorPoolSize {
                ty: vk::DescriptorType::UNIFORM_BUFFER,
                descriptor_count: MAX_FRAMES_IN_FLIGHT as u32,
            },
            vk::DescriptorPoolSize {
                ty: vk::DescriptorType::COMBINED_IMAGE_SAMPLER,
                descriptor_count: tex_count,
            },
        ];
        let pool_info = vk::DescriptorPoolCreateInfo::default()
            .max_sets(MAX_FRAMES_IN_FLIGHT as u32 + tex_count)
            .pool_sizes(&pool_sizes);
        let descriptor_pool = unsafe { device.create_descriptor_pool(&pool_info, None) }
            .expect("failed to create entity descriptor pool");

        let camera_layouts_vec: Vec<_> = (0..MAX_FRAMES_IN_FLIGHT).map(|_| camera_layout).collect();
        let camera_alloc_info = vk::DescriptorSetAllocateInfo::default()
            .descriptor_pool(descriptor_pool)
            .set_layouts(&camera_layouts_vec);
        let camera_sets = unsafe { device.allocate_descriptor_sets(&camera_alloc_info) }
            .expect("failed to allocate entity camera descriptor sets");

        let mut camera_buffers = Vec::with_capacity(MAX_FRAMES_IN_FLIGHT);
        let mut camera_allocations = Vec::with_capacity(MAX_FRAMES_IN_FLIGHT);

        for &set in &camera_sets {
            let (buf, alloc) = util::create_uniform_buffer(
                device,
                allocator,
                std::mem::size_of::<CameraUniform>() as u64,
                "entity_camera_uniform",
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
            camera_allocations.push(alloc);
        }

        let texture_sampler = unsafe { util::create_nearest_sampler(device) };

        let mut mobs = HashMap::new();

        for def in defs {
            let adult = create_mob_variant(
                device,
                queue,
                command_pool,
                allocator,
                descriptor_pool,
                texture_layout,
                texture_sampler,
                jar_assets_dir,
                asset_index,
                def.adult_model,
                def.adult_tex_keys,
                def.adult_tex_size,
            );

            let baby = match (def.baby_model, def.baby_tex_keys) {
                (Some(model), Some(keys)) => Some(create_mob_variant(
                    device,
                    queue,
                    command_pool,
                    allocator,
                    descriptor_pool,
                    texture_layout,
                    texture_sampler,
                    jar_assets_dir,
                    asset_index,
                    model,
                    keys,
                    def.baby_tex_size,
                )),
                _ => None,
            };

            mobs.insert(
                def.kind,
                MobEntry {
                    adult,
                    baby,
                    anim: def.anim,
                },
            );
        }

        Self {
            pipeline,
            pipeline_layout,
            camera_layout,
            texture_layout,
            descriptor_pool,
            camera_sets,
            camera_buffers,
            camera_allocations,
            texture_sampler,
            mobs,
        }
    }

    pub fn update_camera(&mut self, frame: usize, uniform: &CameraUniform) {
        let bytes = bytemuck::bytes_of(uniform);
        self.camera_allocations[frame].mapped_slice_mut().unwrap()[..bytes.len()]
            .copy_from_slice(bytes);
    }

    pub fn draw(
        &self,
        device: &ash::Device,
        cmd: vk::CommandBuffer,
        frame: usize,
        entities: &[EntityRenderInfo],
    ) {
        if entities.is_empty() {
            return;
        }

        unsafe {
            device.cmd_bind_pipeline(cmd, vk::PipelineBindPoint::GRAPHICS, self.pipeline);

            let mut last_variant: *const MobVariant = std::ptr::null();
            for info in entities {
                let Some(entry) = self.mobs.get(&info.entity_kind) else {
                    continue;
                };
                let variant = entry.variant(info.is_baby);

                let variant_ptr: *const MobVariant = variant;
                if last_variant != variant_ptr {
                    device.cmd_bind_descriptor_sets(
                        cmd,
                        vk::PipelineBindPoint::GRAPHICS,
                        self.pipeline_layout,
                        0,
                        &[self.camera_sets[frame], variant.texture_set],
                        &[],
                    );
                    device.cmd_bind_vertex_buffers(cmd, 0, &[variant.vertex_buffer], &[0]);
                    last_variant = variant_ptr;
                }

                let entity_mat =
                    glam::Mat4::from_translation(glam::Vec3::new(
                        info.x as f32,
                        info.y as f32,
                        info.z as f32,
                    )) * glam::Mat4::from_rotation_y((180.0f32 - info.yaw).to_radians());

                let anim_rotations = match entry.anim {
                    AnimationType::Quadruped => entity_model::compute_quadruped_anim(
                        &variant.model,
                        info.pitch,
                        info.head_yaw - info.yaw,
                        info.walk_anim_pos,
                        info.walk_anim_speed,
                    ),
                    AnimationType::Humanoid => entity_model::compute_humanoid_anim(
                        &variant.model,
                        info.pitch,
                        info.head_yaw - info.yaw,
                        info.walk_anim_pos,
                        info.walk_anim_speed,
                    ),
                };

                let part_transforms = variant.model.compute_part_transforms(&anim_rotations);

                for (i, (start, count)) in variant.model.part_ranges.iter().enumerate() {
                    if *count == 0 {
                        continue;
                    }

                    let part_mat = entity_mat * part_transforms[i];

                    let mat_array = part_mat.to_cols_array();
                    let mat_bytes: &[u8] = bytemuck::cast_slice(&mat_array);
                    device.cmd_push_constants(
                        cmd,
                        self.pipeline_layout,
                        vk::ShaderStageFlags::VERTEX,
                        0,
                        mat_bytes,
                    );

                    device.cmd_draw(cmd, *count, 1, *start, 0);
                }
            }
        }
    }

    pub fn recreate_pipeline(&mut self, device: &ash::Device, render_pass: vk::RenderPass) {
        unsafe { device.destroy_pipeline(self.pipeline, None) };
        self.pipeline = create_pipeline(device, render_pass, self.pipeline_layout);
    }

    pub fn destroy(&mut self, device: &ash::Device, allocator: &Arc<Mutex<Allocator>>) {
        let mut alloc = allocator.lock().unwrap();
        for i in 0..MAX_FRAMES_IN_FLIGHT {
            unsafe { device.destroy_buffer(self.camera_buffers[i], None) };
            alloc
                .free(std::mem::replace(&mut self.camera_allocations[i], unsafe {
                    std::mem::zeroed()
                }))
                .ok();
        }

        unsafe { device.destroy_sampler(self.texture_sampler, None) };

        for entry in self.mobs.values_mut() {
            let variants: Vec<&mut MobVariant> = std::iter::once(&mut entry.adult)
                .chain(entry.baby.iter_mut())
                .collect();
            for v in variants {
                unsafe { device.destroy_buffer(v.vertex_buffer, None) };
                alloc
                    .free(std::mem::replace(&mut v.vertex_allocation, unsafe {
                        std::mem::zeroed()
                    }))
                    .ok();
                unsafe { device.destroy_image_view(v.texture_view, None) };
                alloc
                    .free(std::mem::replace(&mut v.texture_allocation, unsafe {
                        std::mem::zeroed()
                    }))
                    .ok();
                unsafe { device.destroy_image(v.texture_image, None) };
            }
        }

        drop(alloc);

        unsafe {
            device.destroy_pipeline(self.pipeline, None);
            device.destroy_pipeline_layout(self.pipeline_layout, None);
            device.destroy_descriptor_pool(self.descriptor_pool, None);
            device.destroy_descriptor_set_layout(self.camera_layout, None);
            device.destroy_descriptor_set_layout(self.texture_layout, None);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn create_mob_variant(
    device: &ash::Device,
    queue: vk::Queue,
    command_pool: vk::CommandPool,
    allocator: &Arc<Mutex<Allocator>>,
    descriptor_pool: vk::DescriptorPool,
    texture_layout: vk::DescriptorSetLayout,
    texture_sampler: vk::Sampler,
    jar_assets_dir: &Path,
    asset_index: &Option<AssetIndex>,
    model: BakedEntityModel,
    tex_keys: &[&str],
    fallback_tex_size: u32,
) -> MobVariant {
    let vert_bytes = bytemuck::cast_slice::<ChunkVertex, u8>(&model.vertices);
    let (vertex_buffer, vertex_allocation) = util::create_mapped_buffer(
        device,
        allocator,
        vert_bytes,
        vk::BufferUsageFlags::VERTEX_BUFFER,
        "entity_vertices",
    );

    let (texture_image, texture_view, texture_allocation) = load_entity_texture(
        device,
        queue,
        command_pool,
        allocator,
        jar_assets_dir,
        asset_index,
        tex_keys,
        fallback_tex_size,
    );

    let tex_layouts = [texture_layout];
    let tex_alloc_info = vk::DescriptorSetAllocateInfo::default()
        .descriptor_pool(descriptor_pool)
        .set_layouts(&tex_layouts);
    let texture_set = unsafe { device.allocate_descriptor_sets(&tex_alloc_info) }
        .expect("failed to allocate entity texture descriptor set")[0];

    let image_info = [vk::DescriptorImageInfo {
        sampler: texture_sampler,
        image_view: texture_view,
        image_layout: vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
    }];
    let tex_write = vk::WriteDescriptorSet::default()
        .dst_set(texture_set)
        .dst_binding(0)
        .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
        .image_info(&image_info);
    unsafe { device.update_descriptor_sets(&[tex_write], &[]) };

    MobVariant {
        model,
        vertex_buffer,
        vertex_allocation,
        texture_image,
        texture_view,
        texture_allocation,
        texture_set,
    }
}

#[allow(clippy::too_many_arguments)]
fn load_entity_texture(
    device: &ash::Device,
    queue: vk::Queue,
    command_pool: vk::CommandPool,
    allocator: &Arc<Mutex<Allocator>>,
    jar_assets_dir: &Path,
    asset_index: &Option<AssetIndex>,
    asset_keys: &[&str],
    fallback_size: u32,
) -> (vk::Image, vk::ImageView, Allocation) {
    let (pixels, width, height) = asset_keys
        .iter()
        .find_map(|key| {
            let path = resolve_asset_path(jar_assets_dir, asset_index, key);
            util::load_png(&path)
        })
        .unwrap_or_else(|| {
            tracing::warn!(
                "Failed to load entity texture {:?}, using fallback",
                asset_keys
            );
            fallback_texture(fallback_size)
        });

    let (image, view, allocation) =
        util::create_gpu_image(device, allocator, width, height, "entity_texture");
    let (staging_buf, staging_alloc) =
        util::create_staging_buffer(device, allocator, &pixels, "entity_texture_staging");
    util::upload_image(
        device,
        queue,
        command_pool,
        staging_buf,
        image,
        width,
        height,
    );
    unsafe { device.destroy_buffer(staging_buf, None) };
    allocator.lock().unwrap().free(staging_alloc).ok();
    (image, view, allocation)
}

fn fallback_texture(size: u32) -> (Vec<u8>, u32, u32) {
    let mut pixels = vec![0u8; (size * size * 4) as usize];
    for pixel in pixels.chunks_exact_mut(4) {
        pixel.copy_from_slice(&[219, 148, 148, 255]);
    }
    (pixels, size, size)
}

fn create_pipeline(
    device: &ash::Device,
    render_pass: vk::RenderPass,
    layout: vk::PipelineLayout,
) -> vk::Pipeline {
    let vert_spv = shader::include_spirv!("entity.vert.spv");
    let frag_spv = shader::include_spirv!("entity.frag.spv");

    let vert_module = shader::create_shader_module(device, vert_spv);
    let frag_module = shader::create_shader_module(device, frag_spv);

    let stages = [
        vk::PipelineShaderStageCreateInfo::default()
            .stage(vk::ShaderStageFlags::VERTEX)
            .module(vert_module)
            .name(c"main"),
        vk::PipelineShaderStageCreateInfo::default()
            .stage(vk::ShaderStageFlags::FRAGMENT)
            .module(frag_module)
            .name(c"main"),
    ];

    let binding_descs = [ChunkVertex::binding_description()];
    let attr_descs = ChunkVertex::attribute_descriptions();

    let vertex_input = vk::PipelineVertexInputStateCreateInfo::default()
        .vertex_binding_descriptions(&binding_descs)
        .vertex_attribute_descriptions(&attr_descs);

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
        blend_enable: vk::FALSE,
        color_write_mask: vk::ColorComponentFlags::RGBA,
        ..Default::default()
    }];
    let color_blending =
        vk::PipelineColorBlendStateCreateInfo::default().attachments(&blend_attachment);

    let dynamic_states = [vk::DynamicState::VIEWPORT, vk::DynamicState::SCISSOR];
    let dynamic_state =
        vk::PipelineDynamicStateCreateInfo::default().dynamic_states(&dynamic_states);

    let pipeline_info = [vk::GraphicsPipelineCreateInfo::default()
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

    let pipeline = unsafe {
        device.create_graphics_pipelines(vk::PipelineCache::null(), &pipeline_info, None)
    }
    .expect("failed to create entity pipeline")[0];

    unsafe {
        device.destroy_shader_module(vert_module, None);
        device.destroy_shader_module(frag_module, None);
    }

    pipeline
}
