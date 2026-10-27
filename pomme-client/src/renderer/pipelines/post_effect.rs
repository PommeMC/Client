//! Vanilla's post chains over the whole frame: the spectator view effects
//! (`post_effect/creeper.json`, `spider.json`, `invert.json`), run after the
//! glowing outline before the HUD, and the menu background blur
//! (`post_effect/blur.json`). The frame is copied out of the swapchain into
//! `main` in GL's bottom-up rows, so the ported shaders and their uv maths run
//! unchanged, then written back.

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

/// `minecraft:main` and every chain's `swap`, kept at the frame's size.
const FRAME_TARGETS: [&str; 2] = ["post_main", "post_swap"];
/// `spider.json`'s own targets, after [`FRAME_TARGETS`] in [`Target`]'s order.
const SPIDER_TARGETS: [&str; 3] = ["post_temp", "post_large_blur", "post_small_blur"];

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

/// A screen-size target per name, with its nearest and bilinear groups.
struct TargetSet {
    textures: Vec<rhi::backend::Texture>,
    nearest: Vec<rhi::backend::BindGroup>,
    linear: Vec<rhi::backend::BindGroup>,
}

impl TargetSet {
    fn new(
        device: &rhi::backend::Device,
        samplers: &Samplers,
        extent: rhi::Extent,
        names: &[&str],
    ) -> Self {
        let textures: Vec<_> = names
            .iter()
            .map(|name| post::create_target(device, extent, name))
            .collect();
        let groups = |sampler| {
            textures
                .iter()
                .map(|texture| samplers.group(device, texture.handle(), sampler))
                .collect()
        };
        Self {
            nearest: groups(samplers.nearest),
            linear: groups(samplers.linear),
            textures,
        }
    }

    fn destroy(&mut self, device: &rhi::backend::Device) {
        for texture in &mut self.textures {
            std::mem::take(texture).destroy(device);
        }
        for group in self.nearest.drain(..).chain(self.linear.drain(..)) {
            group.destroy(device);
        }
    }
}

/// The set `target` lives in, and its index there: [`FRAME_TARGETS`] first,
/// then [`SPIDER_TARGETS`].
fn locate<'a>(
    frame: &'a TargetSet,
    spider: Option<&'a TargetSet>,
    target: Target,
) -> (&'a TargetSet, usize) {
    let i = target as usize;
    let frame_count = FRAME_TARGETS.len();
    if i < frame_count {
        (frame, i)
    } else {
        (
            spider.expect("made with the spider effect"),
            i - frame_count,
        )
    }
}

/// The texture layout and samplers every target's groups use.
struct Samplers {
    layout: rhi::backend::BindGroupLayout,
    nearest: rhi::backend::Sampler,
    linear: rhi::backend::Sampler,
}

impl Samplers {
    fn group(
        &self,
        device: &rhi::backend::Device,
        texture: rhi::backend::TextureHandle,
        sampler: rhi::backend::Sampler,
    ) -> rhi::backend::BindGroup {
        util::create_texture_group(device, self.layout, texture, sampler)
    }
}

/// The frame's copy and [`FRAME_TARGETS`], at the frame's size.
struct FrameTargets {
    extent: rhi::Extent,
    /// The swapchain's copy.
    frame: rhi::backend::Texture,
    frame_group: rhi::backend::BindGroup,
    targets: TargetSet,
}

impl FrameTargets {
    fn new(device: &rhi::backend::Device, samplers: &Samplers, extent: rhi::Extent) -> Self {
        let frame = rhi::backend::Texture::new(
            device,
            &rhi::TextureDesc::new(
                "post_frame",
                extent.width.max(1),
                extent.height.max(1),
                rhi::TextureFormat::Rgba8Unorm,
                rhi::TextureUsages::COPY_DST | rhi::TextureUsages::TEXTURE_BINDING,
            ),
        );
        Self {
            extent,
            frame_group: samplers.group(device, frame.handle(), samplers.nearest),
            frame,
            targets: TargetSet::new(device, samplers, extent, &FRAME_TARGETS),
        }
    }

    fn destroy(&mut self, device: &rhi::backend::Device) {
        std::mem::take(&mut self.frame).destroy(device);
        self.frame_group.destroy(device);
        self.targets.destroy(device);
    }
}

/// [`SPIDER_TARGETS`] and spiderclip's (`In`, `Blur`) groups, made while the
/// spider effect runs.
struct SpiderTargets {
    targets: TargetSet,
    /// By [`CLIP_INPUTS`].
    clip: [rhi::backend::BindGroup; 3],
}

impl SpiderTargets {
    fn destroy(&mut self, device: &rhi::backend::Device) {
        self.targets.destroy(device);
        for group in self.clip {
            group.destroy(device);
        }
    }
}

pub struct PostEffects {
    effect: Option<PostEffect>,
    samplers: Samplers,
    frame: FrameTargets,
    spider: Option<SpiderTargets>,
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
        extent: rhi::Extent,
    ) -> Self {
        let samplers = Samplers {
            layout: rhi::backend::create_bind_group_layout(device, util::FRAGMENT_TEXTURE),
            nearest: rhi::backend::create_sampler(device, &rhi::SamplerDesc::NEAREST),
            linear: rhi::backend::create_sampler(device, &rhi::SamplerDesc::LINEAR),
        };
        let clip_layout = rhi::backend::create_bind_group_layout(
            device,
            &[util::FRAGMENT_TEXTURE, util::FRAGMENT_TEXTURE].concat(),
        );
        let pipeline_layout = rhi::backend::create_pipeline_layout(
            device,
            &[samplers.layout],
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
            frame: FrameTargets::new(device, &samplers, extent),
            spider: None,
            samplers,
            decode: pipeline("post decode", shader::include_shader!("post_flip.frag")),
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
            clip_layout,
            pipeline_layout,
            clip_pipeline_layout,
        }
    }

    pub fn effect(&self) -> Option<PostEffect> {
        self.effect
    }

    /// Sets the effect to run, freeing the spider's targets once it stops.
    pub fn set(&mut self, device: &rhi::backend::Device, effect: Option<PostEffect>) {
        self.effect = effect;
        if effect != Some(PostEffect::Spider)
            && let Some(mut spider) = self.spider.take()
        {
            device.wait_idle();
            spider.destroy(device);
        }
    }

    /// Remakes the targets for a new frame size.
    pub fn resize(&mut self, device: &rhi::backend::Device, extent: rhi::Extent) {
        if extent == self.frame.extent {
            return;
        }
        device.wait_idle();
        if let Some(mut spider) = self.spider.take() {
            spider.destroy(device);
        }
        let mut stale = std::mem::replace(
            &mut self.frame,
            FrameTargets::new(device, &self.samplers, extent),
        );
        stale.destroy(device);
    }

    /// `main`, gamma-space in bottom-up rows, as the last blur left it.
    pub fn blurred_texture(&self) -> rhi::backend::TextureHandle {
        self.handle(Target::Main)
    }

    pub fn blurred_sampler(&self) -> rhi::backend::Sampler {
        self.samplers.linear
    }

    fn locate(&self, target: Target) -> (&TargetSet, usize) {
        locate(
            &self.frame.targets,
            self.spider.as_ref().map(|spider| &spider.targets),
            target,
        )
    }

    fn handle(&self, target: Target) -> rhi::backend::TextureHandle {
        let (set, i) = self.locate(target);
        set.textures[i].handle()
    }

    /// `target`'s group with the nearest or bilinear sampler.
    fn group(&self, target: Target, linear: bool) -> rhi::backend::BindGroup {
        let (set, i) = self.locate(target);
        if linear {
            set.linear[i]
        } else {
            set.nearest[i]
        }
    }

    fn make_spider_targets(&self, device: &rhi::backend::Device) -> SpiderTargets {
        let targets = TargetSet::new(device, &self.samplers, self.frame.extent, &SPIDER_TARGETS);
        let handle = |target| {
            let (set, i) = locate(&self.frame.targets, Some(&targets), target);
            set.textures[i].handle()
        };
        let clip = CLIP_INPUTS.map(|(input, blur)| {
            let [texture, sampler] = util::texture_binding(handle(input), self.samplers.nearest);
            let [blur_texture, blur_sampler] =
                util::texture_binding(handle(blur), self.samplers.nearest);
            device.create_bind_group(
                self.clip_layout,
                &[texture, sampler, (2, blur_texture.1), (3, blur_sampler.1)],
            )
        });
        SpiderTargets { targets, clip }
    }

    /// One full-screen pass of `pipeline` reading `inputs` into `target`.
    fn pass(
        &self,
        device: &rhi::backend::Device,
        encoder: &rhi::backend::CommandEncoder,
        target: Target,
        pipeline: &rhi::backend::RenderPipeline,
        inputs: rhi::backend::BindGroup,
        uniforms: &[f32],
    ) {
        post::run(
            device,
            encoder,
            self.handle(target),
            self.frame.extent,
            pipeline,
            inputs,
            uniforms,
        );
    }

    /// Copies `frame` (the swapchain image, between passes) into `main`.
    fn capture(
        &self,
        device: &rhi::backend::Device,
        encoder: &rhi::backend::CommandEncoder,
        frame: rhi::backend::TextureHandle,
    ) {
        let size = [self.frame.extent.width, self.frame.extent.height];
        util::copy_frame(encoder, frame, size, self.frame.frame.handle(), size);
        self.pass(
            device,
            encoder,
            Target::Main,
            &self.decode,
            self.frame.frame_group,
            &[],
        );
    }

    /// Runs the effect's chain over `frame`; [`Self::finish`] writes it back.
    pub fn apply(
        &mut self,
        device: &rhi::backend::Device,
        encoder: &rhi::backend::CommandEncoder,
        frame: rhi::backend::TextureHandle,
    ) {
        let Some(effect) = self.effect else {
            return;
        };
        if effect == PostEffect::Spider && self.spider.is_none() {
            self.spider = Some(self.make_spider_targets(device));
        }
        self.capture(device, encoder, frame);

        let pass = |target, pipeline, inputs, uniforms: &[f32]| {
            self.pass(device, encoder, target, pipeline, inputs, uniforms);
        };
        let nearest = |target| self.group(target, false);
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
                    self.box_blur(device, encoder, Target::Main, Target::Temp, blurred, radius);
                }
                let clip = &self.spider.as_ref().expect("made above").clip;
                for (inputs, output, uniforms) in SPIDER_CLIPS {
                    pass(output, &self.spiderclip, clip[inputs], &uniforms);
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

    /// Vanilla `processBlurEffect` (`post_effect/blur.json`): three rounds of
    /// box blur at `radius` over `frame`, left in `main`. A radius of 0 only
    /// captures the frame.
    pub fn blur(
        &self,
        device: &rhi::backend::Device,
        encoder: &rhi::backend::CommandEncoder,
        frame: rhi::backend::TextureHandle,
        radius: u8,
    ) {
        self.capture(device, encoder, frame);
        if radius == 0 {
            return;
        }
        for _ in 0..3 {
            self.box_blur(
                device,
                encoder,
                Target::Main,
                Target::Swap,
                Target::Main,
                f32::from(radius),
            );
        }
    }

    /// A bilinear `box_blur` of `radius` from `from`, horizontally into
    /// `through` and then vertically into `to`.
    fn box_blur(
        &self,
        device: &rhi::backend::Device,
        encoder: &rhi::backend::CommandEncoder,
        from: Target,
        through: Target,
        to: Target,
        radius: f32,
    ) {
        for (input, output, direction) in [(from, through, [1.0, 0.0]), (through, to, [0.0, 1.0])] {
            self.pass(
                device,
                encoder,
                output,
                &self.box_blur,
                self.group(input, true),
                &[direction[0], direction[1], radius],
            );
        }
    }

    /// Writes `main` back over the frame, in the reopened frame pass.
    pub fn finish(&self, pass: &mut rhi::backend::RenderPassEncoder) {
        post::draw(pass, &self.encode, self.group(Target::Main, false), &[]);
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
        if let Some(mut spider) = self.spider.take() {
            spider.destroy(device);
        }
        self.frame.destroy(device);
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
        self.samplers.layout.destroy(device);
        self.clip_layout.destroy(device);
        self.samplers.nearest.destroy(device);
        self.samplers.linear.destroy(device);
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
        shader::include_shader!("post_flip.frag"),
        None,
    )
}
