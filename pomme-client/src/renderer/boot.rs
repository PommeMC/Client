//! The boot screen: the loading splash on a bare GPU context, before any game
//! asset exists to build the full `Renderer` from.

use winit::window::Window;

use super::pipelines::menu_overlay::MenuOverlayPipeline;
use super::{RendererError, rhi};
use crate::resources::ResourceManager;
use crate::ui::font::FontSources;

pub struct BootScreen {
    menu: MenuOverlayPipeline,
    swapchain: rhi::backend::Swapchain,
    frames: rhi::backend::Frames,
    vsync: bool,
    ctx: rhi::backend::Context,
}

impl BootScreen {
    pub fn new(
        window: &Window,
        resources: &dyn ResourceManager,
        font_sources: FontSources<'_>,
        vsync: bool,
    ) -> Result<Self, RendererError> {
        let (ctx, swapchain, frames, menu) =
            bring_up(window, resources, font_sources, None, vsync)?;
        Ok(Self {
            menu,
            swapchain,
            frames,
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
        self.ctx.device.wait_idle();
        self.swapchain
            .rebuild(&self.ctx, &mut self.frames, width, height, self.vsync)?;
        Ok(())
    }

    pub fn draw(&mut self, progress: f32, status: &str) -> Result<(), RendererError> {
        let extent = self.swapchain.extent;
        super::Renderer::render_splash(
            &self.ctx,
            &self.swapchain,
            &mut self.frames,
            &mut self.menu,
            extent.width as f32,
            extent.height as f32,
            progress,
            status,
        )
    }
}

/// What any frame needs before game assets: the GPU context, the swapchain
/// and its frames, and the menu overlay (title, bars and text), with the
/// window then shown.
pub(super) fn bring_up(
    window: &Window,
    resources: &dyn ResourceManager,
    font_sources: FontSources<'_>,
    bedrock_ui_dir: Option<&std::path::Path>,
    vsync: bool,
) -> Result<
    (
        rhi::backend::Context,
        rhi::backend::Swapchain,
        rhi::backend::Frames,
        MenuOverlayPipeline,
    ),
    RendererError,
> {
    let ctx = rhi::backend::Context::new(window)?;
    let size = window.inner_size();
    let swapchain =
        rhi::backend::Swapchain::new(&ctx, size.width.max(1), size.height.max(1), vsync)?;
    let frames = rhi::backend::Frames::new(&ctx.device, swapchain.image_count())?;
    let font_layer_limit = ctx.device.max_texture_array_layers;
    let menu = MenuOverlayPipeline::new(
        &ctx.device,
        swapchain.targets(),
        resources,
        font_sources,
        bedrock_ui_dir,
        font_layer_limit,
    )
    .map_err(RendererError::Font)?;
    window.set_visible(true);
    Ok((ctx, swapchain, frames, menu))
}

impl Drop for BootScreen {
    fn drop(&mut self) {
        self.ctx.device.wait_idle();
        self.frames.destroy(&self.ctx.device);
        self.menu.destroy(&self.ctx.device);
        self.swapchain.destroy(&self.ctx);
    }
}
