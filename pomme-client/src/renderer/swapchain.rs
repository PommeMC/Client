use std::sync::{Arc, Mutex};

use ash::khr::{surface, swapchain};
use ash::vk;
use gpu_allocator::MemoryLocation;
use gpu_allocator::vulkan::{Allocation, AllocationCreateDesc, AllocationScheme, Allocator};

use super::context::ContextError;
use super::util;

#[allow(dead_code)]
pub struct SwapchainState {
    pub swapchain: vk::SwapchainKHR,
    pub images: Vec<vk::Image>,
    pub image_views: Vec<vk::ImageView>,
    pub format: vk::SurfaceFormatKHR,
    pub extent: vk::Extent2D,
    pub depth_image: vk::Image,
    pub depth_view: vk::ImageView,
    pub depth_allocation: Option<Allocation>,
    pub render_pass: vk::RenderPass,
    pub render_pass_scene: vk::RenderPass,
    pub render_pass_load: vk::RenderPass,
    pub framebuffers: Vec<vk::Framebuffer>,
    pub framebuffers_scene: Vec<vk::Framebuffer>,
    pub framebuffers_load: Vec<vk::Framebuffer>,
}

impl SwapchainState {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        device: &ash::Device,
        surface_loader: &surface::Instance,
        swapchain_loader: &swapchain::Device,
        physical_device: vk::PhysicalDevice,
        surface: vk::SurfaceKHR,
        width: u32,
        height: u32,
        graphics_family: u32,
        present_family: u32,
        allocator: &Arc<Mutex<Allocator>>,
        old_swapchain: vk::SwapchainKHR,
    ) -> Result<Self, ContextError> {
        let caps = unsafe {
            surface_loader.get_physical_device_surface_capabilities(physical_device, surface)?
        };
        let formats = unsafe {
            surface_loader.get_physical_device_surface_formats(physical_device, surface)?
        };
        let present_modes = unsafe {
            surface_loader.get_physical_device_surface_present_modes(physical_device, surface)?
        };

        let format = formats
            .iter()
            .find(|f| {
                f.format == vk::Format::B8G8R8A8_SRGB
                    && f.color_space == vk::ColorSpaceKHR::SRGB_NONLINEAR
            })
            .copied()
            .unwrap_or(formats[0]);

        let present_mode = if present_modes.contains(&vk::PresentModeKHR::MAILBOX) {
            vk::PresentModeKHR::MAILBOX
        } else if present_modes.contains(&vk::PresentModeKHR::IMMEDIATE) {
            vk::PresentModeKHR::IMMEDIATE
        } else {
            vk::PresentModeKHR::FIFO
        };

        let extent = vk::Extent2D {
            width: width.clamp(caps.min_image_extent.width, caps.max_image_extent.width),
            height: height.clamp(caps.min_image_extent.height, caps.max_image_extent.height),
        };

        let image_count = (caps.min_image_count + 1).min(if caps.max_image_count == 0 {
            u32::MAX
        } else {
            caps.max_image_count
        });

        let (sharing_mode, queue_families) = if graphics_family != present_family {
            (
                vk::SharingMode::CONCURRENT,
                vec![graphics_family, present_family],
            )
        } else {
            (vk::SharingMode::EXCLUSIVE, vec![])
        };

        let swapchain_info = vk::SwapchainCreateInfoKHR::default()
            .surface(surface)
            .min_image_count(image_count)
            .image_format(format.format)
            .image_color_space(format.color_space)
            .image_extent(extent)
            .image_array_layers(1)
            .image_usage(vk::ImageUsageFlags::COLOR_ATTACHMENT | vk::ImageUsageFlags::TRANSFER_SRC)
            .image_sharing_mode(sharing_mode)
            .queue_family_indices(&queue_families)
            .pre_transform(caps.current_transform)
            .composite_alpha(vk::CompositeAlphaFlagsKHR::OPAQUE)
            .present_mode(present_mode)
            .clipped(true)
            .old_swapchain(old_swapchain);

        let swapchain = unsafe { swapchain_loader.create_swapchain(&swapchain_info, None)? };
        let images = unsafe { swapchain_loader.get_swapchain_images(swapchain)? };

        let image_views: Vec<vk::ImageView> = images
            .iter()
            .map(|&img| {
                let view_info = vk::ImageViewCreateInfo::default()
                    .image(img)
                    .view_type(vk::ImageViewType::TYPE_2D)
                    .format(format.format)
                    .subresource_range(util::COLOR_SUBRESOURCE_RANGE);
                unsafe { device.create_image_view(&view_info, None) }
            })
            .collect::<Result<Vec<_>, _>>()?;

        let (depth_image, depth_view, depth_allocation) =
            create_depth_resources(device, extent, allocator)?;

        let render_pass = create_render_pass(device, format.format)?;
        let render_pass_scene = create_render_pass_scene(device, format.format)?;
        let render_pass_load = create_render_pass_load(device, format.format)?;

        let make_fbs = |rp: vk::RenderPass| -> Result<Vec<vk::Framebuffer>, vk::Result> {
            image_views
                .iter()
                .map(|&view| {
                    let attachments = [view, depth_view];
                    let fb_info = vk::FramebufferCreateInfo::default()
                        .render_pass(rp)
                        .attachments(&attachments)
                        .width(extent.width)
                        .height(extent.height)
                        .layers(1);
                    unsafe { device.create_framebuffer(&fb_info, None) }
                })
                .collect()
        };

        let framebuffers = make_fbs(render_pass)?;
        let framebuffers_scene = make_fbs(render_pass_scene)?;
        let framebuffers_load = make_fbs(render_pass_load)?;

        Ok(Self {
            swapchain,
            images,
            image_views,
            format,
            extent,
            depth_image,
            depth_view,
            depth_allocation: Some(depth_allocation),
            render_pass,
            render_pass_scene,
            render_pass_load,
            framebuffers,
            framebuffers_scene,
            framebuffers_load,
        })
    }

    pub fn destroy(
        &mut self,
        device: &ash::Device,
        swapchain_loader: &swapchain::Device,
        allocator: &Arc<Mutex<Allocator>>,
    ) {
        unsafe {
            let _ = device.device_wait_idle();
        }

        for fbs in [
            &mut self.framebuffers,
            &mut self.framebuffers_scene,
            &mut self.framebuffers_load,
        ] {
            for &fb in fbs.iter() {
                unsafe { device.destroy_framebuffer(fb, None) };
            }
            fbs.clear();
        }

        for &rp in &[
            self.render_pass,
            self.render_pass_scene,
            self.render_pass_load,
        ] {
            unsafe { device.destroy_render_pass(rp, None) };
        }

        unsafe { device.destroy_image_view(self.depth_view, None) };
        if let Some(alloc) = self.depth_allocation.take() {
            allocator.lock().unwrap().free(alloc).ok();
        }
        unsafe { device.destroy_image(self.depth_image, None) };

        for &view in &self.image_views {
            unsafe { device.destroy_image_view(view, None) };
        }
        self.image_views.clear();

        unsafe { swapchain_loader.destroy_swapchain(self.swapchain, None) };
    }

    pub fn aspect_ratio(&self) -> f32 {
        self.extent.width as f32 / self.extent.height.max(1) as f32
    }
}

fn create_depth_resources(
    device: &ash::Device,
    extent: vk::Extent2D,
    allocator: &Arc<Mutex<Allocator>>,
) -> Result<(vk::Image, vk::ImageView, Allocation), ContextError> {
    let depth_format = vk::Format::D32_SFLOAT;

    let image_info = vk::ImageCreateInfo::default()
        .image_type(vk::ImageType::TYPE_2D)
        .format(depth_format)
        .extent(vk::Extent3D {
            width: extent.width,
            height: extent.height,
            depth: 1,
        })
        .mip_levels(1)
        .array_layers(1)
        .samples(vk::SampleCountFlags::TYPE_1)
        .tiling(vk::ImageTiling::OPTIMAL)
        .usage(vk::ImageUsageFlags::DEPTH_STENCIL_ATTACHMENT);

    let image = unsafe { device.create_image(&image_info, None)? };
    let mem_reqs = unsafe { device.get_image_memory_requirements(image) };

    let allocation = allocator.lock().unwrap().allocate(&AllocationCreateDesc {
        name: "depth_image",
        requirements: mem_reqs,
        location: MemoryLocation::GpuOnly,
        linear: false,
        allocation_scheme: AllocationScheme::GpuAllocatorManaged,
    })?;

    unsafe { device.bind_image_memory(image, allocation.memory(), allocation.offset())? };

    let view_info = vk::ImageViewCreateInfo::default()
        .image(image)
        .view_type(vk::ImageViewType::TYPE_2D)
        .format(depth_format)
        .subresource_range(util::DEPTH_SUBRESOURCE_RANGE);
    let view = unsafe { device.create_image_view(&view_info, None)? };

    Ok((image, view, allocation))
}

fn create_render_pass(
    device: &ash::Device,
    color_format: vk::Format,
) -> Result<vk::RenderPass, vk::Result> {
    let attachments = [
        vk::AttachmentDescription::default()
            .format(color_format)
            .samples(vk::SampleCountFlags::TYPE_1)
            .load_op(vk::AttachmentLoadOp::CLEAR)
            .store_op(vk::AttachmentStoreOp::STORE)
            .stencil_load_op(vk::AttachmentLoadOp::DONT_CARE)
            .stencil_store_op(vk::AttachmentStoreOp::DONT_CARE)
            .initial_layout(vk::ImageLayout::UNDEFINED)
            .final_layout(vk::ImageLayout::PRESENT_SRC_KHR),
        vk::AttachmentDescription::default()
            .format(vk::Format::D32_SFLOAT)
            .samples(vk::SampleCountFlags::TYPE_1)
            .load_op(vk::AttachmentLoadOp::CLEAR)
            .store_op(vk::AttachmentStoreOp::DONT_CARE)
            .stencil_load_op(vk::AttachmentLoadOp::DONT_CARE)
            .stencil_store_op(vk::AttachmentStoreOp::DONT_CARE)
            .initial_layout(vk::ImageLayout::UNDEFINED)
            .final_layout(vk::ImageLayout::DEPTH_STENCIL_ATTACHMENT_OPTIMAL),
    ];

    let color_ref = [vk::AttachmentReference {
        attachment: 0,
        layout: vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL,
    }];

    let depth_ref = vk::AttachmentReference {
        attachment: 1,
        layout: vk::ImageLayout::DEPTH_STENCIL_ATTACHMENT_OPTIMAL,
    };

    let subpass = [vk::SubpassDescription::default()
        .pipeline_bind_point(vk::PipelineBindPoint::GRAPHICS)
        .color_attachments(&color_ref)
        .depth_stencil_attachment(&depth_ref)];

    let dependency = [vk::SubpassDependency::default()
        .src_subpass(vk::SUBPASS_EXTERNAL)
        .dst_subpass(0)
        .src_stage_mask(
            vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT
                | vk::PipelineStageFlags::EARLY_FRAGMENT_TESTS,
        )
        .src_access_mask(vk::AccessFlags::empty())
        .dst_stage_mask(
            vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT
                | vk::PipelineStageFlags::EARLY_FRAGMENT_TESTS,
        )
        .dst_access_mask(
            vk::AccessFlags::COLOR_ATTACHMENT_WRITE
                | vk::AccessFlags::DEPTH_STENCIL_ATTACHMENT_WRITE,
        )];

    let render_pass_info = vk::RenderPassCreateInfo::default()
        .attachments(&attachments)
        .subpasses(&subpass)
        .dependencies(&dependency);

    unsafe { device.create_render_pass(&render_pass_info, None) }
}

fn create_render_pass_scene(
    device: &ash::Device,
    color_format: vk::Format,
) -> Result<vk::RenderPass, vk::Result> {
    create_render_pass_variant(
        device,
        color_format,
        vk::AttachmentLoadOp::CLEAR,
        vk::ImageLayout::UNDEFINED,
        vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL,
    )
}

fn create_render_pass_load(
    device: &ash::Device,
    color_format: vk::Format,
) -> Result<vk::RenderPass, vk::Result> {
    create_render_pass_variant(
        device,
        color_format,
        vk::AttachmentLoadOp::LOAD,
        vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL,
        vk::ImageLayout::PRESENT_SRC_KHR,
    )
}

fn create_render_pass_variant(
    device: &ash::Device,
    color_format: vk::Format,
    load_op: vk::AttachmentLoadOp,
    initial_layout: vk::ImageLayout,
    final_layout: vk::ImageLayout,
) -> Result<vk::RenderPass, vk::Result> {
    let attachments = [
        vk::AttachmentDescription::default()
            .format(color_format)
            .samples(vk::SampleCountFlags::TYPE_1)
            .load_op(load_op)
            .store_op(vk::AttachmentStoreOp::STORE)
            .stencil_load_op(vk::AttachmentLoadOp::DONT_CARE)
            .stencil_store_op(vk::AttachmentStoreOp::DONT_CARE)
            .initial_layout(initial_layout)
            .final_layout(final_layout),
        vk::AttachmentDescription::default()
            .format(vk::Format::D32_SFLOAT)
            .samples(vk::SampleCountFlags::TYPE_1)
            .load_op(vk::AttachmentLoadOp::CLEAR)
            .store_op(vk::AttachmentStoreOp::DONT_CARE)
            .stencil_load_op(vk::AttachmentLoadOp::DONT_CARE)
            .stencil_store_op(vk::AttachmentStoreOp::DONT_CARE)
            .initial_layout(vk::ImageLayout::UNDEFINED)
            .final_layout(vk::ImageLayout::DEPTH_STENCIL_ATTACHMENT_OPTIMAL),
    ];

    let color_ref = [vk::AttachmentReference {
        attachment: 0,
        layout: vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL,
    }];
    let depth_ref = vk::AttachmentReference {
        attachment: 1,
        layout: vk::ImageLayout::DEPTH_STENCIL_ATTACHMENT_OPTIMAL,
    };

    let subpass = [vk::SubpassDescription::default()
        .pipeline_bind_point(vk::PipelineBindPoint::GRAPHICS)
        .color_attachments(&color_ref)
        .depth_stencil_attachment(&depth_ref)];

    let dependency = [vk::SubpassDependency::default()
        .src_subpass(vk::SUBPASS_EXTERNAL)
        .dst_subpass(0)
        .src_stage_mask(
            vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT
                | vk::PipelineStageFlags::EARLY_FRAGMENT_TESTS,
        )
        .src_access_mask(vk::AccessFlags::empty())
        .dst_stage_mask(
            vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT
                | vk::PipelineStageFlags::EARLY_FRAGMENT_TESTS,
        )
        .dst_access_mask(
            vk::AccessFlags::COLOR_ATTACHMENT_WRITE
                | vk::AccessFlags::DEPTH_STENCIL_ATTACHMENT_WRITE,
        )];

    let info = vk::RenderPassCreateInfo::default()
        .attachments(&attachments)
        .subpasses(&subpass)
        .dependencies(&dependency);

    unsafe { device.create_render_pass(&info, None) }
}
