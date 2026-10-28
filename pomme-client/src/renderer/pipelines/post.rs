//! Shared plumbing for vanilla's post chains (`post_effect/*.json`):
//! screen-size targets and the full-screen passes between them.

use crate::renderer::rhi::TextureUse;
use crate::renderer::rhi::backend::TextureTransition;
use crate::renderer::{rhi, shader, util};

/// Vanilla's post targets are plain RGBA8, so the chains work on the same
/// gamma-space values vanilla's do.
pub const FORMAT: rhi::TextureFormat = rhi::TextureFormat::Rgba8Unorm;

/// Each pass's uniforms, as push constants both stages may read.
pub const PUSH_CONSTANTS: rhi::PushConstants = rhi::PushConstants {
    visibility: rhi::ShaderStages::VERTEX_FRAGMENT,
    size: 64,
};

/// What a post pass draws into.
pub fn targets() -> rhi::backend::PipelineTargets {
    rhi::backend::PipelineTargets::new(rhi::backend::texture_format(FORMAT), false)
}

/// A screen-size target, ready to be sampled.
pub fn create_target(
    device: &rhi::backend::Device,
    extent: rhi::Extent,
    name: &str,
) -> rhi::backend::Texture {
    let texture = rhi::backend::Texture::new(
        device,
        &rhi::TextureDesc::new(
            name,
            extent.width.max(1),
            extent.height.max(1),
            FORMAT,
            rhi::TextureUsages::RENDER_ATTACHMENT | rhi::TextureUsages::TEXTURE_BINDING,
        ),
    );
    util::transition_image_to_shader_read(device, texture.handle());
    texture
}

/// A pipeline over vanilla's full-screen triangle (`core/screenquad`) unless
/// `vertex` replaces it.
pub fn create_pipeline(
    device: &rhi::backend::Device,
    targets: rhi::backend::PipelineTargets,
    layout: rhi::backend::PipelineLayout,
    label: &str,
    vertex: Option<rhi::Shader>,
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
                vertex.unwrap_or(shader::include_shader!("blur.vert")),
                fragment,
                &[],
            )
        },
    )
}

/// Opens a pass into `target`, cleared to transparent.
pub fn open_pass(
    device: &rhi::backend::Device,
    encoder: &rhi::backend::CommandEncoder,
    target: rhi::backend::TextureHandle,
    extent: rhi::Extent,
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
            extra_colors: Vec::new(),
            depth: None,
            extent,
        },
    )
}

/// Ends `pass`, leaving `target` for the next pass to read.
pub fn close_pass(
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

/// One full-screen draw of `pipeline` with `inputs` and `uniforms`.
pub fn draw(
    pass: &mut rhi::backend::RenderPassEncoder,
    pipeline: &rhi::backend::RenderPipeline,
    inputs: rhi::backend::BindGroup,
    uniforms: &[f32],
) {
    pass.set_pipeline(pipeline);
    pass.set_bind_group(0, inputs);
    if !uniforms.is_empty() {
        pass.set_push_constants(PUSH_CONSTANTS.visibility, 0, bytemuck::cast_slice(uniforms));
    }
    pass.draw(3, 1, 0, 0);
}

/// One pass of a chain: `pipeline` reading `inputs` into `target`.
#[allow(clippy::too_many_arguments)]
pub fn run(
    device: &rhi::backend::Device,
    encoder: &rhi::backend::CommandEncoder,
    target: rhi::backend::TextureHandle,
    extent: rhi::Extent,
    pipeline: &rhi::backend::RenderPipeline,
    inputs: rhi::backend::BindGroup,
    uniforms: &[f32],
) {
    let mut pass = open_pass(device, encoder, target, extent);
    draw(&mut pass, pipeline, inputs, uniforms);
    close_pass(encoder, pass, target);
}
