//! The VR runtime's images as render targets: the eyes, both drawn in one
//! multiview pass (`gl_ViewIndex` picks the eye's matrix), and the GUI panel,
//! copied in from `PanelTarget`. Plus the copies onto the window that mirror
//! the headset.

use std::sync::{Arc, Mutex};

use pomme_gpu_allocator::vulkan::Allocator;
use pyronyx::vk;

use super::context::{ContextError, VK_API_VERSION, VulkanContext, to_raw};
use super::panel::{PANEL_EXTENT, PanelTarget};
use super::swapchain::{Attachment, SRGB_FORMATS, create_framebuffer, create_render_pass_variant};
use super::util;
use crate::stereo::{StereoBackend, StereoDevice};

/// Views 0 and 1: the left and right eye, image layers 0 and 1.
const EYES_VIEW_MASK: u32 = 0b11;
const EYE_LAYERS: u32 = 2;

/// The runtime and the targets it renders into.
pub(super) struct Stereo {
    pub backend: Box<dyn StereoBackend>,
    pub eyes: StereoTarget,
    pub panel: PanelTarget,
    /// The runtime's panel images, which `panel` is copied into.
    panel_images: Vec<vk::Image>,
}

impl Stereo {
    /// `window_format` is the window's, so the 2D pipelines built for the
    /// window can draw the panel too.
    pub fn start(
        ctx: &VulkanContext,
        mut backend: Box<dyn StereoBackend>,
        window_format: vk::Format,
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
                (PANEL_EXTENT.width, PANEL_EXTENT.height),
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
        let eye_images = images.images.into_iter().map(vk::Image).collect();
        Ok(Self {
            backend,
            eyes: StereoTarget::new(ctx, format, extent, eye_images)?,
            panel: PanelTarget::new(ctx, window_format)?,
            panel_images: images.panel_images.into_iter().map(vk::Image).collect(),
        })
    }

    /// The left eye, scaled onto the window.
    pub fn record_mirror(&self, cmd: vk::CommandBuffer, eye_index: usize, window: BlitImage) {
        blit(
            cmd,
            BlitImage {
                image: self.eyes.images[eye_index],
                extent: self.eyes.extent,
                before: vk::ImageLayout::ColorAttachmentOptimal,
                after: vk::ImageLayout::ColorAttachmentOptimal,
            },
            window,
        );
    }

    /// The panel into the runtime's panel image, and onto the window too
    /// when there's no eye view to mirror.
    pub fn record_panel_copies(
        &self,
        cmd: vk::CommandBuffer,
        panel_index: Option<usize>,
        window: Option<BlitImage>,
    ) {
        let panel = BlitImage {
            image: self.panel.color.image,
            extent: PANEL_EXTENT,
            before: vk::ImageLayout::ColorAttachmentOptimal,
            after: vk::ImageLayout::TransferSrcOptimal,
        };
        if let Some(index) = panel_index {
            blit(
                cmd,
                panel,
                BlitImage {
                    image: self.panel_images[index],
                    extent: PANEL_EXTENT,
                    before: vk::ImageLayout::Undefined,
                    after: vk::ImageLayout::ColorAttachmentOptimal,
                },
            );
        }
        if let Some(window) = window {
            let before = if panel_index.is_some() {
                vk::ImageLayout::TransferSrcOptimal
            } else {
                panel.before
            };
            blit(cmd, BlitImage { before, ..panel }, window);
        }
    }

    /// Ends the session before the device it renders with goes.
    pub fn destroy(mut self, device: &vk::Device, allocator: &Arc<Mutex<Allocator>>) {
        self.eyes.destroy(device, allocator);
        self.panel.destroy(device, allocator);
        drop(self.backend);
    }
}

pub(super) struct StereoTarget {
    pub render_pass: vk::RenderPass,
    pub extent: vk::Extent2D,
    images: Vec<vk::Image>,
    views: Vec<vk::ImageView>,
    pub framebuffers: Vec<vk::Framebuffer>,
    depth: Attachment,
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
        let depth = Attachment::depth(&ctx.device, &ctx.allocator, extent, EYE_LAYERS)?;
        let views = images
            .iter()
            .map(|&image| {
                let info = vk::ImageViewCreateInfo {
                    image,
                    view_type: vk::ImageViewType::Type2DArray,
                    format,
                    subresource_range: vk::ImageSubresourceRange {
                        layer_count: EYE_LAYERS,
                        ..util::COLOR_SUBRESOURCE_RANGE
                    },
                    ..Default::default()
                };
                ctx.device.create_image_view(&info, None)
            })
            .collect::<Result<Vec<_>, _>>()?;
        let framebuffers = views
            .iter()
            .map(|&view| create_framebuffer(&ctx.device, render_pass, view, depth.view, extent))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self {
            render_pass,
            extent,
            images,
            views,
            framebuffers,
            depth,
        })
    }

    fn destroy(&mut self, device: &vk::Device, allocator: &Arc<Mutex<Allocator>>) {
        for &fb in &self.framebuffers {
            device.destroy_framebuffer(fb, None);
        }
        for &view in &self.views {
            device.destroy_image_view(view, None);
        }
        device.destroy_render_pass(self.render_pass, None);
        self.depth.destroy(device, allocator);
    }
}

/// One side of a `blit`: layer 0 of `image`, and the layouts it's in before
/// and should be left in after.
#[derive(Clone, Copy)]
pub(super) struct BlitImage {
    pub image: vk::Image,
    pub extent: vk::Extent2D,
    pub before: vk::ImageLayout,
    pub after: vk::ImageLayout,
}

/// Scales `src` onto the whole of `dst`, moving each through the transfer
/// layouts.
pub(super) fn blit(cmd: vk::CommandBuffer, src: BlitImage, dst: BlitImage) {
    let barrier = |image, old_layout, new_layout| vk::ImageMemoryBarrier {
        image,
        old_layout,
        new_layout,
        src_access_mask: vk::AccessFlags::MemoryWrite,
        dst_access_mask: vk::AccessFlags::MemoryRead | vk::AccessFlags::MemoryWrite,
        subresource_range: util::COLOR_SUBRESOURCE_RANGE,
        ..Default::default()
    };
    let transition = |barriers: &[vk::ImageMemoryBarrier]| {
        cmd.pipeline_barrier(
            vk::PipelineStageFlags::AllCommands,
            vk::PipelineStageFlags::AllCommands,
            vk::DependencyFlags::empty(),
            &[],
            &[],
            barriers,
        );
    };
    transition(&[
        barrier(src.image, src.before, vk::ImageLayout::TransferSrcOptimal),
        barrier(dst.image, dst.before, vk::ImageLayout::TransferDstOptimal),
    ]);
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
    let region = vk::ImageBlit {
        src_subresource: layer,
        src_offsets: [vk::Offset3D::default(), corner(src.extent)],
        dst_subresource: layer,
        dst_offsets: [vk::Offset3D::default(), corner(dst.extent)],
    };
    cmd.blit_image(
        src.image,
        vk::ImageLayout::TransferSrcOptimal,
        dst.image,
        vk::ImageLayout::TransferDstOptimal,
        &[region],
        vk::Filter::Linear,
    );
    transition(&[
        barrier(src.image, vk::ImageLayout::TransferSrcOptimal, src.after),
        barrier(dst.image, vk::ImageLayout::TransferDstOptimal, dst.after),
    ]);
}
