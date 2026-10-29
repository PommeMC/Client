//! A loaded shader pack's render path, Iris's pipeline in milestone 1: the
//! `shadow` program draws terrain into the shadow maps from the sun or moon,
//! the `gbuffers_*` programs draw the sky, terrain, entities and block
//! entities into the pack's `colortex` targets, `deferred*` and `composite*`
//! passes run over them, and `final` writes the swapchain.

use std::collections::BTreeMap;
use std::sync::Arc;

use glam::{DVec3, IVec2, IVec3, Mat4, Vec3, Vec4};

use crate::renderer::chunk::atlas::TextureAtlas;
use crate::renderer::chunk::buffer::CHUNK_VERTEX_BUFFERS;
use crate::renderer::entity_model::EntityVertex;
use crate::renderer::pipelines::entity_renderer::PackShadow;
use crate::renderer::pipelines::lightmap::Lightmap;
use crate::renderer::pipelines::sky::{
    SKY_PUSH_SIZE, SkyDraw, SkyPipeline, SkyVertex, celestial_pose,
};
use crate::renderer::rhi::{self, TextureTransition, TextureUse};
use crate::renderer::{MAX_FRAMES_IN_FLIGHT, util};
use crate::shaderpack::directives::{
    BufferSettings, ShadowSettings, TargetSettings, buffer_index, shadow_buffer_index,
};
use crate::shaderpack::expr::Value;
use crate::shaderpack::glsl::translate::{SamplerKind, UniformType};
use crate::shaderpack::prelude::Geometry;
use crate::shaderpack::programs::{Gbuffers, full_screen_names};
use crate::shaderpack::uniforms::CustomUniforms;
use crate::shaderpack::{self, CompiledProgram, PackError, ProgramText, ShaderPack};

/// Buffers a pack can name, `colortex0` to `colortex31`.
const BUFFERS: usize = 32;

/// What the geometry passes draw, in order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum PackGeometry {
    /// The `shadow` program over solid and cutout terrain.
    ShadowTerrain,
    /// `shadow_entities`, or `shadow`, over entities.
    ShadowEntities,
    /// `shadow_block`, or `shadow`, over block entities.
    ShadowBlockEntities,
    /// The `shadow` program over translucent terrain.
    ShadowWater,
    SkyBasic,
    SkyTextured,
    Terrain,
    Entities,
    BlockEntities,
    Water,
}

impl PackGeometry {
    /// The geometry it draws: itself, or the one a shadow geometry casts.
    fn drawn(self) -> Self {
        match self {
            Self::ShadowTerrain => Self::Terrain,
            Self::ShadowEntities => Self::Entities,
            Self::ShadowBlockEntities => Self::BlockEntities,
            Self::ShadowWater => Self::Water,
            other => other,
        }
    }

    /// Whether it draws into the shadow maps.
    pub fn shadow(self) -> bool {
        self != self.drawn()
    }

    /// Whether it draws chunk sections, which read the sprite rectangles.
    fn terrain(self) -> bool {
        matches!(self.drawn(), Self::Terrain | Self::Water)
    }

    /// Whether it draws `draw_water`'s translucent sections.
    fn water(self) -> bool {
        self.drawn() == Self::Water
    }

    /// Its `gbuffers_*` program, if it isn't a shadow one.
    fn gbuffers(self) -> Option<Gbuffers> {
        Some(match self {
            Self::ShadowTerrain
            | Self::ShadowEntities
            | Self::ShadowBlockEntities
            | Self::ShadowWater => return None,
            Self::SkyBasic => Gbuffers::SkyBasic,
            Self::SkyTextured => Gbuffers::SkyTextured,
            Self::Terrain => Gbuffers::Terrain,
            Self::Entities => Gbuffers::Entities,
            Self::BlockEntities => Gbuffers::Block,
            Self::Water => Gbuffers::Water,
        })
    }

    /// Its program's source in `pack`'s `dimension`, `None` if it has none.
    fn source(self, pack: &ShaderPack, dimension: &str) -> Result<Option<ProgramText>, PackError> {
        if let Some(program) = self.gbuffers() {
            return pack.gbuffers(dimension, program);
        }
        let own = match self {
            Self::ShadowEntities => pack.program(dimension, "shadow_entities")?,
            Self::ShadowBlockEntities => pack.program(dimension, "shadow_block")?,
            _ => None,
        };
        match own {
            Some(text) => Ok(Some(text)),
            None => pack.program(dimension, "shadow"),
        }
    }

    /// The vertices it draws.
    fn vertices(self) -> Geometry {
        let program = self.drawn().gbuffers();
        shaderpack::gbuffers_geometry(program.expect("drawn geometry has a gbuffers program"))
    }
}

/// Which entities a pack's shadow pass draws: `shaders.properties`'s
/// `shadowEntities`, `shadowPlayer` and `shadowBlockEntities`, with Iris's
/// defaults.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ShadowCasters {
    entities: bool,
    player: bool,
    block_entities: bool,
}

impl ShadowCasters {
    fn read(pack: &ShaderPack) -> Self {
        Self {
            entities: pack.property_bool("shadowEntities", true),
            player: pack.property_bool("shadowPlayer", false),
            // TODO: `shadowLightBlockEntities`, which keeps the light-emitting
            // ones with this off.
            block_entities: pack.property_bool("shadowBlockEntities", true),
        }
    }

    /// Whether an entity that is `role` to the pass casts a shadow.
    pub fn casts(self, role: PackShadow) -> bool {
        match role {
            PackShadow::Entity => self.entities,
            PackShadow::Player | PackShadow::FirstPersonPlayer => self.entities || self.player,
            PackShadow::Spectator => false,
        }
    }

    /// Whether the pass draws `geometry` at all.
    fn draws(self, geometry: PackGeometry) -> bool {
        match geometry {
            PackGeometry::ShadowEntities => self.casts(PackShadow::Player),
            PackGeometry::ShadowBlockEntities => self.block_entities,
            _ => true,
        }
    }
}

/// A geometry program's pipeline and the groups it binds at sets 0-2; the
/// caller binds the albedo texture at set 3.
#[derive(Clone, Copy)]
pub struct PackBinding {
    pub pipeline: rhi::backend::RenderPipeline,
    pub sets: [rhi::backend::BindGroup; 3],
}

/// The world state a frame's uniforms are read from.
pub struct PackFrame {
    /// `gbufferModelView`: rotation and view bob, eye-relative.
    pub view: Mat4,
    /// `gbufferProjection`, in OpenGL's clip conventions.
    pub projection: Mat4,
    /// The eye, in world space.
    pub camera: DVec3,
    /// The eye relative to the render anchor, and the anchor.
    pub eye: Vec3,
    pub anchor: IVec3,
    pub day_time: u64,
    pub sun_angle_degrees: f32,
    pub moon_phase: u8,
    pub rain: f32,
    pub fog_color: Vec3,
    pub sky_color: Vec3,
    pub fog: [f32; 2],
    pub near: f32,
    pub far: f32,
    /// `eyeBrightness`: the eye's block and sky light, 0..240.
    pub eye_light: [f32; 2],
    pub eye_in_water: bool,
    pub screen_brightness: f32,
    /// The block atlas's width and height, in texels.
    pub atlas_size: u32,
    pub world: PackWorld,
}

/// The player's surroundings as Iris reports them, from the game state.
#[derive(Clone, Default)]
pub struct PackWorld {
    /// The biome's registry id at the feet.
    pub biome: i32,
    /// Every biome's name, by registry id, for the `BIOME_*` constants.
    pub biome_names: Arc<[String]>,
    /// `PPT_NONE`, `PPT_RAIN` or `PPT_SNOW` at the feet.
    pub precipitation: i32,
    pub temperature: f32,
    pub rainfall: f32,
    pub blindness: f32,
}

/// `PommeFrame` in the preludes.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct FrameUniform {
    view: [f32; 16],
    projection: [f32; 16],
    eye: [f32; 4],
    anchor: [i32; 4],
    atlas_size: [f32; 4],
    fog: [f32; 4],
    fog_color: [f32; 4],
}

/// One compiled program on the GPU.
struct Program {
    pipeline: rhi::backend::RenderPipeline,
    /// The sky programs' additive variant, for the sun, moon and stars.
    overlay: Option<rhi::backend::RenderPipeline>,
    layout: rhi::backend::PipelineLayout,
    uniform_layout: rhi::backend::BindGroupLayout,
    sampler_layout: rhi::backend::BindGroupLayout,
    uniform_buffers: Vec<rhi::backend::Buffer>,
    uniform_sets: Vec<rhi::backend::BindGroup>,
    sampler_sets: Vec<rhi::backend::BindGroup>,
    compiled: CompiledProgram,
    /// The colortex indices its outputs write, in output order.
    draw_buffers: Vec<u8>,
}

/// The shadow pass's targets, when the pack has a `shadow` program.
struct ShadowTargets {
    extent: rhi::Extent,
    /// `shadowtex0`: everything the shadow pass drew.
    depth: rhi::backend::Texture,
    /// `shadowtex1`: `shadowtex0` copied before translucents.
    depth_opaque: rhi::backend::Texture,
    /// The `shadowcolor` buffers a program uses.
    colors: BTreeMap<u8, ShadowColor>,
    /// `PommeFrame` seen from the sun or moon.
    frame_buffers: Vec<rhi::backend::Buffer>,
}

struct ShadowColor {
    texture: rhi::backend::Texture,
    format: rhi::TextureFormat,
    clear: Clear,
}

/// A colortex buffer: two textures, flipped between when a full-screen pass
/// writes what it reads.
struct Buffer {
    textures: [rhi::backend::Texture; 2],
    format: rhi::TextureFormat,
    /// Which of `textures` holds the latest contents.
    current: usize,
    clear: Clear,
}

/// What a buffer clears to each frame.
#[derive(Clone, Copy)]
enum Clear {
    Keep,
    /// Iris's default for `colortex0`.
    FogColor,
    Color([f32; 4]),
}

pub struct PackRenderer {
    dimension: String,
    extent: rhi::Extent,
    swapchain_targets: rhi::backend::PipelineTargets,
    buffers: BTreeMap<u8, Buffer>,
    /// `depthtex0`: everything drawn.
    depth: rhi::backend::Texture,
    /// `depthtex1`: `depthtex0` copied before translucents.
    depth_opaque: rhi::backend::Texture,
    noise: rhi::backend::Texture,
    /// 1×1 stand-ins: white color, and depth at the far plane for shadow maps.
    white: rhi::backend::Texture,
    far_depth: rhi::backend::Texture,
    nearest: rhi::backend::Sampler,
    linear: rhi::backend::Sampler,
    /// `linear` and `nearest` comparing, for `sampler2DShadow`s.
    shadow: rhi::backend::Sampler,
    shadow_nearest: rhi::backend::Sampler,
    frame_buffers: Vec<rhi::backend::Buffer>,
    empty_layout: rhi::backend::BindGroupLayout,
    empty_group: rhi::backend::BindGroup,
    sprite_layout: rhi::backend::BindGroupLayout,
    sprite_group: rhi::backend::BindGroup,
    albedo_layout: rhi::backend::BindGroupLayout,
    /// The atlas as a set-3 albedo group, for terrain.
    pub atlas_group: rhi::backend::BindGroup,
    geometry: BTreeMap<PackGeometry, Program>,
    deferred: Vec<Program>,
    composite: Vec<Program>,
    final_pass: Option<Program>,
    lightmap: (rhi::backend::TextureHandle, rhi::backend::Sampler),
    shadow_settings: ShadowSettings,
    pub shadow_casters: ShadowCasters,
    shadow_targets: Option<ShadowTargets>,
    /// The shadow view's projection times model-view, from the last
    /// [`Self::prepare`].
    shadow_view_projection: Mat4,
    previous: Option<Previous>,
    frame_counter: u32,
    started: std::time::Instant,
    last_prepare: Option<std::time::Instant>,
    custom: CustomUniforms,
    /// `eyeBrightnessSmooth`, eased toward `eyeBrightness`.
    eye_brightness_smooth: Option<[f32; 2]>,
}

/// Which program [`PackRenderer::bind_samplers`] works on.
#[derive(Clone, Copy)]
enum ProgramRef {
    Geometry(PackGeometry),
    Deferred(usize),
    Composite(usize),
    Final,
}

/// Iris's `colortexNFormat` names onto the RHI's formats.
fn buffer_format(name: Option<&str>) -> rhi::TextureFormat {
    use rhi::TextureFormat as F;
    // Signed-normalised formats aren't always renderable in Vulkan; a float
    // format of the same channels keeps their sign.
    // TODO: integer formats, which also need integer samplers.
    match name.unwrap_or("RGBA8") {
        "R8" => F::R8Unorm,
        "RG8" => F::Rg8Unorm,
        "RGBA16" | "RGB16" | "RGBA12" | "RGB12" | "RG16" | "R16" => F::Rgba16Unorm,
        "RGB10_A2" => F::Rgb10a2Unorm,
        "R16F" | "R8_SNORM" | "R16_SNORM" => F::R16Float,
        "RG16F" | "RG8_SNORM" | "RG16_SNORM" => F::Rg16Float,
        "RGBA16F" | "RGB16F" | "RGBA8_SNORM" | "RGB8_SNORM" | "RGBA16_SNORM" | "RGB16_SNORM" => {
            F::Rgba16Float
        }
        "R32F" => F::R32Float,
        "RG32F" => F::Rg32Float,
        "RGBA32F" | "RGB32F" => F::Rgba32Float,
        "R11F_G11F_B10F" => F::Rg11b10Float,
        _ => F::Rgba8Unorm,
    }
}

impl PackRenderer {
    /// Compiles `pack`'s milestone-1 programs for `dimension` and builds
    /// their targets; an error disables the pack.
    pub fn new(
        device: &rhi::backend::Device,
        pack: &ShaderPack,
        dimension: &str,
        extent: rhi::Extent,
        swapchain_targets: rhi::backend::PipelineTargets,
        atlas: &TextureAtlas,
        lightmap: &Lightmap,
    ) -> Result<Self, String> {
        let shadow_casters = ShadowCasters::read(pack);
        let gbuffers: Vec<(PackGeometry, CompiledProgram)> = [
            PackGeometry::ShadowTerrain,
            PackGeometry::ShadowEntities,
            PackGeometry::ShadowBlockEntities,
            PackGeometry::ShadowWater,
            PackGeometry::SkyBasic,
            PackGeometry::SkyTextured,
            PackGeometry::Terrain,
            PackGeometry::Entities,
            PackGeometry::BlockEntities,
            PackGeometry::Water,
        ]
        .into_iter()
        .filter(|&g| shadow_casters.draws(g))
        .filter_map(|g| {
            let text = g.source(pack, dimension).transpose()?;
            Some(text.and_then(|t| shaderpack::compile(&t, g.vertices()).map(|c| (g, c))))
        })
        .collect::<Result<_, _>>()
        .map_err(|e| e.to_string())?;
        let screen = |prefix: &str, geometry| -> Result<Vec<CompiledProgram>, String> {
            full_screen_names(prefix)
                .filter_map(|name| pack.program(dimension, &name).transpose())
                .map(|t| t.and_then(|t| shaderpack::compile(&t, geometry)))
                .collect::<Result<_, _>>()
                .map_err(|e| e.to_string())
        };
        let deferred = screen("deferred", Geometry::FullScreen)?;
        let composite = screen("composite", Geometry::FullScreen)?;
        let final_pass = pack
            .program(dimension, "final")
            .map(|t| t.unwrap_or_else(shaderpack::copy_program))
            .and_then(|t| shaderpack::compile(&t, Geometry::Final))
            .map_err(|e| e.to_string())?;

        // Targets: every buffer a program writes or reads, set up by the
        // `const` settings across all of them.
        let mut settings = TargetSettings::default();
        let mut used: Vec<u8> = Vec::new();
        let mut used_shadow: Vec<u8> = Vec::new();
        for (geometry, c) in gbuffers.iter().map(|(g, c)| (Some(*g), c)).chain(
            deferred
                .iter()
                .chain(&composite)
                .chain([&final_pass])
                .map(|c| (None, c)),
        ) {
            settings.read(&c.text);
            if geometry.is_some_and(PackGeometry::shadow) {
                used_shadow.extend(draw_buffers(c));
            } else {
                used.extend(draw_buffers(c));
            }
            for s in &c.interface.samplers {
                used.extend(buffer_index(&s.name));
                used_shadow.extend(shadow_buffer_index(&s.name));
            }
        }
        for list in [&mut used, &mut used_shadow] {
            list.sort_unstable();
            list.dedup();
        }
        let shadow_targets = gbuffers.iter().any(|(g, _)| g.shadow()).then(|| {
            let size = settings.shadow.resolution.clamp(1, device.max_texture_size);
            let extent = rhi::Extent {
                width: size,
                height: size,
            };
            let shadowcolor = &settings.shadowcolor;
            ShadowTargets {
                extent,
                depth: create_depth(device, extent),
                depth_opaque: create_depth(device, extent),
                colors: used_shadow
                    .iter()
                    .map(|&i| {
                        let format = buffer_format(shadowcolor.formats.get(&i).map(String::as_str));
                        let color = ShadowColor {
                            texture: create_color(device, extent, format),
                            format,
                            clear: buffer_clear(shadowcolor, i, Clear::Color([1.0; 4])),
                        };
                        (i, color)
                    })
                    .collect(),
                frame_buffers: create_frame_buffers(device),
            }
        });

        let nearest = rhi::backend::create_sampler(device, &rhi::SamplerDesc::NEAREST);
        let linear = rhi::backend::create_sampler(device, &rhi::SamplerDesc::LINEAR);
        let compare = |desc| {
            rhi::backend::create_sampler(
                device,
                &rhi::SamplerDesc {
                    compare: Some(rhi::CompareFunction::LessEqual),
                    ..desc
                },
            )
        };
        let shadow = compare(rhi::SamplerDesc::LINEAR);
        let shadow_nearest = compare(rhi::SamplerDesc::NEAREST);
        let empty_layout = rhi::backend::create_bind_group_layout(device, &[]);
        let empty_group = device.create_bind_group(empty_layout, &[]);
        let sprite_layout = rhi::backend::create_bind_group_layout(
            device,
            &[rhi::BindGroupLayoutEntry::storage(
                rhi::ShaderStages::VERTEX,
            )],
        );
        let sprite_group = device.create_bind_group(
            sprite_layout,
            &[(
                0,
                rhi::backend::BindingResource::storage(
                    &atlas.sprite_rects,
                    atlas.sprite_rects_bytes(),
                ),
            )],
        );
        let albedo_layout = rhi::backend::create_bind_group_layout(device, util::FRAGMENT_TEXTURE);
        let atlas_group = util::create_texture_group(
            device,
            albedo_layout,
            atlas.texture.handle(),
            atlas.sampler,
        );
        let frame_buffers = create_frame_buffers(device);

        let mut this = Self {
            dimension: dimension.to_owned(),
            extent,
            swapchain_targets,
            buffers: BTreeMap::new(),
            depth: create_depth(device, extent),
            depth_opaque: create_depth(device, extent),
            noise: create_noise(device),
            white: create_white(device),
            far_depth: create_far_depth(device),
            nearest,
            linear,
            shadow,
            shadow_nearest,
            frame_buffers,
            empty_layout,
            empty_group,
            sprite_layout,
            sprite_group,
            albedo_layout,
            atlas_group,
            geometry: BTreeMap::new(),
            deferred: Vec::new(),
            composite: Vec::new(),
            final_pass: None,
            lightmap: lightmap.texture(),
            shadow_settings: settings.shadow.clone(),
            shadow_casters,
            shadow_targets,
            shadow_view_projection: Mat4::IDENTITY,
            previous: None,
            frame_counter: 0,
            started: std::time::Instant::now(),
            last_prepare: None,
            custom: pack.custom_uniforms.clone(),
            eye_brightness_smooth: None,
        };

        for index in used.into_iter().filter(|&i| (i as usize) < BUFFERS) {
            let colortex = &settings.colortex;
            let format = buffer_format(colortex.formats.get(&index).map(String::as_str));
            let clear = buffer_clear(
                colortex,
                index,
                match index {
                    0 => Clear::FogColor,
                    1 => Clear::Color([1.0; 4]),
                    _ => Clear::Color([0.0; 4]),
                },
            );
            this.buffers.insert(
                index,
                Buffer {
                    textures: [0, 1].map(|_| create_color(device, extent, format)),
                    format,
                    current: 0,
                    clear,
                },
            );
        }

        match this.build_programs(device, gbuffers, deferred, composite, final_pass) {
            Ok(()) => Ok(this),
            Err(e) => {
                this.destroy(device);
                Err(e)
            }
        }
    }

    fn build_programs(
        &mut self,
        device: &rhi::backend::Device,
        gbuffers: Vec<(PackGeometry, CompiledProgram)>,
        deferred: Vec<CompiledProgram>,
        composite: Vec<CompiledProgram>,
        final_pass: CompiledProgram,
    ) -> Result<(), String> {
        for (geometry, compiled) in gbuffers {
            let program = self.program(device, compiled, Some(geometry))?;
            self.geometry.insert(geometry, program);
        }
        for compiled in deferred {
            let program = self.program(device, compiled, None)?;
            self.deferred.push(program);
        }
        for compiled in composite {
            let program = self.program(device, compiled, None)?;
            self.composite.push(program);
        }
        self.final_pass =
            Some(self.build(device, final_pass, self.swapchain_targets, None, vec![0])?);
        Ok(())
    }

    pub fn dimension(&self) -> &str {
        &self.dimension
    }

    /// Builds a program's pipeline: a geometry one draws into its buffers and
    /// the depth target, a full-screen one into its buffers alone.
    fn program(
        &self,
        device: &rhi::backend::Device,
        compiled: CompiledProgram,
        geometry: Option<PackGeometry>,
    ) -> Result<Program, String> {
        let shadow = geometry.is_some_and(PackGeometry::shadow);
        let buffers = draw_buffers(&compiled);
        let formats = buffers
            .iter()
            .map(|b| {
                let format = if shadow {
                    self.shadow_targets
                        .as_ref()
                        .and_then(|t| t.colors.get(b))
                        .map(|c| c.format)
                } else {
                    self.buffers.get(b).map(|buffer| buffer.format)
                };
                format.map(rhi::backend::texture_format).ok_or_else(|| {
                    let family = if shadow { "shadowcolor" } else { "colortex" };
                    format!(
                        "{}: writes {family}{b}, which isn't a buffer",
                        compiled.path
                    )
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        let mut targets = match formats.split_first() {
            Some((&first, _)) => rhi::backend::PipelineTargets::new(first, geometry.is_some()),
            // Iris draws a shadow program that writes no buffers into depth alone.
            None if shadow => rhi::backend::PipelineTargets::depth_only(),
            None => return Err(format!("{}: writes no buffers", compiled.path)),
        };
        for (slot, format) in targets.extra_colors.iter_mut().zip(formats.iter().skip(1)) {
            *slot = Some(*format);
        }
        self.build(device, compiled, targets, geometry, buffers)
    }

    fn build(
        &self,
        device: &rhi::backend::Device,
        compiled: CompiledProgram,
        targets: rhi::backend::PipelineTargets,
        geometry: Option<PackGeometry>,
        draw_buffers: Vec<u8>,
    ) -> Result<Program, String> {
        let uniform_layout = rhi::backend::create_bind_group_layout(
            device,
            &[
                rhi::BindGroupLayoutEntry::uniform(rhi::ShaderStages::VERTEX_FRAGMENT),
                rhi::BindGroupLayoutEntry::uniform(rhi::ShaderStages::VERTEX_FRAGMENT),
            ],
        );
        let sampler_entries: Vec<_> = compiled
            .interface
            .samplers
            .iter()
            .flat_map(|_| {
                [
                    rhi::BindGroupLayoutEntry::texture(rhi::ShaderStages::VERTEX_FRAGMENT),
                    rhi::BindGroupLayoutEntry::sampler(rhi::ShaderStages::VERTEX_FRAGMENT),
                ]
            })
            .collect();
        let sampler_layout = rhi::backend::create_bind_group_layout(device, &sampler_entries);
        let resources = if geometry.is_some_and(PackGeometry::terrain) {
            self.sprite_layout
        } else {
            self.empty_layout
        };
        let sky = matches!(
            geometry,
            Some(PackGeometry::SkyBasic | PackGeometry::SkyTextured)
        );
        let push_constants = match geometry {
            // `draw_water`'s section origin and fade.
            Some(g) if g.water() => Some(rhi::PushConstants {
                visibility: rhi::ShaderStages::VERTEX,
                size: 16,
            }),
            // The block-entity push block: model, tint, overlay, uv params.
            Some(g) if g.drawn() == PackGeometry::BlockEntities => Some(rhi::PushConstants {
                visibility: rhi::ShaderStages::VERTEX,
                size: 112,
            }),
            _ if sky => Some(rhi::PushConstants {
                visibility: rhi::ShaderStages::VERTEX_FRAGMENT,
                size: SKY_PUSH_SIZE,
            }),
            _ => None,
        };
        let layout = rhi::backend::create_pipeline_layout(
            device,
            &[
                uniform_layout,
                sampler_layout,
                resources,
                self.albedo_layout,
            ],
            push_constants,
        );
        let vertex = bytemuck::cast_slice(&compiled.vertex);
        let fragment = bytemuck::cast_slice(&compiled.fragment);
        let entity_buffers = crate::renderer::pipelines::entity_renderer::INSTANCED_BUFFERS;
        let block_entity_buffers = [EntityVertex::BUFFER];
        let sky_buffers = [SkyVertex::BUFFER];
        let vertex_buffers: &[rhi::VertexBufferLayout] = match geometry.map(PackGeometry::drawn) {
            Some(PackGeometry::SkyBasic | PackGeometry::SkyTextured) => &sky_buffers,
            Some(PackGeometry::Terrain) => &CHUNK_VERTEX_BUFFERS,
            Some(PackGeometry::Water) => &CHUNK_VERTEX_BUFFERS[..1],
            Some(PackGeometry::Entities) => &entity_buffers,
            Some(PackGeometry::BlockEntities) => &block_entity_buffers,
            Some(_) => unreachable!("drawn geometries aren't shadow ones"),
            None => &[],
        };
        let desc = rhi::RenderPipelineDesc {
            // The pack draws in OpenGL's orientation, which Vulkan flips:
            // front faces wind the other way round. Iris draws the shadow
            // pass unculled.
            cull_mode: match geometry {
                Some(PackGeometry::Terrain) => rhi::CullMode::Back,
                _ => rhi::CullMode::None,
            },
            front_face: rhi::FrontFace::Cw,
            // Vanilla's sky writes no depth.
            depth: geometry.map(|_| {
                if sky {
                    rhi::DepthState::READ_ONLY
                } else {
                    rhi::DepthState::WRITE
                }
            }),
            blend: (sky || geometry.is_some_and(PackGeometry::water))
                .then_some(rhi::BlendState::TRANSLUCENT),
            // As the normal sky does, so the disc has no cracks.
            depth_clamp: sky,
            ..rhi::RenderPipelineDesc::new(&compiled.path, vertex, fragment, vertex_buffers)
        };
        let overlay = rhi::RenderPipelineDesc {
            blend: Some(rhi::BlendState::OVERLAY),
            ..desc
        };
        let pipelines = rhi::backend::try_create_render_pipeline(device, targets, layout, &desc)
            .and_then(|pipeline| {
                let overlay = sky
                    .then(|| {
                        rhi::backend::try_create_render_pipeline(device, targets, layout, &overlay)
                    })
                    .transpose()
                    .inspect_err(|_| pipeline.destroy(device))?;
                Ok((pipeline, overlay))
            });
        let (pipeline, overlay) = match pipelines {
            Ok(pipelines) => pipelines,
            Err(e) => {
                layout.destroy(device);
                uniform_layout.destroy(device);
                sampler_layout.destroy(device);
                return Err(format!("{}: {e}", compiled.path));
            }
        };
        let uniform_size = u64::from(compiled.interface.uniform_size);
        let uniform_buffers = util::create_per_frame_host_buffers(
            device,
            uniform_size,
            rhi::BufferUsages::UNIFORM,
            "pack_uniforms",
        );
        let frame_buffers = match &self.shadow_targets {
            Some(targets) if geometry.is_some_and(PackGeometry::shadow) => &targets.frame_buffers,
            _ => &self.frame_buffers,
        };
        let uniform_sets = (0..MAX_FRAMES_IN_FLIGHT)
            .map(|frame| {
                device.create_bind_group(
                    uniform_layout,
                    &[
                        (
                            0,
                            rhi::backend::BindingResource::uniform(
                                &uniform_buffers[frame],
                                uniform_size,
                            ),
                        ),
                        (
                            1,
                            rhi::backend::BindingResource::uniform(
                                &frame_buffers[frame],
                                size_of::<FrameUniform>() as u64,
                            ),
                        ),
                    ],
                )
            })
            .collect();
        let sampler_sets = (0..MAX_FRAMES_IN_FLIGHT)
            .map(|_| {
                device.create_bind_group(sampler_layout, &self.sampler_bindings(&compiled, false))
            })
            .collect();
        Ok(Program {
            pipeline,
            overlay,
            layout,
            uniform_layout,
            sampler_layout,
            uniform_buffers,
            uniform_sets,
            sampler_sets,
            compiled,
            draw_buffers,
        })
    }

    /// What each of a program's samplers reads, by Iris's names.
    fn sampler_bindings(
        &self,
        compiled: &CompiledProgram,
        full_screen: bool,
    ) -> Vec<(u32, rhi::backend::BindingResource)> {
        use rhi::backend::BindingResource as R;
        let mut bindings = Vec::new();
        let watershadow = compiled
            .interface
            .samplers
            .iter()
            .any(|s| s.name == "watershadow");
        for s in &compiled.interface.samplers {
            let name = s.name.as_str();
            let colortex = buffer_index(name)
                .or((full_screen && matches!(name, "tex" | "texture")).then_some(0));
            let (texture, sampler) = if let Some(buffer) =
                colortex.and_then(|i| self.buffers.get(&i))
            {
                (buffer.textures[buffer.current].handle(), self.linear)
            } else if let Some(shadow) = self.shadow_map(name, watershadow) {
                shadow
            } else {
                match name {
                    // Geometry passes draw into `depthtex0`, so they read
                    // the copy.
                    "depthtex0" | "gdepthtex" if full_screen => (self.depth.handle(), self.nearest),
                    "depthtex0" | "gdepthtex" | "depthtex1" | "depthtex2" => {
                        (self.depth_opaque.handle(), self.nearest)
                    }
                    "lightmap" => self.lightmap,
                    "noisetex" => (self.noise.handle(), self.linear),
                    _ if s.kind == SamplerKind::Tex2DShadow => {
                        (self.far_depth.handle(), self.shadow)
                    }
                    // TODO: normals/specular atlases and custom textures;
                    // they read white until then.
                    _ => (self.white.handle(), self.nearest),
                }
            };
            // A `sampler2DShadow` always compares, as Vulkan needs, keeping
            // its filter; Iris compares only with `shadowHardwareFiltering`
            // and leaves the read without it to the driver.
            let sampler = match s.kind {
                SamplerKind::Tex2DShadow if sampler == self.nearest => self.shadow_nearest,
                SamplerKind::Tex2DShadow => self.shadow,
                _ => sampler,
            };
            bindings.push((s.binding, R::Texture(texture)));
            bindings.push((s.binding + 1, R::Sampler(sampler)));
        }
        bindings
    }

    /// What the sampler `name` reads of the shadow pass: `shadow` is
    /// `shadowtex1` when the program also declares `watershadow`, as in Iris.
    fn shadow_map(
        &self,
        name: &str,
        watershadow: bool,
    ) -> Option<(rhi::backend::TextureHandle, rhi::backend::Sampler)> {
        let targets = self.shadow_targets.as_ref()?;
        let settings = &self.shadow_settings;
        let filter = |nearest: bool| if nearest { self.nearest } else { self.linear };
        let depth = |opaque: bool| {
            let texture = if opaque {
                &targets.depth_opaque
            } else {
                &targets.depth
            };
            let nearest = settings.depth_nearest[usize::from(opaque)];
            (texture.handle(), filter(nearest))
        };
        Some(match name {
            "shadowtex0" | "watershadow" | "shadowtex0HW" => depth(false),
            "shadowtex1" | "shadowtex1HW" => depth(true),
            "shadow" => depth(watershadow),
            _ => {
                let i = shadow_buffer_index(name)?;
                let color = targets.colors.get(&i)?;
                let nearest = settings.color_nearest.get(usize::from(i)) == Some(&true);
                (color.texture.handle(), filter(nearest))
            }
        })
    }

    /// `sunPathRotation`, which tilts the sky's sun and moon too.
    pub fn sun_path_rotation(&self) -> f32 {
        self.shadow_settings.sun_path_rotation
    }

    /// The shadow view's projection times model-view, or `None` if the pack
    /// has no shadow pass.
    pub fn shadow_view_projection(&self) -> Option<Mat4> {
        self.shadow_targets
            .is_some()
            .then_some(self.shadow_view_projection)
    }

    /// Fills the frame's uniforms for every program.
    pub fn prepare(&mut self, frame: usize, data: &PackFrame) {
        let current = Previous {
            view: data.view,
            projection: data.projection,
            camera: data.camera,
        };
        let frame_uniform = |view: Mat4, projection: Mat4| FrameUniform {
            view: view.to_cols_array(),
            projection: projection.to_cols_array(),
            eye: data.eye.extend(0.0).to_array(),
            anchor: data.anchor.extend(0).to_array(),
            atlas_size: [data.atlas_size as f32, data.atlas_size as f32, 0.0, 0.0],
            fog: [data.fog[0], data.fog[1], 0.0, 0.0],
            fog_color: data.fog_color.extend(1.0).to_array(),
        };
        self.frame_buffers[frame].write(bytemuck::bytes_of(&frame_uniform(
            data.view,
            data.projection,
        )));
        let shadow = shadow_matrices(
            &self.shadow_settings,
            shadow_angle(data.sun_angle_degrees),
            data.camera,
        );
        self.shadow_view_projection = shadow.projection * shadow.model_view;
        if let Some(targets) = &mut self.shadow_targets {
            targets.frame_buffers[frame].write(bytemuck::bytes_of(&frame_uniform(
                shadow.model_view,
                shadow.projection,
            )));
        }

        let now = std::time::Instant::now();
        let frame_time = self
            .last_prepare
            .map_or(1.0 / 60.0, |last| (now - last).as_secs_f32());
        self.last_prepare = Some(now);
        // Iris's default `eyeBrightnessHalfLife`: a second each way.
        let eased = 0.5f32.powf(frame_time);
        let eye_brightness_smooth = match self.eye_brightness_smooth {
            Some(smooth) => {
                std::array::from_fn(|i| data.eye_light[i] + (smooth[i] - data.eye_light[i]) * eased)
            }
            None => data.eye_light,
        };
        self.eye_brightness_smooth = Some(eye_brightness_smooth);
        let values = UniformValues::new(
            data,
            self.previous.unwrap_or(current),
            shadow,
            self.sun_path_rotation(),
            self.extent,
            Timing {
                frame_counter: self.frame_counter,
                time_counter: self.started.elapsed().as_secs_f32() % 3600.0,
                frame_time,
            },
            eye_brightness_smooth,
        );
        self.custom.evaluate(&|name| values.get(name), frame_time);
        for program in self
            .geometry
            .values_mut()
            .chain(&mut self.deferred)
            .chain(&mut self.composite)
            .chain(&mut self.final_pass)
        {
            let bytes = values.block(&program.compiled, &self.custom);
            program.uniform_buffers[frame].write(&bytes);
        }
        self.previous = Some(current);
        self.frame_counter = self.frame_counter.wrapping_add(1);
    }

    /// Points `program`'s sampler set for `frame` at what its samplers read
    /// now, returning the set.
    fn bind_samplers(
        &mut self,
        device: &rhi::backend::Device,
        frame: usize,
        program: ProgramRef,
        full_screen: bool,
    ) -> Option<rhi::backend::BindGroup> {
        let bindings = self.sampler_bindings(&self.program_ref(program)?.compiled, full_screen);
        let program = self.program_mut(program)?;
        program.sampler_sets[frame].replace(device, program.sampler_layout, &bindings);
        Some(program.sampler_sets[frame])
    }

    fn program_ref(&self, program: ProgramRef) -> Option<&Program> {
        match program {
            ProgramRef::Geometry(g) => self.geometry.get(&g),
            ProgramRef::Deferred(i) => self.deferred.get(i),
            ProgramRef::Composite(i) => self.composite.get(i),
            ProgramRef::Final => self.final_pass.as_ref(),
        }
    }

    fn program_mut(&mut self, program: ProgramRef) -> Option<&mut Program> {
        match program {
            ProgramRef::Geometry(g) => self.geometry.get_mut(&g),
            ProgramRef::Deferred(i) => self.deferred.get_mut(i),
            ProgramRef::Composite(i) => self.composite.get_mut(i),
            ProgramRef::Final => self.final_pass.as_mut(),
        }
    }

    /// Clears both textures of the buffers that clear each frame, the
    /// shadow buffers that do, and the depth targets.
    pub fn clear(&mut self, encoder: &rhi::backend::CommandEncoder, fog_color: Vec3) {
        let color = |clear| match clear {
            Clear::Keep => None,
            Clear::FogColor => Some(fog_color.extend(1.0).to_array()),
            Clear::Color(color) => Some(color),
        };
        let shadow = self.shadow_targets.as_ref();
        let cleared: Vec<(rhi::backend::TextureHandle, [f32; 4])> = self
            .buffers
            .values()
            .flat_map(|b| b.textures.iter().map(move |t| (t, b.clear)))
            .chain(
                shadow
                    .into_iter()
                    .flat_map(|s| s.colors.values().map(|c| (&c.texture, c.clear))),
            )
            .filter_map(|(t, clear)| Some((t.handle(), color(clear)?)))
            .collect();
        let depths: Vec<_> = [&self.depth, &self.depth_opaque]
            .into_iter()
            .chain(shadow.into_iter().flat_map(|s| [&s.depth, &s.depth_opaque]))
            .map(|d| d.handle())
            .collect();
        let transitions: Vec<_> = cleared
            .iter()
            .map(|(t, _)| TextureTransition::color(*t, TextureUse::ShaderRead, TextureUse::CopyDst))
            .chain(depths.iter().map(|&d| {
                TextureTransition::color(d, TextureUse::DepthRead, TextureUse::DepthCopyDst)
            }))
            .collect();
        encoder.transition_textures(&transitions);
        for (texture, color) in &cleared {
            encoder.clear_texture(*texture, rhi::TextureRange::BASE, *color);
        }
        for depth in depths {
            encoder.clear_depth(depth, 1.0);
        }
        encoder.transition_textures(&reversed(&transitions));
    }

    /// The depth target, its opaque copy and their extent: the screen's, or
    /// the shadow pass's.
    fn depths(
        &self,
        shadow: bool,
    ) -> (
        rhi::backend::TextureHandle,
        rhi::backend::TextureHandle,
        rhi::Extent,
    ) {
        match &self.shadow_targets {
            Some(t) if shadow => (t.depth.handle(), t.depth_opaque.handle(), t.extent),
            _ => (self.depth.handle(), self.depth_opaque.handle(), self.extent),
        }
    }

    /// Copies `depthtex0` into `depthtex1`, or `shadowtex0` into `shadowtex1`,
    /// as Iris does before the translucent geometry.
    pub fn copy_opaque_depth(&self, encoder: &rhi::backend::CommandEncoder, shadow: bool) {
        let (depth, opaque, extent) = self.depths(shadow);
        let transitions = [
            TextureTransition::color(depth, TextureUse::DepthRead, TextureUse::DepthCopySrc),
            TextureTransition::color(opaque, TextureUse::DepthRead, TextureUse::DepthCopyDst),
        ];
        encoder.transition_textures(&transitions);
        encoder.copy_depth(depth, opaque, extent);
        encoder.transition_textures(&reversed(&transitions));
    }

    /// The color attachments for `buffers`: their current texture, or the
    /// other one when a full-screen pass `flips` them.
    fn attachments(&self, buffers: &[u8], flip: bool) -> Vec<rhi::backend::ColorAttachment> {
        buffers
            .iter()
            .map(|b| {
                let buffer = &self.buffers[b];
                let side = if flip {
                    1 - buffer.current
                } else {
                    buffer.current
                };
                attachment(&buffer.textures[side], buffer.format)
            })
            .collect()
    }

    /// A geometry program's color attachments, its depth target and their
    /// extent.
    fn geometry_targets(
        &self,
        program: &Program,
        shadow: bool,
    ) -> (
        Vec<rhi::backend::ColorAttachment>,
        rhi::backend::TextureHandle,
        rhi::Extent,
    ) {
        let (depth, _, extent) = self.depths(shadow);
        let attachments = match &self.shadow_targets {
            Some(targets) if shadow => program
                .draw_buffers
                .iter()
                .map(|b| attachment(&targets.colors[b].texture, targets.colors[b].format))
                .collect(),
            _ => self.attachments(&program.draw_buffers, false),
        };
        (attachments, depth, extent)
    }

    /// Moves `attachments` between being sampled and being drawn into, with
    /// the `depth` target too.
    fn transition(
        &self,
        encoder: &rhi::backend::CommandEncoder,
        attachments: &[rhi::backend::ColorAttachment],
        depth: Option<rhi::backend::TextureHandle>,
        to_attachment: bool,
    ) {
        let between = |texture, read, write| {
            if to_attachment {
                TextureTransition::color(texture, read, write)
            } else {
                TextureTransition::color(texture, write, read)
            }
        };
        let mut transitions: Vec<_> = attachments
            .iter()
            .map(|a| {
                between(
                    a.texture,
                    TextureUse::ShaderRead,
                    TextureUse::ColorAttachment,
                )
            })
            .collect();
        if let Some(depth) = depth {
            transitions.push(between(
                depth,
                TextureUse::DepthRead,
                TextureUse::DepthAttachment,
            ));
        }
        encoder.transition_textures(&transitions);
    }

    fn begin(
        &self,
        device: &rhi::backend::Device,
        encoder: &rhi::backend::CommandEncoder,
        attachments: Vec<rhi::backend::ColorAttachment>,
        depth: Option<rhi::backend::TextureHandle>,
        extent: rhi::Extent,
    ) -> rhi::backend::RenderPassEncoder {
        let mut colors = attachments.into_iter();
        encoder.begin_render_pass(
            device,
            &rhi::backend::RenderPassDesc {
                color: colors.next(),
                extra_colors: colors.collect(),
                depth: depth.map(|texture| rhi::backend::DepthAttachment {
                    texture,
                    load: rhi::LoadOp::Load,
                    store: rhi::StoreOp::Store,
                }),
                extent,
            },
        )
    }

    /// Begins `geometry`'s pass into its program's buffers and depth target,
    /// the shadow pass's for a shadow geometry, with the program bound;
    /// `None` if the pack has no program for it.
    // TODO: Iris draws a geometry with no program, even through the fallback
    // chain, with its built-in fallback shader into colortex0.
    pub fn begin_geometry(
        &mut self,
        device: &rhi::backend::Device,
        encoder: &rhi::backend::CommandEncoder,
        frame: usize,
        geometry: PackGeometry,
    ) -> Option<(rhi::backend::RenderPassEncoder, PackBinding)> {
        let sampler_set =
            self.bind_samplers(device, frame, ProgramRef::Geometry(geometry), false)?;
        let program = self.program_ref(ProgramRef::Geometry(geometry))?;
        let resources = if geometry.terrain() {
            self.sprite_group
        } else {
            self.empty_group
        };
        let binding = PackBinding {
            pipeline: program.pipeline,
            sets: [program.uniform_sets[frame], sampler_set, resources],
        };
        let (attachments, depth, extent) = self.geometry_targets(program, geometry.shadow());
        self.transition(encoder, &attachments, Some(depth), true);
        let mut pass = self.begin(device, encoder, attachments, Some(depth), extent);
        pass.set_pipeline(&binding.pipeline);
        pass.set_bind_groups(0, &binding.sets);
        Some((pass, binding))
    }

    /// Ends a [`Self::begin_geometry`] pass, leaving its targets readable.
    pub fn end_geometry(
        &self,
        encoder: &rhi::backend::CommandEncoder,
        pass: rhi::backend::RenderPassEncoder,
        geometry: PackGeometry,
    ) {
        pass.end();
        if let Some(program) = self.program_ref(ProgramRef::Geometry(geometry)) {
            let (attachments, depth, _) = self.geometry_targets(program, geometry.shadow());
            self.transition(encoder, &attachments, Some(depth), false);
        }
    }

    /// Draws `draws` through `gbuffers_skybasic` and `gbuffers_skytextured`,
    /// a pass for each run of draws one program takes.
    pub fn draw_sky(
        &mut self,
        device: &rhi::backend::Device,
        encoder: &rhi::backend::CommandEncoder,
        frame: usize,
        sky: &SkyPipeline,
        draws: &[SkyDraw],
    ) {
        for run in draws.chunk_by(|a, b| a.stage.textured() == b.stage.textured()) {
            let geometry = if run[0].stage.textured() {
                PackGeometry::SkyTextured
            } else {
                PackGeometry::SkyBasic
            };
            let Some((mut pass, binding)) = self.begin_geometry(device, encoder, frame, geometry)
            else {
                continue;
            };
            let overlay = self.geometry[&geometry].overlay;
            for draw in run {
                let pipeline = overlay.filter(|_| draw.stage.overlay());
                pass.set_pipeline(&pipeline.unwrap_or(binding.pipeline));
                sky.record(&pass, draw, 3);
            }
            self.end_geometry(encoder, pass, geometry);
        }
    }

    /// Runs the `deferred*` passes, or the `composite*` ones.
    pub fn run_full_screen(
        &mut self,
        device: &rhi::backend::Device,
        encoder: &rhi::backend::CommandEncoder,
        frame: usize,
        deferred: bool,
    ) {
        let count = if deferred {
            self.deferred.len()
        } else {
            self.composite.len()
        };
        for i in 0..count {
            let program = if deferred {
                ProgramRef::Deferred(i)
            } else {
                ProgramRef::Composite(i)
            };
            let Some(sampler_set) = self.bind_samplers(device, frame, program, true) else {
                continue;
            };
            let Some(p) = self.program_ref(program) else {
                continue;
            };
            let (pipeline, uniform_set, written) =
                (p.pipeline, p.uniform_sets[frame], p.draw_buffers.clone());
            // Writes go to each buffer's other texture, so the pass still
            // reads the current one.
            let attachments = self.attachments(&written, true);
            self.transition(encoder, &attachments, None, true);
            let mut pass = self.begin(device, encoder, attachments.clone(), None, self.extent);
            pass.set_pipeline(&pipeline);
            pass.set_bind_groups(
                0,
                &[uniform_set, sampler_set, self.empty_group, self.atlas_group],
            );
            pass.draw(3, 1, 0, 0);
            pass.end();
            self.transition(encoder, &attachments, None, false);
            for b in written {
                if let Some(buffer) = self.buffers.get_mut(&b) {
                    buffer.current = 1 - buffer.current;
                }
            }
        }
    }

    /// Draws `final` into the swapchain pass.
    pub fn draw_final(
        &mut self,
        device: &rhi::backend::Device,
        pass: &mut rhi::backend::RenderPassEncoder,
        frame: usize,
    ) {
        let Some(sampler_set) = self.bind_samplers(device, frame, ProgramRef::Final, true) else {
            return;
        };
        let Some(program) = self.program_ref(ProgramRef::Final) else {
            return;
        };
        pass.set_pipeline(&program.pipeline);
        pass.set_bind_groups(
            0,
            &[
                program.uniform_sets[frame],
                sampler_set,
                self.empty_group,
                self.atlas_group,
            ],
        );
        pass.draw(3, 1, 0, 0);
    }

    /// Rebuilds the screen-size targets for a new swapchain extent.
    pub fn resize(&mut self, device: &rhi::backend::Device, extent: rhi::Extent) {
        if extent == self.extent {
            return;
        }
        self.extent = extent;
        for depth in [&mut self.depth, &mut self.depth_opaque] {
            std::mem::replace(depth, create_depth(device, extent)).destroy(device);
        }
        for buffer in self.buffers.values_mut() {
            for texture in &mut buffer.textures {
                std::mem::replace(texture, create_color(device, extent, buffer.format))
                    .destroy(device);
            }
            buffer.current = 0;
        }
        self.previous = None;
    }

    pub fn destroy(&mut self, device: &rhi::backend::Device) {
        let programs: Vec<Program> = std::mem::take(&mut self.deferred)
            .into_iter()
            .chain(std::mem::take(&mut self.composite))
            .chain(self.final_pass.take())
            .chain(std::mem::take(&mut self.geometry).into_values())
            .collect();
        for mut program in programs {
            program.pipeline.destroy(device);
            if let Some(overlay) = program.overlay {
                overlay.destroy(device);
            }
            program.layout.destroy(device);
            util::destroy_buffers(device, &mut program.uniform_buffers);
            util::destroy_bind_groups(device, program.uniform_sets.drain(..));
            util::destroy_bind_groups(device, program.sampler_sets.drain(..));
            program.uniform_layout.destroy(device);
            program.sampler_layout.destroy(device);
        }
        for buffer in std::mem::take(&mut self.buffers).into_values() {
            for texture in buffer.textures {
                texture.destroy(device);
            }
        }
        if let Some(mut targets) = self.shadow_targets.take() {
            for texture in [targets.depth, targets.depth_opaque]
                .into_iter()
                .chain(targets.colors.into_values().map(|c| c.texture))
            {
                texture.destroy(device);
            }
            util::destroy_buffers(device, &mut targets.frame_buffers);
        }
        for texture in [
            &mut self.depth,
            &mut self.depth_opaque,
            &mut self.noise,
            &mut self.white,
            &mut self.far_depth,
        ] {
            std::mem::take(texture).destroy(device);
        }
        for sampler in [self.nearest, self.linear, self.shadow, self.shadow_nearest] {
            sampler.destroy(device);
        }
        util::destroy_buffers(device, &mut self.frame_buffers);
        self.empty_group.destroy(device);
        self.sprite_group.destroy(device);
        self.atlas_group.destroy(device);
        self.empty_layout.destroy(device);
        self.sprite_layout.destroy(device);
        self.albedo_layout.destroy(device);
    }
}

/// Drawing into `texture` over what it holds.
fn attachment(
    texture: &rhi::backend::Texture,
    format: rhi::TextureFormat,
) -> rhi::backend::ColorAttachment {
    rhi::backend::ColorAttachment {
        texture: texture.handle(),
        format: rhi::backend::texture_format(format),
        load: rhi::LoadOp::Load,
        store: rhi::StoreOp::Store,
    }
}

/// What buffer `index` clears to each frame: `default`, unless `settings`
/// keep it or name a colour.
fn buffer_clear(settings: &BufferSettings, index: u8, default: Clear) -> Clear {
    if settings.no_clear.contains(&index) {
        Clear::Keep
    } else {
        settings
            .clear_colors
            .get(&index)
            .map_or(default, |&color| Clear::Color(color))
    }
}

/// `PommeFrame`'s buffer for each frame in flight.
fn create_frame_buffers(device: &rhi::backend::Device) -> Vec<rhi::backend::Buffer> {
    util::create_per_frame_host_buffers(
        device,
        size_of::<FrameUniform>() as u64,
        rhi::BufferUsages::UNIFORM,
        "pack_frame",
    )
}

/// The shadow pass's matrices.
#[derive(Clone, Copy)]
struct ShadowMatrices {
    model_view: Mat4,
    projection: Mat4,
}

/// Iris's `shadowAngle`: the sun's angle by day, the moon's by night, as a
/// fraction of a turn from sunrise.
fn shadow_angle(sun_angle_degrees: f32) -> f32 {
    let sun_angle = (sun_angle_degrees / 360.0 + 0.25).fract();
    if sun_angle <= 0.5 {
        sun_angle
    } else {
        sun_angle - 0.5
    }
}

/// Iris's `ShadowMatrices`: the world seen from the sun or moon, eye-relative,
/// snapped to `interval` blocks so the shadows don't crawl as the eye moves,
/// through an orthographic box `distance` blocks across each way.
fn shadow_matrices(settings: &ShadowSettings, shadow_angle: f32, camera: DVec3) -> ShadowMatrices {
    let sky = if shadow_angle < 0.25 {
        shadow_angle + 0.75
    } else {
        shadow_angle - 0.25
    };
    let mut model_view = Mat4::from_rotation_x(90f32.to_radians())
        * Mat4::from_rotation_z((-360.0 * sky).to_radians())
        * Mat4::from_rotation_x(settings.sun_path_rotation.to_radians());
    let interval = settings.interval;
    if interval != 0.0 {
        // Java's float `%`, which keeps the camera coordinate's sign.
        let offset = camera.as_vec3().map(|c| c % interval - interval / 2.0);
        model_view *= Mat4::from_translation(offset);
    }
    let d = settings.distance;
    // Iris's `ShadowMatrices.NEAR` and `FAR`.
    let projection = glam::camera::rh::proj::opengl::orthographic(-d, d, -d, d, -100.05, 156.0);
    ShadowMatrices {
        model_view,
        projection,
    }
}

/// A program's buffers: its directive, else the `gl_FragData` indices.
fn draw_buffers(compiled: &CompiledProgram) -> Vec<u8> {
    compiled
        .draw_buffers
        .clone()
        .unwrap_or_else(|| (0..compiled.interface.outputs.max(1) as u8).collect())
}

/// A render target that can also be sampled, moved into `layout`.
fn create_target(
    device: &rhi::backend::Device,
    label: &str,
    extent: rhi::Extent,
    format: rhi::TextureFormat,
    layout: TextureUse,
) -> rhi::backend::Texture {
    let texture = rhi::backend::Texture::new(
        device,
        &rhi::TextureDesc::new(
            label,
            extent.width,
            extent.height,
            format,
            rhi::TextureUsages::RENDER_ATTACHMENT
                | rhi::TextureUsages::TEXTURE_BINDING
                | rhi::TextureUsages::COPY_SRC
                | rhi::TextureUsages::COPY_DST,
        ),
    );
    device.submit(|encoder| {
        encoder.transition_textures(&[TextureTransition::color(
            texture.handle(),
            TextureUse::Undefined,
            layout,
        )]);
    });
    texture
}

/// `transitions` undone, in the same order.
fn reversed(
    transitions: &[rhi::backend::TextureTransition],
) -> Vec<rhi::backend::TextureTransition> {
    transitions
        .iter()
        .map(|t| TextureTransition {
            from: t.to,
            to: t.from,
            ..*t
        })
        .collect()
}

fn create_color(
    device: &rhi::backend::Device,
    extent: rhi::Extent,
    format: rhi::TextureFormat,
) -> rhi::backend::Texture {
    create_target(
        device,
        "pack_colortex",
        extent,
        format,
        TextureUse::ShaderRead,
    )
}

fn create_depth(device: &rhi::backend::Device, extent: rhi::Extent) -> rhi::backend::Texture {
    create_target(
        device,
        "pack_depth",
        extent,
        rhi::TextureFormat::Depth32Float,
        TextureUse::DepthRead,
    )
}

/// A depth texture at the far plane, standing in for shadow maps: every
/// comparison passes, so nothing is in shadow.
fn create_far_depth(device: &rhi::backend::Device) -> rhi::backend::Texture {
    let one = rhi::Extent {
        width: 1,
        height: 1,
    };
    let texture = create_target(
        device,
        "pack_far_depth",
        one,
        rhi::TextureFormat::Depth32Float,
        TextureUse::DepthCopyDst,
    );
    device.submit(|encoder| {
        encoder.clear_depth(texture.handle(), 1.0);
        encoder.transition_textures(&[TextureTransition::color(
            texture.handle(),
            TextureUse::DepthCopyDst,
            TextureUse::DepthRead,
        )]);
    });
    texture
}

fn create_white(device: &rhi::backend::Device) -> rhi::backend::Texture {
    util::create_sampled_texture(device, "pack_white", &[255; 4], 1, 1)
}

/// Iris's default `noisetex`: 256² of random RGB.
fn create_noise(device: &rhi::backend::Device) -> rhi::backend::Texture {
    let mut rng = fastrand::Rng::with_seed(0x5eed);
    let pixels: Vec<u8> = (0..256 * 256 * 4)
        .map(|i| if i % 4 == 3 { 255 } else { rng.u8(..) })
        .collect();
    util::create_sampled_texture(device, "pack_noise", &pixels, 256, 256)
}

/// The frame counters the time uniforms read.
struct Timing {
    frame_counter: u32,
    /// `frameTimeCounter`: seconds since the pack loaded, wrapping hourly.
    time_counter: f32,
    /// Seconds since the last frame.
    frame_time: f32,
}

/// Last frame's camera, for the `previous*` uniforms.
#[derive(Clone, Copy)]
struct Previous {
    view: Mat4,
    projection: Mat4,
    camera: DVec3,
}

/// A frame's values for Iris's uniforms, by name.
struct UniformValues {
    values: BTreeMap<&'static str, Value>,
    biome_names: Arc<[String]>,
}

/// Iris's precipitation and biome-category constants.
const CONSTANTS: &[(&str, i32)] = &[
    ("PPT_NONE", 0),
    ("PPT_RAIN", 1),
    ("PPT_SNOW", 2),
    ("CAT_NONE", 0),
    ("CAT_TAIGA", 1),
    ("CAT_EXTREME_HILLS", 2),
    ("CAT_JUNGLE", 3),
    ("CAT_MESA", 4),
    ("CAT_PLAINS", 5),
    ("CAT_SAVANNA", 6),
    ("CAT_ICY", 7),
    ("CAT_THE_END", 8),
    ("CAT_BEACH", 9),
    ("CAT_FOREST", 10),
    ("CAT_OCEAN", 11),
    ("CAT_DESERT", 12),
    ("CAT_RIVER", 13),
    ("CAT_SWAMP", 14),
    ("CAT_MUSHROOM", 15),
    ("CAT_NETHER", 16),
    ("CAT_MOUNTAIN", 17),
    ("CAT_UNDERGROUND", 18),
];

impl UniformValues {
    fn new(
        data: &PackFrame,
        previous: Previous,
        shadow: ShadowMatrices,
        sun_path_rotation: f32,
        extent: rhi::Extent,
        timing: Timing,
        eye_brightness_smooth: [f32; 2],
    ) -> Self {
        let sky_angle = data.sun_angle_degrees / 360.0;
        // Iris's `CelestialUniforms`: the sun 100 blocks up the celestial
        // rotation, in view space.
        let celestial = celestial_pose(data.sun_angle_degrees, sun_path_rotation);
        let sun = (data.view * celestial * Vec4::new(0.0, 100.0, 0.0, 0.0)).truncate();
        let moon = (data.view * celestial * Vec4::new(0.0, -100.0, 0.0, 0.0)).truncate();
        let up = (data.view * Vec4::new(0.0, 100.0, 0.0, 0.0)).truncate();
        let sun_angle = (sky_angle + 0.25).fract();
        let day = sun_angle <= 0.5;
        let camera = data.camera;
        let camera_fract = (camera - camera.floor()).as_vec3();
        let previous_fract = (previous.camera - previous.camera.floor()).as_vec3();
        let (w, h) = (extent.width as f32, extent.height as f32);
        let mut values = BTreeMap::new();
        let mut set = |name: &'static str, value| {
            values.insert(name, value);
        };
        set("gbufferModelView", Value::Mat4(data.view));
        set("gbufferModelViewInverse", Value::Mat4(data.view.inverse()));
        set("gbufferProjection", Value::Mat4(data.projection));
        set(
            "gbufferProjectionInverse",
            Value::Mat4(data.projection.inverse()),
        );
        set("gbufferPreviousModelView", Value::Mat4(previous.view));
        set(
            "gbufferPreviousProjection",
            Value::Mat4(previous.projection),
        );
        set("cameraPosition", Value::Vec3(camera.as_vec3()));
        set("cameraPositionFract", Value::Vec3(camera_fract));
        set("cameraPositionInt", Value::Vec3(camera.floor().as_vec3()));
        set(
            "previousCameraPosition",
            Value::Vec3(previous.camera.as_vec3()),
        );
        set("previousCameraPositionFract", Value::Vec3(previous_fract));
        set("sunPosition", Value::Vec3(sun));
        set("moonPosition", Value::Vec3(moon));
        set(
            "shadowLightPosition",
            Value::Vec3(if day { sun } else { moon }),
        );
        set("upPosition", Value::Vec3(up));
        set("sunAngle", Value::Float(sun_angle));
        set(
            "shadowAngle",
            Value::Float(shadow_angle(data.sun_angle_degrees)),
        );
        set("worldTime", Value::Int((data.day_time % 24000) as i32));
        set("worldDay", Value::Int((data.day_time / 24000) as i32));
        set("moonPhase", Value::Int(i32::from(data.moon_phase)));
        set(
            "frameCounter",
            Value::Int((timing.frame_counter % 720_720) as i32),
        );
        set("frameTimeCounter", Value::Float(timing.time_counter));
        set("frameTime", Value::Float(timing.frame_time));
        set("rainStrength", Value::Float(data.rain));
        set("wetness", Value::Float(data.rain));
        set("viewWidth", Value::Float(w));
        set("viewHeight", Value::Float(h));
        set("aspectRatio", Value::Float(w / h.max(1.0)));
        set("near", Value::Float(data.near));
        set("far", Value::Float(data.far));
        set("fogColor", Value::Vec3(data.fog_color));
        set("skyColor", Value::Vec3(data.sky_color));
        set("fogStart", Value::Float(data.fog[0]));
        set("fogEnd", Value::Float(data.fog[1]));
        set(
            "eyeBrightness",
            Value::IVec2(IVec2::from_array(data.eye_light.map(|l| l as i32))),
        );
        set(
            "eyeBrightnessSmooth",
            Value::IVec2(IVec2::from_array(
                eye_brightness_smooth.map(|l| l.round() as i32),
            )),
        );
        set("isEyeInWater", Value::Int(i32::from(data.eye_in_water)));
        set("blindness", Value::Float(data.world.blindness));
        set("eyeAltitude", Value::Float(camera.y as f32));
        set("biome", Value::Int(data.world.biome));
        // TODO: Iris's biome categories, from the biome tags.
        set("biome_category", Value::Int(0));
        set("biome_precipitation", Value::Int(data.world.precipitation));
        set("temperature", Value::Float(data.world.temperature));
        set("rainfall", Value::Float(data.world.rainfall));
        set("darknessFactor", Value::Float(0.0));
        set("nightVision", Value::Float(0.0));
        set("screenBrightness", Value::Float(data.screen_brightness));
        set("heldBlockLightValue", Value::Int(0));
        set("heldBlockLightValue2", Value::Int(0));
        set(
            "atlasSize",
            Value::IVec2(IVec2::splat(data.atlas_size as i32)),
        );
        set("gtexture", Value::Int(0));
        set("shadowModelView", Value::Mat4(shadow.model_view));
        set(
            "shadowModelViewInverse",
            Value::Mat4(shadow.model_view.inverse()),
        );
        set("shadowProjection", Value::Mat4(shadow.projection));
        set(
            "shadowProjectionInverse",
            Value::Mat4(shadow.projection.inverse()),
        );
        Self {
            values,
            biome_names: Arc::clone(&data.world.biome_names),
        }
    }

    /// A built-in uniform or constant, by name.
    fn get(&self, name: &str) -> Option<Value> {
        if let Some(value) = self.values.get(name) {
            return Some(*value);
        }
        if let Some(biome) = name.strip_prefix("BIOME_") {
            let id = self.biome_names.iter().position(|n| {
                n.rsplit(':')
                    .next()
                    .is_some_and(|path| path.eq_ignore_ascii_case(biome))
            })?;
            return Some(Value::Int(id as i32));
        }
        CONSTANTS
            .iter()
            .find(|(n, _)| *n == name)
            .map(|(_, v)| Value::Int(*v))
    }

    /// `compiled`'s uniforms block, std140, from the values it declares:
    /// the pack's custom uniforms first, then the built-in ones.
    fn block(&self, compiled: &CompiledProgram, custom: &CustomUniforms) -> Vec<u8> {
        let mut bytes = vec![0u8; compiled.interface.uniform_size as usize];
        for uniform in &compiled.interface.uniforms {
            let name = uniform.name.as_str();
            let Some(value) = custom.get(name).or_else(|| self.get(name)) else {
                continue;
            };
            let floats = |n: usize| {
                value
                    .components()
                    .map(|(c, _)| bytemuck::cast_slice::<f32, u8>(&c.to_array()[..n]).to_vec())
            };
            let ints = |n: usize| {
                value.components().map(|(c, _)| {
                    let ints = c.to_array().map(|v| v as i32);
                    bytemuck::cast_slice::<i32, u8>(&ints[..n]).to_vec()
                })
            };
            let data = match (uniform.ty, value) {
                (UniformType::Mat4, Value::Mat4(m)) => {
                    Some(bytemuck::cast_slice(&m.to_cols_array()).to_vec())
                }
                (UniformType::Float, _) => Some(value.as_f32().to_le_bytes().to_vec()),
                (UniformType::Int | UniformType::Bool, _) => {
                    Some(value.as_i32().to_le_bytes().to_vec())
                }
                (UniformType::UInt, _) => Some((value.as_i32() as u32).to_le_bytes().to_vec()),
                (UniformType::Vec2, _) => floats(2),
                (UniformType::Vec3, _) => floats(3),
                (UniformType::Vec4, _) => floats(4),
                (UniformType::IVec2, _) => ints(2),
                (UniformType::IVec3, _) => ints(3),
                (UniformType::IVec4, _) => ints(4),
                _ => None,
            };
            let Some(data) = data else {
                continue;
            };
            let at = uniform.offset as usize;
            let end = (at + data.len()).min(bytes.len());
            bytes[at..end].copy_from_slice(&data[..end - at]);
        }
        bytes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: Vec3, b: Vec3) {
        assert!(a.abs_diff_eq(b, 1e-4), "{a} != {b}");
    }

    #[test]
    fn entities_cast_with_their_own_program_or_shadow() {
        let pack = ShaderPack::for_test(
            &[
                "shadow.vsh",
                "shadow.fsh",
                "shadow_block.vsh",
                "shadow_block.fsh",
            ],
            &[],
        );
        let path = |g: PackGeometry| {
            g.source(&pack, "minecraft:overworld")
                .unwrap()
                .unwrap()
                .path
        };
        assert_eq!(path(PackGeometry::ShadowEntities), "shadow");
        assert_eq!(path(PackGeometry::ShadowBlockEntities), "shadow_block");
        assert_eq!(path(PackGeometry::ShadowTerrain), "shadow");
        assert_eq!(PackGeometry::ShadowEntities.vertices(), Geometry::Entity);
    }

    #[test]
    fn shadow_casters_follow_the_properties() {
        let casters = |properties| ShadowCasters::read(&ShaderPack::for_test(&[], properties));
        let iris = casters(&[]);
        assert!(iris.casts(PackShadow::Entity) && iris.casts(PackShadow::FirstPersonPlayer));
        assert!(!iris.casts(PackShadow::Spectator));
        assert!(iris.draws(PackGeometry::ShadowBlockEntities));

        let player = casters(&[("shadowEntities", "false"), ("shadowPlayer", "true")]);
        assert!(!player.casts(PackShadow::Entity) && player.casts(PackShadow::Player));
        let none = casters(&[
            ("shadowEntities", "false"),
            ("shadowBlockEntities", "false"),
        ]);
        assert!(!none.draws(PackGeometry::ShadowEntities));
        assert!(!none.draws(PackGeometry::ShadowBlockEntities));
    }

    #[test]
    fn noon_shadows_look_straight_down() {
        let settings = ShadowSettings {
            interval: 0.0,
            ..ShadowSettings::default()
        };
        let noon = shadow_angle(0.0);
        assert_eq!(noon, 0.25);
        let m = shadow_matrices(&settings, noon, DVec3::ZERO);
        // Up faces the light, along the view's +z.
        close(m.model_view.transform_vector3(Vec3::Y), Vec3::Z);
        close(
            m.projection
                .project_point3(Vec3::new(settings.distance, 0.0, 0.0)),
            Vec3::new(1.0, 0.0, m.projection.project_point3(Vec3::ZERO).z),
        );
        // Night swaps in the moon, half a turn round.
        assert_eq!(shadow_angle(180.0), 0.25);
    }

    #[test]
    fn shadows_snap_to_the_interval() {
        let settings = ShadowSettings::default();
        let m = shadow_matrices(&settings, 0.25, DVec3::new(5.3, 0.0, -0.5));
        // Java's `%` keeps the sign: (5.3 % 2 - 1, 0 % 2 - 1, -0.5 % 2 - 1).
        let offset = Vec3::new(0.3, -1.0, -1.5);
        close(
            m.model_view.transform_point3(Vec3::ZERO),
            Mat4::from_rotation_x(90f32.to_radians()).transform_point3(offset),
        );
    }
}
