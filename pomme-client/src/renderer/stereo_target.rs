//! The VR runtime's images as a multiview render target: both eyes drawn in
//! one pass (`gl_ViewIndex` picks the eye's matrix), plus the copy of the
//! left eye that mirrors the headset on the window.

use std::sync::{Arc, Mutex};

use pomme_gpu_allocator::vulkan::{Allocation, Allocator};
use pyronyx::vk;

use super::context::{ContextError, VK_API_VERSION, VulkanContext, to_raw};
use super::swapchain::{
    SRGB_FORMATS, create_depth_resources, create_render_pass_variant, destroy_depth_resources,
};
use super::util;
use crate::stereo::{StereoBackend, StereoDevice};

/// Views 0 and 1: the left and right eye, image layers 0 and 1.
const EYES_VIEW_MASK: u32 = 0b11;
const EYE_LAYERS: u32 = 2;

/// The runtime and the target it renders into.
pub(super) struct Stereo {
    pub backend: Box<dyn StereoBackend>,
    pub target: StereoTarget,
}

impl Stereo {
    pub fn start(
        ctx: &VulkanContext,
        mut backend: Box<dyn StereoBackend>,
    ) -> Result<Self, ContextError> {
        let images = backend
            .start(
                StereoDevice {
                    instance: to_raw(ctx.instance.handle()),
                    physical_device: to_raw(ctx.physical_device.handle()),
                    device: to_raw(ctx.device.handle()),
                    queue_family: ctx.graphics_family,
                    queue_index: 0,
                    api_version: VK_API_VERSION,
                },
                &SRGB_FORMATS.map(|f| f as i64),
            )
            .map_err(ContextError::Stereo)?;
        let format = SRGB_FORMATS
            .into_iter()
            .find(|&f| f as i64 == images.format)
            .ok_or_else(|| ContextError::Stereo("runtime picked an unrequested format".into()))?;
        let extent = vk::Extent2D {
            width: images.width,
            height: images.height,
        };
        let images = images.images.into_iter().map(vk::Image).collect();
        let target = StereoTarget::new(ctx, format, extent, images)?;
        Ok(Self { backend, target })
    }

    /// Ends the session before the device it renders with goes.
    pub fn destroy(mut self, device: &vk::Device, allocator: &Arc<Mutex<Allocator>>) {
        self.target.destroy(device, allocator);
        drop(self.backend);
    }
}

pub(super) struct StereoTarget {
    pub render_pass: vk::RenderPass,
    pub extent: vk::Extent2D,
    images: Vec<vk::Image>,
    views: Vec<vk::ImageView>,
    pub framebuffers: Vec<vk::Framebuffer>,
    depth_image: vk::Image,
    depth_view: vk::ImageView,
    depth_allocation: Option<Allocation>,
}

impl StereoTarget {
    fn new(
        ctx: &VulkanContext,
        format: vk::Format,
        extent: vk::Extent2D,
        images: Vec<vk::Image>,
    ) -> Result<Self, ContextError> {
        // Ends in ColorAttachmentOptimal, the layout the runtime takes images
        // back in.
        let render_pass = create_render_pass_variant(
            &ctx.device,
            format,
            vk::AttachmentLoadOp::Clear,
            vk::ImageLayout::Undefined,
            vk::ImageLayout::ColorAttachmentOptimal,
            EYES_VIEW_MASK,
        )?;
        let (depth_image, depth_view, depth_allocation) =
            create_depth_resources(&ctx.device, extent, EYE_LAYERS, &ctx.allocator)?;
        let views = images
            .iter()
            .map(|&image| {
                let info = vk::ImageViewCreateInfo {
                    image,
                    view_type: vk::ImageViewType::Type2DArray,
                    format,
                    subresource_range: EYES_RANGE,
                    ..Default::default()
                };
                ctx.device.create_image_view(&info, None)
            })
            .collect::<Result<Vec<_>, _>>()?;
        let framebuffers = views
            .iter()
            .map(|&view| {
                let attachments = [view, depth_view];
                // Multiview framebuffers have one layer; the view mask picks
                // the image layers.
                let info = vk::FramebufferCreateInfo {
                    render_pass,
                    attachment_count: attachments.len() as u32,
                    attachments: attachments.as_ptr(),
                    width: extent.width,
                    height: extent.height,
                    layers: 1,
                    ..Default::default()
                };
                ctx.device.create_framebuffer(&info, None)
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self {
            render_pass,
            extent,
            images,
            views,
            framebuffers,
            depth_image,
            depth_view,
            depth_allocation: Some(depth_allocation),
        })
    }

    /// Scales the left eye onto the window image, leaving the eye image in
    /// ColorAttachmentOptimal for the runtime and the window image in
    /// ColorAttachmentOptimal for `render_pass_load`.
    pub fn record_mirror(
        &self,
        cmd: vk::CommandBuffer,
        image_index: usize,
        window_image: vk::Image,
        window_extent: vk::Extent2D,
    ) {
        let eye = self.images[image_index];
        let barrier = |image, old_layout, new_layout, src_access_mask, dst_access_mask| {
            vk::ImageMemoryBarrier {
                image,
                old_layout,
                new_layout,
                src_access_mask,
                dst_access_mask,
                subresource_range: util::COLOR_SUBRESOURCE_RANGE,
                ..Default::default()
            }
        };
        cmd.pipeline_barrier(
            vk::PipelineStageFlags::ColorAttachmentOutput,
            vk::PipelineStageFlags::Transfer,
            vk::DependencyFlags::empty(),
            &[],
            &[],
            &[
                barrier(
                    eye,
                    vk::ImageLayout::ColorAttachmentOptimal,
                    vk::ImageLayout::TransferSrcOptimal,
                    vk::AccessFlags::ColorAttachmentWrite,
                    vk::AccessFlags::TransferRead,
                ),
                barrier(
                    window_image,
                    vk::ImageLayout::Undefined,
                    vk::ImageLayout::TransferDstOptimal,
                    vk::AccessFlags::empty(),
                    vk::AccessFlags::TransferWrite,
                ),
            ],
        );
        let layer = vk::ImageSubresourceLayers {
            aspect_mask: vk::ImageAspectFlags::Color,
            mip_level: 0,
            base_array_layer: 0,
            layer_count: 1,
        };
        let corner = |e: vk::Extent2D| vk::Offset3D {
            x: e.width as i32,
            y: e.height as i32,
            z: 1,
        };
        let blit = vk::ImageBlit {
            src_subresource: layer,
            src_offsets: [vk::Offset3D::default(), corner(self.extent)],
            dst_subresource: layer,
            dst_offsets: [vk::Offset3D::default(), corner(window_extent)],
        };
        cmd.blit_image(
            eye,
            vk::ImageLayout::TransferSrcOptimal,
            window_image,
            vk::ImageLayout::TransferDstOptimal,
            &[blit],
            vk::Filter::Linear,
        );
        cmd.pipeline_barrier(
            vk::PipelineStageFlags::Transfer,
            vk::PipelineStageFlags::ColorAttachmentOutput,
            vk::DependencyFlags::empty(),
            &[],
            &[],
            &[
                barrier(
                    eye,
                    vk::ImageLayout::TransferSrcOptimal,
                    vk::ImageLayout::ColorAttachmentOptimal,
                    vk::AccessFlags::TransferRead,
                    vk::AccessFlags::empty(),
                ),
                barrier(
                    window_image,
                    vk::ImageLayout::TransferDstOptimal,
                    vk::ImageLayout::ColorAttachmentOptimal,
                    vk::AccessFlags::TransferWrite,
                    vk::AccessFlags::ColorAttachmentRead | vk::AccessFlags::ColorAttachmentWrite,
                ),
            ],
        );
    }

    fn destroy(&mut self, device: &vk::Device, allocator: &Arc<Mutex<Allocator>>) {
        for &fb in &self.framebuffers {
            device.destroy_framebuffer(fb, None);
        }
        for &view in &self.views {
            device.destroy_image_view(view, None);
        }
        device.destroy_render_pass(self.render_pass, None);
        destroy_depth_resources(
            device,
            allocator,
            self.depth_image,
            self.depth_view,
            self.depth_allocation.take(),
        );
    }
}

const EYES_RANGE: vk::ImageSubresourceRange = vk::ImageSubresourceRange {
    layer_count: EYE_LAYERS,
    ..util::COLOR_SUBRESOURCE_RANGE
};
