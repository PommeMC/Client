//! Vanilla's glowing outline (`post_effect/entity_outline.json`): outlined
//! entities draw flat into `outline`, a sobel pass keeps their edges, two box
//! blurs soften the band, and the result blends over the frame after the
//! hand, before the HUD.

use crate::renderer::rhi::TextureUse;
use crate::renderer::rhi::backend::TextureTransition;
use crate::renderer::{rhi, shader, util};

/// Vanilla's `entity_outline` and `swap` targets are plain RGBA8, so the post
/// chain works on the same gamma-space values vanilla's does.
const FORMAT: rhi::TextureFormat = rhi::TextureFormat::Rgba8Unorm;

/// The target the outline geometry draws into: colour only, so outlines show
/// through the world.
pub fn targets() -> rhi::backend::PipelineTargets {
    rhi::backend::PipelineTargets {
        color: rhi::backend::texture_format(FORMAT),
        depth: false,
    }
}

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
    width: u32,
    height: u32,
}

impl EntityOutline {
    pub fn new(
        device: &rhi::backend::Device,
        frame_targets: rhi::backend::PipelineTargets,
        width: u32,
        height: u32,
    ) -> Self {
        let outline = create_target(device, width, height, "entity_outline");
        let swap = create_target(device, width, height, "entity_outline_swap");
        let nearest = rhi::backend::create_sampler(device, &rhi::SamplerDesc::NEAREST);
        let linear = rhi::backend::create_sampler(device, &rhi::SamplerDesc::LINEAR);
        let texture_layout = rhi::backend::create_bind_group_layout(device, util::FRAGMENT_TEXTURE);
        let pipeline_layout = rhi::backend::create_pipeline_layout(
            device,
            &[texture_layout],
            Some(rhi::PushConstants {
                visibility: rhi::ShaderStages::FRAGMENT,
                size: 8,
            }),
        );
        let post = |label, fragment| {
            create_post_pipeline(device, targets(), pipeline_layout, label, fragment, None)
        };
        let group = |texture: &rhi::backend::Texture, sampler| {
            util::create_texture_group(device, texture_layout, texture.handle(), sampler)
        };
        Self {
            sobel: post("entity sobel", shader::include_shader!("entity_sobel.frag")),
            blur: post(
                "entity outline blur",
                shader::include_shader!("entity_outline_box_blur.frag"),
            ),
            blit: post(
                "entity outline blit",
                shader::include_shader!("entity_outline_blit.frag"),
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
            width,
            height,
        }
    }

    /// Opens the outline geometry's pass, cleared to transparent.
    pub fn begin(
        &self,
        device: &rhi::backend::Device,
        encoder: &rhi::backend::CommandEncoder,
    ) -> rhi::backend::RenderPassEncoder {
        self.open_pass(device, encoder, self.outline.handle())
    }

    /// Closes the geometry pass and runs vanilla's chain: sobel into `swap`,
    /// a horizontal then a vertical box blur, and a blit back to `outline`.
    pub fn finish(
        &self,
        device: &rhi::backend::Device,
        encoder: &rhi::backend::CommandEncoder,
        geometry: rhi::backend::RenderPassEncoder,
    ) {
        self.close_pass(encoder, geometry, self.outline.handle());
        let (outline, swap) = (self.outline.handle(), self.swap.handle());
        self.post_pass(
            device,
            encoder,
            swap,
            self.sobel,
            self.outline_nearest,
            [0.0; 2],
        );
        self.post_pass(
            device,
            encoder,
            outline,
            self.blur,
            self.swap_linear,
            [1.0, 0.0],
        );
        self.post_pass(
            device,
            encoder,
            swap,
            self.blur,
            self.outline_linear,
            [0.0, 1.0],
        );
        self.post_pass(
            device,
            encoder,
            outline,
            self.blit,
            self.swap_nearest,
            [0.0; 2],
        );
    }

    /// Vanilla `blitEntityOutline`: the outlines over the frame.
    pub fn composite(&self, pass: &mut rhi::backend::RenderPassEncoder) {
        pass.set_pipeline(&self.composite);
        pass.set_bind_group(0, self.outline_nearest);
        pass.draw(3, 1, 0, 0);
    }

    /// One full-screen pass of `pipeline` reading `input` into `target`.
    fn post_pass(
        &self,
        device: &rhi::backend::Device,
        encoder: &rhi::backend::CommandEncoder,
        target: rhi::backend::TextureHandle,
        pipeline: rhi::backend::RenderPipeline,
        input: rhi::backend::BindGroup,
        blur_dir: [f32; 2],
    ) {
        let mut pass = self.open_pass(device, encoder, target);
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(0, input);
        pass.set_push_constants(
            rhi::ShaderStages::FRAGMENT,
            0,
            bytemuck::cast_slice(&blur_dir),
        );
        pass.draw(3, 1, 0, 0);
        self.close_pass(encoder, pass, target);
    }

    fn open_pass(
        &self,
        device: &rhi::backend::Device,
        encoder: &rhi::backend::CommandEncoder,
        target: rhi::backend::TextureHandle,
    ) -> rhi::backend::RenderPassEncoder {
        encoder.transition_textures(&[TextureTransition::color(
            target,
            TextureUse::ShaderRead,
            TextureUse::ColorAttachment,
        )]);
        encoder.begin_render_pass(
            device,
            &rhi::backend::RenderPassDesc {
                color: rhi::backend::ColorAttachment {
                    texture: target,
                    format: rhi::backend::texture_format(FORMAT),
                    load: rhi::LoadOp::Clear([0.0; 4]),
                    store: rhi::StoreOp::Store,
                },
                depth: None,
                extent: rhi::Extent {
                    width: self.width,
                    height: self.height,
                },
            },
        )
    }

    /// Ends `pass`, leaving `target` for the next pass to read.
    fn close_pass(
        &self,
        encoder: &rhi::backend::CommandEncoder,
        pass: rhi::backend::RenderPassEncoder,
        target: rhi::backend::TextureHandle,
    ) {
        pass.end();
        encoder.transition_textures(&[TextureTransition::color(
            target,
            TextureUse::ColorAttachment,
            TextureUse::ShaderRead,
        )]);
    }

    pub fn resize(&mut self, device: &rhi::backend::Device, width: u32, height: u32) {
        if (width, height) == (self.width, self.height) {
            return;
        }
        device.wait_idle();
        self.destroy_targets(device);
        self.outline = create_target(device, width, height, "entity_outline");
        self.swap = create_target(device, width, height, "entity_outline_swap");
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
        self.width = width;
        self.height = height;
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

/// A screen-size target, ready to be sampled.
fn create_target(
    device: &rhi::backend::Device,
    width: u32,
    height: u32,
    name: &str,
) -> rhi::backend::Texture {
    let texture = rhi::backend::Texture::new(
        device,
        &rhi::TextureDesc::new(
            name,
            width.max(1),
            height.max(1),
            FORMAT,
            rhi::TextureUsages::RENDER_ATTACHMENT | rhi::TextureUsages::TEXTURE_BINDING,
        ),
    );
    util::transition_image_to_shader_read(device, texture.handle());
    texture
}

fn create_post_pipeline(
    device: &rhi::backend::Device,
    targets: rhi::backend::PipelineTargets,
    layout: rhi::backend::PipelineLayout,
    label: &str,
    fragment: rhi::Shader,
    blend: Option<rhi::BlendState>,
) -> rhi::backend::RenderPipeline {
    rhi::backend::create_render_pipeline(
        device,
        targets,
        layout,
        &rhi::RenderPipelineDesc {
            blend,
            ..rhi::RenderPipelineDesc::new(
                label,
                shader::include_shader!("blur.vert"),
                fragment,
                &[],
            )
        },
    )
}

/// Vanilla `ENTITY_OUTLINE_BLIT`: alpha-blended colour over the frame, its
/// alpha kept (`BlendFunction.ENTITY_OUTLINE_BLIT`), with no depth test.
fn create_composite_pipeline(
    device: &rhi::backend::Device,
    frame_targets: rhi::backend::PipelineTargets,
    layout: rhi::backend::PipelineLayout,
) -> rhi::backend::RenderPipeline {
    create_post_pipeline(
        device,
        frame_targets,
        layout,
        "entity outline composite",
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
