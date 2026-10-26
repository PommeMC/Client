//! Vanilla's glowing outline (`post_effect/entity_outline.json`): outlined
//! entities draw flat into `outline`, a sobel pass keeps their edges, two box
//! blurs soften the band, and the result blends over the frame after the
//! hand, before the HUD.

/// The target the outline geometry draws into: colour only, so outlines show
/// through the world.
pub use post::targets;

use super::post;
use crate::renderer::{rhi, shader, util};

pub struct EntityOutline {
    outline: rhi::backend::Texture,
    swap: rhi::backend::Texture,
    nearest: rhi::backend::Sampler,
    linear: rhi::backend::Sampler,
    texture_layout: rhi::backend::BindGroupLayout,
    pipeline_layout: rhi::backend::PipelineLayout,
    sobel: rhi::backend::RenderPipeline,
    blur: rhi::backend::RenderPipeline,
    blit: rhi::backend::RenderPipeline,
    composite: rhi::backend::RenderPipeline,
    outline_nearest: rhi::backend::BindGroup,
    outline_linear: rhi::backend::BindGroup,
    swap_nearest: rhi::backend::BindGroup,
    swap_linear: rhi::backend::BindGroup,
    extent: rhi::Extent,
}

impl EntityOutline {
    pub fn new(
        device: &rhi::backend::Device,
        frame_targets: rhi::backend::PipelineTargets,
        extent: rhi::Extent,
    ) -> Self {
        let outline = post::create_target(device, extent, "entity_outline");
        let swap = post::create_target(device, extent, "entity_outline_swap");
        let nearest = rhi::backend::create_sampler(device, &rhi::SamplerDesc::NEAREST);
        let linear = rhi::backend::create_sampler(device, &rhi::SamplerDesc::LINEAR);
        let texture_layout = rhi::backend::create_bind_group_layout(device, util::FRAGMENT_TEXTURE);
        let pipeline_layout = rhi::backend::create_pipeline_layout(
            device,
            &[texture_layout],
            Some(post::PUSH_CONSTANTS),
        );
        let pipeline = |label, fragment| {
            post::create_pipeline(
                device,
                targets(),
                pipeline_layout,
                label,
                None,
                fragment,
                None,
            )
        };
        let group = |texture: &rhi::backend::Texture, sampler| {
            util::create_texture_group(device, texture_layout, texture.handle(), sampler)
        };
        Self {
            sobel: pipeline("entity sobel", shader::include_shader!("entity_sobel.frag")),
            blur: pipeline(
                "entity outline blur",
                shader::include_shader!("entity_outline_box_blur.frag"),
            ),
            blit: pipeline(
                "entity outline blit",
                shader::include_shader!("post_blit.frag"),
            ),
            composite: create_composite_pipeline(device, frame_targets, pipeline_layout),
            outline_nearest: group(&outline, nearest),
            outline_linear: group(&outline, linear),
            swap_nearest: group(&swap, nearest),
            swap_linear: group(&swap, linear),
            outline,
            swap,
            nearest,
            linear,
            texture_layout,
            pipeline_layout,
            extent,
        }
    }

    /// Opens the outline geometry's pass, cleared to transparent.
    pub fn begin(
        &self,
        device: &rhi::backend::Device,
        encoder: &rhi::backend::CommandEncoder,
    ) -> rhi::backend::RenderPassEncoder {
        post::open_pass(device, encoder, self.outline.handle(), self.extent)
    }

    /// Closes the geometry pass and runs vanilla's chain: sobel into `swap`,
    /// a horizontal then a vertical box blur, and a blit back to `outline`.
    pub fn finish(
        &self,
        device: &rhi::backend::Device,
        encoder: &rhi::backend::CommandEncoder,
        geometry: rhi::backend::RenderPassEncoder,
    ) {
        let (outline, swap) = (self.outline.handle(), self.swap.handle());
        post::close_pass(encoder, geometry, outline);
        let pass = |target, pipeline, inputs, uniforms: &[f32]| {
            post::run(
                device,
                encoder,
                target,
                self.extent,
                pipeline,
                inputs,
                uniforms,
            );
        };
        pass(swap, &self.sobel, self.outline_nearest, &[]);
        pass(outline, &self.blur, self.swap_linear, &[1.0, 0.0]);
        // Vanilla's `(0, 1)` steps up its bottom-up GL rows; these run top
        // down. The blur's taps aren't symmetric, so the sign matters.
        pass(swap, &self.blur, self.outline_linear, &[0.0, -1.0]);
        pass(outline, &self.blit, self.swap_nearest, &[1.0; 4]);
    }

    /// Vanilla `blitEntityOutline`: the outlines over the frame.
    pub fn composite(&self, pass: &mut rhi::backend::RenderPassEncoder) {
        post::draw(pass, &self.composite, self.outline_nearest, &[]);
    }

    pub fn resize(&mut self, device: &rhi::backend::Device, extent: rhi::Extent) {
        if extent == self.extent {
            return;
        }
        device.wait_idle();
        self.destroy_targets(device);
        self.outline = post::create_target(device, extent, "entity_outline");
        self.swap = post::create_target(device, extent, "entity_outline_swap");
        for (group, texture, sampler) in [
            (&mut self.outline_nearest, &self.outline, self.nearest),
            (&mut self.outline_linear, &self.outline, self.linear),
            (&mut self.swap_nearest, &self.swap, self.nearest),
            (&mut self.swap_linear, &self.swap, self.linear),
        ] {
            group.replace(
                device,
                self.texture_layout,
                &util::texture_binding(texture.handle(), sampler),
            );
        }
        self.extent = extent;
    }

    /// Rebuilds the composite against a new frame format.
    pub fn recreate_pipeline(
        &mut self,
        device: &rhi::backend::Device,
        frame_targets: rhi::backend::PipelineTargets,
    ) {
        self.composite.destroy(device);
        self.composite = create_composite_pipeline(device, frame_targets, self.pipeline_layout);
    }

    fn destroy_targets(&mut self, device: &rhi::backend::Device) {
        std::mem::take(&mut self.outline).destroy(device);
        std::mem::take(&mut self.swap).destroy(device);
    }

    pub fn destroy(&mut self, device: &rhi::backend::Device) {
        self.destroy_targets(device);
        for pipeline in [self.sobel, self.blur, self.blit, self.composite] {
            pipeline.destroy(device);
        }
        for group in [
            self.outline_nearest,
            self.outline_linear,
            self.swap_nearest,
            self.swap_linear,
        ] {
            group.destroy(device);
        }
        self.pipeline_layout.destroy(device);
        self.texture_layout.destroy(device);
        self.nearest.destroy(device);
        self.linear.destroy(device);
    }
}

/// Vanilla `ENTITY_OUTLINE_BLIT`: alpha-blended colour over the frame, its
/// alpha kept (`BlendFunction.ENTITY_OUTLINE_BLIT`), with no depth test.
fn create_composite_pipeline(
    device: &rhi::backend::Device,
    frame_targets: rhi::backend::PipelineTargets,
    layout: rhi::backend::PipelineLayout,
) -> rhi::backend::RenderPipeline {
    post::create_pipeline(
        device,
        frame_targets,
        layout,
        "entity outline composite",
        None,
        shader::include_shader!("entity_outline_composite.frag"),
        Some(rhi::BlendState {
            color: rhi::BlendComponent {
                src: rhi::BlendFactor::SrcAlpha,
                dst: rhi::BlendFactor::OneMinusSrcAlpha,
            },
            alpha: rhi::BlendComponent {
                src: rhi::BlendFactor::Zero,
                dst: rhi::BlendFactor::One,
            },
        }),
    )
}
