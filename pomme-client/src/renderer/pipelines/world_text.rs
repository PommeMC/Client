//! Text drawn in the world, vanilla's `rendertype_text` family: name tags
//! ([`super::nameplate`]) and sign text ([`super::sign_text`]) build their
//! quads with [`push_spans`] and draw through one [`WorldTextPipeline`].

use glam::Vec3;

use crate::renderer::camera::CameraUniform;
use crate::renderer::pipelines::lightmap::Lightmap;
use crate::renderer::{MAX_FRAMES_IN_FLIGHT, rhi, shader, util};
use crate::ui::font::{GLYPH_ATLAS_SIZE, GlyphMap};
use crate::ui::text::TextSpan;

pub const MAX_WORLD_TEXT_VERTS: usize = 65536;

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
pub struct WorldTextVertex {
    pub position: [f32; 3],
    pub uv: [f32; 2],
    pub layer: f32,
    pub tex: f32,
    pub color: [f32; 4],
    /// The light lane (`LightLevels::lane`).
    pub light: f32,
}

impl WorldTextVertex {
    pub const BUFFER: rhi::VertexBufferLayout<'static> = rhi::vertex_buffer!(
        Self,
        [Float32x3, Float32x2, Float32, Float32, Float32x4, Float32]
    );
}

/// One frame's text: drawn over terrain, then depth-tested.
#[derive(Default)]
pub struct WorldTextVertices {
    pub see_through: Vec<WorldTextVertex>,
    pub depth_tested: Vec<WorldTextVertex>,
}

impl WorldTextVertices {
    pub(crate) fn len(&self) -> usize {
        self.see_through.len() + self.depth_tested.len()
    }
}

pub struct WorldTextPipeline {
    depth_tested_pipeline: rhi::backend::RenderPipeline,
    see_through_pipeline: rhi::backend::RenderPipeline,
    pipeline_layout: rhi::backend::PipelineLayout,
    camera_layout: rhi::backend::BindGroupLayout,
    font_layout: rhi::backend::BindGroupLayout,
    camera_sets: Vec<rhi::backend::BindGroup>,
    camera_buffers: Vec<rhi::backend::Buffer>,
    font_set: rhi::backend::BindGroup,
    /// The shared [`Lightmap`]'s group, set 2.
    lightmap_set: rhi::backend::BindGroup,
    vertex_buffers: Vec<rhi::backend::Buffer>,
    see_through_counts: Vec<u32>,
    depth_tested_counts: Vec<u32>,
}

impl WorldTextPipeline {
    pub fn new(
        device: &rhi::backend::Device,
        targets: rhi::backend::PipelineTargets,
        font_texture: rhi::backend::TextureHandle,
        font_color_texture: rhi::backend::TextureHandle,
        font_sampler: rhi::backend::Sampler,
        lightmap: &Lightmap,
    ) -> Self {
        let camera_layout = rhi::backend::create_bind_group_layout(device, util::VERTEX_UNIFORM);
        let font_layout = rhi::backend::create_bind_group_layout(
            device,
            &[
                rhi::BindGroupLayoutEntry::texture(rhi::ShaderStages::FRAGMENT),
                rhi::BindGroupLayoutEntry::texture(rhi::ShaderStages::FRAGMENT),
                rhi::BindGroupLayoutEntry::sampler(rhi::ShaderStages::FRAGMENT),
            ],
        );
        let pipeline_layout = rhi::backend::create_pipeline_layout(
            device,
            &[camera_layout, font_layout, lightmap.layout],
            None,
        );

        let depth_tested_pipeline = create_pipeline(device, targets, pipeline_layout, true);
        let see_through_pipeline = create_pipeline(device, targets, pipeline_layout, false);

        let (camera_sets, camera_buffers) = util::create_uniform_sets(
            device,
            camera_layout,
            std::mem::size_of::<CameraUniform>() as u64,
            "world_text_camera",
        );

        let font_set = device.create_bind_group(
            font_layout,
            &[
                (0, rhi::backend::BindingResource::Texture(font_texture)),
                (
                    1,
                    rhi::backend::BindingResource::Texture(font_color_texture),
                ),
                (2, rhi::backend::BindingResource::Sampler(font_sampler)),
            ],
        );

        let vertex_buffers = util::create_per_frame_host_buffers(
            device,
            (MAX_WORLD_TEXT_VERTS * std::mem::size_of::<WorldTextVertex>()) as u64,
            rhi::BufferUsages::VERTEX,
            "world_text_vertices",
        );

        Self {
            depth_tested_pipeline,
            see_through_pipeline,
            pipeline_layout,
            camera_layout,
            font_layout,
            camera_sets,
            camera_buffers,
            font_set,
            lightmap_set: lightmap.group,
            vertex_buffers,
            see_through_counts: vec![0; MAX_FRAMES_IN_FLIGHT],
            depth_tested_counts: vec![0; MAX_FRAMES_IN_FLIGHT],
        }
    }

    pub fn rebind_font(
        &mut self,
        device: &rhi::backend::Device,
        font_texture: rhi::backend::TextureHandle,
        font_color_texture: rhi::backend::TextureHandle,
        font_sampler: rhi::backend::Sampler,
    ) {
        self.font_set.replace(
            device,
            self.font_layout,
            &[
                (0, rhi::backend::BindingResource::Texture(font_texture)),
                (
                    1,
                    rhi::backend::BindingResource::Texture(font_color_texture),
                ),
                (2, rhi::backend::BindingResource::Sampler(font_sampler)),
            ],
        );
    }

    pub fn update_camera(&mut self, frame: usize, uniform: &CameraUniform) {
        let bytes = bytemuck::bytes_of(uniform);
        self.camera_buffers[frame].write(bytes);
    }

    pub fn recreate_pipeline(
        &mut self,
        device: &rhi::backend::Device,
        targets: rhi::backend::PipelineTargets,
    ) {
        self.depth_tested_pipeline.destroy(device);
        self.see_through_pipeline.destroy(device);
        self.depth_tested_pipeline = create_pipeline(device, targets, self.pipeline_layout, true);
        self.see_through_pipeline = create_pipeline(device, targets, self.pipeline_layout, false);
    }

    /// Uploads this frame's vertices, see-through first.
    pub fn prepare(&mut self, frame: usize, verts: WorldTextVertices) {
        self.see_through_counts[frame] = 0;
        self.depth_tested_counts[frame] = 0;
        if verts.len() == 0 || verts.len() > MAX_WORLD_TEXT_VERTS {
            return;
        }
        self.see_through_counts[frame] = verts.see_through.len() as u32;
        self.depth_tested_counts[frame] = verts.depth_tested.len() as u32;
        let mut combined = verts.see_through;
        combined.extend(verts.depth_tested);
        self.vertex_buffers[frame].write(bytemuck::cast_slice(&combined));
    }

    pub fn draw(&self, pass: &mut rhi::backend::RenderPassEncoder, frame: usize) {
        let see_count = self.see_through_counts[frame];
        let depth_count = self.depth_tested_counts[frame];

        if see_count == 0 && depth_count == 0 {
            return;
        }

        pass.set_vertex_buffer(0, self.vertex_buffers[frame].handle());

        if see_count > 0 {
            pass.set_pipeline(&self.see_through_pipeline);
            pass.set_bind_groups(
                0,
                &[self.camera_sets[frame], self.font_set, self.lightmap_set],
            );
            pass.draw(see_count, 1, 0, 0);
        }

        if depth_count > 0 {
            pass.set_pipeline(&self.depth_tested_pipeline);
            pass.set_bind_groups(
                0,
                &[self.camera_sets[frame], self.font_set, self.lightmap_set],
            );
            pass.draw(depth_count, 1, see_count, 0);
        }
    }

    pub fn destroy(&mut self, device: &rhi::backend::Device) {
        util::destroy_buffers(device, &mut self.camera_buffers);
        util::destroy_buffers(device, &mut self.vertex_buffers);
        self.depth_tested_pipeline.destroy(device);
        self.see_through_pipeline.destroy(device);
        self.pipeline_layout.destroy(device);
        util::destroy_bind_groups(device, self.camera_sets.drain(..));
        self.font_set.destroy(device);
        self.camera_layout.destroy(device);
        self.font_layout.destroy(device);
    }
}

fn create_pipeline(
    device: &rhi::backend::Device,
    targets: rhi::backend::PipelineTargets,
    layout: rhi::backend::PipelineLayout,
    depth_tested: bool,
) -> rhi::backend::RenderPipeline {
    let depth = if depth_tested {
        Some(rhi::DepthState::READ_ONLY)
    } else {
        None
    };

    rhi::backend::create_render_pipeline(
        device,
        targets,
        layout,
        &rhi::RenderPipelineDesc {
            cull_mode: rhi::CullMode::None,
            depth,
            blend: Some(rhi::BlendState::TRANSLUCENT),
            ..rhi::RenderPipelineDesc::new(
                if depth_tested {
                    "world_text_depth"
                } else {
                    "world_text_see_through"
                },
                shader::include_shader!("world_text.vert"),
                shader::include_shader!("world_text.frag"),
                &[WorldTextVertex::BUFFER],
            )
        },
    )
}

/// Two triangles over `corners` (clockwise from the top-left), with the
/// glyph UVs `[u0, v0, u1, v1]`.
pub(crate) fn push_quad(
    verts: &mut Vec<WorldTextVertex>,
    corners: [Vec3; 4],
    [u0, v0, u1, v1]: [f32; 4],
    layer: f32,
    tex: f32,
    color: [f32; 4],
    light: f32,
) {
    if verts.len() + 6 > MAX_WORLD_TEXT_VERTS {
        return;
    }
    let vertex = |corner: usize, uv: [f32; 2]| WorldTextVertex {
        position: corners[corner].into(),
        uv,
        layer,
        tex,
        color,
        light,
    };
    let quad = [
        vertex(0, [u0, v0]),
        vertex(1, [u1, v0]),
        vertex(2, [u1, v1]),
        vertex(3, [u0, v1]),
    ];
    verts.extend([0, 1, 3, 1, 2, 3].map(|i| quad[i]));
}

/// The glyphs of `spans` from text x `start_x` on a line whose top is text
/// y 0; `at` maps text space (x right, y down) to world space and `color`
/// gives each span's colour.
pub(crate) fn push_spans(
    verts: &mut Vec<WorldTextVertex>,
    glyph_map: &GlyphMap,
    spans: &[TextSpan],
    start_x: f32,
    at: &dyn Fn(f32, f32) -> Vec3,
    color: &dyn Fn(&TextSpan) -> [f32; 4],
    light: f32,
) {
    let inv_atlas = 1.0 / GLYPH_ATLAS_SIZE as f32;
    let mut cur_x = start_x;
    for span in spans {
        let color = color(span);
        let font = span.font.as_deref();
        for ch in span.text.chars() {
            let gi = glyph_map.glyph(ch, font);
            if gi.pixel_w > 0 && gi.pixel_h > 0 {
                let offsets: &[f32] = if span.bold {
                    &[0.0, gi.bold_offset]
                } else {
                    &[0.0]
                };
                let uv = [
                    gi.atlas_x as f32 * inv_atlas,
                    gi.atlas_y as f32 * inv_atlas,
                    (gi.atlas_x + gi.pixel_w) as f32 * inv_atlas,
                    (gi.atlas_y + gi.pixel_h) as f32 * inv_atlas,
                ];
                let tex = if gi.colored { 1.0 } else { 0.0 };
                for &offset in offsets {
                    let x = cur_x + gi.left + offset;
                    let y = gi.top;
                    let corners = [
                        at(x, y),
                        at(x + gi.draw_w, y),
                        at(x + gi.draw_w, y + gi.draw_h),
                        at(x, y + gi.draw_h),
                    ];
                    push_quad(verts, corners, uv, gi.atlas_layer as f32, tex, color, light);
                }
            }
            cur_x += gi.advance;
            if span.bold {
                cur_x += gi.bold_offset;
            }
        }
    }
}

/// `Font.width`.
pub(crate) fn spans_width(spans: &[TextSpan], glyph_map: &GlyphMap) -> f32 {
    spans_advance(spans, glyph_map).ceil()
}

/// `StringSplitter.stringWidth`: the unrounded advance.
pub(crate) fn spans_advance(spans: &[TextSpan], glyph_map: &GlyphMap) -> f32 {
    let mut w = 0.0f32;
    for span in spans {
        let font = span.font.as_deref();
        for ch in span.text.chars() {
            let gi = glyph_map.glyph(ch, font);
            w += gi.advance;
            if span.bold {
                w += gi.bold_offset;
            }
        }
    }
    w
}
