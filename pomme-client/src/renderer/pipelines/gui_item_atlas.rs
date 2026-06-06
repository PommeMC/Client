use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use pomme_gpu_allocator::MemoryLocation;
use pomme_gpu_allocator::vulkan::{Allocation, AllocationCreateDesc, AllocationScheme, Allocator};
use pyronyx::vk;

use crate::renderer::util;

// Target capacity per atlas page in slots. The atlas is sized to fit this many
// at the active slot size, clamped to MIN_ATLAS_PX so very small slots still
// hit the device's framebuffer minimums. MAX_ATLAS_PX is a conservative cap;
// vanilla queries `RenderSystem.getDevice().limits().maxTextureSizeForFormat`.
const TARGET_SLOT_CAPACITY: u32 = 256;
const MIN_ATLAS_PX: u32 = 512;
const MAX_ATLAS_PX: u32 = 4096;

pub fn is_animated_item(item_name: &str) -> bool {
    matches!(item_name, "compass" | "recovery_compass" | "clock")
}

const COLOR_FORMAT: vk::Format = vk::Format::R8G8B8A8Unorm;
const DEPTH_FORMAT: vk::Format = vk::Format::D32Sfloat;

/// Returns the slot size in pixels matching `16 * gui_scale`, the vanilla
/// formula (`GuiRenderer.prepareItemElements`). 16 is the base item size in
/// logical GUI units; multiplying by gui_scale makes the bake 1:1 with display.
pub fn slot_px_for_gui_scale(gui_scale: f32) -> u32 {
    (16.0 * gui_scale.max(1.0)).round() as u32
}

/// Picks the smallest power-of-two atlas size that fits TARGET_SLOT_CAPACITY
/// slots of the given size, clamped to [MIN_ATLAS_PX, MAX_ATLAS_PX].
fn atlas_px_for_slot(slot_px: u32) -> u32 {
    let slots_per_side = (TARGET_SLOT_CAPACITY as f32).sqrt().ceil() as u32;
    let needed = slots_per_side * slot_px;
    needed.next_power_of_two().clamp(MIN_ATLAS_PX, MAX_ATLAS_PX)
}

#[derive(Clone, Copy)]
pub enum SlotState {
    Empty,
    Stale,
    Ready,
}

#[derive(Clone, Copy)]
pub struct Slot {
    x: u32,
    y: u32,
}

struct SlotInternal {
    x: u32,
    y: u32,
    fresh: bool,
    discard_after_frame: bool,
}

struct DynamicAtlasAllocator {
    slots: Vec<SlotInternal>,
    used_by_key: HashMap<String, usize>,
    free: Vec<bool>,
}

impl DynamicAtlasAllocator {
    fn new(width: u32, height: u32) -> Self {
        let total = (width * height) as usize;
        let mut slots = Vec::with_capacity(total);
        for y in 0..height {
            for x in 0..width {
                slots.push(SlotInternal {
                    x,
                    y,
                    fresh: true,
                    discard_after_frame: false,
                });
            }
        }
        Self {
            slots,
            used_by_key: HashMap::new(),
            free: vec![true; total],
        }
    }

    fn get_or_allocate(
        &mut self,
        key: &str,
        discard_after_frame: bool,
    ) -> Option<(Slot, SlotState)> {
        if let Some(&idx) = self.used_by_key.get(key) {
            let s = &mut self.slots[idx];
            s.discard_after_frame |= discard_after_frame;
            return Some((Slot { x: s.x, y: s.y }, SlotState::Ready));
        }
        let idx = self.free.iter().position(|f| *f)?;
        self.free[idx] = false;
        let s = &mut self.slots[idx];
        let state = if s.fresh {
            SlotState::Empty
        } else {
            SlotState::Stale
        };
        s.fresh = false;
        s.discard_after_frame = discard_after_frame;
        let slot = Slot { x: s.x, y: s.y };
        self.used_by_key.insert(key.to_string(), idx);
        Some((slot, state))
    }

    fn has_space_for_all(&self, keys: &HashSet<String>) -> bool {
        let mut total = self.used_by_key.len();
        for key in keys {
            if !self.used_by_key.contains_key(key) {
                total += 1;
            }
        }
        total <= self.slots.len()
    }

    fn reclaim_space_for(&mut self, keys: &HashSet<String>) -> bool {
        let preexisting = keys
            .iter()
            .filter(|k| self.used_by_key.contains_key(*k))
            .count();
        if preexisting == keys.len() {
            return true;
        }
        let mut needed = keys.len() - preexisting;
        self.free_slot_if(|key, _| {
            if needed == 0 || keys.contains(key) {
                false
            } else {
                needed -= 1;
                true
            }
        });
        needed == 0
    }

    fn end_frame(&mut self) {
        self.free_slot_if(|_, slot| slot.discard_after_frame);
    }

    fn free_slot_if(&mut self, mut predicate: impl FnMut(&str, &SlotInternal) -> bool) {
        let to_remove: Vec<String> = self
            .used_by_key
            .iter()
            .filter_map(|(k, &idx)| {
                if predicate(k, &self.slots[idx]) {
                    Some(k.clone())
                } else {
                    None
                }
            })
            .collect();
        for key in to_remove {
            if let Some(idx) = self.used_by_key.remove(&key) {
                self.free[idx] = true;
                self.slots[idx].discard_after_frame = false;
            }
        }
    }
}

pub struct GuiItemAtlas {
    color_image: vk::Image,
    color_view: vk::ImageView,
    color_alloc: Option<Allocation>,
    depth_image: vk::Image,
    depth_view: vk::ImageView,
    depth_alloc: Option<Allocation>,
    render_pass: vk::RenderPass,
    framebuffer: vk::Framebuffer,
    sampler: vk::Sampler,
    slot_px: u32,
    atlas_px: u32,
    allocator: DynamicAtlasAllocator,
}

impl GuiItemAtlas {
    pub fn new(
        device: &vk::Device,
        gpu_alloc: &Arc<Mutex<Allocator>>,
        queue: vk::Queue,
        command_pool: vk::CommandPool,
        slot_px: u32,
    ) -> Self {
        let atlas_px = atlas_px_for_slot(slot_px);
        let slots_per_side = atlas_px / slot_px;

        let (color_image, color_view, color_alloc) =
            create_color_image(device, gpu_alloc, atlas_px, atlas_px);
        let (depth_image, depth_view, depth_alloc) =
            create_depth_image(device, gpu_alloc, atlas_px, atlas_px);

        init_color_clear_and_transition(device, queue, command_pool, color_image);
        transition_depth_to_attachment(device, queue, command_pool, depth_image);

        let render_pass = create_render_pass(device);
        let framebuffer = create_framebuffer(
            device,
            render_pass,
            color_view,
            depth_view,
            atlas_px,
            atlas_px,
        );

        let sampler_info = vk::SamplerCreateInfo {
            mag_filter: vk::Filter::Nearest,
            min_filter: vk::Filter::Nearest,
            mipmap_mode: vk::SamplerMipmapMode::Nearest,
            address_mode_u: vk::SamplerAddressMode::ClampToEdge,
            address_mode_v: vk::SamplerAddressMode::ClampToEdge,
            address_mode_w: vk::SamplerAddressMode::ClampToEdge,
            min_lod: 0.0,
            max_lod: 0.0,
            ..Default::default()
        };
        let sampler = device
            .create_sampler(&sampler_info, None)
            .expect("failed to create gui_item_atlas sampler");

        Self {
            color_image,
            color_view,
            color_alloc: Some(color_alloc),
            depth_image,
            depth_view,
            depth_alloc: Some(depth_alloc),
            render_pass,
            framebuffer,
            sampler,
            slot_px,
            atlas_px,
            allocator: DynamicAtlasAllocator::new(slots_per_side, slots_per_side),
        }
    }

    pub fn slot_px(&self) -> u32 {
        self.slot_px
    }

    pub fn atlas_px(&self) -> u32 {
        self.atlas_px
    }

    pub fn render_pass(&self) -> vk::RenderPass {
        self.render_pass
    }

    pub fn get_or_allocate(
        &mut self,
        item_name: &str,
        discard_after_frame: bool,
    ) -> Option<(Slot, SlotState)> {
        self.allocator
            .get_or_allocate(item_name, discard_after_frame)
    }

    pub fn has_space_for_all(&self, keys: &HashSet<String>) -> bool {
        self.allocator.has_space_for_all(keys)
    }

    pub fn reclaim_space_for(&mut self, keys: &HashSet<String>) -> bool {
        self.allocator.reclaim_space_for(keys)
    }

    pub fn end_frame(&mut self) {
        self.allocator.end_frame();
    }

    /// Top-origin pixel coordinates of the slot's upper-left corner; pair with
    /// `slot_px()` for the size.
    pub fn slot_origin_pixels(&self, slot: &Slot) -> (u32, u32) {
        (slot.x * self.slot_px, slot.y * self.slot_px)
    }

    /// Bottom-origin framebuffer rect for the bake-pass scissor and slot clear.
    pub fn scissor_rect(&self, slot: &Slot) -> vk::Rect2D {
        vk::Rect2D {
            offset: vk::Offset2D {
                x: (slot.x * self.slot_px) as i32,
                y: self.atlas_px as i32 - (slot.y as i32 + 1) * self.slot_px as i32,
            },
            extent: vk::Extent2D {
                width: self.slot_px,
                height: self.slot_px,
            },
        }
    }

    pub fn slot_uv(&self, slot: &Slot) -> [f32; 4] {
        // V decreases because the bake projection Y-flips the mesh into the
        // atlas image, so the slot's top-of-mesh lands at the higher V row.
        let step = self.slot_px as f32 / self.atlas_px as f32;
        let u0 = slot.x as f32 * step;
        let v0 = 1.0 - slot.y as f32 * step;
        [u0, v0, u0 + step, v0 - step]
    }

    pub fn begin_bake_pass(&self, cmd: vk::CommandBuffer) {
        let clears = [
            vk::ClearValue {
                color: vk::ClearColorValue {
                    float32: [0.0, 0.0, 0.0, 0.0],
                },
            },
            vk::ClearValue {
                depth_stencil: vk::ClearDepthStencilValue {
                    depth: 1.0,
                    stencil: 0,
                },
            },
        ];
        let info = vk::RenderPassBeginInfo {
            render_pass: self.render_pass,
            framebuffer: self.framebuffer,
            render_area: vk::Rect2D {
                offset: vk::Offset2D { x: 0, y: 0 },
                extent: vk::Extent2D {
                    width: self.atlas_px,
                    height: self.atlas_px,
                },
            },
            clear_value_count: clears.len() as u32,
            clear_values: clears.as_ptr(),
            ..Default::default()
        };
        cmd.begin_render_pass(&info, vk::SubpassContents::Inline);
    }

    pub fn end_bake_pass(&self, cmd: vk::CommandBuffer) {
        cmd.end_render_pass();
    }

    pub fn clear_slot_color(&self, cmd: vk::CommandBuffer, slot: &Slot) {
        let clear_attachment = vk::ClearAttachment {
            aspect_mask: vk::ImageAspectFlags::Color,
            color_attachment: 0,
            clear_value: vk::ClearValue {
                color: vk::ClearColorValue {
                    float32: [0.0, 0.0, 0.0, 0.0],
                },
            },
        };
        let clear_rect = vk::ClearRect {
            rect: self.scissor_rect(slot),
            base_array_layer: 0,
            layer_count: 1,
        };
        cmd.clear_attachments(&[clear_attachment], &[clear_rect]);
    }

    pub fn color_view(&self) -> vk::ImageView {
        self.color_view
    }

    pub fn sampler(&self) -> vk::Sampler {
        self.sampler
    }

    pub fn destroy(&mut self, device: &vk::Device, gpu_alloc: &Arc<Mutex<Allocator>>) {
        device.destroy_framebuffer(self.framebuffer, None);
        device.destroy_render_pass(self.render_pass, None);
        device.destroy_sampler(self.sampler, None);
        device.destroy_image_view(self.color_view, None);
        device.destroy_image(self.color_image, None);
        device.destroy_image_view(self.depth_view, None);
        device.destroy_image(self.depth_image, None);
        if let Some(a) = self.color_alloc.take() {
            gpu_alloc.lock().unwrap().free(a).ok();
        }
        if let Some(a) = self.depth_alloc.take() {
            gpu_alloc.lock().unwrap().free(a).ok();
        }
    }
}

fn create_color_image(
    device: &vk::Device,
    allocator: &Arc<Mutex<Allocator>>,
    width: u32,
    height: u32,
) -> (vk::Image, vk::ImageView, Allocation) {
    create_atlas_image(
        device,
        allocator,
        width,
        height,
        COLOR_FORMAT,
        vk::ImageUsageFlags::ColorAttachment
            | vk::ImageUsageFlags::Sampled
            | vk::ImageUsageFlags::TransferDst,
        util::COLOR_SUBRESOURCE_RANGE,
        "gui_item_atlas_color",
    )
}

fn create_depth_image(
    device: &vk::Device,
    allocator: &Arc<Mutex<Allocator>>,
    width: u32,
    height: u32,
) -> (vk::Image, vk::ImageView, Allocation) {
    create_atlas_image(
        device,
        allocator,
        width,
        height,
        DEPTH_FORMAT,
        vk::ImageUsageFlags::DepthStencilAttachment,
        util::DEPTH_SUBRESOURCE_RANGE,
        "gui_item_atlas_depth",
    )
}

#[allow(clippy::too_many_arguments)]
fn create_atlas_image(
    device: &vk::Device,
    allocator: &Arc<Mutex<Allocator>>,
    width: u32,
    height: u32,
    format: vk::Format,
    usage: vk::ImageUsageFlags,
    subresource_range: vk::ImageSubresourceRange,
    name: &str,
) -> (vk::Image, vk::ImageView, Allocation) {
    let image_info = vk::ImageCreateInfo {
        image_type: vk::ImageType::Type2D,
        format,
        extent: vk::Extent3D {
            width,
            height,
            depth: 1,
        },
        mip_levels: 1,
        array_layers: 1,
        samples: vk::SampleCountFlags::Type1,
        tiling: vk::ImageTiling::Optimal,
        usage,
        ..Default::default()
    };
    let image = device
        .create_image(&image_info, None)
        .expect("failed to create atlas image");
    let mem_reqs = device.get_image_memory_requirements(image);
    let allocation = allocator
        .lock()
        .unwrap()
        .allocate(&AllocationCreateDesc {
            name,
            requirements: mem_reqs,
            location: MemoryLocation::GpuOnly,
            linear: false,
            allocation_scheme: AllocationScheme::GpuAllocatorManaged,
        })
        .expect("failed to allocate atlas image memory");
    unsafe {
        device
            .bind_image_memory(image, allocation.memory(), allocation.offset())
            .expect("failed to bind atlas image memory");
    }
    let view_info = vk::ImageViewCreateInfo {
        image,
        view_type: vk::ImageViewType::Type2D,
        format,
        subresource_range,
        ..Default::default()
    };
    let view = device
        .create_image_view(&view_info, None)
        .expect("failed to create atlas image view");
    (image, view, allocation)
}

fn init_color_clear_and_transition(
    device: &vk::Device,
    queue: vk::Queue,
    command_pool: vk::CommandPool,
    image: vk::Image,
) {
    util::submit_one_time(device, queue, command_pool, |cmd| {
        let to_transfer = vk::ImageMemoryBarrier {
            image,
            old_layout: vk::ImageLayout::Undefined,
            new_layout: vk::ImageLayout::TransferDstOptimal,
            src_access_mask: vk::AccessFlags::empty(),
            dst_access_mask: vk::AccessFlags::TransferWrite,
            subresource_range: util::COLOR_SUBRESOURCE_RANGE,
            ..Default::default()
        };
        cmd.pipeline_barrier(
            vk::PipelineStageFlags::TopOfPipe,
            vk::PipelineStageFlags::Transfer,
            vk::DependencyFlags::empty(),
            &[],
            &[],
            &[to_transfer],
        );

        let clear_color = vk::ClearColorValue {
            float32: [0.0, 0.0, 0.0, 0.0],
        };
        cmd.clear_color_image(
            image,
            vk::ImageLayout::TransferDstOptimal,
            &clear_color,
            &[util::COLOR_SUBRESOURCE_RANGE],
        );

        let to_shader_read = vk::ImageMemoryBarrier {
            image,
            old_layout: vk::ImageLayout::TransferDstOptimal,
            new_layout: vk::ImageLayout::ShaderReadOnlyOptimal,
            src_access_mask: vk::AccessFlags::TransferWrite,
            dst_access_mask: vk::AccessFlags::ShaderRead,
            subresource_range: util::COLOR_SUBRESOURCE_RANGE,
            ..Default::default()
        };
        cmd.pipeline_barrier(
            vk::PipelineStageFlags::Transfer,
            vk::PipelineStageFlags::FragmentShader,
            vk::DependencyFlags::empty(),
            &[],
            &[],
            &[to_shader_read],
        );
    });
}

fn transition_depth_to_attachment(
    device: &vk::Device,
    queue: vk::Queue,
    command_pool: vk::CommandPool,
    image: vk::Image,
) {
    util::submit_one_time(device, queue, command_pool, |cmd| {
        let barrier = vk::ImageMemoryBarrier {
            image,
            old_layout: vk::ImageLayout::Undefined,
            new_layout: vk::ImageLayout::DepthStencilAttachmentOptimal,
            src_access_mask: vk::AccessFlags::empty(),
            dst_access_mask: vk::AccessFlags::DepthStencilAttachmentWrite,
            subresource_range: util::DEPTH_SUBRESOURCE_RANGE,
            ..Default::default()
        };
        cmd.pipeline_barrier(
            vk::PipelineStageFlags::TopOfPipe,
            vk::PipelineStageFlags::EarlyFragmentTests,
            vk::DependencyFlags::empty(),
            &[],
            &[],
            &[barrier],
        );
    });
}

fn create_render_pass(device: &vk::Device) -> vk::RenderPass {
    let attachments = [
        vk::AttachmentDescription {
            format: COLOR_FORMAT,
            samples: vk::SampleCountFlags::Type1,
            load_op: vk::AttachmentLoadOp::Load,
            store_op: vk::AttachmentStoreOp::Store,
            initial_layout: vk::ImageLayout::ShaderReadOnlyOptimal,
            final_layout: vk::ImageLayout::ShaderReadOnlyOptimal,
            ..Default::default()
        },
        vk::AttachmentDescription {
            format: DEPTH_FORMAT,
            samples: vk::SampleCountFlags::Type1,
            load_op: vk::AttachmentLoadOp::Clear,
            store_op: vk::AttachmentStoreOp::DontCare,
            initial_layout: vk::ImageLayout::DepthStencilAttachmentOptimal,
            final_layout: vk::ImageLayout::DepthStencilAttachmentOptimal,
            ..Default::default()
        },
    ];

    let color_ref = vk::AttachmentReference {
        attachment: 0,
        layout: vk::ImageLayout::ColorAttachmentOptimal,
    };
    let depth_ref = vk::AttachmentReference {
        attachment: 1,
        layout: vk::ImageLayout::DepthStencilAttachmentOptimal,
    };
    let subpass = vk::SubpassDescription {
        pipeline_bind_point: vk::PipelineBindPoint::Graphics,
        color_attachments: &color_ref,
        color_attachment_count: 1,
        depth_stencil_attachment: &depth_ref,
        ..Default::default()
    };

    let dependencies = [
        vk::SubpassDependency {
            src_subpass: vk::SUBPASS_EXTERNAL,
            dst_subpass: 0,
            src_stage_mask: vk::PipelineStageFlags::FragmentShader,
            src_access_mask: vk::AccessFlags::ShaderRead,
            dst_stage_mask: vk::PipelineStageFlags::ColorAttachmentOutput
                | vk::PipelineStageFlags::EarlyFragmentTests,
            dst_access_mask: vk::AccessFlags::ColorAttachmentRead
                | vk::AccessFlags::ColorAttachmentWrite
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
        subpass_count: 1,
        subpasses: &subpass,
        dependency_count: dependencies.len() as u32,
        dependencies: dependencies.as_ptr(),
        ..Default::default()
    };
    device
        .create_render_pass(&info, None)
        .expect("failed to create gui_item_atlas render pass")
}

fn create_framebuffer(
    device: &vk::Device,
    render_pass: vk::RenderPass,
    color_view: vk::ImageView,
    depth_view: vk::ImageView,
    width: u32,
    height: u32,
) -> vk::Framebuffer {
    let attachments = [color_view, depth_view];
    let info = vk::FramebufferCreateInfo {
        render_pass,
        attachment_count: attachments.len() as u32,
        attachments: attachments.as_ptr(),
        width,
        height,
        layers: 1,
        ..Default::default()
    };
    device
        .create_framebuffer(&info, None)
        .expect("failed to create gui_item_atlas framebuffer")
}
