use std::path::Path;
use std::slice;
use std::sync::{Arc, Mutex};

use glam::camera::rh::proj;
use glam::{Mat4, Vec3};
use pomme_gpu_allocator::vulkan::{Allocation, Allocator};
use pyronyx::vk;

use crate::renderer::camera::CameraUniform;
use crate::renderer::chunk::atlas::TextureAtlas;
use crate::renderer::pipelines::item_display::{DisplayResolver, DisplayTransform};
use crate::renderer::pipelines::item_entity::{self, ItemEntityPipeline, push_model_light};
use crate::renderer::util;

pub struct GuiItemPipeline {
    pipeline: vk::Pipeline,
    pipeline_layout: vk::PipelineLayout,
    camera_layout: vk::DescriptorSetLayout,
    atlas_layout: vk::DescriptorSetLayout,
    descriptor_pool: vk::DescriptorPool,
    camera_set: vk::DescriptorSet,
    atlas_set: vk::DescriptorSet,
    camera_buffer: vk::Buffer,
    camera_alloc: Option<Allocation>,
    sampler: vk::Sampler,
    atlas_px: u32,
    display: DisplayResolver,
}

impl GuiItemPipeline {
    pub fn new(
        device: &vk::Device,
        atlas_render_pass: vk::RenderPass,
        atlas_px: u32,
        allocator: &Arc<Mutex<Allocator>>,
        atlas: &TextureAtlas,
        jar_assets_dir: &Path,
    ) -> Self {
        let camera_layout = util::create_descriptor_set_layout(
            device,
            vk::DescriptorType::UniformBuffer,
            vk::ShaderStageFlags::Vertex,
        );
        let atlas_layout = util::create_descriptor_set_layout(
            device,
            vk::DescriptorType::CombinedImageSampler,
            vk::ShaderStageFlags::Fragment,
        );

        let push_range = vk::PushConstantRange {
            stage_flags: vk::ShaderStageFlags::Vertex | vk::ShaderStageFlags::Fragment,
            offset: 0,
            size: 68,
        };
        let layouts = [camera_layout, atlas_layout];
        let layout_info = vk::PipelineLayoutCreateInfo {
            set_layout_count: layouts.len() as u32,
            set_layouts: layouts.as_ptr(),
            push_constant_range_count: 1,
            push_constant_ranges: &push_range,
            ..Default::default()
        };
        let pipeline_layout = device
            .create_pipeline_layout(&layout_info, None)
            .expect("failed to create gui_item pipeline layout");

        let pipeline = item_entity::create_pipeline_with_front_face(
            device,
            atlas_render_pass,
            pipeline_layout,
            vk::FrontFace::Clockwise,
        );

        let pool_sizes = [
            vk::DescriptorPoolSize {
                ty: vk::DescriptorType::UniformBuffer,
                descriptor_count: 1,
            },
            vk::DescriptorPoolSize {
                ty: vk::DescriptorType::CombinedImageSampler,
                descriptor_count: 1,
            },
        ];
        let pool_info = vk::DescriptorPoolCreateInfo {
            max_sets: 2,
            pool_size_count: pool_sizes.len() as u32,
            pool_sizes: pool_sizes.as_ptr(),
            ..Default::default()
        };
        let descriptor_pool = device
            .create_descriptor_pool(&pool_info, None)
            .expect("failed to create gui_item descriptor pool");

        let cam_alloc_info = vk::DescriptorSetAllocateInfo {
            descriptor_pool,
            descriptor_set_count: 1,
            set_layouts: &camera_layout,
            ..Default::default()
        };
        let mut camera_set = vk::DescriptorSet::null();
        device
            .allocate_descriptor_sets(&cam_alloc_info, slice::from_mut(&mut camera_set))
            .expect("failed to allocate gui_item camera set");

        let atlas_alloc_info = vk::DescriptorSetAllocateInfo {
            descriptor_pool,
            descriptor_set_count: 1,
            set_layouts: &atlas_layout,
            ..Default::default()
        };
        let mut atlas_set = vk::DescriptorSet::null();
        device
            .allocate_descriptor_sets(&atlas_alloc_info, slice::from_mut(&mut atlas_set))
            .expect("failed to allocate gui_item atlas set");

        let (camera_buffer, camera_alloc) = util::create_uniform_buffer(
            device,
            allocator,
            size_of::<CameraUniform>() as u64,
            "gui_item_camera",
        );
        let cam_buf_info = vk::DescriptorBufferInfo {
            buffer: camera_buffer,
            offset: 0,
            range: size_of::<CameraUniform>() as u64,
        };
        let cam_write = vk::WriteDescriptorSet {
            dst_set: camera_set,
            dst_binding: 0,
            descriptor_type: vk::DescriptorType::UniformBuffer,
            descriptor_count: 1,
            buffer_info: &cam_buf_info,
            ..Default::default()
        };

        let sampler_info = vk::SamplerCreateInfo {
            mag_filter: vk::Filter::Nearest,
            min_filter: vk::Filter::Nearest,
            mipmap_mode: vk::SamplerMipmapMode::Nearest,
            address_mode_u: vk::SamplerAddressMode::ClampToEdge,
            address_mode_v: vk::SamplerAddressMode::ClampToEdge,
            address_mode_w: vk::SamplerAddressMode::ClampToEdge,
            min_lod: 0.0,
            max_lod: 0.0,
            ..Default::default()
        };
        let sampler = device
            .create_sampler(&sampler_info, None)
            .expect("failed to create gui_item sampler");

        let atlas_img_info = vk::DescriptorImageInfo {
            sampler,
            image_view: atlas.view,
            image_layout: vk::ImageLayout::ShaderReadOnlyOptimal,
        };
        let atlas_write = vk::WriteDescriptorSet {
            dst_set: atlas_set,
            dst_binding: 0,
            descriptor_type: vk::DescriptorType::CombinedImageSampler,
            descriptor_count: 1,
            image_info: &atlas_img_info,
            ..Default::default()
        };
        device.update_descriptor_sets(&[cam_write, atlas_write], &[]);

        let mut this = Self {
            pipeline,
            pipeline_layout,
            camera_layout,
            atlas_layout,
            descriptor_pool,
            camera_set,
            atlas_set,
            camera_buffer,
            camera_alloc: Some(camera_alloc),
            sampler,
            atlas_px,
            display: DisplayResolver::new(jar_assets_dir, "gui"),
        };
        this.write_atlas_ortho(atlas_px as f32);
        this
    }

    pub fn set_atlas_px(&mut self, atlas_px: u32) {
        self.atlas_px = atlas_px;
        self.write_atlas_ortho(atlas_px as f32);
    }

    pub fn recreate_pipeline(&mut self, device: &vk::Device, atlas_render_pass: vk::RenderPass) {
        device.destroy_pipeline(self.pipeline, None);
        self.pipeline = item_entity::create_pipeline_with_front_face(
            device,
            atlas_render_pass,
            self.pipeline_layout,
            vk::FrontFace::Clockwise,
        );
    }

    fn write_atlas_ortho(&mut self, atlas_px: f32) {
        // Bottom/top swapped to Y-invert the projection (matches vanilla's
        // `invertY=true`).
        let view_proj =
            proj::directx::orthographic(0.0, atlas_px, atlas_px, 0.0, -10000.0, 10000.0);
        let uniform = CameraUniform::with_view_proj(view_proj);
        let bytes = bytemuck::bytes_of(&uniform);
        if let Some(alloc) = self.camera_alloc.as_mut() {
            alloc.mapped_slice_mut().unwrap()[..bytes.len()].copy_from_slice(bytes);
        }
    }

    pub fn rebind_atlas(&self, device: &vk::Device, atlas: &TextureAtlas) {
        let img_info = vk::DescriptorImageInfo {
            sampler: self.sampler,
            image_view: atlas.view,
            image_layout: vk::ImageLayout::ShaderReadOnlyOptimal,
        };
        let write = vk::WriteDescriptorSet {
            dst_set: self.atlas_set,
            dst_binding: 0,
            descriptor_type: vk::DescriptorType::CombinedImageSampler,
            descriptor_count: 1,
            image_info: &img_info,
            ..Default::default()
        };
        device.update_descriptor_sets(&[write], &[]);
    }

    pub fn bind_for_bake_pass(&self, cmd: vk::CommandBuffer) {
        cmd.bind_pipeline(vk::PipelineBindPoint::Graphics, self.pipeline);
        cmd.bind_descriptor_sets(
            vk::PipelineBindPoint::Graphics,
            self.pipeline_layout,
            0,
            &[self.camera_set, self.atlas_set],
            &[],
        );
        let viewport = vk::Viewport {
            x: 0.0,
            y: 0.0,
            width: self.atlas_px as f32,
            height: self.atlas_px as f32,
            min_depth: 0.0,
            max_depth: 1.0,
        };
        cmd.set_viewport(0, &[viewport]);
    }

    #[allow(clippy::too_many_arguments)]
    pub fn bake_to_slot(
        &self,
        cmd: vk::CommandBuffer,
        item_entity: &ItemEntityPipeline,
        slot_x_px: u32,
        slot_y_px: u32,
        slot_size_px: u32,
        item_name: &str,
        is_block: bool,
    ) {
        let Some((buffer, vertex_count)) = item_entity.mesh_handle(item_name) else {
            return;
        };

        let display = self.display.resolve(item_name, default_display(is_block));
        let model = slot_model_matrix(
            slot_x_px as f32,
            slot_y_px as f32,
            slot_size_px as f32,
            slot_size_px as f32,
            display,
        );

        cmd.bind_vertex_buffers(0, &[buffer], &[0]);
        push_model_light(cmd, self.pipeline_layout, &model, 1.0);
        cmd.draw(vertex_count, 1, 0, 0);
    }

    pub fn destroy(&mut self, device: &vk::Device, allocator: &Arc<Mutex<Allocator>>) {
        device.destroy_pipeline(self.pipeline, None);
        device.destroy_pipeline_layout(self.pipeline_layout, None);
        device.destroy_descriptor_pool(self.descriptor_pool, None);
        device.destroy_descriptor_set_layout(self.camera_layout, None);
        device.destroy_descriptor_set_layout(self.atlas_layout, None);
        device.destroy_sampler(self.sampler, None);
        device.destroy_buffer(self.camera_buffer, None);
        if let Some(a) = self.camera_alloc.take() {
            allocator.lock().unwrap().free(a).ok();
        }
    }
}

fn slot_model_matrix(x: f32, y: f32, w: f32, h: f32, display: DisplayTransform) -> Mat4 {
    let depth_scale = w.max(h);
    // Vertices are pre-centered to `[-0.5, +0.5]` by `build_item_mesh`, so no
    // T(-0.5) here.
    Mat4::from_translation(Vec3::new(x + w * 0.5, y + h * 0.5, 0.0))
        * Mat4::from_scale(Vec3::new(w, -h, depth_scale))
        * display.to_matrix()
}

fn default_display(is_block: bool) -> DisplayTransform {
    if is_block {
        DisplayTransform {
            rotation: Vec3::new(30.0, 225.0, 0.0),
            translation: Vec3::ZERO,
            scale: Vec3::splat(0.625),
        }
    } else {
        DisplayTransform::IDENTITY
    }
}
