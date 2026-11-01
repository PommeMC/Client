//! Distant terrain's passes (Voxy's `BoundRenderer`, `AbstractRenderPipeline`
//! and `NormalRenderPipeline`): the drawn chunks' boxes into a depth bound,
//! the far terrain into its own image under its own projection where the
//! chunks drew nothing, and that image blitted into the frame with its depth
//! carried over.

use glam::DVec3;

use crate::renderer::camera::{Camera, CameraUniform};
use crate::renderer::distant::gpu::{DISTANT_FAR, DistantBuffers, POSITIONS_BYTES};
use crate::renderer::pipelines::lightmap::Lightmap;
use crate::renderer::rhi::TextureUse;
use crate::renderer::rhi::backend::TextureTransition;
use crate::renderer::{MAX_FRAMES_IN_FLIGHT, rhi, shader, util};

/// Distant terrain's image: plain RGBA8, as Voxy's.
const FORMAT: rhi::TextureFormat = rhi::TextureFormat::Rgba8Unorm;
const DEPTH: rhi::TextureFormat = rhi::TextureFormat::Depth32Float;

/// A sampled texture and its sampler, for the fragment stage.
const TEXTURE_SAMPLER: &[rhi::BindGroupLayoutEntry] = &[
    rhi::BindGroupLayoutEntry::texture(rhi::ShaderStages::FRAGMENT),
    rhi::BindGroupLayoutEntry::sampler(rhi::ShaderStages::FRAGMENT),
];
/// The blit's inputs: the image, its depth, and the sampler.
const BLIT_INPUTS: &[rhi::BindGroupLayoutEntry] = &[
    rhi::BindGroupLayoutEntry::texture(rhi::ShaderStages::FRAGMENT),
    rhi::BindGroupLayoutEntry::texture(rhi::ShaderStages::FRAGMENT),
    rhi::BindGroupLayoutEntry::sampler(rhi::ShaderStages::FRAGMENT),
];
/// The quads' set 3: each draw's section position, and the depth bound.
const DRAW_SET: &[rhi::BindGroupLayoutEntry] = &[
    rhi::BindGroupLayoutEntry::storage(rhi::ShaderStages::VERTEX),
    rhi::BindGroupLayoutEntry::texture(rhi::ShaderStages::FRAGMENT),
];

/// The screen-size targets, remade with the swapchain.
struct Targets {
    extent: rhi::Extent,
    color: rhi::backend::Texture,
    depth: rhi::backend::Texture,
    /// The far side of the drawn chunks' boxes (`depthBoundingBuffer`).
    bound: rhi::backend::Texture,
    /// Per frame in flight, its position list with the depth bound.
    draw_sets: Vec<rhi::backend::BindGroup>,
    setup_set: rhi::backend::BindGroup,
    blit_set: rhi::backend::BindGroup,
}

pub struct DistantPipeline {
    /// The quads, opaque then translucent, into the image.
    quads: [rhi::backend::RenderPipeline; 2],
    bound: rhi::backend::RenderPipeline,
    setup: rhi::backend::RenderPipeline,
    blit: rhi::backend::RenderPipeline,
    quads_layout: rhi::backend::PipelineLayout,
    bound_layout: rhi::backend::PipelineLayout,
    setup_layout: rhi::backend::PipelineLayout,
    blit_layout: rhi::backend::PipelineLayout,
    camera_layout: rhi::backend::BindGroupLayout,
    camera_fragment_layout: rhi::backend::BindGroupLayout,
    texture_layout: rhi::backend::BindGroupLayout,
    blit_inputs_layout: rhi::backend::BindGroupLayout,
    draw_layout: rhi::backend::BindGroupLayout,
    camera_sets: Vec<rhi::backend::BindGroup>,
    /// The same buffers for the blit, which fogs in the fragment stage.
    camera_fragment_sets: Vec<rhi::backend::BindGroup>,
    camera_buffers: Vec<rhi::backend::Buffer>,
    sampler: rhi::backend::Sampler,
    targets: Option<Targets>,
    /// Set 2 of the quads, which [`Lightmap`] owns.
    lightmap_set: rhi::backend::BindGroup,
}

impl DistantPipeline {
    /// `arena_layout` is the quads' set 1
    /// ([`DistantBuffers::layout`]), bound per arena as they draw;
    /// `bound_store_layout` the bound pass's section list
    /// ([`crate::renderer::distant::gpu::BoundStore::layout`]).
    pub fn new(
        device: &rhi::backend::Device,
        swapchain: rhi::backend::PipelineTargets,
        lightmap: &Lightmap,
        arena_layout: rhi::backend::BindGroupLayout,
        bound_store_layout: rhi::backend::BindGroupLayout,
    ) -> Self {
        let layout = |entries| rhi::backend::create_bind_group_layout(device, entries);
        let camera_layout = layout(util::VERTEX_UNIFORM);
        let camera_fragment_layout = layout(util::VERTEX_FRAGMENT_UNIFORM);
        let texture_layout = layout(TEXTURE_SAMPLER);
        let blit_inputs_layout = layout(BLIT_INPUTS);
        let draw_layout = layout(DRAW_SET);
        let push = |visibility, size| Some(rhi::PushConstants { visibility, size });
        let pipeline_layout = |sets: &[rhi::backend::BindGroupLayout], push| {
            rhi::backend::create_pipeline_layout(device, sets, push)
        };
        // The view projection, once a pass.
        let quads_layout = pipeline_layout(
            &[camera_layout, arena_layout, lightmap.layout, draw_layout],
            push(rhi::ShaderStages::VERTEX, 64),
        );
        let bound_layout = pipeline_layout(
            &[camera_layout, bound_store_layout],
            push(rhi::ShaderStages::VERTEX, 64),
        );
        let setup_layout = pipeline_layout(&[texture_layout], None);
        // The inverse distant view projection, then the fade.
        let blit_layout = pipeline_layout(
            &[camera_fragment_layout, blit_inputs_layout],
            push(rhi::ShaderStages::FRAGMENT, 80),
        );
        let (camera_sets, camera_buffers) = util::create_uniform_sets(
            device,
            camera_layout,
            size_of::<CameraUniform>() as u64,
            "distant_camera_uniform",
        );
        let camera_fragment_sets = camera_buffers
            .iter()
            .map(|buffer| {
                device.create_bind_group(
                    camera_fragment_layout,
                    &[(
                        0,
                        rhi::backend::BindingResource::uniform(
                            buffer,
                            size_of::<CameraUniform>() as u64,
                        ),
                    )],
                )
            })
            .collect();
        Self {
            quads: create_quad_pipelines(device, quads_layout),
            bound: create_bound_pipeline(device, bound_layout),
            setup: create_setup_pipeline(device, setup_layout),
            blit: create_blit_pipeline(device, swapchain, blit_layout),
            quads_layout,
            bound_layout,
            setup_layout,
            blit_layout,
            camera_layout,
            camera_fragment_layout,
            texture_layout,
            blit_inputs_layout,
            draw_layout,
            camera_sets,
            camera_fragment_sets,
            camera_buffers,
            sampler: rhi::backend::create_sampler(device, &rhi::SamplerDesc::NEAREST),
            targets: None,
            lightmap_set: lightmap.group,
        }
    }

    /// The swapchain changed: the blit draws into it, and the targets
    /// follow its size.
    pub fn rebuild(
        &mut self,
        device: &rhi::backend::Device,
        swapchain: rhi::backend::PipelineTargets,
    ) {
        self.blit.destroy(device);
        self.blit = create_blit_pipeline(device, swapchain, self.blit_layout);
        if let Some(targets) = self.targets.take() {
            self.destroy_targets(device, targets);
        }
    }

    pub fn update_camera(&mut self, frame: usize, uniform: &CameraUniform) {
        self.camera_buffers[frame].write(bytemuck::bytes_of(uniform));
    }

    /// The targets at `extent`, made with `frame_depth` (the swapchain's,
    /// which the setup pass reads) as the swapchain is.
    fn ensure_targets(
        &mut self,
        device: &rhi::backend::Device,
        extent: rhi::Extent,
        frame_depth: rhi::backend::TextureHandle,
        buffers: &DistantBuffers,
    ) {
        if self.targets.as_ref().is_some_and(|t| t.extent != extent) {
            let targets = self.targets.take().expect("checked");
            self.destroy_targets(device, targets);
        }
        if self.targets.is_none() {
            let texture = |label, format, resting| {
                let texture = rhi::backend::Texture::new(
                    device,
                    &rhi::TextureDesc::new(
                        label,
                        extent.width.max(1),
                        extent.height.max(1),
                        format,
                        rhi::TextureUsages::RENDER_ATTACHMENT | rhi::TextureUsages::TEXTURE_BINDING,
                    ),
                );
                device.submit(|encoder| {
                    encoder.transition_textures(&[TextureTransition::color(
                        texture.handle(),
                        TextureUse::Undefined,
                        resting,
                    )]);
                });
                texture
            };
            let color = texture("distant_color", FORMAT, TextureUse::ShaderRead);
            let depth = texture("distant_depth", DEPTH, TextureUse::DepthRead);
            let bound = texture("distant_depth_bound", DEPTH, TextureUse::DepthRead);
            let draw_sets = (0..MAX_FRAMES_IN_FLIGHT)
                .map(|frame| {
                    device.create_bind_group(
                        self.draw_layout,
                        &[
                            (
                                0,
                                rhi::backend::BindingResource::storage(
                                    buffers.position_buffer(frame),
                                    POSITIONS_BYTES,
                                ),
                            ),
                            (1, rhi::backend::BindingResource::Texture(bound.handle())),
                        ],
                    )
                })
                .collect();
            let blit_set = device.create_bind_group(
                self.blit_inputs_layout,
                &[
                    (0, rhi::backend::BindingResource::Texture(color.handle())),
                    (1, rhi::backend::BindingResource::Texture(depth.handle())),
                    (2, rhi::backend::BindingResource::Sampler(self.sampler)),
                ],
            );
            self.targets = Some(Targets {
                extent,
                setup_set: util::create_texture_group(
                    device,
                    self.texture_layout,
                    frame_depth,
                    self.sampler,
                ),
                color,
                depth,
                bound,
                draw_sets,
                blit_set,
            });
        }
    }

    /// The bound pass, then distant terrain into its image: Voxy's
    /// `renderOpaque` and `runPipeline` up to the blit, after the chunks'
    /// solid and cutout passes, whose depth `frame_depth` holds.
    #[allow(clippy::too_many_arguments)]
    pub fn render(
        &mut self,
        device: &rhi::backend::Device,
        encoder: &rhi::backend::CommandEncoder,
        frame: usize,
        buffers: &mut DistantBuffers,
        camera: &Camera,
        eye: DVec3,
        frame_depth: rhi::backend::TextureHandle,
        extent: rhi::Extent,
    ) {
        self.ensure_targets(device, extent, frame_depth, buffers);
        buffers.build_draws(frame, camera, eye);
        let targets = self.targets.as_ref().expect("made above");
        let (color, depth, bound) = (
            targets.color.handle(),
            targets.depth.handle(),
            targets.bound.handle(),
        );
        let view_proj = camera.distant_view_projection(DISTANT_FAR);

        // `BoundRenderer`: the farthest back face of each drawn section's
        // box, from a depth cleared to the near plane.
        encoder.transition_textures(&[TextureTransition::color(
            bound,
            TextureUse::DepthRead,
            TextureUse::DepthAttachment,
        )]);
        let mut pass = encoder.begin_render_pass(
            device,
            &rhi::backend::RenderPassDesc {
                color: None,
                extra_colors: Vec::new(),
                depth: Some(depth_attachment(bound, 0.0)),
                extent,
            },
        );
        let bounds = buffers.bounds();
        if bounds.count() > 0 {
            pass.set_pipeline(&self.bound);
            pass.set_bind_groups(0, &[self.camera_sets[frame], bounds.group()]);
            pass.set_push_constants(
                rhi::ShaderStages::VERTEX,
                0,
                bytemuck::cast_slice(&view_proj.to_cols_array()),
            );
            pass.draw(36, bounds.count(), 0, 0);
        }
        pass.end();
        encoder.transition_textures(&[
            TextureTransition::color(bound, TextureUse::DepthAttachment, TextureUse::DepthRead),
            TextureTransition::color(
                frame_depth,
                TextureUse::DepthAttachment,
                TextureUse::DepthRead,
            ),
            TextureTransition::color(color, TextureUse::ShaderRead, TextureUse::ColorAttachment),
            TextureTransition::color(depth, TextureUse::DepthRead, TextureUse::DepthAttachment),
        ]);

        // `initDepthStencil`, then the quads.
        let mut pass = encoder.begin_render_pass(
            device,
            &rhi::backend::RenderPassDesc {
                color: Some(rhi::backend::ColorAttachment {
                    texture: color,
                    format: rhi::backend::texture_format(FORMAT),
                    load: rhi::LoadOp::Clear([0.0; 4]),
                    store: rhi::StoreOp::Store,
                }),
                extra_colors: Vec::new(),
                depth: Some(depth_attachment(depth, 1.0)),
                extent,
            },
        );
        pass.set_pipeline(&self.setup);
        pass.set_bind_group(0, targets.setup_set);
        pass.draw(3, 1, 0, 0);
        for (pipeline, translucent) in self.quads.iter().zip([false, true]) {
            pass.set_pipeline(pipeline);
            pass.set_bind_group(0, self.camera_sets[frame]);
            pass.set_bind_group(2, self.lightmap_set);
            pass.set_bind_group(3, targets.draw_sets[frame]);
            pass.set_push_constants(
                rhi::ShaderStages::VERTEX,
                0,
                bytemuck::cast_slice(&view_proj.to_cols_array()),
            );
            buffers.draw(&pass, frame, translucent);
        }
        pass.end();
        encoder.transition_textures(&[
            TextureTransition::color(
                frame_depth,
                TextureUse::DepthRead,
                TextureUse::DepthAttachment,
            ),
            TextureTransition::color(color, TextureUse::ColorAttachment, TextureUse::ShaderRead),
            TextureTransition::color(depth, TextureUse::DepthAttachment, TextureUse::DepthRead),
        ]);
    }

    /// `NormalRenderPipeline.finish`: the image into the frame, fogged, and
    /// faded over the last tenth of `distant_blocks` (never short of
    /// `render_distance_blocks`, where the chunks end).
    pub fn composite(
        &self,
        pass: &mut rhi::backend::RenderPassEncoder,
        frame: usize,
        camera: &Camera,
        render_distance_blocks: f32,
        distant_blocks: f32,
    ) {
        let Some(targets) = &self.targets else {
            return;
        };
        let rd = distant_blocks - 32.0;
        let start = render_distance_blocks.max(rd * 0.9);
        let end = render_distance_blocks.max(rd);
        let scale = if end > start {
            1.0 / (end - start)
        } else {
            0.0
        };
        let inverse = camera.distant_view_projection(DISTANT_FAR).inverse();
        let mut constants = inverse.to_cols_array().to_vec();
        constants.extend([-start * scale, scale, 0.0, 0.0]);
        pass.set_pipeline(&self.blit);
        pass.set_bind_groups(0, &[self.camera_fragment_sets[frame], targets.blit_set]);
        pass.set_push_constants(
            rhi::ShaderStages::FRAGMENT,
            0,
            bytemuck::cast_slice(&constants),
        );
        pass.draw(3, 1, 0, 0);
    }

    fn destroy_targets(&self, device: &rhi::backend::Device, mut targets: Targets) {
        for set in targets
            .draw_sets
            .iter()
            .chain([&targets.setup_set, &targets.blit_set])
        {
            set.destroy(device);
        }
        for texture in [&mut targets.color, &mut targets.depth, &mut targets.bound] {
            std::mem::take(texture).destroy(device);
        }
    }

    pub fn destroy(&mut self, device: &rhi::backend::Device) {
        if let Some(targets) = self.targets.take() {
            self.destroy_targets(device, targets);
        }
        util::destroy_buffers(device, &mut self.camera_buffers);
        util::destroy_bind_groups(device, self.camera_sets.drain(..));
        util::destroy_bind_groups(device, self.camera_fragment_sets.drain(..));
        for pipeline in
            self.quads
                .iter_mut()
                .chain([&mut self.bound, &mut self.setup, &mut self.blit])
        {
            pipeline.destroy(device);
        }
        for layout in [
            self.quads_layout,
            self.bound_layout,
            self.setup_layout,
            self.blit_layout,
        ] {
            layout.destroy(device);
        }
        for layout in [
            self.camera_layout,
            self.camera_fragment_layout,
            self.texture_layout,
            self.blit_inputs_layout,
            self.draw_layout,
        ] {
            layout.destroy(device);
        }
        self.sampler.destroy(device);
    }
}

/// A `texture` cleared to `clear` and kept after the pass.
fn depth_attachment(
    texture: rhi::backend::TextureHandle,
    clear: f32,
) -> rhi::backend::DepthAttachment {
    rhi::backend::DepthAttachment {
        texture,
        load: rhi::LoadOp::Clear(clear),
        store: rhi::StoreOp::Store,
    }
}

/// What the quads and the setup draw into: the image and its depth.
fn image_targets() -> rhi::backend::PipelineTargets {
    rhi::backend::PipelineTargets::new(rhi::backend::texture_format(FORMAT), true)
}

/// Writing depth under `compare`.
fn depth_state(compare: rhi::CompareFunction) -> Option<rhi::DepthState> {
    Some(rhi::DepthState {
        compare,
        ..rhi::DepthState::WRITE
    })
}

/// A full-screen pass of `fragment`.
fn fullscreen_pipeline(
    device: &rhi::backend::Device,
    targets: rhi::backend::PipelineTargets,
    layout: rhi::backend::PipelineLayout,
    label: &str,
    fragment: rhi::Shader,
    depth: Option<rhi::DepthState>,
    blend: Option<rhi::BlendState>,
) -> rhi::backend::RenderPipeline {
    rhi::backend::create_render_pipeline(
        device,
        targets,
        layout,
        &rhi::RenderPipelineDesc {
            depth,
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

/// The opaque quads, then the translucent ones blended as Voxy's translucent
/// pass does; both write depth under the distant projection. Neither culls
/// faces: the quads are drawn from every side.
fn create_quad_pipelines(
    device: &rhi::backend::Device,
    layout: rhi::backend::PipelineLayout,
) -> [rhi::backend::RenderPipeline; 2] {
    let targets = image_targets();
    let opaque = rhi::RenderPipelineDesc {
        depth: Some(rhi::DepthState::WRITE),
        ..rhi::RenderPipelineDesc::new(
            "distant terrain",
            shader::include_shader!("distant.vert"),
            shader::include_shader!("distant.frag"),
            &[],
        )
    };
    let translucent = rhi::RenderPipelineDesc {
        label: "distant translucent",
        blend: Some(rhi::BlendState::TRANSLUCENT),
        ..opaque
    };
    [opaque, translucent]
        .map(|desc| rhi::backend::create_render_pipeline(device, targets, layout, &desc))
}

/// The drawn chunks' boxes, back faces only and the farthest kept.
fn create_bound_pipeline(
    device: &rhi::backend::Device,
    layout: rhi::backend::PipelineLayout,
) -> rhi::backend::RenderPipeline {
    rhi::backend::create_render_pipeline(
        device,
        rhi::backend::PipelineTargets::depth_only(),
        layout,
        &rhi::RenderPipelineDesc {
            cull_mode: rhi::CullMode::Front,
            depth: depth_state(rhi::CompareFunction::Greater),
            ..rhi::RenderPipelineDesc::new(
                "distant chunk bounds",
                shader::include_shader!("distant_bound.vert"),
                shader::include_shader!("distant_bound.frag"),
                &[],
            )
        },
    )
}

/// The image's depth set to the near plane where the chunks drew.
fn create_setup_pipeline(
    device: &rhi::backend::Device,
    layout: rhi::backend::PipelineLayout,
) -> rhi::backend::RenderPipeline {
    fullscreen_pipeline(
        device,
        image_targets(),
        layout,
        "distant depth setup",
        shader::include_shader!("distant_setup.frag"),
        depth_state(rhi::CompareFunction::Always),
        None,
    )
}

/// The image over the frame, depth-tested against the chunks and written.
fn create_blit_pipeline(
    device: &rhi::backend::Device,
    swapchain: rhi::backend::PipelineTargets,
    layout: rhi::backend::PipelineLayout,
) -> rhi::backend::RenderPipeline {
    fullscreen_pipeline(
        device,
        swapchain,
        layout,
        "distant blit",
        shader::include_shader!("distant_blit.frag"),
        Some(rhi::DepthState::WRITE),
        Some(rhi::BlendState::TRANSLUCENT),
    )
}
