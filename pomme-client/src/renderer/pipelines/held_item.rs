use glam::{Mat3, Mat4, Vec3};

use crate::renderer::camera::CameraUniform;
use crate::renderer::chunk::atlas::TextureAtlas;
use crate::renderer::chunk::block_lighter::LightLevels;
use crate::renderer::pipelines::hand;
use crate::renderer::pipelines::item_display::{DisplayResolver, DisplayTransform};
use crate::renderer::pipelines::item_entity::{self, ItemEntityPipeline, ItemPipelineShared};
use crate::renderer::pipelines::lightmap::Lightmap;
use crate::renderer::rhi;
use crate::world::block::model::{CardinalLightType, ModelInputs};

pub struct HeldItemInfo {
    /// The item, whose model JSON gives the display transforms.
    pub name: String,
    /// The baked mesh this stack draws with
    /// (`BlockRenderAssets::stack_model_key`).
    pub model: String,
    pub item_tints: Vec<u32>,
    pub has_3d_model: bool,
    pub charged_crossbow: bool,
}

/// One first-person hand holding an item (vanilla `submitArmWithItem` for a
/// non-empty stack).
pub struct HeldHand<'a> {
    pub item: &'a HeldItemInfo,
    pub main_hand: bool,
    pub right_arm: bool,
    pub swing_progress: f32,
    pub use_anim: Option<UseAnim>,
}

/// An item in a hand of the inventory's player preview (vanilla
/// `ItemInHandLayer.submitArmWithItem` drawn under `ENTITY_IN_UI` light).
pub struct PreviewHand<'a> {
    pub item: &'a HeldItemInfo,
    pub right_arm: bool,
    pub in_use: bool,
    /// The hand's transform, up to the item's display transform.
    pub hand: Mat4,
}

/// A hand's first-person use animation (vanilla
/// `FirstPersonHandsAndItemsRenderer`'s use-animation switch).
#[derive(Clone, Copy)]
pub enum UseAnim {
    /// Eating or drinking.
    Eat(EatAnim),
    /// A shield held up: its blocking display, without the swing.
    Block,
}

/// The inputs to vanilla `applyEatTransform`.
#[derive(Clone, Copy)]
pub struct EatAnim {
    /// Vanilla `useItemRemaining - partialTick + 1`.
    pub curr_usage_time: f32,
    pub duration: f32,
}

/// Vanilla `ItemInHandRenderer.HandRenderSelection`: which hands first
/// person draws.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HandRenderSelection {
    pub main: bool,
    pub off: bool,
}

impl HandRenderSelection {
    pub const BOTH: Self = Self {
        main: true,
        off: true,
    };
    pub const MAIN_ONLY: Self = Self {
        main: true,
        off: false,
    };
    pub const OFF_ONLY: Self = Self {
        main: false,
        off: true,
    };
    pub const NONE: Self = Self {
        main: false,
        off: false,
    };
}

/// One display context's transforms for each hand: the left hand tries its
/// own key first, then the right's, and mirrors whichever it finds (vanilla
/// `ItemTransform.apply`'s left-hand fix).
struct HandDisplays {
    right: DisplayResolver,
    left: DisplayResolver,
}

const FIRST_PERSON_KEYS: [&[&str]; 2] = [
    &["firstperson_righthand"],
    &["firstperson_lefthand", "firstperson_righthand"],
];
const THIRD_PERSON_KEYS: [&[&str]; 2] = [
    &["thirdperson_righthand"],
    &["thirdperson_lefthand", "thirdperson_righthand"],
];

impl HandDisplays {
    fn new(inputs: &ModelInputs, [right, left]: [&'static [&'static str]; 2]) -> Self {
        Self {
            right: DisplayResolver::new(inputs, right),
            left: DisplayResolver::new(inputs, left),
        }
    }

    fn resolve(
        &self,
        item: &HeldItemInfo,
        in_use: bool,
        default: DisplayTransform,
        right_arm: bool,
    ) -> DisplayTransform {
        if right_arm {
            self.right.resolve(&item.name, in_use, default)
        } else {
            self.left.resolve(&item.name, in_use, default).mirrored()
        }
    }
}

pub struct HeldItemPipeline {
    pipeline: rhi::backend::RenderPipeline,
    shared: ItemPipelineShared,
    /// The player preview's own camera and instances, as first person draws
    /// in the same frame.
    preview: ItemPipelineShared,
    first_person: HandDisplays,
    third_person: HandDisplays,
}

impl HeldItemPipeline {
    pub fn new(
        device: &rhi::backend::Device,
        targets: rhi::backend::PipelineTargets,
        atlas: &TextureAtlas,
        inputs: &ModelInputs,
        lightmap: &Lightmap,
    ) -> Self {
        let shared = ItemPipelineShared::new(device, atlas, lightmap, "held_item");
        let preview = ItemPipelineShared::new(device, atlas, lightmap, "preview_item");
        let pipeline = item_entity::create_pipeline(device, targets, shared.pipeline_layout);
        Self {
            pipeline,
            shared,
            preview,
            first_person: HandDisplays::new(inputs, FIRST_PERSON_KEYS),
            third_person: HandDisplays::new(inputs, THIRD_PERSON_KEYS),
        }
    }

    pub fn rebind_atlas(&mut self, device: &rhi::backend::Device, atlas: &TextureAtlas) {
        self.shared.rebind_atlas(device, atlas);
        self.preview.rebind_atlas(device, atlas);
    }

    pub(crate) fn reload_display(&mut self, inputs: &ModelInputs) {
        self.first_person = HandDisplays::new(inputs, FIRST_PERSON_KEYS);
        self.third_person = HandDisplays::new(inputs, THIRD_PERSON_KEYS);
    }

    #[allow(clippy::too_many_arguments)]
    pub fn update_and_draw(
        &mut self,
        device: &rhi::backend::Device,
        pass: &mut rhi::backend::RenderPassEncoder,
        frame: usize,
        aspect: f32,
        hud_fov: f32,
        hands: &[HeldHand<'_>],
        meshes: &ItemEntityPipeline,
        bob: Mat4,
        hand_pose: Mat4,
        light: LightLevels,
        cardinal_light: CardinalLightType,
    ) {
        let uniform = CameraUniform::with_view_proj(hand::projection(aspect, hud_fov) * bob)
            .with_cardinal_light(cardinal_light);
        self.shared.update_camera(frame, &uniform);

        let tint_count = hands.iter().map(|hand| hand.item.item_tints.len()).sum();
        self.shared
            .begin_frame(device, frame, hands.len(), tint_count);
        self.shared.bind(pass, frame, self.pipeline);
        for hand in hands {
            let item = hand.item;
            let Some((buffer, vertex_count)) = meshes.mesh_handle(&item.model) else {
                continue;
            };
            let default = default_first_person(item.has_3d_model, hand.right_arm);
            let display =
                self.first_person
                    .resolve(item, hand.use_anim.is_some(), default, hand.right_arm);
            let invert = hand::arm_invert(hand.right_arm);
            let mut arm = match hand.use_anim {
                Some(UseAnim::Eat(anim)) => eat_item_matrix(anim, invert),
                Some(UseAnim::Block) => item_arm_matrix(invert),
                None => first_person_item_matrix(hand.swing_progress, invert),
            };
            // Vanilla `submitArmWithItem`'s crossbow branch: a charged one
            // held still in the main hand turns toward the crosshair.
            if item.charged_crossbow && hand.main_hand && hand.swing_progress < 0.001 {
                arm *= Mat4::from_translation(Vec3::new(invert * -0.641864, 0.0, 0.0))
                    * Mat4::from_rotation_y((invert * 10.0).to_radians());
            }
            let model = arm * display.to_matrix();
            let instance = self.shared.push_item(
                frame,
                &model,
                Mat3::from_mat4(hand_pose),
                light.lane(),
                &item.item_tints,
                0,
            );
            pass.set_vertex_buffer(0, buffer);
            pass.draw(vertex_count, 1, 0, instance);
        }
    }

    /// Draws the preview's held items: `view_proj` is the preview's, and
    /// `normal_basis` turns its space into the GUI view space vanilla lights.
    #[allow(clippy::too_many_arguments)]
    pub fn draw_preview(
        &mut self,
        device: &rhi::backend::Device,
        pass: &mut rhi::backend::RenderPassEncoder,
        frame: usize,
        view_proj: Mat4,
        normal_basis: Mat3,
        hands: &[PreviewHand<'_>],
        meshes: &ItemEntityPipeline,
    ) {
        let uniform = CameraUniform::with_view_proj(view_proj).with_entity_in_ui_light();
        self.preview.update_camera(frame, &uniform);
        let tint_count = hands.iter().map(|hand| hand.item.item_tints.len()).sum();
        self.preview
            .begin_frame(device, frame, hands.len(), tint_count);
        self.preview.bind(pass, frame, self.pipeline);
        for hand in hands {
            let item = hand.item;
            let Some((buffer, vertex_count)) = meshes.mesh_handle(&item.model) else {
                continue;
            };
            let default = default_third_person(item.has_3d_model);
            let display = self
                .third_person
                .resolve(item, hand.in_use, default, hand.right_arm);
            let instance = self.preview.push_item(
                frame,
                &(hand.hand * display.to_matrix()),
                normal_basis,
                LightLevels::FULL_BRIGHT.lane(),
                &item.item_tints,
                0,
            );
            pass.set_vertex_buffer(0, buffer);
            pass.draw(vertex_count, 1, 0, instance);
        }
    }

    pub fn recreate_pipeline(
        &mut self,
        device: &rhi::backend::Device,
        targets: rhi::backend::PipelineTargets,
    ) {
        self.pipeline.destroy(device);
        self.pipeline = item_entity::create_pipeline(device, targets, self.shared.pipeline_layout);
    }

    pub fn destroy(&mut self, device: &rhi::backend::Device) {
        self.pipeline.destroy(device);
        self.shared.destroy(device);
        self.preview.destroy(device);
    }
}

/// Vanilla `ItemInHandRenderer.applyItemArmTransform` (inverseArmHeight 0):
/// `invert` is [`hand::arm_invert`].
fn item_arm_matrix(invert: f32) -> Mat4 {
    Mat4::from_translation(Vec3::new(invert * 0.56, -0.52, -0.72))
}

// Vanilla ItemInHandRenderer: applyItemArmTransform + swingArm.
fn first_person_item_matrix(swing_progress: f32, invert: f32) -> Mat4 {
    let a = swing_progress;
    let sq = a.sqrt();
    let pi = std::f32::consts::PI;

    item_arm_matrix(invert)
        * Mat4::from_translation(Vec3::new(
            invert * -0.4 * (sq * pi).sin(),
            0.2 * (sq * pi * 2.0).sin(),
            -0.2 * (a * pi).sin(),
        ))
        * Mat4::from_rotation_y((invert * (45.0 + (a * a * pi).sin() * -20.0)).to_radians())
        * Mat4::from_rotation_z((invert * (sq * pi).sin() * -20.0).to_radians())
        * Mat4::from_rotation_x(((sq * pi).sin() * -80.0).to_radians())
        * Mat4::from_rotation_y((invert * -45.0).to_radians())
}

// Vanilla ItemInHandRenderer: applyEatTransform then applyItemArmTransform
// (EAT/DRINK skip the usual pre-transform).
fn eat_item_matrix(anim: EatAnim, invert: f32) -> Mat4 {
    let scaled = anim.curr_usage_time / anim.duration;
    // The chew bob runs after the first 20% of the eat, oscillating every 4
    // ticks; the jiggle shoves the item into the mouth over the last bite.
    let bob = if scaled < 0.8 {
        ((anim.curr_usage_time / 4.0 * std::f32::consts::PI).cos() * 0.1).abs()
    } else {
        0.0
    };
    let jiggle = 1.0 - (scaled as f64).powf(27.0) as f32;

    Mat4::from_translation(Vec3::new(invert * jiggle * 0.6, bob + jiggle * -0.5, 0.0))
        * Mat4::from_rotation_y((invert * jiggle * 90.0).to_radians())
        * Mat4::from_rotation_x((jiggle * 10.0).to_radians())
        * Mat4::from_rotation_z((invert * jiggle * 30.0).to_radians())
        * item_arm_matrix(invert)
}

/// The fallback first-person display: `block/block.json`'s for the hand, or
/// `item/generated.json`'s right-hand one, which its left hand reuses.
fn default_first_person(has_3d_model: bool, right_arm: bool) -> DisplayTransform {
    if has_3d_model {
        DisplayTransform {
            rotation: Vec3::new(0.0, if right_arm { 45.0 } else { 225.0 }, 0.0),
            translation: Vec3::ZERO,
            scale: Vec3::splat(0.40),
        }
    } else {
        DisplayTransform {
            rotation: Vec3::new(0.0, -90.0, 25.0),
            translation: Vec3::new(1.13, 3.2, 1.13) / 16.0,
            scale: Vec3::splat(0.68),
        }
    }
}

/// The fallback third-person right-hand display: `block/block.json`'s or
/// `item/generated.json`'s.
fn default_third_person(has_3d_model: bool) -> DisplayTransform {
    if has_3d_model {
        DisplayTransform {
            rotation: Vec3::new(75.0, 45.0, 0.0),
            translation: Vec3::new(0.0, 2.5, 0.0) / 16.0,
            scale: Vec3::splat(0.375),
        }
    } else {
        DisplayTransform {
            rotation: Vec3::ZERO,
            translation: Vec3::new(0.0, 3.0, 1.0) / 16.0,
            scale: Vec3::splat(0.55),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `m` reflected across the X axis, as the left hand mirrors the right.
    fn mirror(m: Mat4) -> Mat4 {
        let flip = Mat4::from_scale(Vec3::new(-1.0, 1.0, 1.0));
        flip * m * flip
    }

    #[test]
    fn the_left_arm_mirrors_the_right() {
        for swing in [0.0, 0.3, 0.7, 1.0] {
            assert!(
                first_person_item_matrix(swing, -1.0)
                    .abs_diff_eq(mirror(first_person_item_matrix(swing, 1.0)), 1e-5),
                "swing {swing}"
            );
        }
        let anim = EatAnim {
            curr_usage_time: 12.0,
            duration: 32.0,
        };
        assert!(eat_item_matrix(anim, -1.0).abs_diff_eq(mirror(eat_item_matrix(anim, 1.0)), 1e-5));
        let display = default_first_person(false, true);
        assert!(
            display
                .mirrored()
                .to_matrix()
                .abs_diff_eq(mirror(display.to_matrix()), 1e-5)
        );
    }
}
