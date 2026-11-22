//! Entity geometry built per frame rather than from a model, as vanilla's
//! `submitCustomGeometry` callers emit it: paintings (`PaintingRenderer`,
//! `entitySolidZOffsetForward` on the paintings atlas) and experience orbs
//! (`ExperienceOrbRenderer`, `entityTranslucent`). Each vertex carries its
//! own light, since a painting samples one per block it covers.

use std::collections::HashMap;

use crate::renderer::camera::CameraUniform;
use crate::renderer::pipelines::lightmap::Lightmap;
use crate::renderer::{rhi, shader, util};
use crate::resources::{ResourceManager, resource_id};

/// The textures the geometry samples. Paintings key theirs by sprite id
/// (`minecraft:kebab`), as `AtlasIds.PAINTINGS` does.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum CustomTexture {
    Painting(String),
    /// `PaintingRenderer.BACK_SPRITE_LOCATION`.
    PaintingBack,
    /// `ExperienceOrbRenderer.EXPERIENCE_ORB_LOCATION`.
    ExperienceOrb,
}

#[repr(C)]
#[derive(Copy, Clone, Debug, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct CustomVertex {
    /// Anchor-relative, like the entity pipelines' model space.
    pub position: [f32; 3],
    pub uv: [f32; 2],
    pub color: [u8; 4],
    pub normal: [i8; 4],
    /// The light lane (`LightLevels::lane`).
    pub light: f32,
}

impl CustomVertex {
    const BUFFER: rhi::VertexBufferLayout<'static> =
        rhi::vertex_buffer!(Self, [Float32x3, Float32x2, Unorm8x4, Snorm8x4, Float32]);
}

/// One quad, its corners in vanilla's emission order.
#[derive(Clone, Debug, PartialEq)]
pub struct CustomQuad {
    pub texture: CustomTexture,
    pub translucent: bool,
    pub corners: [CustomVertex; 4],
}

struct LoadedTexture {
    texture: rhi::backend::Texture,
    set: rhi::backend::BindGroup,
}

struct Run {
    texture: rhi::backend::BindGroup,
    translucent: bool,
    vertices: std::ops::Range<u32>,
}

pub struct CustomGeometryPipeline {
    solid: rhi::backend::RenderPipeline,
    translucent: rhi::backend::RenderPipeline,
    pipeline_layout: rhi::backend::PipelineLayout,
    camera_layout: rhi::backend::BindGroupLayout,
    tex_layout: rhi::backend::BindGroupLayout,
    camera_sets: Vec<rhi::backend::BindGroup>,
    camera_buffers: Vec<rhi::backend::Buffer>,
    lightmap_set: rhi::backend::BindGroup,
    sampler: rhi::backend::Sampler,
    textures: HashMap<CustomTexture, LoadedTexture>,
    vertices: super::item_entity::FrameArena<CustomVertex>,
    runs: Vec<Run>,
}

impl CustomGeometryPipeline {
    pub fn new(
        device: &rhi::backend::Device,
        targets: rhi::backend::PipelineTargets,
        resources: &dyn ResourceManager,
        lightmap: &Lightmap,
    ) -> Self {
        let camera_layout = rhi::backend::create_bind_group_layout(device, util::VERTEX_UNIFORM);
        let tex_layout = rhi::backend::create_bind_group_layout(device, util::FRAGMENT_TEXTURE);
        let pipeline_layout = rhi::backend::create_pipeline_layout(
            device,
            &[camera_layout, tex_layout, lightmap.layout],
            None,
        );
        let (camera_sets, camera_buffers) = util::create_uniform_sets(
            device,
            camera_layout,
            std::mem::size_of::<CameraUniform>() as u64,
            "custom_geometry_camera",
        );
        let vertices = super::item_entity::FrameArena::new(
            device,
            rhi::BufferUsages::VERTEX,
            "custom_geometry_vertices".into(),
        );
        let sampler = rhi::backend::create_sampler(device, &rhi::SamplerDesc::NEAREST);
        let mut pipeline = Self {
            solid: create_pipeline(device, targets, pipeline_layout, false),
            translucent: create_pipeline(device, targets, pipeline_layout, true),
            pipeline_layout,
            camera_layout,
            tex_layout,
            camera_sets,
            camera_buffers,
            lightmap_set: lightmap.group,
            sampler,
            textures: HashMap::new(),
            vertices,
            runs: Vec::new(),
        };
        pipeline.reload_textures(device, resources);
        pipeline
    }

    /// The paintings atlas's sources (`atlases/paintings.json`: the
    /// `painting` directory) and the orb texture.
    pub fn reload_textures(
        &mut self,
        device: &rhi::backend::Device,
        resources: &dyn ResourceManager,
    ) {
        self.destroy_textures(device);
        // The orb's texture moved into `experience/` after 1.21.11.
        let mut sources = vec![(
            CustomTexture::ExperienceOrb,
            vec![
                resource_id!("minecraft:textures/entity/experience/experience_orb.png"),
                resource_id!("minecraft:textures/entity/experience_orb.png"),
            ],
        )];
        match resources.list_resources("textures/painting", &|id| id.path().ends_with(".png")) {
            Ok(entries) => sources.extend(entries.into_keys().filter_map(|id| {
                let name = id
                    .path()
                    .strip_prefix("textures/painting/")?
                    .strip_suffix(".png")?;
                let texture = if id.namespace() == "minecraft" && name == "back" {
                    CustomTexture::PaintingBack
                } else {
                    CustomTexture::Painting(format!("{}:{name}", id.namespace()))
                };
                Some((texture, vec![id]))
            })),
            Err(error) => tracing::warn!("Failed to list painting textures: {error}"),
        }
        for (key, ids) in sources {
            let texture = super::entity_renderer::load_entity_texture(device, resources, &ids, 16);
            let set =
                util::create_texture_group(device, self.tex_layout, texture.handle(), self.sampler);
            self.textures.insert(key, LoadedTexture { texture, set });
        }
    }

    fn destroy_textures(&mut self, device: &rhi::backend::Device) {
        for (_, loaded) in self.textures.drain() {
            loaded.set.destroy(device);
            loaded.texture.destroy(device);
        }
    }

    pub fn has_texture(&self, texture: &CustomTexture) -> bool {
        self.textures.contains_key(texture)
    }

    pub fn update_camera(&mut self, frame: usize, uniform: &CameraUniform) {
        self.camera_buffers[frame].write(bytemuck::bytes_of(uniform));
    }

    /// Uploads `quads`: the solid ones first, then the translucent ones far
    /// to near from the camera, as `sortOnUpload` orders them.
    pub fn prepare(
        &mut self,
        device: &rhi::backend::Device,
        frame: usize,
        camera: glam::Vec3,
        quads: &[CustomQuad],
    ) {
        self.runs.clear();
        let center = |quad: &CustomQuad| {
            quad.corners
                .iter()
                .map(|v| glam::Vec3::from(v.position))
                .sum::<glam::Vec3>()
                / 4.0
        };
        let mut order: Vec<&CustomQuad> = quads
            .iter()
            .filter(|quad| self.textures.contains_key(&quad.texture))
            .collect();
        order.sort_by(|a, b| {
            a.translucent.cmp(&b.translucent).then_with(|| {
                if a.translucent {
                    let distance = |q| center(q).distance_squared(camera);
                    distance(b).total_cmp(&distance(a))
                } else {
                    a.texture.cmp(&b.texture)
                }
            })
        });
        let mut vertices = Vec::with_capacity(order.len() * 6);
        for quad in order {
            let start = vertices.len() as u32;
            for i in [0, 1, 2, 0, 2, 3] {
                vertices.push(quad.corners[i]);
            }
            let texture = self.textures[&quad.texture].set;
            match self.runs.last_mut() {
                Some(run) if run.texture == texture && run.translucent == quad.translucent => {
                    run.vertices.end = vertices.len() as u32;
                }
                _ => self.runs.push(Run {
                    texture,
                    translucent: quad.translucent,
                    vertices: start..vertices.len() as u32,
                }),
            }
        }
        self.vertices.begin_frame(device, frame, vertices.len());
        self.vertices.push(frame, &vertices);
    }

    pub fn draw(&self, pass: &mut rhi::backend::RenderPassEncoder, frame: usize) {
        if self.runs.is_empty() {
            return;
        }
        pass.set_vertex_buffer(0, self.vertices.buffer(frame).handle());
        let mut bound = None;
        for run in &self.runs {
            if bound != Some(run.translucent) {
                pass.set_pipeline(if run.translucent {
                    &self.translucent
                } else {
                    &self.solid
                });
                // Bound after the pipeline, whose layout they're checked
                // against.
                pass.set_bind_group(0, self.camera_sets[frame]);
                pass.set_bind_group(2, self.lightmap_set);
                bound = Some(run.translucent);
            }
            pass.set_bind_group(1, run.texture);
            pass.draw(
                run.vertices.end - run.vertices.start,
                1,
                run.vertices.start,
                0,
            );
        }
    }

    pub fn recreate_pipeline(
        &mut self,
        device: &rhi::backend::Device,
        targets: rhi::backend::PipelineTargets,
    ) {
        self.solid.destroy(device);
        self.translucent.destroy(device);
        self.solid = create_pipeline(device, targets, self.pipeline_layout, false);
        self.translucent = create_pipeline(device, targets, self.pipeline_layout, true);
    }

    pub fn destroy(&mut self, device: &rhi::backend::Device) {
        self.destroy_textures(device);
        util::destroy_buffers(device, &mut self.camera_buffers);
        self.vertices.destroy(device);
        self.sampler.destroy(device);
        self.solid.destroy(device);
        self.translucent.destroy(device);
        self.pipeline_layout.destroy(device);
        util::destroy_bind_groups(device, self.camera_sets.drain(..));
        self.camera_layout.destroy(device);
        self.tex_layout.destroy(device);
    }
}

fn create_pipeline(
    device: &rhi::backend::Device,
    targets: rhi::backend::PipelineTargets,
    layout: rhi::backend::PipelineLayout,
    translucent: bool,
) -> rhi::backend::RenderPipeline {
    rhi::backend::create_render_pipeline(
        device,
        targets,
        layout,
        &rhi::RenderPipelineDesc {
            depth: Some(rhi::DepthState {
                // `VIEW_OFFSET_Z_LAYERING_FORWARD`: a painting's front sits
                // over the wall behind it.
                bias: (!translucent).then_some(rhi::DepthBias {
                    constant: -1,
                    slope: -1.0,
                }),
                ..rhi::DepthState::WRITE
            }),
            blend: translucent.then_some(rhi::BlendState::TRANSLUCENT),
            // Paintings cull their back faces like `ENTITY_SOLID`; an orb's
            // quad faces the camera.
            cull_mode: if translucent {
                rhi::CullMode::None
            } else {
                rhi::CullMode::Back
            },
            ..rhi::RenderPipelineDesc::new(
                "custom_geometry",
                shader::include_shader!("custom_geometry.vert"),
                shader::include_shader!("custom_geometry.frag"),
                &[CustomVertex::BUFFER],
            )
        },
    )
}
