//! The first-run download screen: the loading splash on a bare Vulkan
//! context, before any game asset exists to build the full `Renderer` from.

use pyronyx::vk;
use winit::window::Window;

use super::context::VulkanContext;
use super::pipelines::menu_overlay::MenuOverlayPipeline;
use super::swapchain::Swapchain;
use super::{Renderer, RendererError};
use crate::ui::font::FontSources;

pub struct BootScreen {
    menu: MenuOverlayPipeline,
    swapchain: Swapchain,
    vsync: bool,
    ctx: VulkanContext,
}

impl BootScreen {
    pub fn new(
        window: &Window,
        font_sources: FontSources<'_>,
        vsync: bool,
    ) -> Result<Self, RendererError> {
        let (ctx, swapchain, menu) = bring_up(window, font_sources, vsync)?;
        Ok(Self {
            menu,
            swapchain,
            vsync,
            ctx,
        })
    }

    /// A minimised window (0×0) keeps the old swapchain, as
    /// `Renderer::resize` does, until it's restored.
    pub fn resize(&mut self, width: u32, height: u32) -> Result<(), RendererError> {
        if width == 0 || height == 0 {
            return Ok(());
        }
        let _ = self.ctx.device.wait_idle();
        let new = Swapchain::new(&self.ctx, width, height, self.vsync, self.swapchain.handle)?;
        let mut old = std::mem::replace(&mut self.swapchain, new);
        old.destroy(&self.ctx.device, &self.ctx.allocator);
        Ok(())
    }

    pub fn draw(&mut self, progress: f32, status: &str) -> Result<(), RendererError> {
        let extent = self.swapchain.extent;
        Renderer::render_splash(
            &self.ctx,
            &self.swapchain,
            &mut self.menu,
            extent.width as f32,
            extent.height as f32,
            progress,
            status,
        )
    }
}

/// What any frame needs before game assets: the GPU context, the swapchain
/// and the menu overlay (title, bars and text), with the window then shown.
pub(super) fn bring_up(
    window: &Window,
    font_sources: FontSources<'_>,
    vsync: bool,
) -> Result<(VulkanContext, Swapchain, MenuOverlayPipeline), RendererError> {
    let ctx = VulkanContext::new(window)?;
    let size = window.inner_size();
    let swapchain = Swapchain::new(
        &ctx,
        size.width.max(1),
        size.height.max(1),
        vsync,
        vk::SwapchainKHR::null(),
    )?;
    let font_layer_limit = ctx
        .physical_device
        .get_properties()
        .limits
        .max_image_array_layers;
    let menu = MenuOverlayPipeline::new(
        &ctx.device,
        ctx.graphics_queue,
        ctx.command_pool,
        swapchain.render_pass,
        &ctx.allocator,
        font_sources,
        font_layer_limit,
    )
    .map_err(RendererError::Font)?;
    window.set_visible(true);
    Ok((ctx, swapchain, menu))
}

impl Drop for BootScreen {
    fn drop(&mut self) {
        let _ = self.ctx.device.wait_idle();
        self.menu.destroy(&self.ctx.device, &self.ctx.allocator);
        self.swapchain
            .destroy(&self.ctx.device, &self.ctx.allocator);
    }
}
