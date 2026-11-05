use glam::DVec3;

use crate::renderer::camera::CameraUniform;
use crate::renderer::{MAX_FRAMES_IN_FLIGHT, rhi, shader, util};
use crate::ui::font::{GLYPH_ATLAS_SIZE, GlyphMap};
use crate::ui::text::{TextSpan, font_spans};

pub const MAX_NAMEPLATE_VERTS: usize = 32768;
pub const NAME_TAG_SCALE: f32 = 0.025;

#[derive(Clone, Debug)]
pub struct NameplateRenderInfo {
    pub position: DVec3,
    pub spans: Vec<TextSpan>,
    pub discrete: bool,
    pub max_distance: f32,
}

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
pub struct NameplateVertex {
    pub position: [f32; 3],
    pub uv: [f32; 2],
    pub layer: f32,
    pub tex: f32,
    pub color: [f32; 4],
}

impl NameplateVertex {
    pub const BUFFER: rhi::VertexBufferLayout<'static> =
        rhi::vertex_buffer!(Self, [Float32x3, Float32x2, Float32, Float32, Float32x4]);
}

pub struct NameplatePipeline {
    depth_tested_pipeline: rhi::backend::RenderPipeline,
    see_through_pipeline: rhi::backend::RenderPipeline,
    pipeline_layout: rhi::backend::PipelineLayout,
    font_layout: rhi::backend::BindGroupLayout,
    camera_sets: Vec<rhi::backend::BindGroup>,
    camera_buffers: Vec<rhi::backend::Buffer>,
    font_set: rhi::backend::BindGroup,
    font_sampler: rhi::backend::Sampler,
    vertex_buffers: Vec<rhi::backend::Buffer>,
    see_through_counts: Vec<u32>,
    depth_tested_counts: Vec<u32>,
}

impl NameplatePipeline {
    pub fn new(
        device: &rhi::backend::Device,
        targets: rhi::backend::PipelineTargets,
        font_texture: rhi::backend::TextureHandle,
        font_color_texture: rhi::backend::TextureHandle,
        font_sampler: rhi::backend::Sampler,
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
        let pipeline_layout =
            rhi::backend::create_pipeline_layout(device, &[camera_layout, font_layout], None);

        let depth_tested_pipeline = create_pipeline(device, targets, pipeline_layout, true);
        let see_through_pipeline = create_pipeline(device, targets, pipeline_layout, false);

        let (camera_sets, camera_buffers) = util::create_uniform_sets(
            device,
            camera_layout,
            std::mem::size_of::<CameraUniform>() as u64,
            "nameplate_camera",
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
            (MAX_NAMEPLATE_VERTS * std::mem::size_of::<NameplateVertex>()) as u64,
            rhi::BufferUsages::VERTEX,
            "nameplate_vertices",
        );

        Self {
            depth_tested_pipeline,
            see_through_pipeline,
            pipeline_layout,
            font_layout,
            camera_sets,
            camera_buffers,
            font_set,
            font_sampler,
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
        self.font_sampler = font_sampler;
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

    pub fn prepare(
        &mut self,
        frame: usize,
        camera: &crate::renderer::camera::Camera,
        camera_render_pos: DVec3,
        anchor: DVec3,
        glyph_map: Option<&GlyphMap>,
        nameplates: &[NameplateRenderInfo],
    ) {
        self.see_through_counts[frame] = 0;
        self.depth_tested_counts[frame] = 0;

        let Some(glyph_map) = glyph_map else {
            return;
        };
        if nameplates.is_empty() {
            return;
        }

        let inv_atlas = 1.0 / GLYPH_ATLAS_SIZE as f32;
        let (right, up) = camera.billboard_axes();
        let forward = camera.forward();
        let away = forward * 0.002;

        let mut see_through_verts: Vec<NameplateVertex> = Vec::with_capacity(1024);
        let mut depth_tested_verts: Vec<NameplateVertex> = Vec::with_capacity(1024);

        for plate in nameplates {
            if see_through_verts.len() + depth_tested_verts.len() + 128 > MAX_NAMEPLATE_VERTS {
                break;
            }
            let dist_sq = (plate.position - camera_render_pos).length_squared();
            let max_d = plate.max_distance;
            if dist_sq > (max_d * max_d) as f64 || dist_sq < 0.01 {
                continue;
            }

            let to_cam = (camera_render_pos - plate.position).as_vec3();
            if to_cam.dot(forward) >= 0.0 {
                continue;
            }

            let spans = font_spans(&plate.spans);
            let text_w = spans_width(&spans, glyph_map);
            if text_w <= 0.0 {
                continue;
            }

            let half_w = text_w / 2.0;
            let start_x = -half_w;
            let line_y = 0.0f32;
            let center = (plate.position - anchor).as_vec3();

            let emit_bg = |verts: &mut Vec<NameplateVertex>, alpha: f32| {
                if verts.len() + 6 > MAX_NAMEPLATE_VERTS {
                    return;
                }
                let bx0 = start_x - 1.0;
                let bx1 = start_x + text_w + 1.0;
                let by0 = line_y - 1.0;
                let by1 = line_y + 9.0;
                let color = [0.0, 0.0, 0.0, alpha];

                let b0 = center + (right * bx0 - up * by0) * NAME_TAG_SCALE + away;
                let b1 = center + (right * bx1 - up * by0) * NAME_TAG_SCALE + away;
                let b2 = center + (right * bx1 - up * by1) * NAME_TAG_SCALE + away;
                let b3 = center + (right * bx0 - up * by1) * NAME_TAG_SCALE + away;

                let v0 = NameplateVertex {
                    position: b0.into(),
                    uv: [0.0, 0.0],
                    layer: 0.0,
                    tex: -1.0,
                    color,
                };
                let v1 = NameplateVertex {
                    position: b1.into(),
                    uv: [0.0, 0.0],
                    layer: 0.0,
                    tex: -1.0,
                    color,
                };
                let v2 = NameplateVertex {
                    position: b2.into(),
                    uv: [0.0, 0.0],
                    layer: 0.0,
                    tex: -1.0,
                    color,
                };
                let v3 = NameplateVertex {
                    position: b3.into(),
                    uv: [0.0, 0.0],
                    layer: 0.0,
                    tex: -1.0,
                    color,
                };

                verts.push(v0);
                verts.push(v1);
                verts.push(v3);
                verts.push(v1);
                verts.push(v2);
                verts.push(v3);
            };

            let emit_text = |verts: &mut Vec<NameplateVertex>, alpha_scale: f32| {
                let mut cur_x = start_x;
                for span in spans.iter() {
                    let mut col = span.color;
                    col[3] *= alpha_scale;
                    let font = span.font.as_deref();

                    for ch in span.text.chars() {
                        let gi = glyph_map.glyph(ch, font);
                        if gi.pixel_w > 0 && gi.pixel_h > 0 {
                            let draw_instances = if span.bold {
                                [Some(0.0), Some(gi.bold_offset)]
                            } else {
                                [Some(0.0), None]
                            };

                            for offset in draw_instances.into_iter().flatten() {
                                if verts.len() + 6 > MAX_NAMEPLATE_VERTS {
                                    return;
                                }
                                let sx = cur_x + gi.left + offset;
                                let sy = line_y + gi.top;
                                let w = gi.draw_w;
                                let h = gi.draw_h;

                                let u0 = gi.atlas_x as f32 * inv_atlas;
                                let v0 = gi.atlas_y as f32 * inv_atlas;
                                let u1 = (gi.atlas_x + gi.pixel_w) as f32 * inv_atlas;
                                let v1 = (gi.atlas_y + gi.pixel_h) as f32 * inv_atlas;

                                let layer = gi.atlas_layer as f32;
                                let tex = if gi.colored { 1.0 } else { 0.0 };

                                let p0 = center + (right * sx - up * sy) * NAME_TAG_SCALE;
                                let p1 = center + (right * (sx + w) - up * sy) * NAME_TAG_SCALE;
                                let p2 =
                                    center + (right * (sx + w) - up * (sy + h)) * NAME_TAG_SCALE;
                                let p3 = center + (right * sx - up * (sy + h)) * NAME_TAG_SCALE;

                                let vert0 = NameplateVertex {
                                    position: p0.into(),
                                    uv: [u0, v0],
                                    layer,
                                    tex,
                                    color: col,
                                };
                                let vert1 = NameplateVertex {
                                    position: p1.into(),
                                    uv: [u1, v0],
                                    layer,
                                    tex,
                                    color: col,
                                };
                                let vert2 = NameplateVertex {
                                    position: p2.into(),
                                    uv: [u1, v1],
                                    layer,
                                    tex,
                                    color: col,
                                };
                                let vert3 = NameplateVertex {
                                    position: p3.into(),
                                    uv: [u0, v1],
                                    layer,
                                    tex,
                                    color: col,
                                };

                                verts.push(vert0);
                                verts.push(vert1);
                                verts.push(vert3);
                                verts.push(vert1);
                                verts.push(vert2);
                                verts.push(vert3);
                            }
                        }
                        cur_x += gi.advance;
                        if span.bold {
                            cur_x += gi.bold_offset;
                        }
                    }
                }
            };

            if plate.discrete {
                emit_bg(&mut depth_tested_verts, 0.25);
                emit_text(&mut depth_tested_verts, 0.5);
            } else {
                emit_bg(&mut see_through_verts, 0.25);
                emit_text(&mut see_through_verts, 0.5);
                emit_text(&mut depth_tested_verts, 1.0);
            }
        }

        let see_through_count = see_through_verts.len() as u32;
        let depth_tested_count = depth_tested_verts.len() as u32;
        let total_verts = see_through_verts.len() + depth_tested_verts.len();
        if total_verts == 0 || total_verts > MAX_NAMEPLATE_VERTS {
            return;
        }

        let mut combined = Vec::with_capacity(total_verts);
        combined.extend(see_through_verts);
        combined.extend(depth_tested_verts);

        let bytes = bytemuck::cast_slice(&combined);
        self.vertex_buffers[frame].write(bytes);

        self.see_through_counts[frame] = see_through_count;
        self.depth_tested_counts[frame] = depth_tested_count;
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
            pass.set_bind_groups(0, &[self.camera_sets[frame], self.font_set]);
            pass.draw(see_count, 1, 0, 0);
        }

        if depth_count > 0 {
            pass.set_pipeline(&self.depth_tested_pipeline);
            pass.set_bind_groups(0, &[self.camera_sets[frame], self.font_set]);
            pass.draw(depth_count, 1, see_count, 0);
        }
    }

    pub fn destroy(&mut self, device: &rhi::backend::Device) {
        util::destroy_buffers(device, &mut self.camera_buffers);
        util::destroy_buffers(device, &mut self.vertex_buffers);
        self.depth_tested_pipeline.destroy(device);
        self.see_through_pipeline.destroy(device);
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
                    "nameplate_depth"
                } else {
                    "nameplate_see_through"
                },
                shader::include_shader!("nameplate.vert"),
                shader::include_shader!("nameplate.frag"),
                &[NameplateVertex::BUFFER],
            )
        },
    )
}

fn spans_width(spans: &[TextSpan], glyph_map: &GlyphMap) -> f32 {
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
    w.ceil()
}
