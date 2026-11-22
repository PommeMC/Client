//! Armor stands: `ArmorStandModel` (the body), `ArmorStandArmorModel` (its
//! armor), and the small layers `HumanoidModel.BABY_TRANSFORMER` bakes from
//! them.

use glam::Vec3;

use super::{
    BakedEntityModel, EntityPart, HumanoidArmorSlot, ModelPose, PartAnim, armor_parts, bake_model,
    find_part, mirrored, vbox, vpart,
};

/// What `ArmorStandArmorModel.setupAnim` and `ArmorStandModel.setupAnim`
/// read off `ArmorStandRenderState`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ArmorStandPose {
    /// Head, body, left arm, right arm, left leg, right leg, in degrees.
    pub poses: [[f32; 3]; 6],
    pub show_arms: bool,
    pub show_base_plate: bool,
    /// `ArmorStandRenderState.yRot`, the absolute yaw the base plate cancels.
    pub y_rot_deg: f32,
}

/// One `addOrReplaceChild`: name, pose offset, texture offset, box origin,
/// box size, mirrored.
type BodyPart = (
    &'static str,
    [f32; 3],
    (i32, i32),
    (f32, f32, f32),
    (f32, f32, f32),
    bool,
);

/// Vanilla `ArmorStandModel.createBodyLayer`, 64x64.
#[rustfmt::skip]
const BODY: [BodyPart; 10] = [
    ("head", [0.0, 1.0, 0.0], (0, 0), (-1.0, -7.0, -1.0), (2.0, 7.0, 2.0), false),
    ("body", [0.0, 0.0, 0.0], (0, 26), (-6.0, 0.0, -1.5), (12.0, 3.0, 3.0), false),
    ("right_arm", [-5.0, 2.0, 0.0], (24, 0), (-2.0, -2.0, -1.0), (2.0, 12.0, 2.0), false),
    ("left_arm", [5.0, 2.0, 0.0], (32, 16), (0.0, -2.0, -1.0), (2.0, 12.0, 2.0), true),
    ("right_leg", [-1.9, 12.0, 0.0], (8, 0), (-1.0, 0.0, -1.0), (2.0, 11.0, 2.0), false),
    ("left_leg", [1.9, 12.0, 0.0], (40, 16), (-1.0, 0.0, -1.0), (2.0, 11.0, 2.0), true),
    ("right_body_stick", [0.0, 0.0, 0.0], (16, 0), (-3.0, 3.0, -1.0), (2.0, 7.0, 2.0), false),
    ("left_body_stick", [0.0, 0.0, 0.0], (48, 16), (1.0, 3.0, -1.0), (2.0, 7.0, 2.0), false),
    ("shoulder_stick", [0.0, 0.0, 0.0], (0, 48), (-4.0, 10.0, -1.0), (8.0, 2.0, 2.0), false),
    ("base_plate", [0.0, 12.0, 0.0], (0, 32), (-6.0, 11.0, -6.0), (12.0, 1.0, 12.0), false),
];

fn body_parts() -> Vec<EntityPart> {
    BODY.iter()
        .map(|&(name, offset, uv, origin, size, mirror)| {
            let cube = vbox(uv, origin, size);
            let cube = if mirror { mirrored(cube) } else { cube };
            vpart(name, None, offset.into(), vec![cube])
        })
        .collect()
}

/// `HumanoidModel.BABY_TRANSFORMER` (`BabyModelTransform(true, 16, 0, 2, 2,
/// 24, {head})`) baked: the head's pose moves up 16 and scales 0.75, every
/// other root part's moves up 24 and halves. Children keep theirs.
fn bake_small(mut parts: Vec<EntityPart>, tex_h: u32) -> BakedEntityModel {
    let scales = parts
        .iter_mut()
        .map(|part| {
            if part.parent.is_some() {
                return 1.0;
            }
            let (lift, scale) = if part.name == "head" {
                (16.0, 0.75)
            } else {
                (24.0, 0.5)
            };
            part.offset = (part.offset + Vec3::new(0.0, lift, 0.0)) * scale;
            scale
        })
        .collect();
    let mut model = bake_model(parts, 64, tex_h);
    model.part_scales = scales;
    model
}

pub fn bake_armor_stand_model(is_small: bool) -> BakedEntityModel {
    if is_small {
        bake_small(body_parts(), 64)
    } else {
        bake_model(body_parts(), 64, 64)
    }
}

/// Vanilla `ArmorStandArmorModel.createBaseMesh` per slot: the humanoid armor
/// mesh with the head one up and the legs at 11.
fn armor_stand_armor_parts(slot: HumanoidArmorSlot) -> Vec<EntityPart> {
    let mut parts = armor_parts(slot, 1.9, -0.1);
    for part in &mut parts {
        match part.name.as_str() {
            "head" => part.offset.y = 1.0,
            "right_leg" | "left_leg" => part.offset.y = 11.0,
            _ => {}
        }
    }
    parts
}

pub fn bake_armor_stand_armor_model(slot: HumanoidArmorSlot) -> BakedEntityModel {
    bake_model(armor_stand_armor_parts(slot), 64, 32)
}

/// `ModelLayers.ARMOR_STAND_SMALL_ARMOR`.
pub fn bake_small_armor_stand_armor_model(slot: HumanoidArmorSlot) -> BakedEntityModel {
    bake_small(armor_stand_armor_parts(slot), 32)
}

/// Vanilla `ArmorStandArmorModel.setupAnim` then `ArmorStandModel.setupAnim`
/// (on the body mesh, which has the sticks and base plate): every limb takes
/// its pose, the sticks follow the body, and the base plate cancels the yaw.
/// `HumanoidModel.setupAnim`'s own work is all overwritten for a stand.
pub fn compute_armor_stand_anim(model: &BakedEntityModel, stand: &ArmorStandPose) -> PartAnim {
    let mut pose = ModelPose::new(model);
    let radians = |deg: [f32; 3]| Vec3::from(deg.map(f32::to_radians));
    let [head, body, left_arm, right_arm, left_leg, right_leg] = stand.poses.map(radians);
    for (name, rotation) in [
        ("head", head),
        ("body", body),
        ("left_arm", left_arm),
        ("right_arm", right_arm),
        ("left_leg", left_leg),
        ("right_leg", right_leg),
        ("right_body_stick", body),
        ("left_body_stick", body),
        ("shoulder_stick", body),
    ] {
        if let Some(i) = find_part(&model.parts, name) {
            pose.rot[i] = rotation;
        }
    }
    if let Some(i) = find_part(&model.parts, "base_plate") {
        pose.rot[i].y = (-stand.y_rot_deg).to_radians();
        pose.visible[i] = stand.show_base_plate;
    }
    // Only the body mesh hides its arms; the armor model keeps them.
    if find_part(&model.parts, "base_plate").is_some() {
        for name in ["left_arm", "right_arm"] {
            if let Some(i) = find_part(&model.parts, name) {
                pose.visible[i] = stand.show_arms;
            }
        }
    }
    pose.emit(model)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn small_stand_halves_its_body_and_shrinks_its_head() {
        let model = bake_armor_stand_model(true);
        let head = find_part(&model.parts, "head").unwrap();
        let plate = find_part(&model.parts, "base_plate").unwrap();
        assert_eq!(model.parts[head].offset, Vec3::new(0.0, 17.0 * 0.75, 0.0));
        assert_eq!(model.part_scales[head], 0.75);
        assert_eq!(model.parts[plate].offset, Vec3::new(0.0, 36.0 * 0.5, 0.0));
        assert_eq!(model.part_scales[plate], 0.5);
    }

    #[test]
    fn hidden_arms_and_plate_scale_to_nothing() {
        let model = bake_armor_stand_model(false);
        let stand = ArmorStandPose {
            poses: [[0.0; 3]; 6],
            show_arms: false,
            show_base_plate: false,
            y_rot_deg: 90.0,
        };
        let anim = compute_armor_stand_anim(&model, &stand);
        for name in ["left_arm", "right_arm", "base_plate"] {
            let i = find_part(&model.parts, name).unwrap();
            assert!(anim.scale.contains(&(i, Vec3::ZERO)), "{name}");
        }
        let plate = find_part(&model.parts, "base_plate").unwrap();
        assert!(
            anim.rotation
                .contains(&(plate, Vec3::new(0.0, (-90.0f32).to_radians(), 0.0)))
        );
    }

    #[test]
    fn armor_keeps_its_arms() {
        let model = bake_armor_stand_armor_model(HumanoidArmorSlot::Chest);
        let anim = compute_armor_stand_anim(&model, &ArmorStandPose::default());
        assert!(anim.scale.is_empty());
    }
}
