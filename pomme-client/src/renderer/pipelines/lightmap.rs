//! Vanilla `Lightmap`: a 16×16 texture of block light (x) by sky light (y),
//! redrawn by `core/lightmap.fsh` after a tick. Terrain samples it per
//! vertex (`sample_lightmap` in `lightmap.glsl`).

use crate::renderer::pipelines::post;
use crate::renderer::{rhi, shader};

const EXTENT: rhi::Extent = rhi::Extent {
    width: 16,
    height: 16,
};

/// `lightmap.frag`'s push constants: vanilla's `LightmapInfo`, each colour's
/// fourth lane carrying one of its floats.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct LightmapInfo {
    /// `BlockLightTint`, then `BlockFactor`.
    pub block_light_tint: [f32; 4],
    /// `SkyLightColor`, then `SkyFactor`.
    pub sky_light_color: [f32; 4],
    /// `AmbientColor`, then `BrightnessFactor`.
    pub ambient_color: [f32; 4],
    /// `NightVisionColor`, then `NightVisionFactor`.
    pub night_vision_color: [f32; 4],
    pub darkness_scale: f32,
    pub boss_overlay_world_darkening_factor: f32,
    pub _pad: [f32; 2],
}

/// The lightmap's texture and sampler, for the stages that sample it.
pub const SAMPLED_LAYOUT: &[rhi::BindGroupLayoutEntry] = &[
    rhi::BindGroupLayoutEntry::texture(rhi::ShaderStages::VERTEX),
    rhi::BindGroupLayoutEntry::sampler(rhi::ShaderStages::VERTEX),
];

pub struct Lightmap {
    texture: rhi::backend::Texture,
    sampler: rhi::backend::Sampler,
    pipeline_layout: rhi::backend::PipelineLayout,
    pipeline: rhi::backend::RenderPipeline,
    /// [`SAMPLED_LAYOUT`], and a group of it pointed at the texture.
    pub layout: rhi::backend::BindGroupLayout,
    pub group: rhi::backend::BindGroup,
    info: LightmapInfo,
    /// A tick ran since the last redraw; vanilla's `needsUpdate`.
    needs_update: bool,
}

impl Lightmap {
    /// The texture and its sampler, for binding elsewhere (shader packs).
    pub fn texture(&self) -> (rhi::backend::TextureHandle, rhi::backend::Sampler) {
        (self.texture.handle(), self.sampler)
    }

    pub fn new(device: &rhi::backend::Device) -> Self {
        let texture = post::create_target(device, EXTENT, "lightmap");
        // Vanilla `getClampToEdge(FilterMode.LINEAR)`.
        let sampler = rhi::backend::create_sampler(device, &rhi::SamplerDesc::LINEAR);
        let pipeline_layout = rhi::backend::create_pipeline_layout(
            device,
            &[],
            Some(rhi::PushConstants {
                visibility: rhi::ShaderStages::FRAGMENT,
                size: size_of::<LightmapInfo>() as u32,
            }),
        );
        let pipeline = post::create_pipeline(
            device,
            post::targets(),
            pipeline_layout,
            "lightmap",
            None,
            shader::include_shader!("lightmap.frag"),
            None,
        );
        let layout = rhi::backend::create_bind_group_layout(device, SAMPLED_LAYOUT);
        let group = device.create_bind_group(
            layout,
            &crate::renderer::util::texture_binding(texture.handle(), sampler),
        );
        Self {
            texture,
            sampler,
            pipeline_layout,
            pipeline,
            layout,
            group,
            // White, as vanilla clears it before the first tick.
            info: LightmapInfo {
                ambient_color: [1.0, 1.0, 1.0, 0.0],
                ..bytemuck::Zeroable::zeroed()
            },
            needs_update: true,
        }
    }

    /// The Brightness option, as the last tick passed it.
    pub fn brightness(&self) -> f32 {
        self.info.ambient_color[3]
    }

    /// Vanilla `LightmapRenderStateExtractor.extract` after a tick: the next
    /// frame redraws with `info`.
    pub fn tick(&mut self, info: LightmapInfo) {
        self.info = info;
        self.needs_update = true;
    }

    /// Redraws the lightmap if a tick ran since the last redraw.
    pub fn record(
        &mut self,
        device: &rhi::backend::Device,
        encoder: &rhi::backend::CommandEncoder,
    ) {
        if !std::mem::take(&mut self.needs_update) {
            return;
        }
        let target = self.texture.handle();
        let mut pass = post::open_pass(device, encoder, target, EXTENT);
        pass.set_pipeline(&self.pipeline);
        pass.set_push_constants(
            rhi::ShaderStages::FRAGMENT,
            0,
            bytemuck::bytes_of(&self.info),
        );
        pass.draw(3, 1, 0, 0);
        post::close_pass(encoder, pass, target);
    }

    pub fn destroy(&mut self, device: &rhi::backend::Device) {
        std::mem::take(&mut self.texture).destroy(device);
        self.group.destroy(device);
        self.layout.destroy(device);
        self.pipeline.destroy(device);
        self.pipeline_layout.destroy(device);
        self.sampler.destroy(device);
    }
}
