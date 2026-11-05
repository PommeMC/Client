use std::f32::consts::PI;

use glam::{DVec3, Mat4, Vec3};

use crate::renderer::camera::CameraUniform;
use crate::renderer::pipelines::lightmap::Lightmap;
use crate::renderer::{MAX_FRAMES_IN_FLIGHT, rhi, shader, util};
use crate::ui::font::{GLYPH_ATLAS_SIZE, GlyphMap};
use crate::world::block_entity::SignRenderInfo;

pub const MAX_SIGN_VERTS: usize = 65536;

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
pub struct SignTextVertex {
    pub position: [f32; 3],
    pub uv: [f32; 2],
    pub layer: f32,
    pub tex: f32,
    pub color: [f32; 4],
    pub light: f32,
}

impl SignTextVertex {
    pub const BUFFER: rhi::VertexBufferLayout<'static> = rhi::vertex_buffer!(
        Self,
        [Float32x3, Float32x2, Float32, Float32, Float32x4, Float32]
    );
}

fn get_dark_color(color_rgb: u32, glowing: bool) -> [f32; 4] {
    if color_rgb == 0 && glowing {
        [0.941, 0.922, 0.8, 1.0]
    } else {
        let r = ((color_rgb >> 16) & 0xff) as f32 / 255.0 * 0.4;
        let g = ((color_rgb >> 8) & 0xff) as f32 / 255.0 * 0.4;
        let b = (color_rgb & 0xff) as f32 / 255.0 * 0.4;
        [r, g, b, 1.0]
    }
}

fn rgb_to_vec4(rgb: u32) -> [f32; 4] {
    let r = ((rgb >> 16) & 0xff) as f32 / 255.0;
    let g = ((rgb >> 8) & 0xff) as f32 / 255.0;
    let b = (rgb & 0xff) as f32 / 255.0;
    [r, g, b, 1.0]
}

pub struct SignTextPipeline {
    pipeline: rhi::backend::RenderPipeline,
    pipeline_layout: rhi::backend::PipelineLayout,
    font_layout: rhi::backend::BindGroupLayout,
    camera_sets: Vec<rhi::backend::BindGroup>,
    camera_buffers: Vec<rhi::backend::Buffer>,
    font_set: rhi::backend::BindGroup,
    font_sampler: rhi::backend::Sampler,
    lightmap_set: rhi::backend::BindGroup,
    vertex_buffers: Vec<rhi::backend::Buffer>,
    vertex_counts: Vec<u32>,
}

impl SignTextPipeline {
    pub fn new(
        device: &rhi::backend::Device,
        targets: rhi::backend::PipelineTargets,
        lightmap: &Lightmap,
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
        let pipeline_layout = rhi::backend::create_pipeline_layout(
            device,
            &[camera_layout, font_layout, lightmap.layout],
            None,
        );

        let pipeline = create_pipeline(device, targets, pipeline_layout);

        let (camera_sets, camera_buffers) = util::create_uniform_sets(
            device,
            camera_layout,
            std::mem::size_of::<CameraUniform>() as u64,
            "sign_text_camera",
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
            (MAX_SIGN_VERTS * std::mem::size_of::<SignTextVertex>()) as u64,
            rhi::BufferUsages::VERTEX,
            "sign_text_vertices",
        );

        Self {
            pipeline,
            pipeline_layout,
            font_layout,
            camera_sets,
            camera_buffers,
            font_set,
            font_sampler,
            lightmap_set: lightmap.group,
            vertex_buffers,
            vertex_counts: vec![0; MAX_FRAMES_IN_FLIGHT],
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
        self.pipeline.destroy(device);
        self.pipeline = create_pipeline(device, targets, self.pipeline_layout);
    }

    pub fn prepare(
        &mut self,
        frame: usize,
        anchor: DVec3,
        camera_eye: DVec3,
        glyph_map: Option<&GlyphMap>,
        signs: &[SignRenderInfo],
    ) {
        self.vertex_counts[frame] = 0;
        let Some(glyph_map) = glyph_map else {
            return;
        };
        if signs.is_empty() {
            return;
        }

        let inv_atlas = 1.0 / GLYPH_ATLAS_SIZE as f32;
        let mut verts: Vec<SignTextVertex> = Vec::with_capacity(4096);

        let s = Vec3::new(1.0, -1.0, 1.0);

        for sign in signs {
            let block_center = DVec3::new(
                sign.pos.x as f64 + 0.5,
                sign.pos.y as f64 + 0.5,
                sign.pos.z as f64 + 0.5,
            );
            let dist_sq = block_center.distance_squared(camera_eye);
            if dist_sq > 32.0 * 32.0 {
                continue;
            }

            let block_rel = (DVec3::new(sign.pos.x as f64, sign.pos.y as f64, sign.pos.z as f64)
                - anchor)
                .as_vec3();
            let base = Mat4::from_translation(block_rel);

            let (line_height, midpoint, max_width) = if sign.is_hanging {
                (9.0f32, 18.0f32, 60.0f32)
            } else {
                (10.0f32, 20.0f32, 90.0f32)
            };

            for (is_front, side) in [(true, &sign.front), (false, &sign.back)] {
                if side.is_empty() {
                    continue;
                }

                let mut m = base;
                if sign.is_hanging {
                    m *= Mat4::from_translation(Vec3::new(0.5, 0.9375, 0.5))
                        * Mat4::from_rotation_y((-sign.angle).to_radians())
                        * Mat4::from_translation(Vec3::new(0.0, -0.3125, 0.0));
                    if !is_front {
                        m *= Mat4::from_rotation_y(PI);
                    }
                    m *= Mat4::from_translation(Vec3::new(0.0, -0.32, 0.076))
                        * Mat4::from_scale(s * 0.014_062_5);
                } else {
                    m *= Mat4::from_translation(Vec3::splat(0.5))
                        * Mat4::from_rotation_y((-sign.angle).to_radians());
                    if sign.is_wall {
                        m *= Mat4::from_translation(Vec3::new(0.0, -0.3125, -0.4375));
                    }
                    if !is_front {
                        m *= Mat4::from_rotation_y(PI);
                    }
                    m *= Mat4::from_translation(Vec3::new(0.0, 1.0 / 3.0, 0.052))
                        * Mat4::from_scale(s * 0.010_416_667);
                }

                let normal_world = m.transform_vector3(Vec3::new(0.0, 0.0, 1.0)).normalize();
                let face_center = m.transform_point3(Vec3::ZERO);
                let to_cam = ((camera_eye - anchor).as_vec3() - face_center).normalize();
                if normal_world.dot(to_cam) <= 0.0 {
                    continue;
                }

                let default_dark = get_dark_color(side.color_rgb, side.glowing);

                for (line_idx, line) in side.messages.iter().enumerate() {
                    if line.text.is_empty() {
                        continue;
                    }

                    let effective_rgb = line.color_rgb.unwrap_or(side.color_rgb);
                    let (text_color, outline_color, light_val) = if side.glowing {
                        let c = rgb_to_vec4(effective_rgb);
                        let outline = Some(get_dark_color(effective_rgb, true));
                        (
                            c,
                            outline,
                            crate::renderer::chunk::block_lighter::LightLevels::FULL_BRIGHT.lane(),
                        )
                    } else {
                        let c = if effective_rgb == 0 {
                            default_dark
                        } else {
                            rgb_to_vec4(effective_rgb)
                        };
                        (c, None, sign.light.lane())
                    };

                    let shown = first_line(&line.text, max_width, glyph_map);
                    let line_w = text_width(&shown, glyph_map);
                    let start_x = -((line_w as i32 / 2) as f32);
                    let y0 = line_idx as f32 * line_height - midpoint;

                    let mut emit_glyph_mesh = |dx: f32, dy: f32, z: f32, color: [f32; 4]| {
                        let mut cur_x = start_x + dx;
                        let line_y = y0 + dy;
                        for ch in shown.chars() {
                            if verts.len() + 6 > MAX_SIGN_VERTS {
                                return;
                            }
                            let gi = glyph_map.glyph(ch, None);
                            if gi.pixel_w > 0 && gi.pixel_h > 0 {
                                let sx = cur_x + gi.left;
                                let sy = line_y + gi.top;
                                let w = gi.draw_w;
                                let h = gi.draw_h;

                                let u0 = gi.atlas_x as f32 * inv_atlas;
                                let v0 = gi.atlas_y as f32 * inv_atlas;
                                let u1 = (gi.atlas_x + gi.pixel_w) as f32 * inv_atlas;
                                let v1 = (gi.atlas_y + gi.pixel_h) as f32 * inv_atlas;

                                let layer = gi.atlas_layer as f32;
                                let tex = if gi.colored { 1.0 } else { 0.0 };

                                let p0 = m.transform_point3(Vec3::new(sx, sy, z));
                                let p1 = m.transform_point3(Vec3::new(sx + w, sy, z));
                                let p2 = m.transform_point3(Vec3::new(sx + w, sy + h, z));
                                let p3 = m.transform_point3(Vec3::new(sx, sy + h, z));

                                let v_0 = SignTextVertex {
                                    position: p0.into(),
                                    uv: [u0, v0],
                                    layer,
                                    tex,
                                    color,
                                    light: light_val,
                                };
                                let v_1 = SignTextVertex {
                                    position: p1.into(),
                                    uv: [u1, v0],
                                    layer,
                                    tex,
                                    color,
                                    light: light_val,
                                };
                                let v_2 = SignTextVertex {
                                    position: p2.into(),
                                    uv: [u1, v1],
                                    layer,
                                    tex,
                                    color,
                                    light: light_val,
                                };
                                let v_3 = SignTextVertex {
                                    position: p3.into(),
                                    uv: [u0, v1],
                                    layer,
                                    tex,
                                    color,
                                    light: light_val,
                                };

                                verts.push(v_0);
                                verts.push(v_1);
                                verts.push(v_3);
                                verts.push(v_1);
                                verts.push(v_2);
                                verts.push(v_3);
                            }
                            cur_x += gi.advance;
                        }
                    };

                    if let Some(out_col) = outline_color {
                        for (ox, oy) in [
                            (-1.0, -1.0),
                            (0.0, -1.0),
                            (1.0, -1.0),
                            (-1.0, 0.0),
                            (1.0, 0.0),
                            (-1.0, 1.0),
                            (0.0, 1.0),
                            (1.0, 1.0),
                        ] {
                            emit_glyph_mesh(ox, oy, 0.0, out_col);
                        }
                    }

                    let z_offset = if outline_color.is_some() { 0.1 } else { 0.0 };
                    emit_glyph_mesh(0.0, 0.0, z_offset, text_color);
                }
            }
        }

        if !verts.is_empty() {
            let bytes = bytemuck::cast_slice(&verts);
            self.vertex_buffers[frame].write(bytes);
            self.vertex_counts[frame] = verts.len() as u32;
        }
    }

    pub fn draw(&self, pass: &mut rhi::backend::RenderPassEncoder, frame: usize) {
        let count = self.vertex_counts[frame];
        if count == 0 {
            return;
        }
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_groups(
            0,
            &[self.camera_sets[frame], self.font_set, self.lightmap_set],
        );
        pass.set_vertex_buffer(0, self.vertex_buffers[frame].handle());
        pass.draw(count, 1, 0, 0);
    }

    pub fn destroy(&mut self, device: &rhi::backend::Device) {
        util::destroy_buffers(device, &mut self.camera_buffers);
        util::destroy_buffers(device, &mut self.vertex_buffers);
        self.pipeline.destroy(device);
    }
}

fn create_pipeline(
    device: &rhi::backend::Device,
    targets: rhi::backend::PipelineTargets,
    layout: rhi::backend::PipelineLayout,
) -> rhi::backend::RenderPipeline {
    rhi::backend::create_render_pipeline(
        device,
        targets,
        layout,
        &rhi::RenderPipelineDesc {
            cull_mode: rhi::CullMode::None,
            depth: Some(rhi::DepthState::READ_ONLY),
            blend: Some(rhi::BlendState::TRANSLUCENT),
            ..rhi::RenderPipelineDesc::new(
                "sign_text",
                shader::include_shader!("sign_text.vert"),
                shader::include_shader!("sign_text.frag"),
                &[SignTextVertex::BUFFER],
            )
        },
    )
}

fn first_line(line: &str, max_width: f32, glyph_map: &GlyphMap) -> String {
    let mut width = 0.0f32;
    let mut last_space = None;
    for (i, c) in line.char_indices() {
        if c == '\n' {
            return line[..i].to_owned();
        }
        if c == ' ' {
            last_space = Some(i);
        }
        let adv = glyph_map.glyph(c, None).advance;
        width += adv;
        if width > max_width {
            let end = match last_space {
                Some(s) if s > 0 => s,
                _ if i > 0 => i,
                _ => c.len_utf8(),
            };
            return line[..end].to_owned();
        }
    }
    line.to_owned()
}

fn text_width(line: &str, glyph_map: &GlyphMap) -> f32 {
    line.chars()
        .map(|c| glyph_map.glyph(c, None).advance)
        .sum::<f32>()
        .ceil()
}
