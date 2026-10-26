//! Vanilla's spectator view effects (`post_effect/creeper.json`,
//! `spider.json`, `invert.json`), run over the frame after the glowing
//! outline, before the HUD. The frame is copied out of the sRGB swapchain into
//! `main` as vanilla's gamma-space colours, in GL's bottom-up rows, so the
//! ported shaders and their uv maths run unchanged, then written back.

use super::post;
use crate::renderer::{rhi, shader, util};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PostEffect {
    Creeper,
    Spider,
    Invert,
}

impl PostEffect {
    /// The `post_effect/` chain's id.
    #[cfg(feature = "test-harness")]
    pub fn id(self) -> &'static str {
        match self {
            Self::Creeper => "creeper",
            Self::Spider => "spider",
            Self::Invert => "invert",
        }
    }
}

/// The chains' screen-size targets: vanilla's `minecraft:main` and each
/// chain's own.
#[derive(Clone, Copy)]
enum Target {
    Main,
    Swap,
    Temp,
    LargeBlur,
    SmallBlur,
}

const TARGET_NAMES: [&str; 5] = [
    "post_main",
    "post_swap",
    "post_temp",
    "post_large_blur",
    "post_small_blur",
];

/// The (`In`, `Blur`) pairs spiderclip reads.
const CLIP_INPUTS: [(Target, Target); 3] = [
    (Target::Main, Target::LargeBlur),
    (Target::SmallBlur, Target::Temp),
    (Target::SmallBlur, Target::Swap),
];

/// `spider.json`'s spiderclip passes: their [`CLIP_INPUTS`] pair, output,
/// and `RotScaleConfig` + `SpiderConfig` uniforms.
const SPIDER_CLIPS: [(usize, Target, [f32; 16]); 5] = {
    use Target::*;
    const FULL: [f32; 4] = [0.0, 0.0, 1.0, 1.0];
    const NARROW: [f32; 4] = [0.21, 0.0, 0.79, 1.0];
    const VIGNETTE: [f32; 4] = [0.31, 0.1, 0.69, 0.9];
    [
        (
            0,
            Temp,
            clip([1.25, 2.0], [-0.125, -0.1], 0.0, FULL, [0.1, 0.1, 0.9, 0.9]),
        ),
        (
            1,
            Swap,
            clip([2.35, 4.2], [-1.1, -1.5], -45.0, NARROW, VIGNETTE),
        ),
        (
            2,
            Temp,
            clip([2.35, 4.2], [0.45, -4.45], 45.0, NARROW, VIGNETTE),
        ),
        (
            1,
            Swap,
            clip([2.35, 2.35], [-0.385, -1.29], 0.0, FULL, VIGNETTE),
        ),
        (
            2,
            Temp,
            clip([2.35, 2.35], [-0.965, -1.29], 0.0, FULL, VIGNETTE),
        ),
    ]
};

/// spiderclip's push constants: rotscale's `InScale`, `InOffset` and
/// `InRotation`, then `Scissor` and `Vignette` at their std430 offsets.
const fn clip(
    scale: [f32; 2],
    offset: [f32; 2],
    rotation: f32,
    scissor: [f32; 4],
    vignette: [f32; 4],
) -> [f32; 16] {
    [
        scale[0],
        scale[1],
        offset[0],
        offset[1],
        rotation,
        0.0,
        0.0,
        0.0,
        scissor[0],
        scissor[1],
        scissor[2],
        scissor[3],
        vignette[0],
        vignette[1],
        vignette[2],
        vignette[3],
    ]
}

/// The targets, made while an effect is on.
struct Targets {
    extent: rhi::Extent,
    /// The swapchain's copy, sRGB so sampling decodes it.
    frame: rhi::backend::Texture,
    textures: [rhi::backend::Texture; 5],
    frame_group: rhi::backend::BindGroup,
    nearest: [rhi::backend::BindGroup; 5],
    linear: [rhi::backend::BindGroup; 5],
    /// By [`CLIP_INPUTS`].
    clip: [rhi::backend::BindGroup; 3],
}

impl Targets {
    fn new(device: &rhi::backend::Device, effects: &PostEffects, extent: rhi::Extent) -> Self {
        let frame = rhi::backend::Texture::new(
            device,
            &rhi::TextureDesc::new(
                "post_frame",
                extent.width.max(1),
                extent.height.max(1),
                rhi::TextureFormat::Rgba8UnormSrgb,
                rhi::TextureUsages::COPY_DST | rhi::TextureUsages::TEXTURE_BINDING,
            ),
        );
        let textures = TARGET_NAMES.map(|name| post::create_target(device, extent, name));
        let group = |texture: &rhi::backend::Texture, sampler| {
            util::create_texture_group(device, effects.texture_layout, texture.handle(), sampler)
        };
        let clip = CLIP_INPUTS.map(|(input, blur)| {
            let [texture, sampler] =
                util::texture_binding(textures[input as usize].handle(), effects.nearest);
            let [blur_texture, blur_sampler] =
                util::texture_binding(textures[blur as usize].handle(), effects.nearest);
            device.create_bind_group(
                effects.clip_layout,
                &[texture, sampler, (2, blur_texture.1), (3, blur_sampler.1)],
            )
        });
        Self {
            extent,
            frame_group: group(&frame, effects.nearest),
            nearest: std::array::from_fn(|i| group(&textures[i], effects.nearest)),
            linear: std::array::from_fn(|i| group(&textures[i], effects.linear)),
            clip,
            frame,
            textures,
        }
    }

    fn handle(&self, target: Target) -> rhi::backend::TextureHandle {
        self.textures[target as usize].handle()
    }

    fn destroy(self, device: &rhi::backend::Device) {
        self.frame.destroy(device);
        for texture in self.textures {
            texture.destroy(device);
        }
        for group in [self.frame_group]
            .into_iter()
            .chain(self.nearest)
            .chain(self.linear)
            .chain(self.clip)
        {
            group.destroy(device);
        }
    }
}

pub struct PostEffects {
    effect: Option<PostEffect>,
    targets: Option<Targets>,
    nearest: rhi::backend::Sampler,
    linear: rhi::backend::Sampler,
    texture_layout: rhi::backend::BindGroupLayout,
    clip_layout: rhi::backend::BindGroupLayout,
    pipeline_layout: rhi::backend::PipelineLayout,
    clip_pipeline_layout: rhi::backend::PipelineLayout,
    decode: rhi::backend::RenderPipeline,
    encode: rhi::backend::RenderPipeline,
    invert: rhi::backend::RenderPipeline,
    color_convolve: rhi::backend::RenderPipeline,
    bits: rhi::backend::RenderPipeline,
    box_blur: rhi::backend::RenderPipeline,
    blit: rhi::backend::RenderPipeline,
    spiderclip: rhi::backend::RenderPipeline,
}

impl PostEffects {
    pub fn new(
        device: &rhi::backend::Device,
        frame_targets: rhi::backend::PipelineTargets,
    ) -> Self {
        let texture_layout = rhi::backend::create_bind_group_layout(device, util::FRAGMENT_TEXTURE);
        let clip_layout = rhi::backend::create_bind_group_layout(
            device,
            &[util::FRAGMENT_TEXTURE, util::FRAGMENT_TEXTURE].concat(),
        );
        let pipeline_layout = rhi::backend::create_pipeline_layout(
            device,
            &[texture_layout],
            Some(post::PUSH_CONSTANTS),
        );
        let clip_pipeline_layout = rhi::backend::create_pipeline_layout(
            device,
            &[clip_layout],
            Some(post::PUSH_CONSTANTS),
        );
        let pipeline = |label, fragment| {
            post::create_pipeline(
                device,
                post::targets(),
                pipeline_layout,
                label,
                None,
                fragment,
                None,
            )
        };
        Self {
            effect: None,
            targets: None,
            nearest: rhi::backend::create_sampler(device, &rhi::SamplerDesc::NEAREST),
            linear: rhi::backend::create_sampler(device, &rhi::SamplerDesc::LINEAR),
            decode: pipeline("post decode", shader::include_shader!("post_gamma.frag")),
            encode: create_encode_pipeline(device, frame_targets, pipeline_layout),
            invert: pipeline("post invert", shader::include_shader!("post_invert.frag")),
            color_convolve: pipeline(
                "post color convolve",
                shader::include_shader!("post_color_convolve.frag"),
            ),
            bits: pipeline("post bits", shader::include_shader!("post_bits.frag")),
            box_blur: pipeline(
                "post box blur",
                shader::include_shader!("post_box_blur.frag"),
            ),
            blit: pipeline("post blit", shader::include_shader!("post_blit.frag")),
            spiderclip: post::create_pipeline(
                device,
                post::targets(),
                clip_pipeline_layout,
                "post spiderclip",
                Some(shader::include_shader!("post_rotscale.vert")),
                shader::include_shader!("post_spiderclip.frag"),
                None,
            ),
            texture_layout,
            clip_layout,
            pipeline_layout,
            clip_pipeline_layout,
        }
    }

    pub fn effect(&self) -> Option<PostEffect> {
        self.effect
    }

    /// Sets the effect to run, freeing the targets once none is.
    pub fn set(&mut self, device: &rhi::backend::Device, effect: Option<PostEffect>) {
        self.effect = effect;
        if effect.is_none()
            && let Some(targets) = self.targets.take()
        {
            device.wait_idle();
            targets.destroy(device);
        }
    }

    /// Copies `frame` (the swapchain image, between passes) into `main` and
    /// runs the effect's chain over it; [`Self::finish`] writes it back.
    pub fn apply(
        &mut self,
        device: &rhi::backend::Device,
        encoder: &rhi::backend::CommandEncoder,
        frame: rhi::backend::TextureHandle,
        extent: rhi::Extent,
    ) {
        let Some(effect) = self.effect else {
            return;
        };
        if self.targets.as_ref().is_none_or(|t| t.extent != extent) {
            if let Some(stale) = self.targets.take() {
                device.wait_idle();
                stale.destroy(device);
            }
            self.targets = Some(Targets::new(device, self, extent));
        }
        let t = self.targets.as_ref().expect("made above");

        let size = [extent.width, extent.height];
        util::copy_frame(encoder, frame, size, t.frame.handle(), size);

        let pass = |target: Target, pipeline, inputs, uniforms: &[f32]| {
            post::run(
                device,
                encoder,
                t.handle(target),
                extent,
                pipeline,
                inputs,
                uniforms,
            );
        };
        let nearest = |target: Target| t.nearest[target as usize];
        let linear = |target: Target| t.linear[target as usize];
        pass(Target::Main, &self.decode, t.frame_group, &[1.0 / 2.2]);
        match effect {
            PostEffect::Invert => {
                pass(Target::Swap, &self.invert, nearest(Target::Main), &[0.8]);
                pass(Target::Main, &self.blit, nearest(Target::Swap), &[1.0; 4]);
            }
            PostEffect::Creeper => {
                #[rustfmt::skip]
                let matrices = [
                    0.0, 0.0, 0.0, 0.0,
                    0.3, 0.59, 0.11, 0.0,
                    0.0, 0.0, 0.0, 0.0,
                ];
                pass(
                    Target::Swap,
                    &self.color_convolve,
                    nearest(Target::Main),
                    &matrices,
                );
                pass(
                    Target::Main,
                    &self.bits,
                    nearest(Target::Swap),
                    &[16.0, 4.0],
                );
            }
            PostEffect::Spider => {
                for (radius, blurred) in [(15.0, Target::LargeBlur), (7.0, Target::SmallBlur)] {
                    pass(
                        Target::Temp,
                        &self.box_blur,
                        linear(Target::Main),
                        &[1.0, 0.0, radius],
                    );
                    pass(
                        blurred,
                        &self.box_blur,
                        linear(Target::Temp),
                        &[0.0, 1.0, radius],
                    );
                }
                for (inputs, output, uniforms) in SPIDER_CLIPS {
                    pass(output, &self.spiderclip, t.clip[inputs], &uniforms);
                }
                pass(
                    Target::Main,
                    &self.blit,
                    nearest(Target::Temp),
                    &[1.0, 0.8, 0.8, 1.0],
                );
            }
        }
    }

    /// Writes `main` back over the frame, in the reopened frame pass.
    pub fn finish(&self, pass: &mut rhi::backend::RenderPassEncoder) {
        if let Some(t) = &self.targets {
            post::draw(pass, &self.encode, t.nearest[Target::Main as usize], &[2.2]);
        }
    }

    /// Rebuilds the write-back against a new frame format.
    pub fn recreate_pipeline(
        &mut self,
        device: &rhi::backend::Device,
        frame_targets: rhi::backend::PipelineTargets,
    ) {
        self.encode.destroy(device);
        self.encode = create_encode_pipeline(device, frame_targets, self.pipeline_layout);
    }

    pub fn destroy(&mut self, device: &rhi::backend::Device) {
        if let Some(targets) = self.targets.take() {
            targets.destroy(device);
        }
        for pipeline in [
            self.decode,
            self.encode,
            self.invert,
            self.color_convolve,
            self.bits,
            self.box_blur,
            self.blit,
            self.spiderclip,
        ] {
            pipeline.destroy(device);
        }
        self.pipeline_layout.destroy(device);
        self.clip_pipeline_layout.destroy(device);
        self.texture_layout.destroy(device);
        self.clip_layout.destroy(device);
        self.nearest.destroy(device);
        self.linear.destroy(device);
    }
}

fn create_encode_pipeline(
    device: &rhi::backend::Device,
    frame_targets: rhi::backend::PipelineTargets,
    layout: rhi::backend::PipelineLayout,
) -> rhi::backend::RenderPipeline {
    post::create_pipeline(
        device,
        frame_targets,
        layout,
        "post encode",
        None,
        shader::include_shader!("post_gamma.frag"),
        None,
    )
}
