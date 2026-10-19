//! VR's GUI panel: the HUD, screens and menus drawn offscreen at Vivecraft's
//! virtual screen size, then shown in the headset as a quad layer.

use std::sync::{Arc, Mutex};

use pomme_gpu_allocator::vulkan::Allocator;
use pyronyx::vk;

use super::context::{ContextError, VulkanContext};
use super::swapchain::{Attachment, create_framebuffer, create_render_pass_variant};
use super::util;

/// Vivecraft `GUI_WIDTH`/`GUI_HEIGHT`.
pub(super) const PANEL_EXTENT: vk::Extent2D = vk::Extent2D {
    width: 1280,
    height: 720,
};
/// Height over width.
pub(super) const PANEL_ASPECT: f32 = PANEL_EXTENT.height as f32 / PANEL_EXTENT.width as f32;

/// What the panel shows, which decides where it floats.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GuiPanel {
    /// In the world with no screen open.
    Hud,
    /// A screen over the world: inventory, pause, death.
    Screen,
    /// The chat screen, which Vivecraft lifts above the others.
    Chat,
    /// The title and other out-of-world screens.
    Menu,
}

/// The panel in the window's format, so the 2D pipelines built for the window
/// draw into it unchanged. Both passes end in ColorAttachmentOptimal, ready
/// to blur or copy out.
pub(super) struct PanelTarget {
    pub color: Attachment,
    depth: Attachment,
    pub clear_pass: vk::RenderPass,
    pub clear_framebuffer: vk::Framebuffer,
    pub load_pass: vk::RenderPass,
    pub load_framebuffer: vk::Framebuffer,
}

impl PanelTarget {
    pub fn new(ctx: &VulkanContext, format: vk::Format) -> Result<Self, ContextError> {
        let color = Attachment::new(
            &ctx.device,
            &ctx.allocator,
            PANEL_EXTENT,
            1,
            format,
            vk::ImageUsageFlags::ColorAttachment | vk::ImageUsageFlags::TransferSrc,
            util::COLOR_SUBRESOURCE_RANGE,
        )?;
        let depth = Attachment::depth(&ctx.device, &ctx.allocator, PANEL_EXTENT, 1)?;
        let pass = |load_op, initial_layout| {
            create_render_pass_variant(
                &ctx.device,
                format,
                load_op,
                initial_layout,
                vk::ImageLayout::ColorAttachmentOptimal,
                0,
            )
        };
        let clear_pass = pass(vk::AttachmentLoadOp::Clear, vk::ImageLayout::Undefined)?;
        let load_pass = pass(
            vk::AttachmentLoadOp::Load,
            vk::ImageLayout::ColorAttachmentOptimal,
        )?;
        let framebuffer =
            |rp| create_framebuffer(&ctx.device, rp, color.view, depth.view, PANEL_EXTENT);
        Ok(Self {
            clear_framebuffer: framebuffer(clear_pass)?,
            load_framebuffer: framebuffer(load_pass)?,
            color,
            depth,
            clear_pass,
            load_pass,
        })
    }

    pub fn destroy(&mut self, device: &vk::Device, allocator: &Arc<Mutex<Allocator>>) {
        for (fb, rp) in [
            (self.clear_framebuffer, self.clear_pass),
            (self.load_framebuffer, self.load_pass),
        ] {
            device.destroy_framebuffer(fb, None);
            device.destroy_render_pass(rp, None);
        }
        self.color.destroy(device, allocator);
        self.depth.destroy(device, allocator);
    }
}
