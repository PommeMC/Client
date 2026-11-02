//! The boot screen: the loading splash on a bare GPU context, before any game
//! asset exists to build the full `Renderer` from.

use winit::window::Window;

use super::pipelines::menu_overlay::MenuOverlayPipeline;
use super::{RendererError, rhi};
use crate::resources::ResourceManager;
use crate::ui::font::FontSources;

const PARTS_TAKEN: &str = "boot screen parts are taken only by `into_parts`";

pub struct BootScreen {
    /// Taken by [`Self::into_parts`] when the renderer is built on them.
    parts: Option<BootParts>,
    vsync: bool,
}

pub(super) struct BootParts {
    pub ctx: rhi::backend::Context,
    pub swapchain: rhi::backend::Swapchain,
    pub frames: rhi::backend::Frames,
    pub menu: MenuOverlayPipeline,
}

impl BootScreen {
    pub fn new(
        window: &Window,
        resources: &dyn ResourceManager,
        font_sources: FontSources<'_>,
        vsync: bool,
    ) -> Result<Self, RendererError> {
        let (ctx, swapchain, frames, menu) = bring_up(window, resources, font_sources, vsync)?;
        Ok(Self {
            parts: Some(BootParts {
                ctx,
                swapchain,
                frames,
                menu,
            }),
            vsync,
        })
    }

    /// A minimised window (0×0) keeps the old swapchain, as
    /// `Renderer::resize` does, until it's restored.
    pub fn resize(&mut self, width: u32, height: u32) -> Result<(), RendererError> {
        if width == 0 || height == 0 {
            return Ok(());
        }
        let vsync = self.vsync;
        let parts = self.parts();
        parts.ctx.device.wait_idle();
        parts
            .swapchain
            .rebuild(&parts.ctx, &mut parts.frames, width, height, vsync)?;
        Ok(())
    }

    pub fn draw(&mut self, progress: f32, status: &str) -> Result<(), RendererError> {
        let parts = self.parts();
        let extent = parts.swapchain.extent;
        super::Renderer::render_splash(
            &parts.ctx,
            &parts.swapchain,
            &mut parts.frames,
            &mut parts.menu,
            extent.width as f32,
            extent.height as f32,
            progress,
            status,
        )
    }

    pub(super) fn vsync(&self) -> bool {
        self.vsync
    }

    fn parts(&mut self) -> &mut BootParts {
        self.parts.as_mut().expect(PARTS_TAKEN)
    }

    /// The GPU context and splash, for the renderer to build on.
    pub(super) fn into_parts(mut self) -> BootParts {
        self.parts.take().expect(PARTS_TAKEN)
    }
}

/// What any frame needs before game assets: the GPU context, the swapchain
/// and its frames, and the menu overlay (title, bars and text), with the
/// window then shown.
pub(super) fn bring_up(
    window: &Window,
    resources: &dyn ResourceManager,
    font_sources: FontSources<'_>,
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
        font_layer_limit,
    )
    .map_err(RendererError::Font)?;
    window.set_visible(true);
    Ok((ctx, swapchain, frames, menu))
}

impl Drop for BootScreen {
    fn drop(&mut self) {
        if let Some(mut parts) = self.parts.take() {
            parts.ctx.device.wait_idle();
            parts.frames.destroy(&parts.ctx.device);
            parts.menu.destroy(&parts.ctx.device);
            parts.swapchain.destroy(&parts.ctx);
        }
    }
}
