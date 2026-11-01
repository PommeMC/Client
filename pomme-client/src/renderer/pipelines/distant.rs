//! Distant terrain's passes (Voxy's `BoundRenderer`, `AbstractRenderPipeline`,
//! `NormalRenderPipeline` and `MDICSectionRenderer`): the drawn chunks' boxes
//! into a depth bound; last frame's draw list into the far terrain's own
//! image under its own projection where the chunks drew nothing; the listed
//! sections' boxes tested against that depth; this frame's draw list built
//! from the ones that showed and the newly seen drawn after; then the image
//! blitted into the frame with its depth carried over.

use glam::{DVec3, Mat4};

use crate::renderer::camera::{Camera, CameraUniform};
use crate::renderer::chunk::buffer::DrawCommand;
use crate::renderer::distant::gpu::{
    COUNTS_BYTES, COUNTS_CULL_DRAW, CUBE_FIRST_INDEX, DISTANT_FAR, DRAWS_BYTES, DistantBuffers,
    DrawBuffers, POSITIONS_BYTES, RENDER_LIST_BYTES, Region, SECTIONS_BYTES, TRANSLUCENT_BUCKETS,
    TRANSLUCENT_BYTES, VISIBILITY_BYTES,
};
use crate::renderer::pipelines::lightmap::Lightmap;
use crate::renderer::rhi::TextureUse;
use crate::renderer::rhi::backend::TextureTransition;
use crate::renderer::{MAX_FRAMES_IN_FLIGHT, rhi, shader, util};

/// Distant terrain's image: plain RGBA8, as Voxy's.
const FORMAT: rhi::TextureFormat = rhi::TextureFormat::Rgba8Unorm;
const DEPTH: rhi::TextureFormat = rhi::TextureFormat::Depth32Float;

/// Voxy's `SceneUniform` (`distant_section.glsl`): the distant view
/// projection moved by the camera's position within its 32-block section,
/// that section, the frame, and whether the occlusion pass was skipped.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct SceneUniform {
    view_proj: [f32; 16],
    camera_section: [i32; 3],
    frame_id: u32,
    camera_sub_pos: [f32; 3],
    occlusion_skipped: u32,
}

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
const COMPUTE_UNIFORM: rhi::BindGroupLayoutEntry =
    rhi::BindGroupLayoutEntry::uniform(rhi::ShaderStages::COMPUTE);
const COMPUTE_STORAGE: rhi::BindGroupLayoutEntry =
    rhi::BindGroupLayoutEntry::storage(rhi::ShaderStages::COMPUTE);
/// `distant_prep.comp`: the draw counts and the render list.
const PREP_SET: &[rhi::BindGroupLayoutEntry] = &[COMPUTE_STORAGE; 2];
/// `distant_cmdgen.comp`: the scene, then the draws, counts, sections,
/// visibility, render list, positions and translucent data.
const CMDGEN_SET: &[rhi::BindGroupLayoutEntry] = &[
    COMPUTE_UNIFORM,
    COMPUTE_STORAGE,
    COMPUTE_STORAGE,
    COMPUTE_STORAGE,
    COMPUTE_STORAGE,
    COMPUTE_STORAGE,
    COMPUTE_STORAGE,
    COMPUTE_STORAGE,
];
/// `distant_prefix_sum.comp`: the translucent data.
const PREFIX_SET: &[rhi::BindGroupLayoutEntry] = &[COMPUTE_STORAGE];
/// `distant_translucents.comp`: the scene, then the draws, counts,
/// sections, render list and translucent data.
const TRANSLUCENTS_SET: &[rhi::BindGroupLayoutEntry] = &[
    COMPUTE_UNIFORM,
    COMPUTE_STORAGE,
    COMPUTE_STORAGE,
    COMPUTE_STORAGE,
    COMPUTE_STORAGE,
    COMPUTE_STORAGE,
];
/// `distant_cull.vert`/`.frag`: the scene, the sections, the visibility the
/// fragments write, and the render list.
const CULL_SET: &[rhi::BindGroupLayoutEntry] = &[
    rhi::BindGroupLayoutEntry::uniform(rhi::ShaderStages::VERTEX),
    rhi::BindGroupLayoutEntry::storage(rhi::ShaderStages::VERTEX),
    rhi::BindGroupLayoutEntry::storage(rhi::ShaderStages::VERTEX_FRAGMENT),
    rhi::BindGroupLayoutEntry::storage(rhi::ShaderStages::VERTEX),
];

/// The screen-size targets, remade with the swapchain.
struct Targets {
    extent: rhi::Extent,
    color: rhi::backend::Texture,
    depth: rhi::backend::Texture,
    /// The far side of the drawn chunks' boxes (`depthBoundingBuffer`).
    bound: rhi::backend::Texture,
    /// The draw positions with the depth bound.
    draw_set: rhi::backend::BindGroup,
    setup_set: rhi::backend::BindGroup,
    blit_set: rhi::backend::BindGroup,
}

pub struct DistantPipeline {
    /// The quads, opaque then translucent, into the image.
    quads: [rhi::backend::RenderPipeline; 2],
    bound: rhi::backend::RenderPipeline,
    setup: rhi::backend::RenderPipeline,
    blit: rhi::backend::RenderPipeline,
    /// The occlusion pass; `None` where the fragment stage can't write
    /// buffers, and every listed section then counts as seen.
    cull: Option<rhi::backend::RenderPipeline>,
    quads_layout: rhi::backend::PipelineLayout,
    bound_layout: rhi::backend::PipelineLayout,
    setup_layout: rhi::backend::PipelineLayout,
    blit_layout: rhi::backend::PipelineLayout,
    cull_layout: rhi::backend::PipelineLayout,
    /// A vertex-stage uniform: the camera for the bound pass, the scene
    /// for the quads.
    camera_layout: rhi::backend::BindGroupLayout,
    camera_fragment_layout: rhi::backend::BindGroupLayout,
    texture_layout: rhi::backend::BindGroupLayout,
    blit_inputs_layout: rhi::backend::BindGroupLayout,
    draw_layout: rhi::backend::BindGroupLayout,
    cull_set_layout: rhi::backend::BindGroupLayout,
    /// The camera for the bound pass and the blit, which fogs in the
    /// fragment stage.
    camera_sets: Vec<rhi::backend::BindGroup>,
    camera_fragment_sets: Vec<rhi::backend::BindGroup>,
    camera_buffers: Vec<rhi::backend::Buffer>,
    scene_sets: Vec<rhi::backend::BindGroup>,
    scene_buffers: Vec<rhi::backend::Buffer>,
    cull_sets: Vec<rhi::backend::BindGroup>,
    prep: util::ComputePass,
    cmdgen: util::ComputePass,
    prefix_sum: util::ComputePass,
    translucents: util::ComputePass,
    sampler: rhi::backend::Sampler,
    targets: Option<Targets>,
    /// Set 2 of the quads, which [`Lightmap`] owns.
    lightmap_set: rhi::backend::BindGroup,
}

impl DistantPipeline {
    pub fn new(
        device: &rhi::backend::Device,
        swapchain: rhi::backend::PipelineTargets,
        lightmap: &Lightmap,
        buffers: &DistantBuffers,
        fragment_stores: bool,
    ) -> Self {
        let layout = |entries| rhi::backend::create_bind_group_layout(device, entries);
        let camera_layout = layout(util::VERTEX_UNIFORM);
        let camera_fragment_layout = layout(util::VERTEX_FRAGMENT_UNIFORM);
        let texture_layout = layout(TEXTURE_SAMPLER);
        let blit_inputs_layout = layout(BLIT_INPUTS);
        let draw_layout = layout(DRAW_SET);
        let cull_set_layout = layout(CULL_SET);
        let push = |visibility, size| Some(rhi::PushConstants { visibility, size });
        let pipeline_layout = |sets: &[rhi::backend::BindGroupLayout], push| {
            rhi::backend::create_pipeline_layout(device, sets, push)
        };
        let quads_layout = pipeline_layout(
            &[
                camera_layout,
                buffers.layout(),
                lightmap.layout,
                draw_layout,
            ],
            None,
        );
        // The view projection, once a pass.
        let bound_layout = pipeline_layout(
            &[camera_layout, buffers.bounds().layout()],
            push(rhi::ShaderStages::VERTEX, 64),
        );
        let setup_layout = pipeline_layout(&[texture_layout], None);
        // The inverse distant view projection, then the fade.
        let blit_layout = pipeline_layout(
            &[camera_fragment_layout, blit_inputs_layout],
            push(rhi::ShaderStages::FRAGMENT, 80),
        );
        let cull_layout = pipeline_layout(&[cull_set_layout], None);
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
        let scene_bytes = size_of::<SceneUniform>() as u64;
        let (scene_sets, scene_buffers) =
            util::create_uniform_sets(device, camera_layout, scene_bytes, "distant_scene_uniform");
        let draw = buffers.draw_buffers();
        let scene = |frame: usize| {
            (
                0,
                rhi::backend::BindingResource::uniform(&scene_buffers[frame], scene_bytes),
            )
        };
        let render_list = |binding, frame: usize| {
            (
                binding,
                rhi::backend::BindingResource::storage(
                    buffers.render_list(frame),
                    RENDER_LIST_BYTES,
                ),
            )
        };
        let storage = |binding, buffer, bytes| {
            (
                binding,
                rhi::backend::BindingResource::storage(buffer, bytes),
            )
        };
        let counts = |binding| storage(binding, draw.counts, COUNTS_BYTES);
        let draws = |binding| storage(binding, draw.draws, DRAWS_BYTES);
        let sections = |binding| storage(binding, draw.sections, SECTIONS_BYTES);
        let visibility = |binding| storage(binding, draw.visibility, VISIBILITY_BYTES);
        let translucent = |binding| storage(binding, draw.translucent, TRANSLUCENT_BYTES);
        let cull_sets = (0..MAX_FRAMES_IN_FLIGHT)
            .map(|frame| {
                device.create_bind_group(
                    cull_set_layout,
                    &[
                        scene(frame),
                        sections(1),
                        visibility(2),
                        render_list(3, frame),
                    ],
                )
            })
            .collect();
        let pass = |label, shader, entries, frames, bindings: &dyn Fn(usize) -> Vec<_>| {
            util::ComputePass::new(device, label, shader, entries, None, frames, bindings)
        };
        Self {
            quads: create_quad_pipelines(device, quads_layout),
            bound: create_bound_pipeline(device, bound_layout),
            setup: create_setup_pipeline(device, setup_layout),
            blit: create_blit_pipeline(device, swapchain, blit_layout),
            cull: fragment_stores.then(|| create_cull_pipeline(device, cull_layout)),
            prep: pass(
                "distant prep",
                shader::include_shader!("distant_prep.comp"),
                PREP_SET,
                MAX_FRAMES_IN_FLIGHT,
                &|frame| vec![counts(0), render_list(1, frame)],
            ),
            cmdgen: pass(
                "distant command generation",
                shader::include_shader!("distant_cmdgen.comp"),
                CMDGEN_SET,
                MAX_FRAMES_IN_FLIGHT,
                &|frame| {
                    vec![
                        scene(frame),
                        draws(1),
                        counts(2),
                        sections(3),
                        visibility(4),
                        render_list(5, frame),
                        storage(6, draw.positions, POSITIONS_BYTES),
                        translucent(7),
                    ]
                },
            ),
            prefix_sum: pass(
                "distant prefix sum",
                shader::include_shader!("distant_prefix_sum.comp"),
                PREFIX_SET,
                1,
                &|_| vec![translucent(0)],
            ),
            translucents: pass(
                "distant translucents",
                shader::include_shader!("distant_translucents.comp"),
                TRANSLUCENTS_SET,
                MAX_FRAMES_IN_FLIGHT,
                &|frame| {
                    vec![
                        scene(frame),
                        draws(1),
                        counts(2),
                        sections(3),
                        render_list(4, frame),
                        translucent(5),
                    ]
                },
            ),
            quads_layout,
            bound_layout,
            setup_layout,
            blit_layout,
            cull_layout,
            camera_layout,
            camera_fragment_layout,
            texture_layout,
            blit_inputs_layout,
            draw_layout,
            cull_set_layout,
            camera_sets,
            camera_fragment_sets,
            camera_buffers,
            scene_sets,
            scene_buffers,
            cull_sets,
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
        draw: DrawBuffers,
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
            let draw_set = device.create_bind_group(
                self.draw_layout,
                &[
                    (
                        0,
                        rhi::backend::BindingResource::storage(draw.positions, POSITIONS_BYTES),
                    ),
                    (1, rhi::backend::BindingResource::Texture(bound.handle())),
                ],
            );
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
                draw_set,
                blit_set,
            });
        }
    }

    /// Voxy's `renderOpaque` and `runPipeline` up to the blit, after the
    /// chunks' solid and cutout passes, whose depth `frame_depth` holds:
    /// the bound pass, last frame's opaque draws into the image, the
    /// occlusion pass over the listed sections, this frame's draw list, and
    /// the newly seen and translucent draws.
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
        let draw = buffers.draw_buffers();
        self.ensure_targets(device, extent, frame_depth, draw);
        let targets = self.targets.as_ref().expect("made above");
        let (color, depth, bound) = (
            targets.color.handle(),
            targets.depth.handle(),
            targets.bound.handle(),
        );
        let view_proj = camera.distant_view_projection(DISTANT_FAR);
        // `SceneUniform.setup`: the camera's 32-block section, and the
        // projection moved by where it is within it.
        let camera_section = (eye / 32.0).floor();
        let sub_pos = (eye - camera_section * 32.0).as_vec3();
        self.scene_buffers[frame].write(bytemuck::bytes_of(&SceneUniform {
            view_proj: (view_proj * Mat4::from_translation(-sub_pos)).to_cols_array(),
            camera_section: camera_section.as_ivec3().to_array(),
            frame_id: buffers.frame_id(),
            camera_sub_pos: sub_pos.to_array(),
            occlusion_skipped: u32::from(self.cull.is_none()),
        }));
        let (counts, translucent) = (draw.counts.handle(), draw.translucent.handle());
        let indices = buffers.index_buffer();
        buffers.write_render_list(frame);

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
                depth: Some(depth_attachment(bound, rhi::LoadOp::Clear(0.0))),
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
            pass.set_index_buffer(indices);
            pass.draw_indexed(36, bounds.count(), CUBE_FIRST_INDEX, 0, 0);
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

        // `initDepthStencil`, then `renderOpaque`: the draw list built last
        // frame.
        let mut pass = self.begin_image_pass(device, encoder, targets, extent, true);
        pass.set_pipeline(&self.setup);
        pass.set_bind_group(0, targets.setup_set);
        pass.draw(3, 1, 0, 0);
        self.bind_quads(&mut pass, frame, targets, false);
        buffers.draw(&pass, Region::Opaque);
        pass.end();

        // `prep.comp`, after last frame's draws and dispatches are done
        // with the counts, positions and translucent buckets.
        encoder.transition_buffers(rhi::BufferUse::Indirect, rhi::BufferUse::Storage);
        encoder.transition_buffers(rhi::BufferUse::ShaderRead, rhi::BufferUse::Storage);
        encoder.transition_buffers(rhi::BufferUse::Storage, rhi::BufferUse::CopyDst);
        encoder.dispatch(&self.prep.pipeline, self.prep.set(frame), &[], [1, 1, 1]);
        encoder.fill_buffer(translucent, 0, u64::from(TRANSLUCENT_BUCKETS) * 4, 0);
        encoder.transition_buffers(rhi::BufferUse::StorageWrite, rhi::BufferUse::Indirect);
        encoder.transition_buffers(rhi::BufferUse::StorageWrite, rhi::BufferUse::Storage);
        encoder.transition_buffers(rhi::BufferUse::CopyDst, rhi::BufferUse::Storage);

        // The occlusion pass: each listed section's box against the image's
        // depth, the ones that show marked seen this frame.
        if let Some(cull) = &self.cull {
            encoder.transition_textures(&[TextureTransition::color(
                depth,
                TextureUse::DepthAttachment,
                TextureUse::DepthAttachment,
            )]);
            let mut pass = encoder.begin_render_pass(
                device,
                &rhi::backend::RenderPassDesc {
                    color: None,
                    extra_colors: Vec::new(),
                    depth: Some(depth_attachment(depth, rhi::LoadOp::Load)),
                    extent,
                },
            );
            pass.set_pipeline(cull);
            pass.set_bind_group(0, self.cull_sets[frame]);
            pass.set_index_buffer(indices);
            pass.draw_indexed_indirect(
                counts,
                COUNTS_CULL_DRAW,
                1,
                size_of::<DrawCommand>() as u32,
            );
            pass.end();
            encoder.transition_buffers(rhi::BufferUse::FragmentWrite, rhi::BufferUse::Storage);
        }

        // `cmdgen.comp`, then the translucent draws sorted far to near.
        encoder.dispatch_indirect(
            &self.cmdgen.pipeline,
            self.cmdgen.set(frame),
            &[],
            counts,
            0,
        );
        encoder.transition_buffers(rhi::BufferUse::StorageWrite, rhi::BufferUse::Storage);
        encoder.dispatch(
            &self.prefix_sum.pipeline,
            self.prefix_sum.set(frame),
            &[],
            [1, 1, 1],
        );
        encoder.transition_buffers(rhi::BufferUse::StorageWrite, rhi::BufferUse::Storage);
        encoder.dispatch_indirect(
            &self.translucents.pipeline,
            self.translucents.set(frame),
            &[],
            counts,
            0,
        );
        encoder.transition_buffers(rhi::BufferUse::StorageWrite, rhi::BufferUse::Indirect);
        encoder.transition_buffers(rhi::BufferUse::StorageWrite, rhi::BufferUse::ShaderRead);

        // `renderTemporal` and `renderTranslucent`: the sections seen this
        // frame but not last, then the translucent quads.
        let mut pass = self.begin_image_pass(device, encoder, targets, extent, false);
        self.bind_quads(&mut pass, frame, targets, false);
        buffers.draw(&pass, Region::Temporal);
        self.bind_quads(&mut pass, frame, targets, true);
        buffers.draw(&pass, Region::Translucent);
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

    /// A pass into the image and its depth, cleared when `first`.
    fn begin_image_pass(
        &self,
        device: &rhi::backend::Device,
        encoder: &rhi::backend::CommandEncoder,
        targets: &Targets,
        extent: rhi::Extent,
        first: bool,
    ) -> rhi::backend::RenderPassEncoder {
        encoder.begin_render_pass(
            device,
            &rhi::backend::RenderPassDesc {
                color: Some(rhi::backend::ColorAttachment {
                    texture: targets.color.handle(),
                    format: rhi::backend::texture_format(FORMAT),
                    load: if first {
                        rhi::LoadOp::Clear([0.0; 4])
                    } else {
                        rhi::LoadOp::Load
                    },
                    store: rhi::StoreOp::Store,
                }),
                extra_colors: Vec::new(),
                depth: Some(depth_attachment(
                    targets.depth.handle(),
                    if first {
                        rhi::LoadOp::Clear(1.0)
                    } else {
                        rhi::LoadOp::Load
                    },
                )),
                extent,
            },
        )
    }

    /// The quad pipeline and its sets but the arena's, which
    /// [`DistantBuffers::draw`] binds.
    fn bind_quads(
        &self,
        pass: &mut rhi::backend::RenderPassEncoder,
        frame: usize,
        targets: &Targets,
        translucent: bool,
    ) {
        pass.set_pipeline(&self.quads[usize::from(translucent)]);
        pass.set_bind_group(0, self.scene_sets[frame]);
        pass.set_bind_group(2, self.lightmap_set);
        pass.set_bind_group(3, targets.draw_set);
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
        for set in [targets.draw_set, targets.setup_set, targets.blit_set] {
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
        util::destroy_buffers(device, &mut self.scene_buffers);
        util::destroy_bind_groups(device, self.camera_sets.drain(..));
        util::destroy_bind_groups(device, self.camera_fragment_sets.drain(..));
        util::destroy_bind_groups(device, self.scene_sets.drain(..));
        util::destroy_bind_groups(device, self.cull_sets.drain(..));
        for pass in [
            &mut self.prep,
            &mut self.cmdgen,
            &mut self.prefix_sum,
            &mut self.translucents,
        ] {
            pass.destroy(device);
        }
        for pipeline in self
            .quads
            .iter_mut()
            .chain([&mut self.bound, &mut self.setup, &mut self.blit])
            .chain(self.cull.as_mut())
        {
            pipeline.destroy(device);
        }
        for layout in [
            self.quads_layout,
            self.bound_layout,
            self.setup_layout,
            self.blit_layout,
            self.cull_layout,
        ] {
            layout.destroy(device);
        }
        for layout in [
            self.camera_layout,
            self.camera_fragment_layout,
            self.texture_layout,
            self.blit_inputs_layout,
            self.draw_layout,
            self.cull_set_layout,
        ] {
            layout.destroy(device);
        }
        self.sampler.destroy(device);
    }
}

/// A `texture` loaded by `load` and kept after the pass.
fn depth_attachment(
    texture: rhi::backend::TextureHandle,
    load: rhi::LoadOp<f32>,
) -> rhi::backend::DepthAttachment {
    rhi::backend::DepthAttachment {
        texture,
        load,
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

/// The listed sections' boxes, front faces tested against the image's
/// depth and nothing written but the visibility.
fn create_cull_pipeline(
    device: &rhi::backend::Device,
    layout: rhi::backend::PipelineLayout,
) -> rhi::backend::RenderPipeline {
    rhi::backend::create_render_pipeline(
        device,
        rhi::backend::PipelineTargets::depth_only(),
        layout,
        &rhi::RenderPipelineDesc {
            cull_mode: rhi::CullMode::Back,
            depth: Some(rhi::DepthState::READ_ONLY),
            ..rhi::RenderPipelineDesc::new(
                "distant occlusion",
                shader::include_shader!("distant_cull.vert"),
                shader::include_shader!("distant_cull.frag"),
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
