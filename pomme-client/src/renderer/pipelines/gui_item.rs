use glam::camera::rh::proj;
use glam::{Mat4, Vec3};

use crate::renderer::camera::CameraUniform;
use crate::renderer::chunk::atlas::TextureAtlas;
use crate::renderer::pipelines::item_entity::{
    self, ITEM_BIND_GROUPS, ITEM_PUSH_CONSTANTS, ItemEntityPipeline, TintPaletteArena,
    push_model_light,
};
use crate::renderer::{MAX_FRAMES_IN_FLIGHT, rhi, util};
use crate::world::block::model::DisplayTransform;

pub struct GuiItemPipeline {
    pipeline: rhi::backend::RenderPipeline,
    pipeline_layout: rhi::backend::PipelineLayout,
    camera_layout: rhi::backend::BindGroupLayout,
    atlas_layout: rhi::backend::BindGroupLayout,
    tint_layout: rhi::backend::BindGroupLayout,
    camera_sets: Vec<rhi::backend::BindGroup>,
    atlas_set: rhi::backend::BindGroup,
    camera_buffers: Vec<rhi::backend::Buffer>,
    /// The atlas size each frame's camera buffer was last written for.
    camera_px: [Option<u32>; MAX_FRAMES_IN_FLIGHT],
    tint_arena: TintPaletteArena,
    sampler: rhi::backend::Sampler,
    atlas_px: u32,
}

impl GuiItemPipeline {
    pub fn new(
        device: &rhi::backend::Device,
        atlas_targets: rhi::backend::PipelineTargets,
        atlas_px: u32,
        atlas: &TextureAtlas,
    ) -> Self {
        let [camera_layout, atlas_layout, tint_layout] =
            ITEM_BIND_GROUPS.map(|entries| rhi::backend::create_bind_group_layout(device, entries));

        let pipeline_layout = rhi::backend::create_pipeline_layout(
            device,
            &[camera_layout, atlas_layout, tint_layout],
            Some(ITEM_PUSH_CONSTANTS),
        );

        let pipeline = item_entity::create_pipeline_with_front_face(
            device,
            atlas_targets,
            pipeline_layout,
            rhi::FrontFace::Cw,
        );

        let (camera_sets, camera_buffers) = util::create_uniform_sets(
            device,
            camera_layout,
            size_of::<CameraUniform>() as u64,
            "gui_item_camera",
        );

        let sampler = rhi::backend::create_sampler(device, &rhi::SamplerDesc::NEAREST);

        let atlas_set =
            util::create_texture_group(device, atlas_layout, atlas.texture.handle(), sampler);
        let tint_arena = TintPaletteArena::new(device, tint_layout, "gui_item");

        Self {
            pipeline,
            pipeline_layout,
            camera_layout,
            atlas_layout,
            tint_layout,
            camera_sets,
            atlas_set,
            camera_buffers,
            camera_px: [None; MAX_FRAMES_IN_FLIGHT],
            tint_arena,
            sampler,
            atlas_px,
        }
    }

    pub fn set_atlas_px(&mut self, atlas_px: u32) {
        self.atlas_px = atlas_px;
    }

    pub fn recreate_pipeline(
        &mut self,
        device: &rhi::backend::Device,
        atlas_targets: rhi::backend::PipelineTargets,
    ) {
        self.pipeline.destroy(device);
        self.pipeline = item_entity::create_pipeline_with_front_face(
            device,
            atlas_targets,
            self.pipeline_layout,
            rhi::FrontFace::Cw,
        );
    }

    /// Brings `frame`'s camera buffer up to the current atlas size.
    fn write_atlas_ortho(&mut self, frame: usize) {
        if self.camera_px[frame] == Some(self.atlas_px) {
            return;
        }
        self.camera_px[frame] = Some(self.atlas_px);
        let atlas_px = self.atlas_px as f32;
        // Bottom/top swapped to Y-invert the projection (matches vanilla's
        // `invertY=true`).
        let view_proj =
            proj::directx::orthographic(0.0, atlas_px, atlas_px, 0.0, -10000.0, 10000.0);
        let uniform = CameraUniform::with_view_proj(view_proj);
        let bytes = bytemuck::bytes_of(&uniform);
        self.camera_buffers[frame].write(bytes);
    }

    pub fn rebind_atlas(&mut self, device: &rhi::backend::Device, atlas: &TextureAtlas) {
        self.atlas_set.replace(
            device,
            self.atlas_layout,
            &util::texture_binding(atlas.texture.handle(), self.sampler),
        );
    }

    pub fn bind_for_bake_pass(
        &mut self,
        device: &rhi::backend::Device,
        pass: &mut rhi::backend::RenderPassEncoder,
        frame: usize,
        required_colors: usize,
    ) {
        self.tint_arena.begin_frame(device, frame, required_colors);
        self.write_atlas_ortho(frame);
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_groups(
            0,
            &[
                self.camera_sets[frame],
                self.atlas_set,
                self.tint_arena.set(frame),
            ],
        );
    }

    /// Records into a pass [`Self::bind_for_bake_pass`] has bound.
    #[allow(clippy::too_many_arguments)]
    pub fn bake_to_slot(
        &mut self,
        pass: &rhi::backend::RenderPassEncoder,
        frame: usize,
        item_entity: &ItemEntityPipeline,
        slot_x_px: u32,
        slot_y_px: u32,
        slot_size_px: u32,
        item_name: &str,
        display: DisplayTransform,
        item_tints: &[u32],
    ) {
        let Some((buffer, vertex_count)) = item_entity.mesh_handle(item_name) else {
            return;
        };

        let model = slot_model_matrix(
            slot_x_px as f32,
            slot_y_px as f32,
            slot_size_px as f32,
            slot_size_px as f32,
            display,
        );

        let tint_range = self.tint_arena.push(frame, item_tints);
        pass.set_vertex_buffer(0, buffer);
        push_model_light(pass, &model, 1.0, tint_range);
        pass.draw(vertex_count, 1, 0, 0);
    }

    pub fn destroy(&mut self, device: &rhi::backend::Device) {
        self.pipeline.destroy(device);
        self.pipeline_layout.destroy(device);
        self.tint_arena.destroy(device);
        util::destroy_bind_groups(device, self.camera_sets.drain(..));
        self.atlas_set.destroy(device);
        self.camera_layout.destroy(device);
        self.atlas_layout.destroy(device);
        self.tint_layout.destroy(device);
        self.sampler.destroy(device);
        util::destroy_buffers(device, &mut self.camera_buffers);
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
