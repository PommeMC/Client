//! The enchanting table's 3D book (vanilla `BookModel` +
//! `GuiBookModelRenderer`), drawn into a scissored GUI box like the
//! inventory's player preview. The seven model parts are posed on the CPU
//! each frame and written to a per-frame vertex buffer; the uniform holds the
//! screen-space projection.

use glam::{Mat4, Vec3, Vec4};

use super::entity_renderer::WHITE_TINT;
use super::skin_preview::{
    PREVIEW_ALPHA_CUTOFF, Uniform, Vertex, create_pipeline, preview_push_constant_range,
    push_preview_style, write_uniform,
};
use crate::resources::{ResourceId, ResourceManager, resource_id};

const BOOK_TEXTURE: ResourceId =
    resource_id!("minecraft:textures/entity/enchantment/enchanting_table_book.png");
use crate::renderer::{BookPreview, rhi, util};

/// Base mesh of one book part, posed per frame.
struct Part {
    verts: Vec<Vertex>,
    /// Pose offset in model pixels (vanilla `PartPose.offset`).
    offset: Vec3,
}

pub struct BookPreviewPipeline {
    pipeline: rhi::backend::RenderPipeline,
    pipeline_layout: rhi::backend::PipelineLayout,
    mvp_layout: rhi::backend::BindGroupLayout,
    tex_layout: rhi::backend::BindGroupLayout,
    mvp_sets: Vec<rhi::backend::BindGroup>,
    mvp_buffers: Vec<rhi::backend::Buffer>,
    tex_set: rhi::backend::BindGroup,
    texture: rhi::backend::Texture,
    sampler: rhi::backend::Sampler,
    vertex_buffers: Vec<rhi::backend::Buffer>,
    parts: Vec<Part>,
    vert_count: u32,
}

impl BookPreviewPipeline {
    pub fn new(
        device: &rhi::backend::Device,
        targets: rhi::backend::PipelineTargets,
        resources: &ResourceManager,
    ) -> Self {
        let mvp_layout = rhi::backend::create_bind_group_layout(device, util::VERTEX_UNIFORM);
        let tex_layout = rhi::backend::create_bind_group_layout(device, util::FRAGMENT_TEXTURE);
        let pipeline_layout = rhi::backend::create_pipeline_layout(
            device,
            &[mvp_layout, tex_layout],
            Some(preview_push_constant_range()),
        );

        let pipeline = create_pipeline(device, targets, pipeline_layout);

        let (mvp_sets, mvp_buffers) = util::create_uniform_sets(
            device,
            mvp_layout,
            std::mem::size_of::<Uniform>() as u64,
            "book_mvp",
        );

        let texture = load_book_texture(device, resources);

        let sampler = rhi::backend::create_sampler(device, &rhi::SamplerDesc::NEAREST);
        let tex_set = util::create_texture_group(device, tex_layout, texture.handle(), sampler);

        let parts = build_parts();
        let vert_count: u32 = parts.iter().map(|p| p.verts.len() as u32).sum();
        let vertex_buffers = util::create_per_frame_host_buffers(
            device,
            (vert_count as usize * size_of::<Vertex>()) as u64,
            rhi::BufferUsages::VERTEX,
            "book_vertices",
        );

        Self {
            pipeline,
            pipeline_layout,
            mvp_layout,
            tex_layout,
            mvp_sets,
            mvp_buffers,
            tex_set,
            texture,
            sampler,
            vertex_buffers,
            parts,
            vert_count,
        }
    }

    /// The caller must idle the device before replacing the shared texture set.
    pub fn reload_texture(&mut self, device: &rhi::backend::Device, resources: &ResourceManager) {
        let texture = load_book_texture(device, resources);
        self.tex_set.replace(
            device,
            self.tex_layout,
            &util::texture_binding(texture.handle(), self.sampler),
        );
        std::mem::replace(&mut self.texture, texture).destroy(device);
    }

    /// Draws the book in its GUI box, replicating vanilla's picture-in-picture
    /// setup: origin at (box center x, box top + 17 GUI px), 40 GUI px per
    /// unit, then `GuiBookModelRenderer`'s pose chain and `BookModel`'s
    /// `setupAnim`.
    pub fn draw_in_box(
        &mut self,
        pass: &mut rhi::backend::RenderPassEncoder,
        frame: usize,
        p: BookPreview,
        sw: f32,
        sh: f32,
    ) {
        let s = 40.0 * p.gui_scale;
        // Screen-space ortho: x/y in framebuffer pixels (y down, matching
        // vanilla's invertY ortho), z to depth like vanilla's [-1000, 1000]
        // range, negated so vanilla's reversed-GEQUAL test becomes LESS here.
        let ax = 2.0 * s / sw;
        let ay = 2.0 * s / sh;
        let bx = (p.rect[0] + p.rect[2] / 2.0) / sw * 2.0 - 1.0;
        let by = (p.rect[1] + 17.0 * p.gui_scale) / sh * 2.0 - 1.0;
        let clip = Mat4::from_cols(
            Vec4::new(ax, 0.0, 0.0, 0.0),
            Vec4::new(0.0, ay, 0.0, 0.0),
            Vec4::new(0.0, 0.0, s / 2000.0, 0.0),
            Vec4::new(bx, by, 0.5, 1.0),
        );
        write_uniform(&mut self.mvp_buffers[frame], &clip);

        // GuiBookModelRenderer.renderToTexture's pose chain.
        let open = p.open;
        let pose = Mat4::from_rotation_y(180.0f32.to_radians())
            * Mat4::from_rotation_x(25.0f32.to_radians())
            * Mat4::from_translation(Vec3::new(
                (1.0 - open) * 0.2,
                (1.0 - open) * 0.1,
                (1.0 - open) * 0.25,
            ))
            * Mat4::from_rotation_y((-(1.0 - open) * 90.0 - 90.0).to_radians())
            * Mat4::from_rotation_x(180.0f32.to_radians());

        // BookModel.setupAnim: openness = 1.25 * open (State.forAnimation with
        // progress 0), page flips from the fractional flip phases.
        let o = 1.25 * open;
        let frac = |v: f32| v - v.floor();
        let page1 = (frac(p.flip + 0.25) * 1.6 - 0.3).clamp(0.0, 1.0);
        let page2 = (frac(p.flip + 0.75) * 1.6 - 0.3).clamp(0.0, 1.0);
        let page_x = o.sin();
        let y_rots = [
            std::f32::consts::PI + o,    // left_lid
            -o,                          // right_lid
            std::f32::consts::FRAC_PI_2, // seam (static)
            o,                           // left_pages
            -o,                          // right_pages
            o - o * 2.0 * page1,         // flip_page1
            o - o * 2.0 * page2,         // flip_page2
        ];
        let x_offsets = [0.0, 0.0, 0.0, page_x, page_x, page_x, page_x];

        let mut verts = Vec::with_capacity(self.vert_count as usize);
        for (i, part) in self.parts.iter().enumerate() {
            // ModelPart.translateAndRotate: offsets are pixels / 16.
            let m = pose
                * Mat4::from_translation((part.offset + Vec3::new(x_offsets[i], 0.0, 0.0)) / 16.0)
                * Mat4::from_rotation_y(y_rots[i]);
            verts.extend(part.verts.iter().map(|v| Vertex {
                position: m.transform_point3(Vec3::from(v.position)).into(),
                uv: v.uv,
            }));
        }
        self.vertex_buffers[frame].write(bytemuck::cast_slice(&verts));

        pass.set_pipeline(&self.pipeline);
        push_preview_style(pass, WHITE_TINT, PREVIEW_ALPHA_CUTOFF);
        pass.set_bind_groups(0, &[self.mvp_sets[frame], self.tex_set]);
        pass.set_vertex_buffer(0, self.vertex_buffers[frame].handle());
        pass.draw(self.vert_count, 1, 0, 0);
    }

    pub fn recreate_pipeline(
        &mut self,
        device: &rhi::backend::Device,
        targets: rhi::backend::PipelineTargets,
    ) {
        self.pipeline.destroy(device);
        self.pipeline = create_pipeline(device, targets, self.pipeline_layout);
    }

    pub fn destroy(&mut self, device: &rhi::backend::Device) {
        util::destroy_buffers(device, &mut self.mvp_buffers);
        util::destroy_buffers(device, &mut self.vertex_buffers);
        std::mem::take(&mut self.texture).destroy(device);

        self.sampler.destroy(device);
        self.pipeline.destroy(device);
        self.pipeline_layout.destroy(device);
        util::destroy_bind_groups(device, self.mvp_sets.drain(..));
        self.tex_set.destroy(device);
        self.mvp_layout.destroy(device);
        self.tex_layout.destroy(device);
    }
}

fn load_book_texture(
    device: &rhi::backend::Device,
    resources: &ResourceManager,
) -> rhi::backend::Texture {
    let image = util::load_resource_image(resources, &BOOK_TEXTURE, util::MAX_RESOURCE_IMAGE_BYTES);
    let (pixels, tex_w, tex_h) = match image {
        Ok(image) => {
            let (w, h) = image.dimensions();
            (image.into_raw(), w, h)
        }
        Err(error) => {
            // Preserve the existing magenta fallback without bypassing pack filters
            // or retrying a selected resource's read/decode failure via the filesystem.
            tracing::warn!("Failed to load enchanting book texture: {error}");
            (vec![255, 0, 255, 255], 1, 1)
        }
    };
    util::create_sampled_texture(device, "book_texture", &pixels, tex_w, tex_h)
}

/// The seven `BookModel` parts, in `setupAnim` application order.
fn build_parts() -> Vec<Part> {
    let page = || cube(24.0, 10.0, [0.0, -4.0, 0.0], [5.0, 8.0, 0.005]);
    vec![
        Part {
            verts: cube(0.0, 0.0, [-6.0, -5.0, -0.005], [6.0, 10.0, 0.005]),
            offset: Vec3::new(0.0, 0.0, -1.0),
        },
        Part {
            verts: cube(16.0, 0.0, [0.0, -5.0, -0.005], [6.0, 10.0, 0.005]),
            offset: Vec3::new(0.0, 0.0, 1.0),
        },
        Part {
            verts: cube(12.0, 0.0, [-1.0, -5.0, 0.0], [2.0, 10.0, 0.005]),
            offset: Vec3::ZERO,
        },
        Part {
            verts: cube(0.0, 10.0, [0.0, -4.0, -0.99], [5.0, 8.0, 1.0]),
            offset: Vec3::ZERO,
        },
        Part {
            verts: cube(12.0, 10.0, [0.0, -4.0, -0.01], [5.0, 8.0, 1.0]),
            offset: Vec3::ZERO,
        },
        Part {
            verts: page(),
            offset: Vec3::ZERO,
        },
        Part {
            verts: page(),
            offset: Vec3::ZERO,
        },
    ]
}

const TEX_W: f32 = 64.0;
const TEX_H: f32 = 32.0;

/// A vanilla `ModelPart.Cube`: box at `min` (model pixels) of `size`, with
/// the standard [d][w][d][w] x [d][h] UV unwrap at `(tx, ty)`. Vertex
/// positions come out in world units (pixels / 16), faces in vanilla's
/// polygon order and winding.
fn cube(tx: f32, ty: f32, min: [f32; 3], size: [f32; 3]) -> Vec<Vertex> {
    let [w, h, d] = size;
    let (x0, y0, z0) = (min[0], min[1], min[2]);
    let (x1, y1, z1) = (x0 + w, y0 + h, z0 + d);

    // Vanilla Cube's eight corners: t* on the minZ face, l* on maxZ.
    let t0 = [x0, y0, z0];
    let t1 = [x1, y0, z0];
    let t2 = [x1, y1, z0];
    let t3 = [x0, y1, z0];
    let l0 = [x0, y0, z1];
    let l1 = [x1, y0, z1];
    let l2 = [x1, y1, z1];
    let l3 = [x0, y1, z1];

    let u0 = tx;
    let u1 = tx + d;
    let u2 = tx + d + w;
    let u22 = tx + d + w + w;
    let u3 = tx + d + w + d;
    let u4 = tx + d + w + d + w;
    let v0 = ty;
    let v1 = ty + d;
    let v2 = ty + d + h;

    let mut verts = Vec::with_capacity(36);
    let mut face = |quad: [[f32; 3]; 4], fu0: f32, fv0: f32, fu1: f32, fv1: f32| {
        // Vanilla Polygon: vertices 0..3 get UVs (u1,v0), (u0,v0), (u0,v1),
        // (u1,v1); quads become two triangles preserving winding.
        let uvs = [
            [fu1 / TEX_W, fv0 / TEX_H],
            [fu0 / TEX_W, fv0 / TEX_H],
            [fu0 / TEX_W, fv1 / TEX_H],
            [fu1 / TEX_W, fv1 / TEX_H],
        ];
        for &i in &[0usize, 1, 2, 0, 2, 3] {
            verts.push(Vertex {
                position: [quad[i][0] / 16.0, quad[i][1] / 16.0, quad[i][2] / 16.0],
                uv: uvs[i],
            });
        }
    };

    face([l1, l0, t0, t1], u1, v0, u2, v1); // down
    face([t2, t3, l3, l2], u2, v1, u22, v0); // up
    face([t0, l0, l3, t3], u0, v1, u1, v2); // west
    face([t1, t0, t3, t2], u1, v1, u2, v2); // north
    face([l1, t1, t2, l2], u2, v1, u3, v2); // east
    face([l0, l1, l2, l3], u3, v1, u4, v2); // south
    verts
}
