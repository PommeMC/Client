use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::slice;
use std::sync::{Arc, Mutex};

use glam::{Mat4, Vec3};
use pomme_gpu_allocator::vulkan::{Allocation, Allocator};
use pyronyx::vk;

use crate::renderer::camera::CameraUniform;
use crate::renderer::chunk::atlas::TextureAtlas;
use crate::renderer::pipelines::item_entity::{self, ItemEntityPipeline};
use crate::renderer::util;
use crate::world::block::model::{find_first_model_string, find_first_string_for_key};

const MODEL_PARENT_LIMIT: u32 = 16;

#[derive(Debug, Clone, Copy)]
struct DisplayTransform {
    rotation: Vec3,
    translation: Vec3,
    scale: Vec3,
}

impl DisplayTransform {
    const IDENTITY: Self = Self {
        rotation: Vec3::ZERO,
        translation: Vec3::ZERO,
        scale: Vec3::ONE,
    };

    fn to_matrix(self) -> Mat4 {
        let t = Mat4::from_translation(self.translation);
        let r = Mat4::from_rotation_x(self.rotation.x.to_radians())
            * Mat4::from_rotation_y(self.rotation.y.to_radians())
            * Mat4::from_rotation_z(self.rotation.z.to_radians());
        let s = Mat4::from_scale(self.scale);
        t * r * s
    }
}

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
    display_cache: RefCell<HashMap<String, DisplayTransform>>,
    items_dir: PathBuf,
    models_dir: PathBuf,
}

impl GuiItemPipeline {
    pub fn new(
        device: &vk::Device,
        render_pass: vk::RenderPass,
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
            render_pass,
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

        let mc_base = jar_assets_dir.join("minecraft");
        Self {
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
            display_cache: RefCell::new(HashMap::new()),
            items_dir: mc_base.join("items"),
            models_dir: mc_base.join("models"),
        }
    }

    pub fn update_camera(&mut self, screen_w: f32, screen_h: f32) {
        let view_proj = Mat4::orthographic_rh(0.0, screen_w, 0.0, screen_h, -10000.0, 10000.0);
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

    #[allow(clippy::too_many_arguments)]
    pub fn draw_one(
        &self,
        cmd: vk::CommandBuffer,
        item_entity: &ItemEntityPipeline,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        item_name: &str,
        is_block: bool,
    ) {
        let Some((buffer, vertex_count)) = item_entity.mesh_handle(item_name) else {
            return;
        };

        let display = self.resolve_display(item_name, is_block);
        let model = slot_model_matrix(x, y, w, h, display);

        let depth_clear_rect = vk::ClearRect {
            rect: vk::Rect2D {
                offset: vk::Offset2D {
                    x: x.floor() as i32,
                    y: y.floor() as i32,
                },
                extent: vk::Extent2D {
                    width: w.ceil() as u32,
                    height: h.ceil() as u32,
                },
            },
            base_array_layer: 0,
            layer_count: 1,
        };
        let depth_clear = vk::ClearAttachment {
            aspect_mask: vk::ImageAspectFlags::Depth,
            color_attachment: 0,
            clear_value: vk::ClearValue {
                depth_stencil: vk::ClearDepthStencilValue {
                    depth: 1.0,
                    stencil: 0,
                },
            },
        };
        cmd.clear_attachments(&[depth_clear], &[depth_clear_rect]);

        cmd.bind_pipeline(vk::PipelineBindPoint::Graphics, self.pipeline);
        cmd.bind_descriptor_sets(
            vk::PipelineBindPoint::Graphics,
            self.pipeline_layout,
            0,
            &[self.camera_set, self.atlas_set],
            &[],
        );
        cmd.bind_vertex_buffers(0, &[buffer], &[0]);

        let mvp_data = model.to_cols_array();
        let mvp_bytes = bytemuck::bytes_of(&mvp_data);
        let light: f32 = 1.0;
        let light_bytes = bytemuck::bytes_of(&light);
        cmd.push_constants(
            self.pipeline_layout,
            vk::ShaderStageFlags::Vertex | vk::ShaderStageFlags::Fragment,
            0,
            mvp_bytes,
        );
        cmd.push_constants(
            self.pipeline_layout,
            vk::ShaderStageFlags::Vertex | vk::ShaderStageFlags::Fragment,
            64,
            light_bytes,
        );
        cmd.draw(vertex_count, 1, 0, 0);
    }

    fn resolve_display(&self, item_name: &str, is_block: bool) -> DisplayTransform {
        if let Some(t) = self.display_cache.borrow().get(item_name) {
            return *t;
        }
        let model_path = resolve_item_model_path(item_name, &self.items_dir);
        let resolved = match model_path {
            Some(path) => resolve_display_gui(&path, &self.models_dir, is_block),
            None => default_display(is_block),
        };
        self.display_cache
            .borrow_mut()
            .insert(item_name.to_string(), resolved);
        resolved
    }

    pub fn recreate_pipeline(&mut self, device: &vk::Device, render_pass: vk::RenderPass) {
        device.destroy_pipeline(self.pipeline, None);
        self.pipeline = item_entity::create_pipeline_with_front_face(
            device,
            render_pass,
            self.pipeline_layout,
            vk::FrontFace::Clockwise,
        );
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
    // The negative-Y scale flips winding; the GUI pipeline uses
    // `FrontFace::Clockwise` to keep `CullMode::Back` correct.
    Mat4::from_translation(Vec3::new(x, y + h, 0.0))
        * Mat4::from_scale(Vec3::new(w, -h, depth_scale))
        * Mat4::from_translation(Vec3::splat(0.5))
        * display.to_matrix()
        * Mat4::from_translation(Vec3::splat(-0.5))
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

fn read_json(path: &Path) -> Option<serde_json::Value> {
    let s = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&s).ok()
}

fn strip_mc_ns(s: &str) -> &str {
    s.strip_prefix("minecraft:").unwrap_or(s)
}

fn resolve_item_model_path(name: &str, items_dir: &Path) -> Option<String> {
    let item_json = read_json(&items_dir.join(format!("{name}.json")))?;
    let model_path = find_first_model_string(&item_json)
        .or_else(|| find_first_string_for_key(&item_json, "base"))?;
    Some(strip_mc_ns(&model_path).to_string())
}

fn parse_vec3(value: &serde_json::Value, default: Vec3) -> Vec3 {
    let Some(arr) = value.as_array() else {
        return default;
    };
    let get = |i: usize| arr.get(i).and_then(|v| v.as_f64()).map(|v| v as f32);
    Vec3::new(
        get(0).unwrap_or(default.x),
        get(1).unwrap_or(default.y),
        get(2).unwrap_or(default.z),
    )
}

fn parse_display_transform(json: &serde_json::Value) -> Option<DisplayTransform> {
    let obj = json.as_object()?;
    let rotation = obj
        .get("rotation")
        .map(|v| parse_vec3(v, Vec3::ZERO))
        .unwrap_or(Vec3::ZERO);
    let translation = obj
        .get("translation")
        .map(|v| parse_vec3(v, Vec3::ZERO))
        .unwrap_or(Vec3::ZERO);
    let scale = obj
        .get("scale")
        .map(|v| parse_vec3(v, Vec3::ONE))
        .unwrap_or(Vec3::ONE);
    Some(DisplayTransform {
        rotation,
        translation: translation * (1.0 / 16.0),
        scale,
    })
}

fn resolve_display_gui(start_path: &str, models_dir: &Path, is_block: bool) -> DisplayTransform {
    let mut current = Some(start_path.to_string());
    let mut depth = 0u32;
    while let Some(path) = current.take() {
        if depth >= MODEL_PARENT_LIMIT {
            break;
        }
        depth += 1;

        let file = models_dir.join(format!("{path}.json"));
        let Some(json) = read_json(&file) else { break };

        if let Some(gui) = json.get("display").and_then(|d| d.get("gui"))
            && let Some(t) = parse_display_transform(gui)
        {
            return t;
        }

        current = json
            .get("parent")
            .and_then(|p| p.as_str())
            .map(|p| strip_mc_ns(p).to_string());
    }

    default_display(is_block)
}
