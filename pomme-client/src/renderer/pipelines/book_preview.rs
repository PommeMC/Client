//! The enchanting table's 3D book (vanilla `BookModel` +
//! `GuiBookModelRenderer`), drawn into a scissored GUI box like the
//! inventory's player preview. The seven model parts are posed on the CPU
//! each frame and written to a per-frame vertex buffer; the uniform holds the
//! screen-space projection.

use std::path::Path;
use std::slice;
use std::sync::{Arc, Mutex};

use glam::{Mat4, Vec3, Vec4};
use pomme_gpu_allocator::vulkan::{Allocation, Allocator};
use pyronyx::vk;

use super::skin_preview::{Uniform, Vertex, create_pipeline, write_uniform};
use crate::assets::{AssetIndex, load_image, resolve_asset_path};
use crate::renderer::{BookPreview, MAX_FRAMES_IN_FLIGHT, util};

/// Base mesh of one book part, posed per frame.
struct Part {
    verts: Vec<Vertex>,
    /// Pose offset in model pixels (vanilla `PartPose.offset`).
    offset: Vec3,
}

pub struct BookPreviewPipeline {
    pipeline: vk::Pipeline,
    pipeline_layout: vk::PipelineLayout,
    mvp_layout: vk::DescriptorSetLayout,
    tex_layout: vk::DescriptorSetLayout,
    descriptor_pool: vk::DescriptorPool,
    mvp_sets: Vec<vk::DescriptorSet>,
    mvp_buffers: Vec<vk::Buffer>,
    mvp_allocations: Vec<Allocation>,
    tex_set: vk::DescriptorSet,
    texture: vk::Image,
    texture_view: vk::ImageView,
    texture_allocation: Allocation,
    sampler: vk::Sampler,
    vertex_buffers: Vec<vk::Buffer>,
    vertex_allocations: Vec<Allocation>,
    parts: Vec<Part>,
    vert_count: u32,
}

impl BookPreviewPipeline {
    pub fn new(
        device: &vk::Device,
        queue: vk::Queue,
        command_pool: vk::CommandPool,
        render_pass: vk::RenderPass,
        allocator: &Arc<Mutex<Allocator>>,
        jar_assets_dir: &Path,
        asset_index: &Option<AssetIndex>,
    ) -> Self {
        let mvp_layout = util::create_descriptor_set_layout(
            device,
            vk::DescriptorType::UniformBuffer,
            vk::ShaderStageFlags::Vertex,
        );
        let tex_layout = util::create_descriptor_set_layout(
            device,
            vk::DescriptorType::CombinedImageSampler,
            vk::ShaderStageFlags::Fragment,
        );

        let layouts = [mvp_layout, tex_layout];
        let layout_info = vk::PipelineLayoutCreateInfo {
            set_layout_count: layouts.len() as u32,
            set_layouts: layouts.as_ptr(),
            ..Default::default()
        };
        let pipeline_layout = device
            .create_pipeline_layout(&layout_info, None)
            .expect("failed to create book preview pipeline layout");

        let pipeline = create_pipeline(device, render_pass, pipeline_layout);

        let pool_sizes = [
            vk::DescriptorPoolSize {
                ty: vk::DescriptorType::UniformBuffer,
                descriptor_count: MAX_FRAMES_IN_FLIGHT as u32,
            },
            vk::DescriptorPoolSize {
                ty: vk::DescriptorType::CombinedImageSampler,
                descriptor_count: 1,
            },
        ];
        let pool_info = vk::DescriptorPoolCreateInfo {
            max_sets: (MAX_FRAMES_IN_FLIGHT + 1) as u32,
            pool_size_count: pool_sizes.len() as u32,
            pool_sizes: pool_sizes.as_ptr(),
            ..Default::default()
        };
        let descriptor_pool = device
            .create_descriptor_pool(&pool_info, None)
            .expect("failed to create book preview descriptor pool");

        let mvp_layouts: Vec<_> = (0..MAX_FRAMES_IN_FLIGHT).map(|_| mvp_layout).collect();
        let mvp_alloc_info = vk::DescriptorSetAllocateInfo {
            descriptor_pool,
            descriptor_set_count: mvp_layouts.len() as u32,
            set_layouts: mvp_layouts.as_ptr(),
            ..Default::default()
        };
        let mut mvp_sets = vec![vk::DescriptorSet::null(); mvp_layouts.len()];
        device
            .allocate_descriptor_sets(&mvp_alloc_info, &mut mvp_sets)
            .expect("failed to allocate book preview mvp sets");

        let tex_alloc_info = vk::DescriptorSetAllocateInfo {
            descriptor_pool,
            descriptor_set_count: 1,
            set_layouts: &tex_layout,
            ..Default::default()
        };
        let mut tex_set = vk::DescriptorSet::null();
        device
            .allocate_descriptor_sets(&tex_alloc_info, slice::from_mut(&mut tex_set))
            .expect("failed to allocate book preview tex set");

        let mut mvp_buffers = Vec::with_capacity(MAX_FRAMES_IN_FLIGHT);
        let mut mvp_allocations = Vec::with_capacity(MAX_FRAMES_IN_FLIGHT);
        for set in &mvp_sets {
            let (buf, alloc) = util::create_uniform_buffer(
                device,
                allocator,
                std::mem::size_of::<Uniform>() as u64,
                "book_mvp",
            );
            let buffer_info = vk::DescriptorBufferInfo {
                buffer: buf,
                offset: 0,
                range: std::mem::size_of::<Uniform>() as u64,
            };
            let write = vk::WriteDescriptorSet {
                dst_set: *set,
                dst_binding: 0,
                descriptor_type: vk::DescriptorType::UniformBuffer,
                descriptor_count: 1,
                buffer_info: &buffer_info,
                ..Default::default()
            };
            device.update_descriptor_sets(&[write], &[]);
            mvp_buffers.push(buf);
            mvp_allocations.push(alloc);
        }

        let path = resolve_asset_path(
            jar_assets_dir,
            asset_index,
            "minecraft/textures/entity/enchantment/enchanting_table_book.png",
        );
        let (pixels, tex_w, tex_h) = match load_image(&path) {
            Ok(img) => {
                let rgba = img.to_rgba8();
                let (w, h) = (rgba.width(), rgba.height());
                (rgba.into_raw(), w, h)
            }
            Err(e) => {
                tracing::warn!("Failed to load enchanting book texture: {e}");
                (vec![255, 0, 255, 255], 1, 1)
            }
        };
        let (texture, texture_view, texture_allocation) =
            util::create_gpu_image(device, allocator, tex_w, tex_h, "book_texture");
        let (staging, staging_alloc) =
            util::create_staging_buffer(device, allocator, &pixels, "book_texture_staging");
        util::upload_image(device, queue, command_pool, staging, texture, tex_w, tex_h);
        device.destroy_buffer(staging, None);
        allocator.lock().unwrap().free(staging_alloc).ok();

        let sampler = unsafe { util::create_nearest_sampler(device) };
        let image_info = vk::DescriptorImageInfo {
            sampler,
            image_view: texture_view,
            image_layout: vk::ImageLayout::ShaderReadOnlyOptimal,
        };
        let tex_write = vk::WriteDescriptorSet {
            dst_set: tex_set,
            dst_binding: 0,
            descriptor_type: vk::DescriptorType::CombinedImageSampler,
            descriptor_count: 1,
            image_info: &image_info,
            ..Default::default()
        };
        device.update_descriptor_sets(&[tex_write], &[]);

        let parts = build_parts();
        let vert_count: u32 = parts.iter().map(|p| p.verts.len() as u32).sum();
        let zeroed = vec![0u8; vert_count as usize * std::mem::size_of::<Vertex>()];
        let mut vertex_buffers = Vec::with_capacity(MAX_FRAMES_IN_FLIGHT);
        let mut vertex_allocations = Vec::with_capacity(MAX_FRAMES_IN_FLIGHT);
        for _ in 0..MAX_FRAMES_IN_FLIGHT {
            let (buf, alloc) = util::create_mapped_buffer(
                device,
                allocator,
                &zeroed,
                vk::BufferUsageFlags::VertexBuffer,
                "book_vertices",
            );
            vertex_buffers.push(buf);
            vertex_allocations.push(alloc);
        }

        Self {
            pipeline,
            pipeline_layout,
            mvp_layout,
            tex_layout,
            descriptor_pool,
            mvp_sets,
            mvp_buffers,
            mvp_allocations,
            tex_set,
            texture,
            texture_view,
            texture_allocation,
            sampler,
            vertex_buffers,
            vertex_allocations,
            parts,
            vert_count,
        }
    }

    /// Draws the book in its GUI box, replicating vanilla's picture-in-picture
    /// setup: origin at (box center x, box top + 17 GUI px), 40 GUI px per
    /// unit, then `GuiBookModelRenderer`'s pose chain and `BookModel`'s
    /// `setupAnim`.
    pub fn draw_in_box(
        &mut self,
        cmd: vk::CommandBuffer,
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
        write_uniform(&mut self.mvp_allocations[frame], &clip);

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

        let byte_len = self.vert_count as usize * std::mem::size_of::<Vertex>();
        let mapped = self.vertex_allocations[frame].mapped_slice_mut().unwrap();
        let out: &mut [Vertex] = bytemuck::cast_slice_mut(&mut mapped[..byte_len]);
        let mut cursor = 0usize;
        for (i, part) in self.parts.iter().enumerate() {
            // ModelPart.translateAndRotate: offsets are pixels / 16.
            let m = pose
                * Mat4::from_translation((part.offset + Vec3::new(x_offsets[i], 0.0, 0.0)) / 16.0)
                * Mat4::from_rotation_y(y_rots[i]);
            for v in &part.verts {
                out[cursor] = Vertex {
                    position: m.transform_point3(Vec3::from(v.position)).into(),
                    uv: v.uv,
                };
                cursor += 1;
            }
        }

        cmd.bind_pipeline(vk::PipelineBindPoint::Graphics, self.pipeline);
        cmd.bind_descriptor_sets(
            vk::PipelineBindPoint::Graphics,
            self.pipeline_layout,
            0,
            &[self.mvp_sets[frame], self.tex_set],
            &[],
        );
        cmd.bind_vertex_buffers(0, &[self.vertex_buffers[frame]], &[0]);
        cmd.draw(self.vert_count, 1, 0, 0);
    }

    pub fn recreate_pipeline(&mut self, device: &vk::Device, render_pass: vk::RenderPass) {
        device.destroy_pipeline(self.pipeline, None);
        self.pipeline = create_pipeline(device, render_pass, self.pipeline_layout);
    }

    pub fn destroy(&mut self, device: &vk::Device, allocator: &Arc<Mutex<Allocator>>) {
        let mut alloc = allocator.lock().unwrap();
        for i in 0..MAX_FRAMES_IN_FLIGHT {
            device.destroy_buffer(self.mvp_buffers[i], None);
            device.destroy_buffer(self.vertex_buffers[i], None);
            alloc
                .free(std::mem::replace(&mut self.mvp_allocations[i], unsafe {
                    std::mem::zeroed()
                }))
                .ok();
            alloc
                .free(std::mem::replace(&mut self.vertex_allocations[i], unsafe {
                    std::mem::zeroed()
                }))
                .ok();
        }
        device.destroy_image_view(self.texture_view, None);
        device.destroy_image(self.texture, None);
        alloc
            .free(std::mem::replace(&mut self.texture_allocation, unsafe {
                std::mem::zeroed()
            }))
            .ok();
        drop(alloc);

        device.destroy_sampler(self.sampler, None);
        device.destroy_pipeline(self.pipeline, None);
        device.destroy_pipeline_layout(self.pipeline_layout, None);
        device.destroy_descriptor_pool(self.descriptor_pool, None);
        device.destroy_descriptor_set_layout(self.mvp_layout, None);
        device.destroy_descriptor_set_layout(self.tex_layout, None);
    }
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
