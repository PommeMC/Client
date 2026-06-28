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
use crate::renderer::pipelines::entity_renderer::{
    BlendMode, ModelInput, WHITE_TINT, create_pipeline, fallback_texture,
};
use crate::renderer::{MAX_FRAMES_IN_FLIGHT, block_entity_model, util};

pub struct BlockEntityRenderInfo {
    pub pos: BlockPos,
    pub kind: BlockEntityKind,
    pub yaw: f32,
    pub variant: u32,
    /// Lid openness for chest/shulker, 0.0=closed to 1.0=open. Raw (un-eased);
    /// the pipeline applies a cubic ease at draw time.
    pub lid_open: f32,
}

struct TextureSlot {
    image: vk::Image,
    view: vk::ImageView,
    allocation: Allocation,
    set: vk::DescriptorSet,
}

struct KindEntry {
    model: BakedEntityModel,
    vertex_buffer: vk::Buffer,
    vertex_allocation: Allocation,
    textures: Vec<TextureSlot>,
}

struct KindDef {
    kind: BlockEntityKind,
    model: BakedEntityModel,
    tex_variants: &'static [&'static [&'static str]],
    tex_size: u32,
}

/// 16 dye colors in vanilla `DyeColor` ordinal order. Used both to build
/// texture-variant arrays and to map block names back to variant indices.
const DYE_COLOR_NAMES: [&str; 16] = [
    "white",
    "orange",
    "magenta",
    "light_blue",
    "yellow",
    "lime",
    "pink",
    "gray",
    "light_gray",
    "cyan",
    "purple",
    "blue",
    "brown",
    "green",
    "red",
    "black",
];

/// Wood/material variants for sign textures, in the order they appear in
/// `SIGN_TEXTURES`.
const SIGN_WOOD_NAMES: [&str; 12] = [
    "oak", "spruce", "birch", "jungle", "acacia", "dark_oak", "mangrove", "cherry", "pale_oak",
    "bamboo", "crimson", "warped",
];

const SIGN_TEXTURES: &[&[&str]] = &[
    &["minecraft/textures/block/oak_sign.png"],
    &["minecraft/textures/block/spruce_sign.png"],
    &["minecraft/textures/block/birch_sign.png"],
    &["minecraft/textures/block/jungle_sign.png"],
    &["minecraft/textures/block/acacia_sign.png"],
    &["minecraft/textures/block/dark_oak_sign.png"],
    &["minecraft/textures/block/mangrove_sign.png"],
    &["minecraft/textures/block/cherry_sign.png"],
    &["minecraft/textures/block/pale_oak_sign.png"],
    &["minecraft/textures/block/bamboo_sign.png"],
    &["minecraft/textures/block/crimson_sign.png"],
    &["minecraft/textures/block/warped_sign.png"],
];

const SHULKER_TEXTURES: &[&[&str]] = &[
    &["minecraft/textures/entity/shulker/shulker_white.png"],
    &["minecraft/textures/entity/shulker/shulker_orange.png"],
    &["minecraft/textures/entity/shulker/shulker_magenta.png"],
    &["minecraft/textures/entity/shulker/shulker_light_blue.png"],
    &["minecraft/textures/entity/shulker/shulker_yellow.png"],
    &["minecraft/textures/entity/shulker/shulker_lime.png"],
    &["minecraft/textures/entity/shulker/shulker_pink.png"],
    &["minecraft/textures/entity/shulker/shulker_gray.png"],
    &["minecraft/textures/entity/shulker/shulker_light_gray.png"],
    &["minecraft/textures/entity/shulker/shulker_cyan.png"],
    &["minecraft/textures/entity/shulker/shulker_purple.png"],
    &["minecraft/textures/entity/shulker/shulker_blue.png"],
    &["minecraft/textures/entity/shulker/shulker_brown.png"],
    &["minecraft/textures/entity/shulker/shulker_green.png"],
    &["minecraft/textures/entity/shulker/shulker_red.png"],
    &["minecraft/textures/entity/shulker/shulker_black.png"],
    &["minecraft/textures/entity/shulker/shulker.png"],
];

fn name_index(table: &[&str], name: &str) -> Option<u32> {
    table.iter().position(|&n| n == name).map(|i| i as u32)
}

/// Build a [`PartAnim`] applying chest/shulker lid motion to part index 0.
/// `openness` is the raw [0, 1] value; vanilla applies cubic easing so the lid
/// decelerates as it approaches the open or closed extreme.
fn lid_anim(kind: BlockEntityKind, openness: f32) -> PartAnim {
    if openness <= 0.0 {
        return PartAnim::default();
    }
    let inv = 1.0 - openness;
    let eased = 1.0 - inv * inv * inv;
    match kind {
        BlockEntityKind::Chest | BlockEntityKind::TrappedChest | BlockEntityKind::EnderChest => {
            PartAnim {
                rotation: vec![(
                    0,
                    glam::Vec3::new(-eased * std::f32::consts::FRAC_PI_2, 0.0, 0.0),
                )],
                ..Default::default()
            }
        }
        BlockEntityKind::ShulkerBox => PartAnim {
            rotation: vec![(0, glam::Vec3::new(0.0, eased * 270.0f32.to_radians(), 0.0))],
            translation: vec![(0, glam::Vec3::new(0.0, -eased * 8.0, 0.0))],
            ..Default::default()
        },
        _ => PartAnim::default(),
    }
}

pub fn variant_for_block(kind: BlockEntityKind, name: &str) -> u32 {
    match kind {
        BlockEntityKind::ShulkerBox => name
            .strip_suffix("_shulker_box")
            .and_then(|s| name_index(&DYE_COLOR_NAMES, s))
            .unwrap_or(16),
        BlockEntityKind::Sign => name
            .strip_suffix("_sign")
            .or_else(|| name.strip_suffix("_wall_sign"))
            .and_then(|s| name_index(&SIGN_WOOD_NAMES, s))
            .unwrap_or(0),
        _ => 0,
    }
}

/// Values mirror vanilla's `direction.asRotation()` offset, since the draw
/// code applies `from_rotation_y((180 - yaw).to_radians())`.
pub fn yaw_for_block(kind: BlockEntityKind, props: &HashMap<&str, &str>) -> f32 {
    match kind {
        BlockEntityKind::Chest
        | BlockEntityKind::TrappedChest
        | BlockEntityKind::EnderChest
        | BlockEntityKind::ShulkerBox => match props.get("facing").copied() {
            Some("south") => 0.0,
            Some("west") => 90.0,
            Some("north") => 180.0,
            Some("east") => 270.0,
            _ => 0.0,
        },
        // TODO: wall signs have no `rotation` (they use `facing`) and vanilla
        // renders them with a postless model offset against the wall; they
        // currently fall back to the standing model facing south.
        BlockEntityKind::Sign => props
            .get("rotation")
            .and_then(|s| s.parse::<f32>().ok())
            .map(|r| r * 22.5)
            .unwrap_or(0.0),
        _ => 0.0,
    }
}

fn kind_definitions() -> Vec<KindDef> {
    vec![
        KindDef {
            kind: BlockEntityKind::Chest,
            model: block_entity_model::bake_chest_model(),
            tex_variants: &[&["minecraft/textures/entity/chest/normal.png"]],
            tex_size: 64,
        },
        KindDef {
            kind: BlockEntityKind::ShulkerBox,
            model: block_entity_model::bake_shulker_box_model(),
            tex_variants: SHULKER_TEXTURES,
            tex_size: 64,
        },
        KindDef {
            kind: BlockEntityKind::Sign,
            model: block_entity_model::bake_sign_model(),
            tex_variants: SIGN_TEXTURES,
            tex_size: 32,
        },
    ]
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
            size: 112,
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

        let pipeline = create_pipeline(
            device,
            render_pass,
            pipeline_layout,
            BlendMode::Opaque,
            ModelInput::PushConstant,
        );

        let defs = kind_definitions();
        let tex_count = defs
            .iter()
            .map(|d| d.tex_variants.len() as u32)
            .sum::<u32>();

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
        let mut pending_uploads: Vec<util::PendingImageUpload> = Vec::new();
        let mut staging_to_free: Vec<(vk::Buffer, Allocation)> = Vec::new();
        for def in defs {
            let entry = build_entry(
                device,
                allocator,
                descriptor_pool,
                texture_layout,
                texture_sampler,
                jar_assets_dir,
                asset_index,
                def.model,
                def.tex_variants,
                def.tex_size,
                &mut pending_uploads,
                &mut staging_to_free,
            );
            entries.insert(def.kind, entry);
        }

        util::upload_images_batched(device, queue, command_pool, &pending_uploads);

        {
            let mut alloc = allocator.lock().unwrap();
            for (buf, a) in staging_to_free {
                device.destroy_buffer(buf, None);
                alloc.free(a).ok();
            }
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

        let mut bound_entry: *const KindEntry = std::ptr::null();
        let mut bound_set: vk::DescriptorSet = vk::DescriptorSet::null();

        for info in items {
            let Some(entry) = self.entries.get(&info.kind) else {
                continue;
            };
            let variant_idx = (info.variant as usize).min(entry.textures.len().saturating_sub(1));
            let slot = &entry.textures[variant_idx];

            let entry_ptr: *const KindEntry = entry;
            if bound_entry != entry_ptr {
                cmd.bind_vertex_buffers(0, &[entry.vertex_buffer], &[0]);
                bound_entry = entry_ptr;
                bound_set = vk::DescriptorSet::null();
            }
            if bound_set != slot.set {
                cmd.bind_descriptor_sets(
                    vk::PipelineBindPoint::Graphics,
                    self.pipeline_layout,
                    0,
                    &[self.camera_sets[frame], slot.set],
                    &[],
                );
                bound_set = slot.set;
            }

            let block_center = glam::Vec3::new(
                info.pos.x as f32 + 0.5,
                info.pos.y as f32,
                info.pos.z as f32 + 0.5,
            );
            let model_mat = glam::Mat4::from_translation(block_center)
                * glam::Mat4::from_rotation_y((180.0f32 - info.yaw).to_radians());

            let anim = lid_anim(info.kind, info.lid_open);
            let part_transforms = entry.model.compute_part_transforms(&anim);
            for (i, (start, count)) in entry.model.part_ranges.iter().enumerate() {
                if *count == 0 {
                    continue;
                }
                let part_mat = model_mat * part_transforms[i];
                let cols = part_mat.to_cols_array();
                // Shared entity shader push block: mat, tint, overlay_color, uv_params.
                // Block entities are opaque with no hurt flash or UV scroll.
                let no_overlay = [0.0f32, 0.0, 0.0, 1.0];
                let uv_params = [0.0f32; 4];
                let mut bytes = [0u8; 112];
                bytes[..64].copy_from_slice(bytemuck::cast_slice(&cols));
                bytes[64..80].copy_from_slice(bytemuck::cast_slice(&WHITE_TINT));
                bytes[80..96].copy_from_slice(bytemuck::cast_slice(&no_overlay));
                bytes[96..112].copy_from_slice(bytemuck::cast_slice(&uv_params));
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
        self.pipeline = create_pipeline(
            device,
            render_pass,
            self.pipeline_layout,
            BlendMode::Opaque,
            ModelInput::PushConstant,
        );
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
            for slot in entry.textures.iter_mut() {
                device.destroy_image_view(slot.view, None);
                alloc
                    .free(std::mem::replace(&mut slot.allocation, unsafe {
                        std::mem::zeroed()
                    }))
                    .ok();
                device.destroy_image(slot.image, None);
            }
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
    allocator: &Arc<Mutex<Allocator>>,
    descriptor_pool: vk::DescriptorPool,
    texture_layout: vk::DescriptorSetLayout,
    texture_sampler: vk::Sampler,
    jar_assets_dir: &Path,
    asset_index: &Option<AssetIndex>,
    model: BakedEntityModel,
    tex_variants: &[&[&str]],
    fallback_tex_size: u32,
    pending_uploads: &mut Vec<util::PendingImageUpload>,
    staging_to_free: &mut Vec<(vk::Buffer, Allocation)>,
) -> KindEntry {
    let vert_bytes = bytemuck::cast_slice::<ChunkVertex, u8>(&model.vertices);
    let (vertex_buffer, vertex_allocation) = util::create_mapped_buffer(
        device,
        allocator,
        vert_bytes,
        vk::BufferUsageFlags::VertexBuffer,
        "block_entity_vertices",
    );

    let textures = tex_variants
        .iter()
        .map(|keys| {
            build_texture_slot(
                device,
                allocator,
                descriptor_pool,
                texture_layout,
                texture_sampler,
                jar_assets_dir,
                asset_index,
                keys,
                fallback_tex_size,
                pending_uploads,
                staging_to_free,
            )
        })
        .collect();

    KindEntry {
        model,
        vertex_buffer,
        vertex_allocation,
        textures,
    }
}

#[allow(clippy::too_many_arguments)]
fn build_texture_slot(
    device: &vk::Device,
    allocator: &Arc<Mutex<Allocator>>,
    descriptor_pool: vk::DescriptorPool,
    texture_layout: vk::DescriptorSetLayout,
    texture_sampler: vk::Sampler,
    jar_assets_dir: &Path,
    asset_index: &Option<AssetIndex>,
    keys: &[&str],
    fallback_tex_size: u32,
    pending_uploads: &mut Vec<util::PendingImageUpload>,
    staging_to_free: &mut Vec<(vk::Buffer, Allocation)>,
) -> TextureSlot {
    let (pixels, width, height) = keys
        .iter()
        .find_map(|key| {
            let path = resolve_asset_path(jar_assets_dir, asset_index, key);
            util::load_png(&path)
        })
        .unwrap_or_else(|| {
            tracing::warn!("Failed to load BE texture {:?}, using fallback", keys);
            fallback_texture(fallback_tex_size)
        });

    let (image, view, allocation) =
        util::create_gpu_image(device, allocator, width, height, "block_entity_texture");
    let (staging_buf, staging_alloc) =
        util::create_staging_buffer(device, allocator, &pixels, "block_entity_texture_staging");
    pending_uploads.push(util::PendingImageUpload {
        staging_buffer: staging_buf,
        staging_size: pixels.len() as u64,
        image,
        width,
        height,
        mip_levels: 1,
    });
    staging_to_free.push((staging_buf, staging_alloc));

    let tex_alloc_info = vk::DescriptorSetAllocateInfo {
        descriptor_pool,
        descriptor_set_count: 1,
        set_layouts: &texture_layout,
        ..Default::default()
    };
    let mut set = vk::DescriptorSet::null();
    device
        .allocate_descriptor_sets(&tex_alloc_info, slice::from_mut(&mut set))
        .expect("failed to allocate BE texture descriptor set");

    let image_info = vk::DescriptorImageInfo {
        sampler: texture_sampler,
        image_view: view,
        image_layout: vk::ImageLayout::ShaderReadOnlyOptimal,
    };
    let tex_write = vk::WriteDescriptorSet {
        dst_set: set,
        dst_binding: 0,
        descriptor_type: vk::DescriptorType::CombinedImageSampler,
        descriptor_count: 1,
        image_info: &image_info,
        ..Default::default()
    };
    device.update_descriptor_sets(&[tex_write], &[]);

    TextureSlot {
        image,
        view,
        allocation,
        set,
    }
}
