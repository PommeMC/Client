use std::collections::{HashMap, HashSet};

use glam::camera::rh::proj;
use glam::{Mat3, Mat4, Vec3};

use crate::renderer::camera::CameraUniform;
use crate::renderer::chunk::atlas::TextureAtlas;
use crate::renderer::chunk::block_lighter::UNLIT_LANE;
use crate::renderer::pipelines::item_entity::{self, ItemEntityPipeline, ItemPipelineShared};
use crate::renderer::pipelines::lightmap::Lightmap;
use crate::renderer::{MAX_FRAMES_IN_FLIGHT, rhi, util};
use crate::world::block::model::DisplayTransform;

pub struct GuiItemPipeline {
    pipeline: rhi::backend::RenderPipeline,
    shared: ItemPipelineShared,
    /// The atlas size each frame's camera buffer was last written for.
    camera_px: [Option<u32>; MAX_FRAMES_IN_FLIGHT],
    atlas_px: u32,
    /// Player heads' skins by head key, bound in the atlas's place while
    /// their head bakes, as vanilla draws a profiled head's skull with the
    /// player's skin.
    head_skins: HashMap<String, HeadSkin>,
}

struct HeadSkin {
    texture: rhi::backend::Texture,
    group: rhi::backend::BindGroup,
}

/// The player head mesh that samples a bound skin rather than the atlas.
pub const PLAYER_HEAD_SKIN_MESH: &str = "player_head#skin";

impl GuiItemPipeline {
    pub fn new(
        device: &rhi::backend::Device,
        atlas_targets: rhi::backend::PipelineTargets,
        atlas_px: u32,
        atlas: &TextureAtlas,
        lightmap: &Lightmap,
    ) -> Self {
        let shared = ItemPipelineShared::new(device, atlas, lightmap, "gui_item");
        let pipeline = item_entity::create_pipeline_with_front_face(
            device,
            atlas_targets,
            shared.pipeline_layout,
            rhi::FrontFace::Cw,
        );
        Self {
            pipeline,
            shared,
            camera_px: [None; MAX_FRAMES_IN_FLIGHT],
            atlas_px,
            head_skins: HashMap::new(),
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
            self.shared.pipeline_layout,
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
        self.shared
            .update_camera(frame, &CameraUniform::with_view_proj(view_proj));
    }

    pub fn rebind_atlas(&mut self, device: &rhi::backend::Device, atlas: &TextureAtlas) {
        self.shared.rebind_atlas(device, atlas);
    }

    /// Binds for `items` bakes with `colors` tints between them.
    pub fn bind_for_bake_pass(
        &mut self,
        device: &rhi::backend::Device,
        pass: &mut rhi::backend::RenderPassEncoder,
        frame: usize,
        items: usize,
        colors: usize,
    ) {
        self.shared.begin_frame(device, frame, items, colors);
        self.write_atlas_ortho(frame);
        self.shared.bind(pass, frame, self.pipeline);
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

        let instance =
            self.shared
                .push_item(frame, &model, Mat3::IDENTITY, UNLIT_LANE, item_tints, 0);
        pass.set_vertex_buffer(0, buffer);
        pass.draw(vertex_count, 1, 0, instance);
    }

    pub fn has_head_skin(&self, key: &str) -> bool {
        self.head_skins.contains_key(key)
    }

    /// Uploads the 64x64 skins of `heads` not held yet and frees the ones no
    /// longer among them.
    pub fn sync_head_skins<'a>(
        &mut self,
        device: &rhi::backend::Device,
        heads: impl IntoIterator<Item = (&'a str, &'a [u8])>,
    ) {
        let mut wanted = HashSet::new();
        for (key, pixels) in heads {
            wanted.insert(key);
            if !self.head_skins.contains_key(key) {
                let texture = util::create_sampled_texture(device, "head_skin", pixels, 64, 64);
                let group = self.shared.texture_group(device, texture.handle());
                self.head_skins
                    .insert(key.to_owned(), HeadSkin { texture, group });
            }
        }
        for (_, skin) in self
            .head_skins
            .extract_if(|key, _| !wanted.contains(key.as_str()))
        {
            skin.texture.destroy(device);
            skin.group.destroy(device);
        }
    }

    /// [`Self::bake_to_slot`] for a player head drawn with the skin
    /// [`Self::sync_head_skins`] holds for `key`.
    #[allow(clippy::too_many_arguments)]
    pub fn bake_head_to_slot(
        &mut self,
        pass: &rhi::backend::RenderPassEncoder,
        frame: usize,
        item_entity: &ItemEntityPipeline,
        slot_x_px: u32,
        slot_y_px: u32,
        slot_size_px: u32,
        key: &str,
        display: DisplayTransform,
    ) {
        let Some(skin) = self.head_skins.get(key) else {
            return;
        };
        self.shared.bind_texture(pass, Some(skin.group));
        self.bake_to_slot(
            pass,
            frame,
            item_entity,
            slot_x_px,
            slot_y_px,
            slot_size_px,
            PLAYER_HEAD_SKIN_MESH,
            display,
            &[],
        );
        self.shared.bind_texture(pass, None);
    }

    pub fn destroy(&mut self, device: &rhi::backend::Device) {
        self.sync_head_skins(device, []);
        self.pipeline.destroy(device);
        self.shared.destroy(device);
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
