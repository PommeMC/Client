use std::path::Path;
use std::sync::{Arc, Mutex};

use glam::{Mat4, Vec3};
use pomme_gpu_allocator::vulkan::Allocator;
use pyronyx::vk;

use crate::renderer::camera::CameraUniform;
use crate::renderer::chunk::atlas::TextureAtlas;
use crate::renderer::pipelines::hand;
use crate::renderer::pipelines::item_display::{DisplayResolver, DisplayTransform};
use crate::renderer::pipelines::item_entity::{
    self, ItemEntityPipeline, ItemPipelineShared, push_model_light,
};

pub struct HeldItemInfo {
    pub name: String,
    pub light: f32,
    pub has_3d_model: bool,
}

/// First-person use-animation state (eat/drink), the inputs to vanilla
/// `ItemInHandRenderer.applyEatTransform`.
#[derive(Clone, Copy)]
pub struct UseAnim {
    /// Vanilla `useItemRemaining - partialTick + 1`.
    pub curr_usage_time: f32,
    pub duration: f32,
}

pub struct HeldItemPipeline {
    pipeline: vk::Pipeline,
    shared: ItemPipelineShared,
    display: DisplayResolver,
}

impl HeldItemPipeline {
    pub fn new(
        device: &vk::Device,
        render_pass: vk::RenderPass,
        allocator: &Arc<Mutex<Allocator>>,
        atlas: &TextureAtlas,
        jar_assets_dir: &Path,
    ) -> Self {
        let shared = ItemPipelineShared::new(device, allocator, atlas, "held_item");
        let pipeline = item_entity::create_pipeline(device, render_pass, shared.pipeline_layout);
        Self {
            pipeline,
            shared,
            display: DisplayResolver::new(jar_assets_dir, "firstperson_righthand"),
        }
    }

    pub fn rebind_atlas(&self, device: &vk::Device, atlas: &TextureAtlas) {
        self.shared.rebind_atlas(device, atlas);
    }

    #[allow(clippy::too_many_arguments)]
    pub fn update_and_draw(
        &mut self,
        cmd: vk::CommandBuffer,
        frame: usize,
        aspect: f32,
        swing_progress: f32,
        use_anim: Option<UseAnim>,
        item: &HeldItemInfo,
        meshes: &ItemEntityPipeline,
        bob: Mat4,
    ) {
        let Some((buffer, vertex_count)) = meshes.mesh_handle(&item.name) else {
            return;
        };

        let uniform = CameraUniform::with_view_proj(hand::projection(aspect) * bob);
        self.shared.update_camera(frame, &uniform);

        let display = self
            .display
            .resolve(&item.name, default_first_person(item.has_3d_model));
        let arm = match use_anim {
            Some(anim) => eat_item_matrix(anim),
            None => first_person_item_matrix(swing_progress),
        };
        let model = arm * display.to_matrix();

        self.shared.bind(cmd, frame, self.pipeline);
        cmd.bind_vertex_buffers(0, &[buffer], &[0]);
        push_model_light(cmd, self.shared.pipeline_layout, &model, item.light);
        cmd.draw(vertex_count, 1, 0, 0);
    }

    pub fn recreate_pipeline(&mut self, device: &vk::Device, render_pass: vk::RenderPass) {
        device.destroy_pipeline(self.pipeline, None);
        self.pipeline =
            item_entity::create_pipeline(device, render_pass, self.shared.pipeline_layout);
    }

    pub fn destroy(&mut self, device: &vk::Device, allocator: &Arc<Mutex<Allocator>>) {
        device.destroy_pipeline(self.pipeline, None);
        self.shared.destroy(device, allocator);
    }
}

// Vanilla ItemInHandRenderer: applyItemArmTransform + swingArm (right hand,
// inverseArmHeight = 0).
fn first_person_item_matrix(swing_progress: f32) -> Mat4 {
    let a = swing_progress;
    let sq = a.sqrt();
    let pi = std::f32::consts::PI;

    Mat4::from_translation(Vec3::new(0.56, -0.52, -0.72))
        * Mat4::from_translation(Vec3::new(
            -0.4 * (sq * pi).sin(),
            0.2 * (sq * pi * 2.0).sin(),
            -0.2 * (a * pi).sin(),
        ))
        * Mat4::from_rotation_y((45.0 + (a * a * pi).sin() * -20.0).to_radians())
        * Mat4::from_rotation_z(((sq * pi).sin() * -20.0).to_radians())
        * Mat4::from_rotation_x(((sq * pi).sin() * -80.0).to_radians())
        * Mat4::from_rotation_y((-45.0_f32).to_radians())
}

// Vanilla ItemInHandRenderer: applyEatTransform then applyItemArmTransform
// (EAT/DRINK skip the usual pre-transform; right hand, inverseArmHeight = 0).
fn eat_item_matrix(anim: UseAnim) -> Mat4 {
    let scaled = anim.curr_usage_time / anim.duration;
    // The chew bob runs after the first 20% of the eat, oscillating every 4
    // ticks; the jiggle shoves the item into the mouth over the last bite.
    let bob = if scaled < 0.8 {
        ((anim.curr_usage_time / 4.0 * std::f32::consts::PI).cos() * 0.1).abs()
    } else {
        0.0
    };
    let jiggle = 1.0 - (scaled as f64).powf(27.0) as f32;

    Mat4::from_translation(Vec3::new(jiggle * 0.6, bob + jiggle * -0.5, 0.0))
        * Mat4::from_rotation_y((jiggle * 90.0).to_radians())
        * Mat4::from_rotation_x((jiggle * 10.0).to_radians())
        * Mat4::from_rotation_z((jiggle * 30.0).to_radians())
        * Mat4::from_translation(Vec3::new(0.56, -0.52, -0.72))
}

fn default_first_person(has_3d_model: bool) -> DisplayTransform {
    if has_3d_model {
        // block/block.json firstperson_righthand
        DisplayTransform {
            rotation: Vec3::new(0.0, 45.0, 0.0),
            translation: Vec3::ZERO,
            scale: Vec3::splat(0.40),
        }
    } else {
        // item/generated.json firstperson_righthand
        DisplayTransform {
            rotation: Vec3::new(0.0, -90.0, 25.0),
            translation: Vec3::new(1.13, 3.2, 1.13) / 16.0,
            scale: Vec3::splat(0.68),
        }
    }
}
