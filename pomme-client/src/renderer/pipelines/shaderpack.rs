//! A loaded shader pack's render path, Iris's pipeline in milestone 1: the
//! `gbuffers_*` programs draw terrain, entities and block entities into the
//! pack's `colortex` targets, `deferred*` and `composite*` passes run over
//! them, and `final` writes the swapchain. No shadow pass yet.

use std::collections::BTreeMap;

use glam::{DVec3, IVec3, Mat4, Vec3, Vec4};

use crate::renderer::chunk::atlas::TextureAtlas;
use crate::renderer::chunk::buffer::CHUNK_VERTEX_BUFFERS;
use crate::renderer::entity_model::EntityVertex;
use crate::renderer::pipelines::lightmap::Lightmap;
use crate::renderer::rhi::{self, TextureTransition, TextureUse};
use crate::renderer::{MAX_FRAMES_IN_FLIGHT, util};
use crate::shaderpack::directives::{TargetSettings, buffer_index};
use crate::shaderpack::glsl::translate::{SamplerKind, UniformType};
use crate::shaderpack::prelude::Geometry;
use crate::shaderpack::programs::{Gbuffers, full_screen_names};
use crate::shaderpack::{self, CompiledProgram, ShaderPack};

/// Buffers a pack can name, `colortex0` to `colortex31`.
const BUFFERS: usize = 32;

/// What the geometry passes draw, in order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum PackGeometry {
    Terrain,
    Entities,
    BlockEntities,
    Water,
}

impl PackGeometry {
    fn program(self) -> Gbuffers {
        match self {
            Self::Terrain => Gbuffers::Terrain,
            Self::Entities => Gbuffers::Entities,
            Self::BlockEntities => Gbuffers::Block,
            Self::Water => Gbuffers::Water,
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

/// A colortex buffer: two textures, flipped between when a full-screen pass
/// writes what it reads.
struct Buffer {
    textures: [rhi::backend::Texture; 2],
    format: rhi::TextureFormat,
    /// Which of `textures` holds the latest contents.
    current: usize,
    clear: Option<[f32; 4]>,
}

pub struct PackRenderer {
    dimension: String,
    extent: rhi::Extent,
    swapchain_targets: rhi::backend::PipelineTargets,
    buffers: BTreeMap<u8, Buffer>,
    depth: rhi::backend::Texture,
    noise: rhi::backend::Texture,
    /// 1×1 stand-ins: white color, and depth at the far plane for shadow maps.
    white: rhi::backend::Texture,
    far_depth: rhi::backend::Texture,
    nearest: rhi::backend::Sampler,
    linear: rhi::backend::Sampler,
    shadow: rhi::backend::Sampler,
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
    previous: Option<Previous>,
    frame_counter: u32,
    started: std::time::Instant,
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
    match name.unwrap_or("RGBA8") {
        "R8" | "R8_SNORM" => F::R8Unorm,
        "RG8" | "RG8_SNORM" => F::Rg8Unorm,
        "RGBA16" | "RGB16" | "RGBA12" | "RGB12" => F::Rgba16Unorm,
        "RGB10_A2" => F::Rgb10a2Unorm,
        "R16F" => F::R16Float,
        "RG16F" | "RG16" | "R16" => F::Rg16Float,
        "RGBA16F" | "RGB16F" => F::Rgba16Float,
        "R32F" => F::R32Float,
        "RGBA32F" | "RGB32F" | "RG32F" => F::Rgba32Float,
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
        let gbuffers: Vec<(PackGeometry, CompiledProgram)> = [
            PackGeometry::Terrain,
            PackGeometry::Entities,
            PackGeometry::BlockEntities,
            PackGeometry::Water,
        ]
        .into_iter()
        .filter_map(|g| {
            let text = pack.gbuffers(dimension, g.program()).transpose()?;
            Some(text.and_then(|t| {
                shaderpack::compile(&t, shaderpack::gbuffers_geometry(g.program())).map(|c| (g, c))
            }))
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
            .and_then(|t| {
                t.map(|t| shaderpack::compile(&t, Geometry::Final))
                    .transpose()
            })
            .map_err(|e| e.to_string())?;

        // Targets: every buffer a program writes or reads, set up by the
        // `const` settings across all of them.
        let mut settings = TargetSettings::default();
        let mut used: Vec<u8> = Vec::new();
        for c in gbuffers
            .iter()
            .map(|(_, c)| c)
            .chain(&deferred)
            .chain(&composite)
            .chain(&final_pass)
        {
            settings.read(&c.text);
            used.extend(draw_buffers(c));
            used.extend(
                c.interface
                    .samplers
                    .iter()
                    .filter_map(|s| buffer_index(&s.name)),
            );
        }
        used.sort_unstable();
        used.dedup();

        let nearest = rhi::backend::create_sampler(device, &rhi::SamplerDesc::NEAREST);
        let linear = rhi::backend::create_sampler(device, &rhi::SamplerDesc::LINEAR);
        let shadow = rhi::backend::create_sampler(
            device,
            &rhi::SamplerDesc {
                compare: Some(rhi::CompareFunction::LessEqual),
                ..rhi::SamplerDesc::LINEAR
            },
        );
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
        let frame_buffers = util::create_per_frame_host_buffers(
            device,
            size_of::<FrameUniform>() as u64,
            rhi::BufferUsages::UNIFORM,
            "pack_frame",
        );

        let mut this = Self {
            dimension: dimension.to_owned(),
            extent,
            swapchain_targets,
            buffers: BTreeMap::new(),
            depth: create_depth(device, extent),
            noise: create_noise(device),
            white: create_white(device),
            far_depth: create_far_depth(device),
            nearest,
            linear,
            shadow,
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
            previous: None,
            frame_counter: 0,
            started: std::time::Instant::now(),
        };

        for index in used.into_iter().filter(|&i| (i as usize) < BUFFERS) {
            let format = buffer_format(settings.formats.get(&index).map(String::as_str));
            let clear = (!settings.no_clear.contains(&index)).then(|| {
                settings
                    .clear_colors
                    .get(&index)
                    .copied()
                    .unwrap_or([if index == 1 { 1.0 } else { 0.0 }; 4])
            });
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

        for (geometry, compiled) in gbuffers {
            let program = this.program(device, compiled, Some(geometry))?;
            this.geometry.insert(geometry, program);
        }
        this.deferred = deferred
            .into_iter()
            .map(|c| this.program(device, c, None))
            .collect::<Result<_, _>>()?;
        this.composite = composite
            .into_iter()
            .map(|c| this.program(device, c, None))
            .collect::<Result<_, _>>()?;
        this.final_pass = final_pass
            .map(|c| this.build(device, c, this.swapchain_targets, None, vec![0]))
            .transpose()?;
        Ok(this)
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
        let buffers = draw_buffers(&compiled);
        let formats = buffers
            .iter()
            .map(|b| {
                self.buffers
                    .get(b)
                    .map(|buffer| rhi::backend::texture_format(buffer.format))
                    .ok_or_else(|| {
                        format!("{}: writes colortex{b}, past colortex31", compiled.path)
                    })
            })
            .collect::<Result<Vec<_>, _>>()?;
        let Some((&first, rest)) = formats.split_first() else {
            return Err(format!("{}: writes no buffers", compiled.path));
        };
        let mut targets = rhi::backend::PipelineTargets::new(first, geometry.is_some());
        for (slot, format) in targets.extra_colors.iter_mut().zip(rest) {
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
        let resources = match geometry {
            Some(PackGeometry::Terrain | PackGeometry::Water) => self.sprite_layout,
            _ => self.empty_layout,
        };
        let push_constants = match geometry {
            // `draw_water`'s section origin and fade.
            Some(PackGeometry::Water) => Some(rhi::PushConstants {
                visibility: rhi::ShaderStages::VERTEX,
                size: 16,
            }),
            // The block-entity push block: model, tint, overlay, uv params.
            Some(PackGeometry::BlockEntities) => Some(rhi::PushConstants {
                visibility: rhi::ShaderStages::VERTEX,
                size: 112,
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
        let vertex_buffers: &[rhi::VertexBufferLayout] = match geometry {
            Some(PackGeometry::Terrain) => &CHUNK_VERTEX_BUFFERS,
            Some(PackGeometry::Water) => &CHUNK_VERTEX_BUFFERS[..1],
            Some(PackGeometry::Entities) => &entity_buffers,
            Some(PackGeometry::BlockEntities) => &block_entity_buffers,
            None => &[],
        };
        let desc = rhi::RenderPipelineDesc {
            // The pack draws in OpenGL's orientation, which Vulkan flips:
            // front faces wind the other way round.
            cull_mode: match geometry {
                Some(PackGeometry::Terrain) => rhi::CullMode::Back,
                _ => rhi::CullMode::None,
            },
            front_face: rhi::FrontFace::Cw,
            depth: geometry.map(|g| rhi::DepthState {
                compare: rhi::CompareFunction::LessEqual,
                ..if g == PackGeometry::Water {
                    rhi::DepthState::READ_ONLY
                } else {
                    rhi::DepthState::WRITE
                }
            }),
            blend: (geometry == Some(PackGeometry::Water)).then_some(rhi::BlendState::TRANSLUCENT),
            ..rhi::RenderPipelineDesc::new(&compiled.path, vertex, fragment, vertex_buffers)
        };
        let pipeline = rhi::backend::try_create_render_pipeline(device, targets, layout, &desc)
            .map_err(|e| format!("{}: {e}", compiled.path))?;
        let uniform_size = u64::from(compiled.interface.uniform_size);
        let uniform_buffers = util::create_per_frame_host_buffers(
            device,
            uniform_size,
            rhi::BufferUsages::UNIFORM,
            "pack_uniforms",
        );
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
                                &self.frame_buffers[frame],
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
        for s in &compiled.interface.samplers {
            let name = s.name.as_str();
            let colortex = buffer_index(name)
                .or((full_screen && matches!(name, "tex" | "texture")).then_some(0));
            let (texture, sampler) =
                if let Some(buffer) = colortex.and_then(|i| self.buffers.get(&i)) {
                    (buffer.textures[buffer.current].handle(), self.nearest)
                } else {
                    match name {
                        "depthtex0" | "depthtex1" | "depthtex2" | "gdepthtex" => {
                            (self.depth.handle(), self.nearest)
                        }
                        "lightmap" => self.lightmap,
                        "noisetex" => (self.noise.handle(), self.linear),
                        _ if s.kind == SamplerKind::Tex2DShadow => {
                            (self.far_depth.handle(), self.shadow)
                        }
                        // TODO: shadow maps, normals/specular atlases and custom
                        // textures; they read white until then.
                        _ => (self.white.handle(), self.nearest),
                    }
                };
            let sampler = if s.kind == SamplerKind::Tex2DShadow {
                self.shadow
            } else {
                sampler
            };
            bindings.push((s.binding, R::Texture(texture)));
            bindings.push((s.binding + 1, R::Sampler(sampler)));
        }
        bindings
    }

    /// Fills the frame's uniforms for every program.
    pub fn prepare(&mut self, frame: usize, data: &PackFrame, atlas_size: u32) {
        let current = Previous {
            view: data.view,
            projection: data.projection,
            camera: data.camera,
        };
        let frame_uniform = FrameUniform {
            view: data.view.to_cols_array(),
            projection: data.projection.to_cols_array(),
            eye: data.eye.extend(0.0).to_array(),
            anchor: data.anchor.extend(0).to_array(),
            atlas_size: [atlas_size as f32, atlas_size as f32, 0.0, 0.0],
            fog: [data.fog[0], data.fog[1], 0.0, 0.0],
            fog_color: data.fog_color.extend(1.0).to_array(),
        };
        self.frame_buffers[frame].write(bytemuck::bytes_of(&frame_uniform));

        let values = UniformValues::new(
            data,
            self.previous.unwrap_or(current),
            self.extent,
            self.frame_counter,
            self.started.elapsed().as_secs_f32() % 3600.0,
        );
        for program in self
            .geometry
            .values_mut()
            .chain(&mut self.deferred)
            .chain(&mut self.composite)
            .chain(&mut self.final_pass)
        {
            let bytes = values.block(&program.compiled);
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

    /// Clears the buffers that clear each frame, and the depth target.
    pub fn clear(&mut self, device: &rhi::backend::Device, encoder: &rhi::backend::CommandEncoder) {
        let cleared: Vec<(rhi::backend::TextureHandle, rhi::TextureFormat, [f32; 4])> = self
            .buffers
            .values()
            .filter_map(|b| Some((b.textures[b.current].handle(), b.format, b.clear?)))
            .collect();
        let transitions: Vec<_> = cleared
            .iter()
            .map(|(t, ..)| {
                TextureTransition::color(*t, TextureUse::ShaderRead, TextureUse::ColorAttachment)
            })
            .chain(std::iter::once(TextureTransition::color(
                self.depth.handle(),
                TextureUse::DepthRead,
                TextureUse::DepthAttachment,
            )))
            .collect();
        encoder.transition_textures(&transitions);
        // One pass per eight buffers; the depth target clears with the first.
        let mut first = true;
        for chunk in cleared.chunks(rhi::MAX_COLOR_TARGETS) {
            let attachment = |(texture, format, color): &(
                rhi::backend::TextureHandle,
                rhi::TextureFormat,
                [f32; 4],
            )| {
                rhi::backend::ColorAttachment {
                    texture: *texture,
                    format: rhi::backend::texture_format(*format),
                    load: rhi::LoadOp::Clear(*color),
                    store: rhi::StoreOp::Store,
                }
            };
            let pass = encoder.begin_render_pass(
                device,
                &rhi::backend::RenderPassDesc {
                    color: attachment(&chunk[0]),
                    extra_colors: chunk[1..].iter().map(attachment).collect(),
                    depth: first.then(|| rhi::backend::DepthAttachment {
                        texture: self.depth.handle(),
                        load: rhi::LoadOp::Clear(1.0),
                        store: rhi::StoreOp::Store,
                    }),
                    extent: self.extent,
                },
            );
            pass.end();
            first = false;
        }
        let back: Vec<_> = transitions
            .iter()
            .map(|t| TextureTransition {
                from: t.to,
                to: t.from,
                ..*t
            })
            .collect();
        encoder.transition_textures(&back);
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
                rhi::backend::ColorAttachment {
                    texture: buffer.textures[side].handle(),
                    format: rhi::backend::texture_format(buffer.format),
                    load: rhi::LoadOp::Load,
                    store: rhi::StoreOp::Store,
                }
            })
            .collect()
    }

    /// Moves `attachments` between being sampled and being drawn into, with
    /// the depth target too if `depth`.
    fn transition(
        &self,
        encoder: &rhi::backend::CommandEncoder,
        attachments: &[rhi::backend::ColorAttachment],
        depth: bool,
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
        if depth {
            transitions.push(between(
                self.depth.handle(),
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
        mut attachments: Vec<rhi::backend::ColorAttachment>,
        depth: bool,
    ) -> rhi::backend::RenderPassEncoder {
        let color = attachments.remove(0);
        encoder.begin_render_pass(
            device,
            &rhi::backend::RenderPassDesc {
                color,
                extra_colors: attachments,
                depth: depth.then(|| rhi::backend::DepthAttachment {
                    texture: self.depth.handle(),
                    load: rhi::LoadOp::Load,
                    store: rhi::StoreOp::Store,
                }),
                extent: self.extent,
            },
        )
    }

    /// Begins `geometry`'s pass into its program's buffers and the depth
    /// target, with the program bound; `None` if the pack has no program
    /// for it.
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
        let resources = match geometry {
            PackGeometry::Terrain | PackGeometry::Water => self.sprite_group,
            _ => self.empty_group,
        };
        let binding = PackBinding {
            pipeline: program.pipeline,
            sets: [program.uniform_sets[frame], sampler_set, resources],
        };
        let attachments = self.attachments(&program.draw_buffers, false);
        self.transition(encoder, &attachments, true, true);
        let mut pass = self.begin(device, encoder, attachments, true);
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
            let attachments = self.attachments(&program.draw_buffers, false);
            self.transition(encoder, &attachments, true, false);
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
            self.transition(encoder, &attachments, false, true);
            let mut pass = self.begin(device, encoder, attachments.clone(), false);
            pass.set_pipeline(&pipeline);
            pass.set_bind_groups(
                0,
                &[uniform_set, sampler_set, self.empty_group, self.atlas_group],
            );
            pass.draw(3, 1, 0, 0);
            pass.end();
            self.transition(encoder, &attachments, false, false);
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
        std::mem::replace(&mut self.depth, create_depth(device, extent)).destroy(device);
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
        for texture in [
            &mut self.depth,
            &mut self.noise,
            &mut self.white,
            &mut self.far_depth,
        ] {
            std::mem::take(texture).destroy(device);
        }
        for sampler in [self.nearest, self.linear, self.shadow] {
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
            rhi::TextureUsages::RENDER_ATTACHMENT | rhi::TextureUsages::TEXTURE_BINDING,
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
        TextureUse::DepthAttachment,
    );
    // The RHI's passes always have a color target: a throwaway one.
    let scratch = create_target(
        device,
        "pack_scratch",
        one,
        rhi::TextureFormat::Rgba8Unorm,
        TextureUse::ColorAttachment,
    );
    device.submit(|encoder| {
        let pass = encoder.begin_render_pass(
            device,
            &rhi::backend::RenderPassDesc {
                color: rhi::backend::ColorAttachment {
                    texture: scratch.handle(),
                    format: rhi::backend::texture_format(rhi::TextureFormat::Rgba8Unorm),
                    load: rhi::LoadOp::Clear([0.0; 4]),
                    store: rhi::StoreOp::Discard,
                },
                extra_colors: Vec::new(),
                depth: Some(rhi::backend::DepthAttachment {
                    texture: texture.handle(),
                    load: rhi::LoadOp::Clear(1.0),
                    store: rhi::StoreOp::Store,
                }),
                extent: one,
            },
        );
        pass.end();
        encoder.transition_textures(&[TextureTransition::color(
            texture.handle(),
            TextureUse::DepthAttachment,
            TextureUse::DepthRead,
        )]);
    });
    scratch.destroy(device);
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

/// Last frame's camera, for the `previous*` uniforms.
#[derive(Clone, Copy)]
struct Previous {
    view: Mat4,
    projection: Mat4,
    camera: DVec3,
}

/// A frame's values for Iris's uniforms, by name.
struct UniformValues(BTreeMap<&'static str, Value>);

#[derive(Clone, Copy)]
enum Value {
    Float(f32),
    Int(i32),
    Vec3([f32; 3]),
    IVec2([i32; 2]),
    Mat4(Mat4),
}

impl UniformValues {
    fn new(
        data: &PackFrame,
        previous: Previous,
        extent: rhi::Extent,
        frame_counter: u32,
        frame_time: f32,
    ) -> Self {
        let sky_angle = data.sun_angle_degrees / 360.0;
        // Iris's `CelestialUniforms`: the sun 100 blocks up the celestial
        // rotation, in view space.
        let celestial = Mat4::from_rotation_y(-90f32.to_radians())
            * Mat4::from_rotation_x(data.sun_angle_degrees.to_radians());
        let sun = (data.view * celestial * Vec4::new(0.0, 100.0, 0.0, 0.0)).truncate();
        let moon = (data.view * celestial * Vec4::new(0.0, -100.0, 0.0, 0.0)).truncate();
        let up = (data.view * Vec4::new(0.0, 100.0, 0.0, 0.0)).truncate();
        let day = sky_angle < 0.5;
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
        set("cameraPosition", Value::Vec3(camera.as_vec3().to_array()));
        set("cameraPositionFract", Value::Vec3(camera_fract.to_array()));
        set(
            "cameraPositionInt",
            Value::Vec3(camera.floor().as_vec3().to_array()),
        );
        set(
            "previousCameraPosition",
            Value::Vec3(previous.camera.as_vec3().to_array()),
        );
        set(
            "previousCameraPositionFract",
            Value::Vec3(previous_fract.to_array()),
        );
        set("sunPosition", Value::Vec3(sun.to_array()));
        set("moonPosition", Value::Vec3(moon.to_array()));
        set(
            "shadowLightPosition",
            Value::Vec3(if day { sun } else { moon }.to_array()),
        );
        set("upPosition", Value::Vec3(up.to_array()));
        set("sunAngle", Value::Float((sky_angle + 0.25).fract()));
        set(
            "shadowAngle",
            Value::Float(
                if day {
                    sky_angle + 0.25
                } else {
                    sky_angle - 0.25
                }
                .rem_euclid(0.5),
            ),
        );
        set("worldTime", Value::Int((data.day_time % 24000) as i32));
        set("worldDay", Value::Int((data.day_time / 24000) as i32));
        set("moonPhase", Value::Int(i32::from(data.moon_phase)));
        set("frameCounter", Value::Int((frame_counter % 720_720) as i32));
        set("frameTimeCounter", Value::Float(frame_time));
        set("frameTime", Value::Float(1.0 / 60.0));
        set("rainStrength", Value::Float(data.rain));
        set("wetness", Value::Float(data.rain));
        set("viewWidth", Value::Float(w));
        set("viewHeight", Value::Float(h));
        set("aspectRatio", Value::Float(w / h.max(1.0)));
        set("near", Value::Float(data.near));
        set("far", Value::Float(data.far));
        set("fogColor", Value::Vec3(data.fog_color.to_array()));
        set("skyColor", Value::Vec3(data.sky_color.to_array()));
        set("fogStart", Value::Float(data.fog[0]));
        set("fogEnd", Value::Float(data.fog[1]));
        let eye_brightness = Value::IVec2(data.eye_light.map(|l| l as i32));
        set("eyeBrightness", eye_brightness);
        set("eyeBrightnessSmooth", eye_brightness);
        set("isEyeInWater", Value::Int(i32::from(data.eye_in_water)));
        set("blindness", Value::Float(0.0));
        set("darknessFactor", Value::Float(0.0));
        set("nightVision", Value::Float(0.0));
        set("screenBrightness", Value::Float(data.screen_brightness));
        set("heldBlockLightValue", Value::Int(0));
        set("heldBlockLightValue2", Value::Int(0));
        set("renderStage", Value::Int(0));
        set("atlasSize", Value::IVec2([0, 0]));
        set("gtexture", Value::Int(0));
        // TODO: the shadow pass's matrices, entity colour, held item ids,
        // biome and player-mood uniforms read zero until they're fed.
        set("shadowModelView", Value::Mat4(Mat4::IDENTITY));
        set("shadowModelViewInverse", Value::Mat4(Mat4::IDENTITY));
        set("shadowProjection", Value::Mat4(Mat4::IDENTITY));
        set("shadowProjectionInverse", Value::Mat4(Mat4::IDENTITY));
        Self(values)
    }

    /// `compiled`'s uniforms block, std140, from the values it declares.
    fn block(&self, compiled: &CompiledProgram) -> Vec<u8> {
        let mut bytes = vec![0u8; compiled.interface.uniform_size as usize];
        for uniform in &compiled.interface.uniforms {
            let Some(value) = self.0.get(uniform.name.as_str()) else {
                continue;
            };
            let at = uniform.offset as usize;
            let mut put = |data: &[u8]| {
                let end = (at + data.len()).min(bytes.len());
                bytes[at..end].copy_from_slice(&data[..end - at]);
            };
            match (uniform.ty, value) {
                (UniformType::Float, Value::Float(v)) => put(&v.to_le_bytes()),
                (UniformType::Float, Value::Int(v)) => put(&(*v as f32).to_le_bytes()),
                (UniformType::Int | UniformType::Bool, Value::Int(v)) => put(&v.to_le_bytes()),
                (UniformType::Int, Value::Float(v)) => put(&(*v as i32).to_le_bytes()),
                (UniformType::Vec2, Value::IVec2(v)) => {
                    put(bytemuck::cast_slice(&v.map(|c| c as f32)));
                }
                (UniformType::IVec2, Value::IVec2(v)) => put(bytemuck::cast_slice(v)),
                (UniformType::Vec3, Value::Vec3(v)) => put(bytemuck::cast_slice(v)),
                (UniformType::IVec3, Value::Vec3(v)) => {
                    put(bytemuck::cast_slice(&v.map(|c| c as i32)));
                }
                (UniformType::Mat4, Value::Mat4(m)) => {
                    put(bytemuck::cast_slice(&m.to_cols_array()))
                }
                _ => {}
            }
        }
        bytes
    }
}
