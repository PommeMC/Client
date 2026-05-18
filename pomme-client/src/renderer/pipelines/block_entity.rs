use std::collections::HashMap;
use std::path::Path;
use std::slice;
use std::sync::{Arc, Mutex};

use azalea_core::position::BlockPos;
use azalea_registry::builtin::BlockEntityKind;
use pomme_gpu_allocator::vulkan::{Allocation, Allocator};
use pyronyx::vk;

use crate::assets::{AssetIndex, resolve_asset_path};
use crate::renderer::camera::CameraUniform;
use crate::renderer::chunk::mesher::ChunkVertex;
use crate::renderer::entity_model::{BakedEntityModel, PartAnim};
use crate::renderer::pipelines::entity_renderer::{WHITE_TINT, create_pipeline, fallback_texture};
use crate::renderer::{MAX_FRAMES_IN_FLIGHT, block_entity_model, util};

pub struct BlockEntityRenderInfo {
    pub pos: BlockPos,
    pub kind: BlockEntityKind,
    pub yaw: f32,
}

struct KindEntry {
    model: BakedEntityModel,
    vertex_buffer: vk::Buffer,
    vertex_allocation: Allocation,
    texture_image: vk::Image,
    texture_view: vk::ImageView,
    texture_allocation: Allocation,
    texture_set: vk::DescriptorSet,
}

struct KindDef {
    kind: BlockEntityKind,
    model: BakedEntityModel,
    tex_keys: &'static [&'static str],
    tex_size: u32,
}

fn kind_definitions() -> Vec<KindDef> {
    vec![KindDef {
        kind: BlockEntityKind::Chest,
        model: block_entity_model::bake_chest_model(),
        tex_keys: &["minecraft/textures/entity/chest/normal.png"],
        tex_size: 64,
    }]
}

pub struct BlockEntityPipeline {
    pipeline: vk::Pipeline,
    pipeline_layout: vk::PipelineLayout,
    camera_layout: vk::DescriptorSetLayout,
    texture_layout: vk::DescriptorSetLayout,
    descriptor_pool: vk::DescriptorPool,
    camera_sets: Vec<vk::DescriptorSet>,
    camera_buffers: Vec<vk::Buffer>,
    camera_allocations: Vec<Allocation>,
    texture_sampler: vk::Sampler,
    entries: HashMap<BlockEntityKind, KindEntry>,
}

impl BlockEntityPipeline {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        device: &vk::Device,
        queue: vk::Queue,
        command_pool: vk::CommandPool,
        render_pass: vk::RenderPass,
        allocator: &Arc<Mutex<Allocator>>,
        jar_assets_dir: &Path,
        asset_index: &Option<AssetIndex>,
    ) -> Self {
        let camera_layout = util::create_descriptor_set_layout(
            device,
            vk::DescriptorType::UniformBuffer,
            vk::ShaderStageFlags::Vertex,
        );
        let texture_layout = util::create_descriptor_set_layout(
            device,
            vk::DescriptorType::CombinedImageSampler,
            vk::ShaderStageFlags::Fragment,
        );

        let push_constant_range = vk::PushConstantRange {
            stage_flags: vk::ShaderStageFlags::Vertex,
            offset: 0,
            size: 80,
        };
        let layouts = [camera_layout, texture_layout];
        let layout_info = vk::PipelineLayoutCreateInfo {
            set_layout_count: layouts.len() as u32,
            set_layouts: layouts.as_ptr(),
            push_constant_range_count: 1,
            push_constant_ranges: &push_constant_range,
            ..Default::default()
        };
        let pipeline_layout = device
            .create_pipeline_layout(&layout_info, None)
            .expect("failed to create block-entity pipeline layout");

        let pipeline = create_pipeline(device, render_pass, pipeline_layout);

        let defs = kind_definitions();
        let tex_count = defs.iter().map(|d| d.tex_keys.len() as u32).sum::<u32>();

        let pool_sizes = [
            vk::DescriptorPoolSize {
                ty: vk::DescriptorType::UniformBuffer,
                descriptor_count: MAX_FRAMES_IN_FLIGHT as u32,
            },
            vk::DescriptorPoolSize {
                ty: vk::DescriptorType::CombinedImageSampler,
                descriptor_count: tex_count.max(1),
            },
        ];
        let pool_info = vk::DescriptorPoolCreateInfo {
            max_sets: MAX_FRAMES_IN_FLIGHT as u32 + tex_count.max(1),
            pool_size_count: pool_sizes.len() as u32,
            pool_sizes: pool_sizes.as_ptr(),
            ..Default::default()
        };
        let descriptor_pool = device
            .create_descriptor_pool(&pool_info, None)
            .expect("failed to create block-entity descriptor pool");

        let camera_layouts_vec: Vec<_> = (0..MAX_FRAMES_IN_FLIGHT).map(|_| camera_layout).collect();
        let camera_alloc_info = vk::DescriptorSetAllocateInfo {
            descriptor_pool,
            descriptor_set_count: camera_layouts_vec.len() as u32,
            set_layouts: camera_layouts_vec.as_ptr(),
            ..Default::default()
        };
        let mut camera_sets = vec![vk::DescriptorSet::null(); camera_layouts_vec.len()];
        device
            .allocate_descriptor_sets(&camera_alloc_info, &mut camera_sets)
            .expect("failed to allocate block-entity camera sets");

        let mut camera_buffers = Vec::with_capacity(MAX_FRAMES_IN_FLIGHT);
        let mut camera_allocations = Vec::with_capacity(MAX_FRAMES_IN_FLIGHT);
        for &set in &camera_sets {
            let (buf, alloc) = util::create_uniform_buffer(
                device,
                allocator,
                size_of::<CameraUniform>() as u64,
                "block_entity_camera_uniform",
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
            camera_allocations.push(alloc);
        }

        let texture_sampler = unsafe { util::create_nearest_sampler(device) };

        let mut entries = HashMap::new();
        for def in defs {
            let entry = build_entry(
                device,
                queue,
                command_pool,
                allocator,
                descriptor_pool,
                texture_layout,
                texture_sampler,
                jar_assets_dir,
                asset_index,
                def.model,
                def.tex_keys,
                def.tex_size,
            );
            entries.insert(def.kind, entry);
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
            entries,
        }
    }

    pub fn update_camera(&mut self, frame: usize, uniform: &CameraUniform) {
        let bytes = bytemuck::bytes_of(uniform);
        self.camera_allocations[frame].mapped_slice_mut().unwrap()[..bytes.len()]
            .copy_from_slice(bytes);
    }

    pub fn draw(&self, cmd: vk::CommandBuffer, frame: usize, items: &[BlockEntityRenderInfo]) {
        if items.is_empty() {
            return;
        }

        cmd.bind_pipeline(vk::PipelineBindPoint::Graphics, self.pipeline);

        let mut last_entry: *const KindEntry = std::ptr::null();
        let anim = PartAnim::default();

        for info in items {
            let Some(entry) = self.entries.get(&info.kind) else {
                continue;
            };

            let ptr: *const KindEntry = entry;
            if last_entry != ptr {
                cmd.bind_descriptor_sets(
                    vk::PipelineBindPoint::Graphics,
                    self.pipeline_layout,
                    0,
                    &[self.camera_sets[frame], entry.texture_set],
                    &[],
                );
                cmd.bind_vertex_buffers(0, &[entry.vertex_buffer], &[0]);
                last_entry = ptr;
            }

            let block_center = glam::Vec3::new(
                info.pos.x as f32 + 0.5,
                info.pos.y as f32,
                info.pos.z as f32 + 0.5,
            );
            let model_mat = glam::Mat4::from_translation(block_center)
                * glam::Mat4::from_rotation_y((180.0f32 - info.yaw).to_radians());

            let part_transforms = entry.model.compute_part_transforms(&anim);
            for (i, (start, count)) in entry.model.part_ranges.iter().enumerate() {
                if *count == 0 {
                    continue;
                }
                let part_mat = model_mat * part_transforms[i];
                let cols = part_mat.to_cols_array();
                let mut bytes = [0u8; 80];
                bytes[..64].copy_from_slice(bytemuck::cast_slice(&cols));
                bytes[64..].copy_from_slice(bytemuck::cast_slice(&WHITE_TINT));
                cmd.push_constants(
                    self.pipeline_layout,
                    vk::ShaderStageFlags::Vertex,
                    0,
                    &bytes,
                );
                cmd.draw(*count, 1, *start, 0);
            }
        }
    }

    pub fn recreate_pipeline(&mut self, device: &vk::Device, render_pass: vk::RenderPass) {
        device.destroy_pipeline(self.pipeline, None);
        self.pipeline = create_pipeline(device, render_pass, self.pipeline_layout);
    }

    pub fn destroy(&mut self, device: &vk::Device, allocator: &Arc<Mutex<Allocator>>) {
        let mut alloc = allocator.lock().unwrap();
        for i in 0..MAX_FRAMES_IN_FLIGHT {
            device.destroy_buffer(self.camera_buffers[i], None);
            alloc
                .free(std::mem::replace(&mut self.camera_allocations[i], unsafe {
                    std::mem::zeroed()
                }))
                .ok();
        }
        device.destroy_sampler(self.texture_sampler, None);
        for entry in self.entries.values_mut() {
            device.destroy_buffer(entry.vertex_buffer, None);
            alloc
                .free(std::mem::replace(&mut entry.vertex_allocation, unsafe {
                    std::mem::zeroed()
                }))
                .ok();
            device.destroy_image_view(entry.texture_view, None);
            alloc
                .free(std::mem::replace(&mut entry.texture_allocation, unsafe {
                    std::mem::zeroed()
                }))
                .ok();
            device.destroy_image(entry.texture_image, None);
        }
        drop(alloc);

        device.destroy_pipeline(self.pipeline, None);
        device.destroy_pipeline_layout(self.pipeline_layout, None);
        device.destroy_descriptor_pool(self.descriptor_pool, None);
        device.destroy_descriptor_set_layout(self.camera_layout, None);
        device.destroy_descriptor_set_layout(self.texture_layout, None);
    }
}

#[allow(clippy::too_many_arguments)]
fn build_entry(
    device: &vk::Device,
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
) -> KindEntry {
    let vert_bytes = bytemuck::cast_slice::<ChunkVertex, u8>(&model.vertices);
    let (vertex_buffer, vertex_allocation) = util::create_mapped_buffer(
        device,
        allocator,
        vert_bytes,
        vk::BufferUsageFlags::VertexBuffer,
        "block_entity_vertices",
    );

    let (pixels, width, height) = tex_keys
        .iter()
        .find_map(|key| {
            let path = resolve_asset_path(jar_assets_dir, asset_index, key);
            util::load_png(&path)
        })
        .unwrap_or_else(|| {
            tracing::warn!("Failed to load BE texture {:?}, using fallback", tex_keys);
            fallback_texture(fallback_tex_size)
        });

    let (texture_image, texture_view, texture_allocation) =
        util::create_gpu_image(device, allocator, width, height, "block_entity_texture");
    let (staging_buf, staging_alloc) =
        util::create_staging_buffer(device, allocator, &pixels, "block_entity_texture_staging");
    util::upload_image(
        device,
        queue,
        command_pool,
        staging_buf,
        texture_image,
        width,
        height,
    );
    device.destroy_buffer(staging_buf, None);
    allocator.lock().unwrap().free(staging_alloc).ok();

    let tex_alloc_info = vk::DescriptorSetAllocateInfo {
        descriptor_pool,
        descriptor_set_count: 1,
        set_layouts: &texture_layout,
        ..Default::default()
    };
    let mut texture_set = vk::DescriptorSet::null();
    device
        .allocate_descriptor_sets(&tex_alloc_info, slice::from_mut(&mut texture_set))
        .expect("failed to allocate BE texture descriptor set");

    let image_info = vk::DescriptorImageInfo {
        sampler: texture_sampler,
        image_view: texture_view,
        image_layout: vk::ImageLayout::ShaderReadOnlyOptimal,
    };
    let tex_write = vk::WriteDescriptorSet {
        dst_set: texture_set,
        dst_binding: 0,
        descriptor_type: vk::DescriptorType::CombinedImageSampler,
        descriptor_count: 1,
        image_info: &image_info,
        ..Default::default()
    };
    device.update_descriptor_sets(&[tex_write], &[]);

    KindEntry {
        model,
        vertex_buffer,
        vertex_allocation,
        texture_image,
        texture_view,
        texture_allocation,
        texture_set,
    }
}
