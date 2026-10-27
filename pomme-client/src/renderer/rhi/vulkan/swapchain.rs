use pyronyx::khr::surface::SurfacePhysicalDevice;
use pyronyx::khr::swapchain::SwapchainDevice;
use pyronyx::vk;

use super::{Context, ContextError};
use crate::renderer::rhi;

/// An 8-bit UNORM format, BGRA (desktop drivers) or RGBA (Android), so the
/// display gets the gamma-space bytes as vanilla's present blit does; else the
/// first one offered.
/// TODO: a surface offering only sRGB formats would encode the frame again.
fn choose_format(formats: &[vk::SurfaceFormatKHR]) -> vk::SurfaceFormatKHR {
    [vk::Format::B8G8R8A8Unorm, vk::Format::R8G8B8A8Unorm]
        .into_iter()
        .find_map(|want| {
            formats
                .iter()
                .copied()
                .find(|f| f.format == want && f.color_space == vk::ColorSpaceKHR::SrgbNonlinear)
        })
        .unwrap_or(formats[0])
}

/// Opaque when the surface allows it; some Android surfaces offer only Inherit.
fn choose_composite_alpha(supported: vk::CompositeAlphaFlagsKHR) -> vk::CompositeAlphaFlagsKHR {
    [
        vk::CompositeAlphaFlagsKHR::Opaque,
        vk::CompositeAlphaFlagsKHR::Inherit,
    ]
    .into_iter()
    .find(|&alpha| supported.contains(alpha))
    .unwrap_or(vk::CompositeAlphaFlagsKHR::Opaque)
}

/// Identity, so the compositor rotates for a phone held sideways from its
/// natural orientation; `current_transform` would claim the image is already
/// rotated.
///
/// TODO: pre-rotate (render rotated and pass `current_transform`) to skip the
/// compositor's rotation pass.
fn choose_pre_transform(caps: &vk::SurfaceCapabilitiesKHR) -> vk::SurfaceTransformFlagsKHR {
    if caps
        .supported_transforms
        .contains(vk::SurfaceTransformFlagsKHR::Identity)
    {
        vk::SurfaceTransformFlagsKHR::Identity
    } else {
        caps.current_transform
    }
}

pub struct Swapchain {
    pub(super) handle: vk::SwapchainKHR,
    images: Vec<vk::Image>,
    image_views: Vec<vk::ImageView>,
    pub format: vk::SurfaceFormatKHR,
    pub extent: rhi::Extent,
    pub present_mode: vk::PresentModeKHR,
    pub depth: super::Texture,
}

impl Swapchain {
    pub fn new(ctx: &Context, width: u32, height: u32, vsync: bool) -> Result<Self, ContextError> {
        Self::replacing(ctx, width, height, vsync, vk::SwapchainKHR::null())
    }

    /// A swapchain taking over presentation from `old_swapchain`.
    fn replacing(
        ctx: &Context,
        width: u32,
        height: u32,
        vsync: bool,
        old_swapchain: vk::SwapchainKHR,
    ) -> Result<Self, ContextError> {
        let caps = ctx.physical_device.get_surface_capabilities(ctx.surface)?;
        let formats = ctx.physical_device.get_surface_formats(ctx.surface)?;
        let present_modes = ctx.physical_device.get_surface_present_modes(ctx.surface)?;

        let format = choose_format(&formats);

        let present_mode = if vsync {
            vk::PresentModeKHR::Fifo
        } else {
            // MoltenVK's Mailbox stays synced to the display refresh, so on
            // Apple only Immediate (displaySyncEnabled = NO) truly uncaps;
            // elsewhere Mailbox is preferred (uncapped and tear-free).
            let prefer = if cfg!(target_vendor = "apple") {
                [vk::PresentModeKHR::Immediate, vk::PresentModeKHR::Mailbox]
            } else {
                [vk::PresentModeKHR::Mailbox, vk::PresentModeKHR::Immediate]
            };
            prefer
                .into_iter()
                .find(|m| present_modes.contains(m))
                .unwrap_or(vk::PresentModeKHR::Fifo)
        };

        // Prefer the surface's reported drawable size: when `current_extent` is
        // defined (not u32::MAX, as on macOS/MoltenVK), the Vulkan spec requires
        // using it. Falling back to the window size here is what letterboxed the
        // image in native fullscreen.
        let extent = if caps.current_extent.width != u32::MAX {
            caps.current_extent
        } else {
            vk::Extent2D {
                width: width.clamp(caps.min_image_extent.width, caps.max_image_extent.width),
                height: height.clamp(caps.min_image_extent.height, caps.max_image_extent.height),
            }
        };
        // Guard against a degenerate (0,0) extent (e.g. a minimized window
        // reporting current_extent = 0) so the aspect ratio stays finite.
        let extent = vk::Extent2D {
            width: extent.width.max(1),
            height: extent.height.max(1),
        };

        let image_count = (caps.min_image_count + 1).min(if caps.max_image_count == 0 {
            u32::MAX
        } else {
            caps.max_image_count
        });

        let (sharing_mode, queue_families): (vk::SharingMode, Vec<u32>) =
            if ctx.graphics_family != ctx.present_family {
                (
                    vk::SharingMode::Concurrent,
                    vec![ctx.graphics_family, ctx.present_family],
                )
            } else {
                (vk::SharingMode::Exclusive, vec![])
            };

        let swapchain_info = vk::SwapchainCreateInfoKHR {
            surface: ctx.surface,
            min_image_count: image_count,
            image_format: format.format,
            image_color_space: format.color_space,
            image_extent: extent,
            image_array_layers: 1,
            image_usage: vk::ImageUsageFlags::ColorAttachment | vk::ImageUsageFlags::TransferSrc,
            image_sharing_mode: sharing_mode,
            queue_family_index_count: queue_families.len() as u32,
            queue_family_indices: queue_families.as_ptr(),
            pre_transform: choose_pre_transform(&caps),
            composite_alpha: choose_composite_alpha(caps.supported_composite_alpha),
            present_mode,
            clipped: vk::TRUE,
            old_swapchain,
            ..Default::default()
        };

        let swapchain = ctx.device.raw.create_swapchain(&swapchain_info, None)?;
        let images = ctx.device.raw.get_swapchain_images(swapchain)?;

        let image_views = images
            .iter()
            .map(|&img| {
                let view_info = vk::ImageViewCreateInfo {
                    image: img,
                    view_type: vk::ImageViewType::Type2D,
                    format: format.format,
                    subresource_range: super::COLOR_SUBRESOURCE_RANGE,
                    ..Default::default()
                };
                ctx.device.raw.create_image_view(&view_info, None)
            })
            .collect::<Result<Vec<_>, _>>()?;

        let extent = rhi::Extent {
            width: extent.width,
            height: extent.height,
        };
        let depth =
            super::Texture::try_new(&ctx.device, &rhi::TextureDesc::swapchain_depth(extent))
                .map_err(ContextError::Texture)?;

        Ok(Self {
            handle: swapchain,
            images,
            image_views,
            format,
            extent,
            present_mode,
            depth,
        })
    }

    /// Replaces this swapchain with one at the new size, handing the old one
    /// over so presentation carries on, and resizes `frames` to match.
    pub fn rebuild(
        &mut self,
        ctx: &Context,
        frames: &mut super::Frames,
        width: u32,
        height: u32,
        vsync: bool,
    ) -> Result<(), ContextError> {
        let new = Swapchain::replacing(ctx, width, height, vsync, self.handle)?;
        std::mem::replace(self, new).destroy(ctx);
        frames.resize(&ctx.device, self.images.len())?;
        Ok(())
    }

    pub fn image_count(&self) -> usize {
        self.images.len()
    }

    /// The targets of pipelines that draw into the swapchain.
    pub fn targets(&self) -> super::PipelineTargets {
        super::PipelineTargets {
            color: self.format.format,
            depth: true,
        }
    }

    /// Image `index`, as the RHI takes it.
    pub fn image(&self, index: u32) -> super::TextureHandle {
        super::TextureHandle::new(
            self.images[index as usize],
            self.image_views[index as usize],
        )
    }

    pub fn pass(&self, index: u32, color: rhi::LoadOp<[f32; 4]>) -> super::RenderPassDesc {
        super::RenderPassDesc::swapchain(
            self.image(index),
            self.format.format,
            color,
            self.depth.handle(),
            self.extent,
        )
    }

    pub fn is_fifo(&self) -> bool {
        self.present_mode == vk::PresentModeKHR::Fifo
    }

    /// Whether a suboptimal swapchain needs rebuilding: only once the surface's
    /// size changed. A phone held sideways is suboptimal on every frame (see
    /// `choose_pre_transform`), and a same-size rebuild each frame stalls it.
    pub fn surface_resized(&self, ctx: &Context) -> bool {
        ctx.physical_device
            .get_surface_capabilities(ctx.surface)
            .is_ok_and(|caps| {
                let extent = caps.current_extent;
                // Undefined (the window decides) or minimised: nothing to match.
                extent.width != u32::MAX
                    && extent.width > 0
                    && (extent.width, extent.height) != (self.extent.width, self.extent.height)
            })
    }

    /// Whether texels come out in BGRA order, as most desktop surfaces have
    /// them.
    pub fn is_bgra(&self) -> bool {
        matches!(
            self.format.format,
            vk::Format::B8G8R8A8Srgb | vk::Format::B8G8R8A8Unorm
        )
    }

    /// Hands image `index` from its last pass over to presenting.
    pub fn ready_to_present(&self, encoder: &super::CommandEncoder, index: u32) {
        encoder.transition_textures(&[super::TextureTransition::color(
            self.image(index),
            rhi::TextureUse::ColorAttachment,
            rhi::TextureUse::Present,
        )]);
    }

    /// Destroys what depends on the surface: the image views and the
    /// swapchain. `destroy` then skips them.
    pub fn release_surface(&mut self, ctx: &Context) {
        let textures: Vec<_> = (0..self.image_views.len() as u32)
            .map(|index| self.image(index))
            .collect();
        ctx.device.forget_targets(&textures);
        for &view in &self.image_views {
            ctx.device.raw.destroy_image_view(view, None);
        }
        self.image_views.clear();

        ctx.device.raw.destroy_swapchain(self.handle, None);
        self.handle = vk::SwapchainKHR::null();
    }

    pub fn destroy(&mut self, ctx: &Context) {
        ctx.device.wait_idle();

        self.release_surface(ctx);

        std::mem::take(&mut self.depth).destroy(&ctx.device);
    }
}

#[cfg(test)]
mod tests {
    use pyronyx::vk;

    use super::{choose_composite_alpha, choose_format};

    fn format(format: vk::Format) -> vk::SurfaceFormatKHR {
        vk::SurfaceFormatKHR {
            format,
            color_space: vk::ColorSpaceKHR::SrgbNonlinear,
        }
    }

    #[test]
    fn prefers_a_unorm_format_in_either_channel_order() {
        let desktop = [
            format(vk::Format::B8G8R8A8Srgb),
            format(vk::Format::B8G8R8A8Unorm),
        ];
        assert_eq!(choose_format(&desktop).format, vk::Format::B8G8R8A8Unorm);
        let android = [
            format(vk::Format::R8G8B8A8Srgb),
            format(vk::Format::R8G8B8A8Unorm),
        ];
        assert_eq!(choose_format(&android).format, vk::Format::R8G8B8A8Unorm);
        let neither = [format(vk::Format::R5G6B5UnormPack16)];
        assert_eq!(
            choose_format(&neither).format,
            vk::Format::R5G6B5UnormPack16
        );
    }

    #[test]
    fn composite_alpha_falls_back_to_inherit() {
        let both = vk::CompositeAlphaFlagsKHR::Opaque | vk::CompositeAlphaFlagsKHR::Inherit;
        assert_eq!(
            choose_composite_alpha(both),
            vk::CompositeAlphaFlagsKHR::Opaque
        );
        assert_eq!(
            choose_composite_alpha(vk::CompositeAlphaFlagsKHR::Inherit),
            vk::CompositeAlphaFlagsKHR::Inherit
        );
    }
}
