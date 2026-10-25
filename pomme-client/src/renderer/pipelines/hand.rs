use std::path::Path;
use std::slice;
use std::sync::{Arc, Mutex};

use glam::camera::rh::proj;
use glam::{Mat4, Vec3};
use pomme_gpu_allocator::vulkan::{Allocation, Allocator};
use pyronyx::vk;

use crate::assets::{AssetIndex, resolve_asset_path};
use crate::renderer::{MAX_FRAMES_IN_FLIGHT, SkinData, shader, util};
const NEAR: f32 = 0.05;
const FAR: f32 = 10.0;

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct HandVertex {
    position: [f32; 3],
    uv: [f32; 2],
}

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct HandUniform {
    mvp: [[f32; 4]; 4],
}

pub struct HandPipeline {
    pipeline: vk::Pipeline,
    pipeline_layout: vk::PipelineLayout,
    mvp_layout: vk::DescriptorSetLayout,
    skin_layout: vk::DescriptorSetLayout,
    descriptor_pool: vk::DescriptorPool,
    mvp_sets: Vec<vk::DescriptorSet>,
    skin_set: vk::DescriptorSet,
    mvp_buffers: Vec<vk::Buffer>,
    mvp_allocations: Vec<Allocation>,
    vertex_buffer: vk::Buffer,
    vertex_allocation: Allocation,
    /// Per arm: the buffer holds the right arm, then the left.
    vertex_count: u32,
    skin_image: vk::Image,
    skin_view: vk::ImageView,
    skin_sampler: vk::Sampler,
    skin_allocation: Allocation,
}

impl HandPipeline {
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
        let skin_layout = util::create_descriptor_set_layout(
            device,
            vk::DescriptorType::CombinedImageSampler,
            vk::ShaderStageFlags::Fragment,
        );

        let layouts = [mvp_layout, skin_layout];
        let layout_info = vk::PipelineLayoutCreateInfo {
            set_layout_count: layouts.len() as u32,
            set_layouts: layouts.as_ptr(),
            ..Default::default()
        };
        let pipeline_layout = device
            .create_pipeline_layout(&layout_info, None)
            .expect("failed to create hand pipeline layout");

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
            .expect("failed to create hand descriptor pool");

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
            .expect("failed to allocate hand mvp descriptor sets");

        let skin_alloc_info = vk::DescriptorSetAllocateInfo {
            descriptor_pool,
            descriptor_set_count: 1,
            set_layouts: &skin_layout,
            ..Default::default()
        };
        let mut skin_set = vk::DescriptorSet::null();
        device
            .allocate_descriptor_sets(&skin_alloc_info, slice::from_mut(&mut skin_set))
            .expect("failed to allocate hand skin descriptor set");

        let mut mvp_buffers = Vec::with_capacity(MAX_FRAMES_IN_FLIGHT);
        let mut mvp_allocations = Vec::with_capacity(MAX_FRAMES_IN_FLIGHT);

        for &set in &mvp_sets {
            let (buf, alloc) = util::create_uniform_buffer(
                device,
                allocator,
                size_of::<HandUniform>() as u64,
                "hand_uniform",
            );

            let buffer_info = vk::DescriptorBufferInfo {
                buffer: buf,
                offset: 0,
                range: size_of::<HandUniform>() as u64,
            };
            let write = vk::WriteDescriptorSet {
                dst_set: set,
                dst_binding: 0,
                descriptor_type: vk::DescriptorType::UniformBuffer,
                descriptor_count: 1,
                buffer_info: &buffer_info,
                image_info: std::ptr::null(),
                ..Default::default()
            };
            device.update_descriptor_sets(&[write], &[]);

            mvp_buffers.push(buf);
            mvp_allocations.push(alloc);
        }

        let (skin_image, skin_view, skin_allocation, skin_w, skin_h) = load_skin_texture(
            device,
            queue,
            command_pool,
            allocator,
            jar_assets_dir,
            asset_index,
        );

        let skin_sampler = unsafe { util::create_nearest_sampler(device) };

        update_skin_descriptor(device, skin_set, skin_view, skin_sampler);

        // Wide arms until the profile's skin (and its model flag) is fetched.
        let (vertex_buffer, vertex_allocation, vertex_count) =
            create_arm_vertex_buffer(device, allocator, skin_w, skin_h, false);

        tracing::info!(
            "Hand pipeline initialized ({vertex_count} vertices, skin {skin_w}x{skin_h})"
        );

        Self {
            pipeline,
            pipeline_layout,
            mvp_layout,
            skin_layout,
            descriptor_pool,
            mvp_sets,
            skin_set,
            mvp_buffers,
            mvp_allocations,
            vertex_buffer,
            vertex_allocation,
            vertex_count,
            skin_image,
            skin_view,
            skin_sampler,
            skin_allocation,
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn update_and_draw(
        &mut self,
        cmd: vk::CommandBuffer,
        frame: usize,
        aspect: f32,
        hud_fov: f32,
        swing_progress: f32,
        right_arm: bool,
        bob: Mat4,
    ) {
        let proj = projection(aspect, hud_fov);
        let model = arm_matrix(swing_progress, arm_invert(right_arm));

        let mvp = proj * bob * model;
        let uniform = HandUniform {
            mvp: mvp.to_cols_array_2d(),
        };
        let bytes = bytemuck::bytes_of(&uniform);
        self.mvp_allocations[frame].mapped_slice_mut().unwrap()[..bytes.len()]
            .copy_from_slice(bytes);

        cmd.bind_pipeline(vk::PipelineBindPoint::Graphics, self.pipeline);
        cmd.bind_descriptor_sets(
            vk::PipelineBindPoint::Graphics,
            self.pipeline_layout,
            0,
            &[self.mvp_sets[frame], self.skin_set],
            &[],
        );
        cmd.bind_vertex_buffers(0, &[self.vertex_buffer], &[0]);
        let first_vertex = if right_arm { 0 } else { self.vertex_count };
        cmd.draw(self.vertex_count, 1, first_vertex, 0);
    }

    pub fn skin_view(&self) -> vk::ImageView {
        self.skin_view
    }

    pub fn skin_sampler(&self) -> vk::Sampler {
        self.skin_sampler
    }

    pub fn reload_skin(
        &mut self,
        device: &vk::Device,
        queue: vk::Queue,
        command_pool: vk::CommandPool,
        allocator: &Arc<Mutex<Allocator>>,
        skin: &SkinData,
    ) {
        let (image, view, allocation) = upload_skin_to_gpu(
            device,
            queue,
            command_pool,
            allocator,
            &skin.pixels,
            skin.width,
            skin.height,
        );

        device.destroy_image_view(self.skin_view, None);
        device.destroy_image(self.skin_image, None);
        allocator
            .lock()
            .unwrap()
            .free(std::mem::replace(&mut self.skin_allocation, unsafe {
                std::mem::zeroed()
            }))
            .ok();

        self.skin_image = image;
        self.skin_view = view;
        self.skin_allocation = allocation;
        update_skin_descriptor(device, self.skin_set, self.skin_view, self.skin_sampler);

        device.destroy_buffer(self.vertex_buffer, None);
        allocator
            .lock()
            .unwrap()
            .free(std::mem::replace(&mut self.vertex_allocation, unsafe {
                std::mem::zeroed()
            }))
            .ok();
        let (vertex_buffer, vertex_allocation, vertex_count) =
            create_arm_vertex_buffer(device, allocator, skin.width, skin.height, skin.slim);
        self.vertex_buffer = vertex_buffer;
        self.vertex_allocation = vertex_allocation;
        self.vertex_count = vertex_count;

        tracing::info!(
            "Skin reloaded: {}x{} (slim: {})",
            skin.width,
            skin.height,
            skin.slim
        );
    }

    pub fn recreate_pipeline(&mut self, device: &vk::Device, render_pass: vk::RenderPass) {
        device.destroy_pipeline(self.pipeline, None);
        self.pipeline = create_pipeline(device, render_pass, self.pipeline_layout);
    }

    pub fn destroy(&mut self, device: &vk::Device, allocator: &Arc<Mutex<Allocator>>) {
        let mut alloc = allocator.lock().unwrap();
        for i in 0..MAX_FRAMES_IN_FLIGHT {
            device.destroy_buffer(self.mvp_buffers[i], None);
            alloc
                .free(std::mem::replace(&mut self.mvp_allocations[i], unsafe {
                    std::mem::zeroed()
                }))
                .ok();
        }

        device.destroy_buffer(self.vertex_buffer, None);
        alloc
            .free(std::mem::replace(&mut self.vertex_allocation, unsafe {
                std::mem::zeroed()
            }))
            .ok();

        device.destroy_sampler(self.skin_sampler, None);
        device.destroy_image_view(self.skin_view, None);

        alloc
            .free(std::mem::replace(&mut self.skin_allocation, unsafe {
                std::mem::zeroed()
            }))
            .ok();
        device.destroy_image(self.skin_image, None);

        drop(alloc);

        device.destroy_pipeline(self.pipeline, None);
        device.destroy_pipeline_layout(self.pipeline_layout, None);
        device.destroy_descriptor_pool(self.descriptor_pool, None);
        device.destroy_descriptor_set_layout(self.mvp_layout, None);
        device.destroy_descriptor_set_layout(self.skin_layout, None);
    }
}

/// Vanilla `ItemInHandRenderer`'s `invert`: 1 for the right arm, -1 for the
/// left.
pub(super) fn arm_invert(right_arm: bool) -> f32 {
    if right_arm { 1.0 } else { -1.0 }
}

/// Vanilla `ItemInHandRenderer.renderPlayerArm`, mirrored by `invert`.
fn arm_matrix(swing_progress: f32, invert: f32) -> Mat4 {
    let sp = swing_progress;
    let sqrt_sp = sp.sqrt();
    let pi = std::f32::consts::PI;

    let x_off = -0.3 * (sqrt_sp * pi).sin();
    let y_off = 0.4 * (sqrt_sp * pi * 2.0).sin();
    let z_off = -0.4 * (sp * pi).sin();

    let swing_y = invert * (sqrt_sp * pi).sin() * 70.0_f32.to_radians();
    let swing_z = invert * (sp * sp * pi).sin() * (-20.0_f32).to_radians();

    // `PlayerModel`'s arm pivots at (-5, 2, 0) on the right and (5, 2, 0)
    // on the left; `renderHand` sets their zRot to 0.1 and -0.1.
    let pivot = Vec3::new(invert * -5.0 / 16.0, 2.0 / 16.0, 0.0);
    let arm_local_rot = Mat4::from_translation(pivot) * Mat4::from_rotation_z(invert * 0.1);

    Mat4::from_translation(Vec3::new(
        invert * (x_off + 0.64),
        y_off - 0.6,
        z_off - 0.72,
    )) * Mat4::from_rotation_y((invert * 45.0_f32).to_radians())
        * Mat4::from_rotation_y(swing_y)
        * Mat4::from_rotation_z(swing_z)
        * Mat4::from_translation(Vec3::new(-invert, 3.6, 3.5))
        * Mat4::from_rotation_z((invert * 120.0_f32).to_radians())
        * Mat4::from_rotation_x(200.0_f32.to_radians())
        * Mat4::from_rotation_y((invert * -135.0_f32).to_radians())
        * Mat4::from_translation(Vec3::new(invert * 5.6, 0.0, 0.0))
        * arm_local_rot
}

pub(super) fn projection(aspect: f32, hud_fov: f32) -> Mat4 {
    let mut proj = proj::directx::perspective(hud_fov, aspect, NEAR, FAR);
    proj.y_axis.y *= -1.0;
    proj
}

fn create_arm_vertex_buffer(
    device: &vk::Device,
    allocator: &Arc<Mutex<Allocator>>,
    skin_w: u32,
    skin_h: u32,
    slim: bool,
) -> (vk::Buffer, Allocation, u32) {
    // The right arm, then the left.
    let mut vertices = build_arm_vertices(skin_w, skin_h, slim, true);
    let arm_vertex_count = vertices.len() as u32;
    vertices.extend(build_arm_vertices(skin_w, skin_h, slim, false));
    let vertex_bytes = bytemuck::cast_slice::<HandVertex, u8>(&vertices);
    let (buffer, allocation) = util::create_mapped_buffer(
        device,
        allocator,
        vertex_bytes,
        vk::BufferUsageFlags::VertexBuffer,
        "hand_vertices",
    );
    (buffer, allocation, arm_vertex_count)
}

fn build_arm_vertices(skin_w: u32, skin_h: u32, slim: bool, right: bool) -> Vec<HandVertex> {
    let sw = skin_w as f32;
    let sh = skin_h as f32;

    // Vanilla right arm addBox(-3, -2, -2, 4, 12, 4), or the slim layout's
    // addBox(-2, -2, -2, 3, 12, 4); the left arm's starts at x -1 either way.
    // Scaled to blocks (1/16). Model space is Y-down: y0 is the shoulder
    // end, y1 the hand end.
    let w = if slim { 3.0 } else { 4.0 };
    let x0: f32 = match (right, slim) {
        (false, _) => -1.0,
        (true, true) => -2.0,
        (true, false) => -3.0,
    } / 16.0;
    let x1: f32 = x0 + w / 16.0;
    let y0: f32 = -2.0 / 16.0;
    let y1: f32 = 10.0 / 16.0;
    let z0: f32 = -2.0 / 16.0;
    let z1: f32 = 2.0 / 16.0;

    // texOffs(40, 16) on the right, (32, 48) on the left; h=12 d=4
    let (u0, v0) = if right { (40.0, 16.0) } else { (32.0, 48.0) };
    let h = 12.0;
    let d = 4.0;

    let u1 = u0 + d;
    let u2 = u1 + w;
    let u3 = u2 + d;
    let u4 = u3 + w;
    let v1 = v0 + d;
    let v2 = v1 + h;

    // Corner naming follows vanilla ModelPart.Cube (t* = min Z, l* = max Z)
    let t0 = [x0, y0, z0];
    let t1 = [x1, y0, z0];
    let t2 = [x1, y1, z0];
    let t3 = [x0, y1, z0];
    let l0 = [x0, y0, z1];
    let l1 = [x1, y0, z1];
    let l2 = [x1, y1, z1];
    let l3 = [x0, y1, z1];

    let mut verts = Vec::with_capacity(36);

    // UV corner order matches vanilla ModelPart.Polygon
    let mut quad = |positions: [[f32; 3]; 4], uv_px: [f32; 4]| {
        let ua = uv_px[0] / sw;
        let va = uv_px[1] / sh;
        let ub = uv_px[2] / sw;
        let vb = uv_px[3] / sh;
        let uvs = [[ub, va], [ua, va], [ua, vb], [ub, vb]];
        for &i in &[0usize, 1, 2, 0, 2, 3] {
            verts.push(HandVertex {
                position: positions[i],
                uv: uvs[i],
            });
        }
    };

    quad([l1, l0, t0, t1], [u1, v0, u2, v1]); // down (shoulder cap)
    quad([t2, t3, l3, l2], [u2, v1, u2 + w, v0]); // up (hand cap, V reversed as vanilla)
    quad([t0, l0, l3, t3], [u0, v1, u1, v2]); // west (outer)
    quad([t1, t0, t3, t2], [u1, v1, u2, v2]); // north (front)
    quad([l1, t1, t2, l2], [u2, v1, u3, v2]); // east (inner)
    quad([l0, l1, l2, l3], [u3, v1, u4, v2]); // south (back)

    verts
}

fn load_skin_texture(
    device: &vk::Device,
    queue: vk::Queue,
    command_pool: vk::CommandPool,
    allocator: &Arc<Mutex<Allocator>>,
    jar_assets_dir: &Path,
    asset_index: &Option<AssetIndex>,
) -> (vk::Image, vk::ImageView, Allocation, u32, u32) {
    let skin_key = "minecraft/textures/entity/player/wide/steve.png";
    let skin_path = resolve_asset_path(jar_assets_dir, asset_index, skin_key);

    let (pixels, width, height) = util::load_png(&skin_path).unwrap_or_else(|| {
        tracing::warn!(
            "Failed to load skin from {}, using fallback",
            skin_path.display()
        );
        fallback_skin()
    });

    let (image, view, allocation) = upload_skin_to_gpu(
        device,
        queue,
        command_pool,
        allocator,
        &pixels,
        width,
        height,
    );
    (image, view, allocation, width, height)
}

fn upload_skin_to_gpu(
    device: &vk::Device,
    queue: vk::Queue,
    command_pool: vk::CommandPool,
    allocator: &Arc<Mutex<Allocator>>,
    pixels: &[u8],
    width: u32,
    height: u32,
) -> (vk::Image, vk::ImageView, Allocation) {
    let (image, view, allocation) =
        util::create_gpu_image(device, allocator, width, height, "skin");
    let (staging_buf, staging_alloc) =
        util::create_staging_buffer(device, allocator, pixels, "skin_staging");
    util::upload_image(
        device,
        queue,
        command_pool,
        staging_buf,
        image,
        width,
        height,
    );
    device.destroy_buffer(staging_buf, None);
    allocator.lock().unwrap().free(staging_alloc).ok();
    (image, view, allocation)
}

fn update_skin_descriptor(
    device: &vk::Device,
    set: vk::DescriptorSet,
    view: vk::ImageView,
    sampler: vk::Sampler,
) {
    let image_info = vk::DescriptorImageInfo {
        sampler,
        image_view: view,
        image_layout: vk::ImageLayout::ShaderReadOnlyOptimal,
    };
    let write = vk::WriteDescriptorSet {
        dst_set: set,
        dst_binding: 0,
        descriptor_type: vk::DescriptorType::CombinedImageSampler,
        descriptor_count: 1,
        buffer_info: std::ptr::null(),
        image_info: &image_info,
        ..Default::default()
    };
    device.update_descriptor_sets(&[write], &[]);
}

fn fallback_skin() -> (Vec<u8>, u32, u32) {
    let w = 64u32;
    let h = 64u32;
    let pixels = [196u8, 161, 125, 255].repeat((w * h) as usize);
    (pixels, w, h)
}

fn create_pipeline(
    device: &vk::Device,
    render_pass: vk::RenderPass,
    layout: vk::PipelineLayout,
) -> vk::Pipeline {
    let vert_spv = shader::include_spirv!("hand.vert.spv");
    let frag_spv = shader::include_spirv!("hand.frag.spv");

    let vert_module = shader::create_shader_module(device, vert_spv);
    let frag_module = shader::create_shader_module(device, frag_spv);

    let stages = [
        vk::PipelineShaderStageCreateInfo {
            stage: vk::ShaderStageFlags::Vertex,
            module: vert_module,
            name: c"main".as_ptr(),
            ..Default::default()
        },
        vk::PipelineShaderStageCreateInfo {
            stage: vk::ShaderStageFlags::Fragment,
            module: frag_module,
            name: c"main".as_ptr(),
            ..Default::default()
        },
    ];

    let binding_descs = [vk::VertexInputBindingDescription {
        binding: 0,
        stride: size_of::<HandVertex>() as u32,
        input_rate: vk::VertexInputRate::Vertex,
    }];

    let attr_descs = [
        vk::VertexInputAttributeDescription {
            location: 0,
            binding: 0,
            format: vk::Format::R32G32B32Sfloat,
            offset: 0,
        },
        vk::VertexInputAttributeDescription {
            location: 1,
            binding: 0,
            format: vk::Format::R32G32Sfloat,
            offset: 12,
        },
    ];

    let vertex_input = vk::PipelineVertexInputStateCreateInfo {
        vertex_binding_description_count: binding_descs.len() as u32,
        vertex_binding_descriptions: binding_descs.as_ptr(),
        vertex_attribute_description_count: attr_descs.len() as u32,
        vertex_attribute_descriptions: attr_descs.as_ptr(),
        ..Default::default()
    };

    let input_assembly = vk::PipelineInputAssemblyStateCreateInfo {
        topology: vk::PrimitiveTopology::TriangleList,
        primitive_restart_enable: vk::FALSE,
        ..Default::default()
    };

    let viewport_state = vk::PipelineViewportStateCreateInfo {
        viewport_count: 1,
        scissor_count: 1,
        ..Default::default()
    };

    let rasterizer = vk::PipelineRasterizationStateCreateInfo {
        polygon_mode: vk::PolygonMode::Fill,
        cull_mode: vk::CullModeFlags::None,
        front_face: vk::FrontFace::CounterClockwise,
        line_width: 1.0,
        ..Default::default()
    };

    let multisampling = vk::PipelineMultisampleStateCreateInfo {
        rasterization_samples: vk::SampleCountFlags::Type1,
        ..Default::default()
    };

    let depth_stencil = vk::PipelineDepthStencilStateCreateInfo {
        depth_test_enable: vk::TRUE,
        depth_write_enable: vk::TRUE,
        depth_compare_op: vk::CompareOp::Less,
        ..Default::default()
    };

    let blend_attachment = [vk::PipelineColorBlendAttachmentState {
        blend_enable: vk::FALSE,
        color_write_mask: vk::ColorComponentFlags::RGBA,
        ..Default::default()
    }];
    let color_blending = vk::PipelineColorBlendStateCreateInfo {
        attachment_count: blend_attachment.len() as u32,
        attachments: blend_attachment.as_ptr(),
        ..Default::default()
    };

    let dynamic_states = [vk::DynamicState::Viewport, vk::DynamicState::Scissor];
    let dynamic_state = vk::PipelineDynamicStateCreateInfo {
        dynamic_state_count: dynamic_states.len() as u32,
        dynamic_states: dynamic_states.as_ptr(),
        ..Default::default()
    };

    let pipeline_info = [vk::GraphicsPipelineCreateInfo {
        stage_count: stages.len() as u32,
        stages: stages.as_ptr(),
        vertex_input_state: &vertex_input,
        input_assembly_state: &input_assembly,
        viewport_state: &viewport_state,
        rasterization_state: &rasterizer,
        multisample_state: &multisampling,
        depth_stencil_state: &depth_stencil,
        color_blend_state: &color_blending,
        dynamic_state: &dynamic_state,
        layout,
        render_pass,
        subpass: 0,
        ..Default::default()
    }];

    let mut pipeline = vk::Pipeline::null();
    device
        .create_graphics_pipelines(
            vk::PipelineCache::null(),
            &pipeline_info,
            None,
            slice::from_mut(&mut pipeline),
        )
        .expect("failed to create hand pipeline");

    device.destroy_shader_module(vert_module, None);
    device.destroy_shader_module(frag_module, None);

    pipeline
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_left_arm_mirrors_the_right() {
        let flip = Mat4::from_scale(Vec3::new(-1.0, 1.0, 1.0));
        for swing in [0.0, 0.3, 0.7, 1.0] {
            let mirrored = flip * arm_matrix(swing, 1.0) * flip;
            assert!(
                arm_matrix(swing, -1.0).abs_diff_eq(mirrored, 1e-4),
                "swing {swing}"
            );
        }
        for slim in [false, true] {
            let x_range = |right| {
                let verts = build_arm_vertices(64, 64, slim, right);
                let xs = verts.iter().map(|v| v.position[0]);
                (
                    xs.clone().fold(f32::MAX, f32::min),
                    xs.fold(f32::MIN, f32::max),
                )
            };
            let (right_min, right_max) = x_range(true);
            assert_eq!(x_range(false), (-right_max, -right_min), "slim {slim}");
        }
    }
}
