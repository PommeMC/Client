use std::slice;
use std::sync::{Arc, Mutex};

use glam::camera::rh::{proj, view};
use glam::{Mat4, Vec3};
use pomme_gpu_allocator::vulkan::{Allocation, Allocator};
use pyronyx::vk;

use crate::renderer::{MAX_FRAMES_IN_FLIGHT, shader, util};

#[repr(C)]
#[derive(Copy, Clone, Debug, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub(crate) struct Vertex {
    pub(crate) position: [f32; 3],
    pub(crate) uv: [f32; 2],
}

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
pub(crate) struct Uniform {
    mvp: [[f32; 4]; 4],
}

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct PreviewPush {
    tint: [f32; 4],
    alpha_cutoff: f32,
}

pub(crate) const WHITE_TINT: [f32; 4] = [1.0, 1.0, 1.0, 1.0];
pub(crate) const PREVIEW_ALPHA_CUTOFF: f32 = 0.5;
pub(crate) const ARMOR_PREVIEW_ALPHA_CUTOFF: f32 = 0.1;

pub(crate) fn preview_push_constant_range() -> vk::PushConstantRange {
    vk::PushConstantRange {
        stage_flags: vk::ShaderStageFlags::Fragment,
        offset: 0,
        size: std::mem::size_of::<PreviewPush>() as u32,
    }
}

pub(crate) fn push_preview_style(
    cmd: vk::CommandBuffer,
    layout: vk::PipelineLayout,
    tint: [f32; 4],
    alpha_cutoff: f32,
) {
    let push = PreviewPush { tint, alpha_cutoff };
    cmd.push_constants(
        layout,
        vk::ShaderStageFlags::Fragment,
        0,
        bytemuck::bytes_of(&push),
    );
}

#[derive(Clone, Copy, Default)]
struct ArmorRange {
    first: u32,
    count: u32,
}

struct PreviewArmorCpuMesh {
    vertices: Vec<Vertex>,
    body: ArmorRange,
    head: ArmorRange,
    arm: ArmorRange,
}

struct PreviewArmorMesh {
    buffer: vk::Buffer,
    allocation: Allocation,
    body: ArmorRange,
    head: ArmorRange,
    arm: ArmorRange,
}

pub struct SkinPreviewPipeline {
    pipeline: vk::Pipeline,
    armor_pipeline: vk::Pipeline,
    pipeline_layout: vk::PipelineLayout,
    mvp_layout: vk::DescriptorSetLayout,
    tex_layout: vk::DescriptorSetLayout,
    descriptor_pool: vk::DescriptorPool,
    mvp_sets: Vec<vk::DescriptorSet>,
    head_mvp_sets: Vec<vk::DescriptorSet>,
    tex_set: vk::DescriptorSet,
    mvp_buffers: Vec<vk::Buffer>,
    mvp_allocations: Vec<Allocation>,
    head_mvp_buffers: Vec<vk::Buffer>,
    head_mvp_allocations: Vec<Allocation>,
    body_buffer: vk::Buffer,
    body_allocation: Allocation,
    body_count: u32,
    head_buffer: vk::Buffer,
    head_allocation: Allocation,
    head_count: u32,
    arm_buffer: vk::Buffer,
    arm_allocation: Allocation,
    arm_count: u32,
    arm_mvp_sets: Vec<vk::DescriptorSet>,
    arm_mvp_buffers: Vec<vk::Buffer>,
    arm_mvp_allocations: Vec<Allocation>,
    armor_meshes: [PreviewArmorMesh; 4],
    swing_start: Option<std::time::Instant>,
}

impl SkinPreviewPipeline {
    pub fn new(
        device: &vk::Device,
        render_pass: vk::RenderPass,
        allocator: &Arc<Mutex<Allocator>>,
        skin_view: vk::ImageView,
        skin_sampler: vk::Sampler,
        slim: bool,
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
        let push_range = preview_push_constant_range();
        let layout_info = vk::PipelineLayoutCreateInfo {
            set_layout_count: layouts.len() as u32,
            set_layouts: layouts.as_ptr(),
            push_constant_range_count: 1,
            push_constant_ranges: &push_range,
            ..Default::default()
        };
        let pipeline_layout = device
            .create_pipeline_layout(&layout_info, None)
            .expect("failed to create skin preview pipeline layout");

        let pipeline = create_pipeline(device, render_pass, pipeline_layout);
        let armor_pipeline = create_armor_pipeline(device, render_pass, pipeline_layout);

        let pool_sizes = [
            vk::DescriptorPoolSize {
                ty: vk::DescriptorType::UniformBuffer,
                descriptor_count: MAX_FRAMES_IN_FLIGHT as u32 * 3,
            },
            vk::DescriptorPoolSize {
                ty: vk::DescriptorType::CombinedImageSampler,
                descriptor_count: 1,
            },
        ];
        let pool_info = vk::DescriptorPoolCreateInfo {
            max_sets: (MAX_FRAMES_IN_FLIGHT * 3 + 1) as u32,
            pool_size_count: pool_sizes.len() as u32,
            pool_sizes: pool_sizes.as_ptr(),
            ..Default::default()
        };
        let descriptor_pool = device
            .create_descriptor_pool(&pool_info, None)
            .expect("failed to create skin preview descriptor pool");

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
            .expect("failed to allocate skin preview mvp sets");

        let head_mvp_alloc_info = vk::DescriptorSetAllocateInfo {
            descriptor_pool,
            descriptor_set_count: mvp_layouts.len() as u32,
            set_layouts: mvp_layouts.as_ptr(),
            ..Default::default()
        };
        let mut head_mvp_sets = vec![vk::DescriptorSet::null(); mvp_layouts.len()];
        device
            .allocate_descriptor_sets(&head_mvp_alloc_info, &mut head_mvp_sets)
            .expect("failed to allocate skin preview head mvp sets");

        let arm_mvp_alloc_info = vk::DescriptorSetAllocateInfo {
            descriptor_pool,
            descriptor_set_count: mvp_layouts.len() as u32,
            set_layouts: mvp_layouts.as_ptr(),
            ..Default::default()
        };
        let mut arm_mvp_sets = vec![vk::DescriptorSet::null(); mvp_layouts.len()];
        device
            .allocate_descriptor_sets(&arm_mvp_alloc_info, &mut arm_mvp_sets)
            .expect("failed to allocate skin preview arm mvp sets");

        let tex_alloc_info = vk::DescriptorSetAllocateInfo {
            descriptor_pool,
            descriptor_set_count: 1,
            set_layouts: &tex_layout,
            ..Default::default()
        };
        let mut tex_set = vk::DescriptorSet::null();
        device
            .allocate_descriptor_sets(&tex_alloc_info, slice::from_mut(&mut tex_set))
            .expect("failed to allocate skin preview tex set");

        let mut mvp_buffers = Vec::with_capacity(MAX_FRAMES_IN_FLIGHT);
        let mut mvp_allocations = Vec::with_capacity(MAX_FRAMES_IN_FLIGHT);
        let mut head_mvp_buffers = Vec::with_capacity(MAX_FRAMES_IN_FLIGHT);
        let mut head_mvp_allocations = Vec::with_capacity(MAX_FRAMES_IN_FLIGHT);
        let mut arm_mvp_buffers = Vec::with_capacity(MAX_FRAMES_IN_FLIGHT);
        let mut arm_mvp_allocations = Vec::with_capacity(MAX_FRAMES_IN_FLIGHT);

        for i in 0..MAX_FRAMES_IN_FLIGHT {
            let (buf, alloc) = util::create_uniform_buffer(
                device,
                allocator,
                std::mem::size_of::<Uniform>() as u64,
                "skin_body_mvp",
            );
            let buffer_info = vk::DescriptorBufferInfo {
                buffer: buf,
                offset: 0,
                range: std::mem::size_of::<Uniform>() as u64,
            };
            let write = vk::WriteDescriptorSet {
                dst_set: mvp_sets[i],
                dst_binding: 0,
                descriptor_type: vk::DescriptorType::UniformBuffer,
                descriptor_count: 1,
                buffer_info: &buffer_info,
                ..Default::default()
            };
            device.update_descriptor_sets(&[write], &[]);
            mvp_buffers.push(buf);
            mvp_allocations.push(alloc);

            let (buf, alloc) = util::create_uniform_buffer(
                device,
                allocator,
                std::mem::size_of::<Uniform>() as u64,
                "skin_head_mvp",
            );
            let buffer_info = vk::DescriptorBufferInfo {
                buffer: buf,
                offset: 0,
                range: std::mem::size_of::<Uniform>() as u64,
            };
            let write = vk::WriteDescriptorSet {
                dst_set: head_mvp_sets[i],
                dst_binding: 0,
                descriptor_type: vk::DescriptorType::UniformBuffer,
                descriptor_count: 1,
                buffer_info: &buffer_info,
                ..Default::default()
            };
            device.update_descriptor_sets(&[write], &[]);
            head_mvp_buffers.push(buf);
            head_mvp_allocations.push(alloc);

            let (buf, alloc) = util::create_uniform_buffer(
                device,
                allocator,
                std::mem::size_of::<Uniform>() as u64,
                "skin_arm_mvp",
            );
            let buffer_info = vk::DescriptorBufferInfo {
                buffer: buf,
                offset: 0,
                range: std::mem::size_of::<Uniform>() as u64,
            };
            let write = vk::WriteDescriptorSet {
                dst_set: arm_mvp_sets[i],
                dst_binding: 0,
                descriptor_type: vk::DescriptorType::UniformBuffer,
                descriptor_count: 1,
                buffer_info: &buffer_info,
                ..Default::default()
            };
            device.update_descriptor_sets(&[write], &[]);
            arm_mvp_buffers.push(buf);
            arm_mvp_allocations.push(alloc);
        }

        let image_info = vk::DescriptorImageInfo {
            sampler: skin_sampler,
            image_view: skin_view,
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

        let body_verts = build_body_mesh(slim);
        let body_bytes = bytemuck::cast_slice(&body_verts);
        let (body_buffer, body_allocation) = util::create_mapped_buffer(
            device,
            allocator,
            body_bytes,
            vk::BufferUsageFlags::VertexBuffer,
            "skin_body",
        );

        let arm_verts = build_right_arm_mesh(slim);
        let arm_bytes = bytemuck::cast_slice(&arm_verts);
        let (arm_buffer, arm_allocation) = util::create_mapped_buffer(
            device,
            allocator,
            arm_bytes,
            vk::BufferUsageFlags::VertexBuffer,
            "skin_arm",
        );

        let head_verts = build_head_mesh();
        let head_bytes = bytemuck::cast_slice(&head_verts);
        let (head_buffer, head_allocation) = util::create_mapped_buffer(
            device,
            allocator,
            head_bytes,
            vk::BufferUsageFlags::VertexBuffer,
            "skin_head",
        );

        let armor_slots = [
            crate::renderer::entity_model::HumanoidArmorSlot::Head,
            crate::renderer::entity_model::HumanoidArmorSlot::Chest,
            crate::renderer::entity_model::HumanoidArmorSlot::Legs,
            crate::renderer::entity_model::HumanoidArmorSlot::Feet,
        ];
        let armor_meshes = std::array::from_fn(|i| {
            let mesh = build_armor_preview_mesh(armor_slots[i]);
            let bytes = bytemuck::cast_slice(&mesh.vertices);
            let (buffer, allocation) = util::create_mapped_buffer(
                device,
                allocator,
                bytes,
                vk::BufferUsageFlags::VertexBuffer,
                "skin_preview_armor",
            );
            PreviewArmorMesh {
                buffer,
                allocation,
                body: mesh.body,
                head: mesh.head,
                arm: mesh.arm,
            }
        });

        Self {
            pipeline,
            armor_pipeline,
            pipeline_layout,
            mvp_layout,
            tex_layout,
            descriptor_pool,
            mvp_sets,
            head_mvp_sets,
            tex_set,
            mvp_buffers,
            mvp_allocations,
            head_mvp_buffers,
            head_mvp_allocations,
            arm_buffer,
            arm_allocation,
            arm_count: arm_verts.len() as u32,
            arm_mvp_sets,
            arm_mvp_buffers,
            arm_mvp_allocations,
            armor_meshes,
            swing_start: None,
            body_buffer,
            body_allocation,
            body_count: body_verts.len() as u32,
            head_buffer,
            head_allocation,
            head_count: head_verts.len() as u32,
        }
    }

    pub fn trigger_swing(&mut self) {
        self.swing_start = Some(std::time::Instant::now());
    }

    #[allow(clippy::too_many_arguments)]
    pub fn draw(
        &mut self,
        device: &vk::Device,
        cmd: vk::CommandBuffer,
        frame: usize,
        aspect: f32,
        screen_x: f32,
        screen_y: f32,
        mouse_px_x: f32,
        mouse_px_y: f32,
        screen_w: f32,
        screen_h: f32,
    ) {
        let _ = device;
        let center_px_x = screen_x * screen_w;
        let center_px_y = screen_y * screen_h;
        let body_y_rot_raw = ((mouse_px_x - center_px_x) / 40.0).atan();
        let head_x_rot_raw = ((mouse_px_y - center_px_y) / 40.0).atan();

        let head_y_rot_deg = body_y_rot_raw * 40.0;
        let body_y_rot_deg = head_y_rot_deg * 0.3;
        let body_rot_rad = std::f32::consts::PI + body_y_rot_deg.to_radians();
        let head_y_rot_rad = head_y_rot_deg.to_radians();
        let head_x_rot_rad = head_x_rot_raw * 20.0f32.to_radians();

        let fov = 0.6f32;
        let mut proj = proj::directx::perspective(fov, aspect, 0.1, 100.0);
        proj.y_axis.y *= -1.0;

        let ndc_x = screen_x * 2.0 - 1.0;
        let ndc_y = screen_y * 2.0 - 1.0;
        let clip_offset = Mat4::from_translation(Vec3::new(ndc_x, ndc_y, 0.0));

        let vp = clip_offset * proj * camera_view();
        self.record(
            cmd,
            frame,
            vp,
            body_rot_rad,
            body_rot_rad + head_y_rot_rad,
            head_x_rot_rad,
        );
    }

    /// Draws the player in a GUI box like vanilla's inventory preview:
    /// orthographic, 30 GUI px per model unit, mouse-follow rotation.
    pub fn draw_in_box(
        &mut self,
        cmd: vk::CommandBuffer,
        frame: usize,
        p: &crate::renderer::PlayerPreview,
        sw: f32,
        sh: f32,
        entity_renderer: &crate::renderer::pipelines::entity_renderer::EntityRenderer,
    ) {
        let cx = p.rect[0] + p.rect[2] / 2.0;
        let cy = p.rect[1] + p.rect[3] / 2.0;
        let xa = ((p.cursor.0 - cx) / (40.0 * p.gui_scale)).atan();
        let ya = ((p.cursor.1 - cy) / (40.0 * p.gui_scale)).atan();
        let body_rot_rad = std::f32::consts::PI + (xa * 20.0).to_radians();
        let head_yaw_rad = std::f32::consts::PI + (xa * 40.0).to_radians();
        let head_pitch_rad = (ya * 20.0).to_radians();

        let units_to_px = p.model_scale * p.gui_scale;
        let half_w = sw / (2.0 * units_to_px);
        let half_h = sh / (2.0 * units_to_px);
        let mut proj = proj::directx::orthographic(-half_w, half_w, -half_h, half_h, 0.1, 100.0);
        proj.y_axis.y *= -1.0;

        let clip_offset =
            Mat4::from_translation(Vec3::new(cx / sw * 2.0 - 1.0, cy / sh * 2.0 - 1.0, 0.0));
        let tilt = Mat4::from_rotation_x(-head_pitch_rad);
        // Vanilla lifts the entity by bbHeight/2 + 0.0625 = 0.9625 from its
        // feet; mesh feet are at y = -1.5, so +0.5375 centers it in the box.
        let center = Mat4::from_translation(Vec3::new(0.0, 0.5375, 0.0));

        self.record(
            cmd,
            frame,
            clip_offset * proj * camera_view() * tilt * center,
            body_rot_rad,
            head_yaw_rad,
            head_pitch_rad,
        );
        self.draw_armor(cmd, frame, p, entity_renderer);
    }

    fn draw_armor(
        &self,
        cmd: vk::CommandBuffer,
        frame: usize,
        p: &crate::renderer::PlayerPreview,
        entity_renderer: &crate::renderer::pipelines::entity_renderer::EntityRenderer,
    ) {
        cmd.bind_pipeline(vk::PipelineBindPoint::Graphics, self.armor_pipeline);
        // Vanilla `HumanoidArmorLayer.submit` draws chest, legs, feet, head.
        for slot in [1usize, 2, 3, 0] {
            let Some(armor) = &p.armor[slot] else {
                continue;
            };
            let mesh = &self.armor_meshes[slot];
            for (texture_set, tint) in entity_renderer.armor_preview_layers(armor, slot) {
                push_preview_style(cmd, self.pipeline_layout, tint, ARMOR_PREVIEW_ALPHA_CUTOFF);
                cmd.bind_vertex_buffers(0, &[mesh.buffer], &[0]);
                self.draw_armor_range(cmd, self.mvp_sets[frame], texture_set, mesh.body);
                self.draw_armor_range(cmd, self.head_mvp_sets[frame], texture_set, mesh.head);
                self.draw_armor_range(cmd, self.arm_mvp_sets[frame], texture_set, mesh.arm);
            }
        }
        // Keep subsequent preview draws independent of the final armor layer.
        push_preview_style(cmd, self.pipeline_layout, WHITE_TINT, PREVIEW_ALPHA_CUTOFF);
    }

    fn draw_armor_range(
        &self,
        cmd: vk::CommandBuffer,
        mvp_set: vk::DescriptorSet,
        texture_set: vk::DescriptorSet,
        range: ArmorRange,
    ) {
        if range.count == 0 {
            return;
        }
        // The entity and preview texture layouts are identically defined
        // single combined-image-sampler sets, so Vulkan permits the cached
        // equipment descriptor set to be rebound here without duplicating it.
        cmd.bind_descriptor_sets(
            vk::PipelineBindPoint::Graphics,
            self.pipeline_layout,
            0,
            &[mvp_set, texture_set],
            &[],
        );
        cmd.draw(range.count, 1, range.first, 0);
    }

    fn record(
        &mut self,
        cmd: vk::CommandBuffer,
        frame: usize,
        vp: Mat4,
        body_rot_rad: f32,
        head_yaw_rad: f32,
        head_pitch_rad: f32,
    ) {
        let body_rot_mat = Mat4::from_rotation_y(body_rot_rad);
        let body_mvp = vp * body_rot_mat;
        let head_rot_mat =
            Mat4::from_rotation_y(head_yaw_rad) * Mat4::from_rotation_x(head_pitch_rad);
        let head_mvp = vp * head_rot_mat;

        // Right arm swing animation
        let swing_angle = if let Some(start) = self.swing_start {
            let t = start.elapsed().as_secs_f32();
            if t > 0.4 {
                self.swing_start = None;
                0.0
            } else {
                let progress = t / 0.4;
                (progress * std::f32::consts::PI).sin() * -1.5
            }
        } else {
            0.0
        };

        // Arm pivot is at shoulder: (-5, 2, 0) in vanilla coords
        // In our Y-flipped space: (-5*PX, -2*PX, 0)
        let shoulder = Vec3::new(-5.0 * PX, -2.0 * PX, 0.0);
        let arm_swing = Mat4::from_translation(shoulder)
            * Mat4::from_rotation_x(swing_angle)
            * Mat4::from_translation(-shoulder);
        let arm_mvp = vp * body_rot_mat * arm_swing;

        write_uniform(&mut self.mvp_allocations[frame], &body_mvp);
        write_uniform(&mut self.head_mvp_allocations[frame], &head_mvp);
        write_uniform(&mut self.arm_mvp_allocations[frame], &arm_mvp);

        cmd.bind_pipeline(vk::PipelineBindPoint::Graphics, self.pipeline);
        push_preview_style(cmd, self.pipeline_layout, WHITE_TINT, PREVIEW_ALPHA_CUTOFF);

        cmd.bind_descriptor_sets(
            vk::PipelineBindPoint::Graphics,
            self.pipeline_layout,
            0,
            &[self.mvp_sets[frame], self.tex_set],
            &[],
        );
        cmd.bind_vertex_buffers(0, &[self.body_buffer], &[0]);
        cmd.draw(self.body_count, 1, 0, 0);

        cmd.bind_descriptor_sets(
            vk::PipelineBindPoint::Graphics,
            self.pipeline_layout,
            0,
            &[self.head_mvp_sets[frame], self.tex_set],
            &[],
        );
        cmd.bind_vertex_buffers(0, &[self.head_buffer], &[0]);
        cmd.draw(self.head_count, 1, 0, 0);

        cmd.bind_descriptor_sets(
            vk::PipelineBindPoint::Graphics,
            self.pipeline_layout,
            0,
            &[self.arm_mvp_sets[frame], self.tex_set],
            &[],
        );
        cmd.bind_vertex_buffers(0, &[self.arm_buffer], &[0]);
        cmd.draw(self.arm_count, 1, 0, 0);
    }

    pub fn recreate_pipeline(&mut self, device: &vk::Device, render_pass: vk::RenderPass) {
        device.destroy_pipeline(self.pipeline, None);
        device.destroy_pipeline(self.armor_pipeline, None);
        self.pipeline = create_pipeline(device, render_pass, self.pipeline_layout);
        self.armor_pipeline = create_armor_pipeline(device, render_pass, self.pipeline_layout);
    }

    pub fn destroy(&mut self, device: &vk::Device, allocator: &Arc<Mutex<Allocator>>) {
        let mut alloc = allocator.lock().unwrap();
        for i in 0..MAX_FRAMES_IN_FLIGHT {
            device.destroy_buffer(self.mvp_buffers[i], None);
            device.destroy_buffer(self.head_mvp_buffers[i], None);
            device.destroy_buffer(self.arm_mvp_buffers[i], None);
            alloc
                .free(std::mem::replace(&mut self.mvp_allocations[i], unsafe {
                    std::mem::zeroed()
                }))
                .ok();
            alloc
                .free(std::mem::replace(
                    &mut self.head_mvp_allocations[i],
                    unsafe { std::mem::zeroed() },
                ))
                .ok();
            alloc
                .free(std::mem::replace(
                    &mut self.arm_mvp_allocations[i],
                    unsafe { std::mem::zeroed() },
                ))
                .ok();
        }

        device.destroy_buffer(self.body_buffer, None);
        device.destroy_buffer(self.head_buffer, None);
        device.destroy_buffer(self.arm_buffer, None);
        for mesh in &mut self.armor_meshes {
            device.destroy_buffer(mesh.buffer, None);
            alloc
                .free(std::mem::replace(&mut mesh.allocation, unsafe {
                    std::mem::zeroed()
                }))
                .ok();
        }

        alloc
            .free(std::mem::replace(&mut self.body_allocation, unsafe {
                std::mem::zeroed()
            }))
            .ok();
        alloc
            .free(std::mem::replace(&mut self.head_allocation, unsafe {
                std::mem::zeroed()
            }))
            .ok();
        alloc
            .free(std::mem::replace(&mut self.arm_allocation, unsafe {
                std::mem::zeroed()
            }))
            .ok();
        drop(alloc);

        device.destroy_pipeline(self.pipeline, None);
        device.destroy_pipeline(self.armor_pipeline, None);
        device.destroy_pipeline_layout(self.pipeline_layout, None);
        device.destroy_descriptor_pool(self.descriptor_pool, None);
        device.destroy_descriptor_set_layout(self.mvp_layout, None);
        device.destroy_descriptor_set_layout(self.tex_layout, None);
    }
}

fn camera_view() -> Mat4 {
    view::look_at_mat4(Vec3::new(0.0, 0.0, -4.5), Vec3::ZERO, Vec3::Y)
}

pub(crate) fn write_uniform(alloc: &mut Allocation, mvp: &Mat4) {
    let uniform = Uniform {
        mvp: mvp.to_cols_array_2d(),
    };
    let bytes = bytemuck::bytes_of(&uniform);
    alloc.mapped_slice_mut().unwrap()[..bytes.len()].copy_from_slice(bytes);
}

/// The textured-model preview pipeline, shared with the enchanting book
/// preview. Vanilla's GL-CCW front faces come out clockwise in Vulkan's
/// y-down framebuffer coords, so this culls the CCW set.
#[derive(Clone, Copy)]
struct PreviewPipelineState {
    cull_mode: vk::CullModeFlags,
    blend_enable: vk::Bool32,
}

const TEXTURED_PREVIEW_STATE: PreviewPipelineState = PreviewPipelineState {
    cull_mode: vk::CullModeFlags::Front,
    blend_enable: vk::TRUE,
};
const ARMOR_PREVIEW_STATE: PreviewPipelineState = PreviewPipelineState {
    cull_mode: vk::CullModeFlags::empty(),
    blend_enable: vk::FALSE,
};

pub(crate) fn create_pipeline(
    device: &vk::Device,
    render_pass: vk::RenderPass,
    layout: vk::PipelineLayout,
) -> vk::Pipeline {
    create_pipeline_with_state(device, render_pass, layout, TEXTURED_PREVIEW_STATE)
}

fn create_armor_pipeline(
    device: &vk::Device,
    render_pass: vk::RenderPass,
    layout: vk::PipelineLayout,
) -> vk::Pipeline {
    // Vanilla renders equipment through ARMOR_CUTOUT_NO_CULL: alpha-tested,
    // opaque color writes and no face culling.
    create_pipeline_with_state(device, render_pass, layout, ARMOR_PREVIEW_STATE)
}

fn create_pipeline_with_state(
    device: &vk::Device,
    render_pass: vk::RenderPass,
    layout: vk::PipelineLayout,
    state: PreviewPipelineState,
) -> vk::Pipeline {
    let vert_spv = shader::include_spirv!("hand.vert.spv");
    let frag_spv = shader::include_spirv!("preview.frag.spv");
    let vert_mod = shader::create_shader_module(device, vert_spv);
    let frag_mod = shader::create_shader_module(device, frag_spv);

    let stages = [
        vk::PipelineShaderStageCreateInfo {
            stage: vk::ShaderStageFlags::Vertex,
            module: vert_mod,
            name: c"main".as_ptr(),
            ..Default::default()
        },
        vk::PipelineShaderStageCreateInfo {
            stage: vk::ShaderStageFlags::Fragment,
            module: frag_mod,
            name: c"main".as_ptr(),
            ..Default::default()
        },
    ];

    let binding = vk::VertexInputBindingDescription {
        binding: 0,
        stride: std::mem::size_of::<Vertex>() as u32,
        input_rate: vk::VertexInputRate::Vertex,
    };
    let attrs = [
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
        vertex_binding_description_count: 1,
        vertex_binding_descriptions: &binding,
        vertex_attribute_description_count: attrs.len() as u32,
        vertex_attribute_descriptions: attrs.as_ptr(),
        ..Default::default()
    };
    let input_assembly = vk::PipelineInputAssemblyStateCreateInfo {
        topology: vk::PrimitiveTopology::TriangleList,
        ..Default::default()
    };
    let viewport_state = vk::PipelineViewportStateCreateInfo {
        viewport_count: 1,
        scissor_count: 1,
        ..Default::default()
    };
    let rasterizer = vk::PipelineRasterizationStateCreateInfo {
        polygon_mode: vk::PolygonMode::Fill,
        cull_mode: state.cull_mode,
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
    let blend_attachment = vk::PipelineColorBlendAttachmentState {
        blend_enable: state.blend_enable,
        src_color_blend_factor: vk::BlendFactor::SrcAlpha,
        dst_color_blend_factor: vk::BlendFactor::OneMinusSrcAlpha,
        color_blend_op: vk::BlendOp::Add,
        src_alpha_blend_factor: vk::BlendFactor::One,
        dst_alpha_blend_factor: vk::BlendFactor::OneMinusSrcAlpha,
        alpha_blend_op: vk::BlendOp::Add,
        color_write_mask: vk::ColorComponentFlags::RGBA,
    };
    let color_blending = vk::PipelineColorBlendStateCreateInfo {
        attachment_count: 1,
        attachments: &blend_attachment,
        ..Default::default()
    };
    let dynamic_states = [vk::DynamicState::Viewport, vk::DynamicState::Scissor];
    let dynamic_state = vk::PipelineDynamicStateCreateInfo {
        dynamic_state_count: dynamic_states.len() as u32,
        dynamic_states: dynamic_states.as_ptr(),
        ..Default::default()
    };

    let info = [vk::GraphicsPipelineCreateInfo {
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
            &info,
            None,
            slice::from_mut(&mut pipeline),
        )
        .expect("failed to create preview pipeline");

    device.destroy_shader_module(vert_mod, None);
    device.destroy_shader_module(frag_mod, None);
    pipeline
}

// Vanilla model coordinates: Y-down, 1 unit = 1 pixel; converted to Y-up by
// negating Y, then scaled by PX. Unlike the entity renderer (X flip + face
// rect on -Z), this mesh is built Z-reflected — face rect on +Z, viewed from
// -Z — which yields the identical image; don't "fix" one convention without
// the other.
const PX: f32 = 1.0 / 16.0;

fn uv(x: u32, y: u32, w: u32, h: u32) -> [[f32; 2]; 4] {
    let s = 64.0;
    [
        [x as f32 / s, y as f32 / s],
        [(x + w) as f32 / s, y as f32 / s],
        [(x + w) as f32 / s, (y + h) as f32 / s],
        [x as f32 / s, (y + h) as f32 / s],
    ]
}

fn quad(verts: &mut Vec<Vertex>, pos: [[f32; 3]; 4], uvs: [[f32; 2]; 4]) {
    for &i in &[0u32, 1, 2, 2, 3, 0] {
        verts.push(Vertex {
            position: pos[i as usize],
            uv: uvs[i as usize],
        });
    }
}

fn add_model_cube(
    verts: &mut Vec<Vertex>,
    part_offset: Vec3,
    cube: &crate::renderer::entity_model::ModelCube,
) {
    let mut translated = *cube;
    translated.origin += part_offset;
    let positions = crate::renderer::entity_model::cube_face_positions(&translated, true);
    let face_uvs = crate::renderer::entity_model::cube_face_uvs(&translated);

    for (positions, uv) in positions.iter().zip(face_uvs) {
        let reflect = |p: [f32; 3]| [p[0], p[1], -p[2]];
        let mut corners = [
            (reflect(positions[0]), [uv[2] / 64.0, uv[1] / 32.0]),
            (reflect(positions[1]), [uv[0] / 64.0, uv[1] / 32.0]),
            (reflect(positions[2]), [uv[0] / 64.0, uv[3] / 32.0]),
            (reflect(positions[3]), [uv[2] / 64.0, uv[3] / 32.0]),
        ];
        // `cube_face_positions` + this corner order already reproduce vanilla
        // mirror semantics. Z reflection reverses winding once more; cancel it
        // here so the preview pipeline's front-face culling matches the skin.
        if !cube.mirror {
            corners.reverse();
        }
        for &i in &[0usize, 1, 2, 0, 2, 3] {
            verts.push(Vertex {
                position: corners[i].0,
                uv: corners[i].1,
            });
        }
    }
}

fn build_armor_preview_mesh(
    slot: crate::renderer::entity_model::HumanoidArmorSlot,
) -> PreviewArmorCpuMesh {
    let mut body = Vec::new();
    let mut head = Vec::new();
    let mut arm = Vec::new();
    for part in crate::renderer::entity_model::humanoid_armor_parts(slot) {
        let target = match part.name.as_str() {
            "head" => &mut head,
            "right_arm" => &mut arm,
            _ => &mut body,
        };
        for cube in &part.cubes {
            add_model_cube(target, part.offset, cube);
        }
    }

    let body_range = ArmorRange {
        first: 0,
        count: body.len() as u32,
    };
    let head_range = ArmorRange {
        first: body_range.count,
        count: head.len() as u32,
    };
    let arm_range = ArmorRange {
        first: body_range.count + head_range.count,
        count: arm.len() as u32,
    };
    body.extend(head);
    body.extend(arm);
    PreviewArmorCpuMesh {
        vertices: body,
        body: body_range,
        head: head_range,
        arm: arm_range,
    }
}

// addBox(ox, oy, oz, w, h, d) at pivot (px, py, pz) with UV origin (tx, ty)
// Vanilla Y-down, we flip to Y-up
#[allow(clippy::too_many_arguments)]
fn add_box(
    verts: &mut Vec<Vertex>,
    px: f32,
    py: f32,
    _pz: f32,
    ox: f32,
    oy: f32,
    oz: f32,
    w: f32,
    h: f32,
    d: f32,
    tx: u32,
    ty: u32,
    tw: u32,
    th: u32,
    td: u32,
) {
    let x0 = (px + ox) * PX;
    let x1 = (px + ox + w) * PX;
    // Flip Y: vanilla Y-down, we want Y-up
    let y0 = -(py + oy + h) * PX;
    let y1 = -(py + oy) * PX;
    let z0 = (_pz + oz) * PX;
    let z1 = (_pz + oz + d) * PX;

    // Front (+Z)
    quad(
        verts,
        [[x0, y1, z1], [x1, y1, z1], [x1, y0, z1], [x0, y0, z1]],
        uv(tx + td, ty + td, tw, th),
    );
    // Back (-Z)
    quad(
        verts,
        [[x1, y1, z0], [x0, y1, z0], [x0, y0, z0], [x1, y0, z0]],
        uv(tx + td + tw + td, ty + td, tw, th),
    );
    // Right (+X)
    quad(
        verts,
        [[x1, y1, z1], [x1, y1, z0], [x1, y0, z0], [x1, y0, z1]],
        uv(tx + td + tw, ty + td, td, th),
    );
    // Left (-X)
    quad(
        verts,
        [[x0, y1, z0], [x0, y1, z1], [x0, y0, z1], [x0, y0, z0]],
        uv(tx, ty + td, td, th),
    );
    // Top (+Y in our space = -Y in vanilla = top of head)
    quad(
        verts,
        [[x0, y1, z0], [x1, y1, z0], [x1, y1, z1], [x0, y1, z1]],
        uv(tx + td, ty, tw, td),
    );
    // Bottom (-Y in our space = +Y in vanilla = bottom). Vanilla reads the
    // maxY face's rect with V reversed (`ModelPart.Cube`'s UP polygon).
    let mut bottom_uv = uv(tx + td + tw, ty, tw, td);
    bottom_uv.reverse();
    quad(
        verts,
        [[x0, y0, z1], [x1, y0, z1], [x1, y0, z0], [x0, y0, z0]],
        bottom_uv,
    );
}

fn build_head_mesh() -> Vec<Vertex> {
    let mut v = Vec::new();
    let e = 0.5; // hat overlay inflation (1 pixel bigger in vanilla)
    // Head: addBox(-4, -8, -4, 8, 8, 8) @ (0, 0, 0), UV (0, 0)
    add_box(
        &mut v, 0.0, 0.0, 0.0, -4.0, -8.0, -4.0, 8.0, 8.0, 8.0, 0, 0, 8, 8, 8,
    );
    // Hat overlay: same box inflated by 0.5, UV (32, 0)
    add_box(
        &mut v,
        0.0,
        0.0,
        0.0,
        -4.0 - e,
        -8.0 - e,
        -4.0 - e,
        8.0 + e * 2.0,
        8.0 + e * 2.0,
        8.0 + e * 2.0,
        32,
        0,
        8,
        8,
        8,
    );
    v
}

fn build_body_mesh(slim: bool) -> Vec<Vertex> {
    let mut v = Vec::new();
    let e = 0.25; // overlay inflation for body/limbs
    let arm_tw: u32 = if slim { 3 } else { 4 };
    let arm_w = arm_tw as f32;

    // Body: addBox(-4, 0, -2, 8, 12, 4) @ (0, 0, 0), UV (16, 16)
    add_box(
        &mut v, 0.0, 0.0, 0.0, -4.0, 0.0, -2.0, 8.0, 12.0, 4.0, 16, 16, 8, 12, 4,
    );
    // Jacket overlay: UV (16, 32)
    add_box(
        &mut v,
        0.0,
        0.0,
        0.0,
        -4.0 - e,
        0.0 - e,
        -2.0 - e,
        8.0 + e * 2.0,
        12.0 + e * 2.0,
        4.0 + e * 2.0,
        16,
        32,
        8,
        12,
        4,
    );

    // Left arm: addBox(-1, -2, -2, 4|3, 12, 4) @ (5, 2, 0), UV (32, 48)
    add_box(
        &mut v, 5.0, 2.0, 0.0, -1.0, -2.0, -2.0, arm_w, 12.0, 4.0, 32, 48, arm_tw, 12, 4,
    );
    // Left sleeve overlay: UV (48, 48)
    add_box(
        &mut v,
        5.0,
        2.0,
        0.0,
        -1.0 - e,
        -2.0 - e,
        -2.0 - e,
        arm_w + e * 2.0,
        12.0 + e * 2.0,
        4.0 + e * 2.0,
        48,
        48,
        arm_tw,
        12,
        4,
    );

    // Right leg: addBox(-2, 0, -2, 4, 12, 4) @ (-1.9, 12, 0), UV (0, 16)
    add_box(
        &mut v, -1.9, 12.0, 0.0, -2.0, 0.0, -2.0, 4.0, 12.0, 4.0, 0, 16, 4, 12, 4,
    );
    // Right pants overlay: UV (0, 32)
    add_box(
        &mut v,
        -1.9,
        12.0,
        0.0,
        -2.0 - e,
        0.0 - e,
        -2.0 - e,
        4.0 + e * 2.0,
        12.0 + e * 2.0,
        4.0 + e * 2.0,
        0,
        32,
        4,
        12,
        4,
    );

    // Left leg: addBox(-2, 0, -2, 4, 12, 4) @ (1.9, 12, 0), UV (16, 48)
    add_box(
        &mut v, 1.9, 12.0, 0.0, -2.0, 0.0, -2.0, 4.0, 12.0, 4.0, 16, 48, 4, 12, 4,
    );
    // Left pants overlay: UV (0, 48)
    add_box(
        &mut v,
        1.9,
        12.0,
        0.0,
        -2.0 - e,
        0.0 - e,
        -2.0 - e,
        4.0 + e * 2.0,
        12.0 + e * 2.0,
        4.0 + e * 2.0,
        0,
        48,
        4,
        12,
        4,
    );
    v
}

fn build_right_arm_mesh(slim: bool) -> Vec<Vertex> {
    let mut v = Vec::new();
    let e = 0.25;
    let arm_tw: u32 = if slim { 3 } else { 4 };
    let arm_w = arm_tw as f32;
    let ox = if slim { -2.0 } else { -3.0 };
    // Right arm: addBox(-3|-2, -2, -2, 4|3, 12, 4) @ (-5, 2, 0), UV (40, 16)
    add_box(
        &mut v, -5.0, 2.0, 0.0, ox, -2.0, -2.0, arm_w, 12.0, 4.0, 40, 16, arm_tw, 12, 4,
    );
    // Right sleeve overlay: UV (40, 32)
    add_box(
        &mut v,
        -5.0,
        2.0,
        0.0,
        ox - e,
        -2.0 - e,
        -2.0 - e,
        arm_w + e * 2.0,
        12.0 + e * 2.0,
        4.0 + e * 2.0,
        40,
        32,
        arm_tw,
        12,
        4,
    );
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::renderer::entity_model::{HumanoidArmorSlot, ModelCube};

    #[test]
    fn model_cube_bridge_matches_existing_reflected_preview_box() {
        let cube = ModelCube {
            origin: Vec3::new(-4.0, 0.0, -2.0),
            size: Vec3::new(8.0, 12.0, 4.0),
            tex_offset: (16, 16),
            deformation: 0.0,
            mirror: false,
        };
        let mut actual = Vec::new();
        add_model_cube(&mut actual, Vec3::ZERO, &cube);

        let mut expected = Vec::new();
        add_box(
            &mut expected,
            0.0,
            0.0,
            0.0,
            -4.0,
            0.0,
            -2.0,
            8.0,
            12.0,
            4.0,
            16,
            16,
            8,
            12,
            4,
        );
        // `add_box` targets a 64x64 skin while equipment textures are 64x32.
        for vertex in &mut expected {
            vertex.uv[1] *= 2.0;
        }

        assert_eq!(actual.len(), expected.len());
        for vertex in &actual {
            assert_eq!(
                actual
                    .iter()
                    .filter(|candidate| *candidate == vertex)
                    .count(),
                expected
                    .iter()
                    .filter(|candidate| *candidate == vertex)
                    .count(),
                "preview bridge changed vertex/UV mapping for {vertex:?}"
            );
        }
    }

    #[test]
    fn armor_preview_pipeline_matches_cutout_no_cull_state() {
        assert_eq!(ARMOR_PREVIEW_STATE.cull_mode, vk::CullModeFlags::empty());
        assert_eq!(ARMOR_PREVIEW_STATE.blend_enable, vk::FALSE);
        assert_eq!(TEXTURED_PREVIEW_STATE.cull_mode, vk::CullModeFlags::Front);
        assert_eq!(TEXTURED_PREVIEW_STATE.blend_enable, vk::TRUE);
    }

    #[test]
    fn armor_preview_meshes_split_slot_parts_by_preview_transform() {
        let head = build_armor_preview_mesh(HumanoidArmorSlot::Head);
        assert_eq!(head.body.count, 0);
        assert!(head.head.count > 0);
        assert_eq!(head.arm.count, 0);

        let chest = build_armor_preview_mesh(HumanoidArmorSlot::Chest);
        assert!(chest.body.count > 0);
        assert_eq!(chest.head.count, 0);
        assert!(chest.arm.count > 0);

        for slot in [HumanoidArmorSlot::Legs, HumanoidArmorSlot::Feet] {
            let mesh = build_armor_preview_mesh(slot);
            assert!(mesh.body.count > 0);
            assert_eq!(mesh.head.count, 0);
            assert_eq!(mesh.arm.count, 0);
        }
    }
}
