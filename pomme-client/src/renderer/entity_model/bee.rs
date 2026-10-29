//! Bee: `AdultBeeModel`/`BabyBeeModel` meshes and `BeeModel.setupAnim`.

// Vanilla's radian literals are kept verbatim, not swapped for `consts`.
#![allow(clippy::approx_constant)]

use std::f32::consts::PI;

use glam::Vec3;

use super::{
    BakedEntityModel, PartAnim, bake_model, deformed, find_part, mirrored, part_index,
    rot_lerp_rad, rotated, vbox, vpart,
};

/// `BeeRenderState`'s animation inputs (`BeeRenderer.extractRenderState`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BeeAnims {
    /// `Bee.getRollAmount(partialTicks)`, 0..1.
    pub roll_amount: f32,
    /// `!Bee.hasStung()`.
    pub has_stinger: bool,
    /// `onGround() && getDeltaMovement().lengthSqr() < 1.0E-7`.
    pub is_on_ground: bool,
    pub is_angry: bool,
}

impl Default for BeeAnims {
    fn default() -> Self {
        Self {
            roll_amount: 0.0,
            has_stinger: true,
            is_on_ground: false,
            is_angry: false,
        }
    }
}

/// Vanilla `AdultBeeModel.createBodyLayer`, 64x64.
pub fn bake_bee_model() -> BakedEntityModel {
    let wing = |origin| deformed(vbox((0, 18), origin, (9.0, 0.0, 6.0)), 0.001);
    let legs = |name, z, v| {
        vpart(
            name,
            Some(0),
            Vec3::new(1.5, 3.0, z),
            vec![vbox((26, v), (-5.0, 0.0, 0.0), (7.0, 2.0, 0.0))],
        )
    };
    let parts = vec![
        vpart("bone", None, Vec3::new(0.0, 19.0, 0.0), vec![]),
        vpart(
            "body",
            Some(0),
            Vec3::ZERO,
            vec![vbox((0, 0), (-3.5, -4.0, -5.0), (7.0, 7.0, 10.0))],
        ),
        vpart(
            "stinger",
            Some(1),
            Vec3::ZERO,
            vec![vbox((26, 7), (0.0, -1.0, 5.0), (0.0, 1.0, 2.0))],
        ),
        vpart(
            "left_antenna",
            Some(1),
            Vec3::new(0.0, -2.0, -5.0),
            vec![vbox((2, 0), (1.5, -2.0, -3.0), (1.0, 2.0, 3.0))],
        ),
        vpart(
            "right_antenna",
            Some(1),
            Vec3::new(0.0, -2.0, -5.0),
            vec![vbox((2, 3), (-2.5, -2.0, -3.0), (1.0, 2.0, 3.0))],
        ),
        rotated(
            vpart(
                "right_wing",
                Some(0),
                Vec3::new(-1.5, -4.0, -3.0),
                vec![wing((-9.0, 0.0, 0.0))],
            ),
            Vec3::new(0.0, -0.2618, 0.0),
        ),
        rotated(
            vpart(
                "left_wing",
                Some(0),
                Vec3::new(1.5, -4.0, -3.0),
                vec![mirrored(wing((0.0, 0.0, 0.0)))],
            ),
            Vec3::new(0.0, 0.2618, 0.0),
        ),
        legs("front_legs", -2.0, 1),
        legs("middle_legs", 0.0, 3),
        legs("back_legs", 2.0, 5),
    ];
    bake_model(parts, 64, 64)
}

/// Vanilla `BabyBeeModel.createBodyLayer`, 32x32 (no antennae).
pub fn bake_baby_bee_model() -> BakedEntityModel {
    let legs = |name, z, v| {
        vpart(
            name,
            Some(0),
            Vec3::new(0.0, 3.3333, z),
            vec![vbox((13, v), (-1.5, 0.0, 0.0), (3.0, 1.0, 0.0))],
        )
    };
    let parts = vec![
        vpart(
            "bone",
            None,
            Vec3::new(0.0, 19.6667, -1.8567),
            vec![
                vbox((6, 12), (1.0, -1.6667, -2.1633), (1.0, 2.0, 2.0)),
                vbox((0, 12), (-2.0, -1.6667, -2.1933), (1.0, 2.0, 2.0)),
            ],
        ),
        vpart(
            "body",
            Some(0),
            Vec3::new(0.0, 1.3333, 2.3567),
            vec![vbox((0, 0), (-2.0, -2.0, -2.5), (4.0, 4.0, 5.0))],
        ),
        vpart(
            "stinger",
            Some(1),
            Vec3::new(0.0, 0.5, 2.5),
            vec![vbox((13, 2), (0.0, -0.5, 0.0), (0.0, 1.0, 1.0))],
        ),
        rotated(
            vpart(
                "right_wing",
                Some(0),
                Vec3::new(-1.0, -0.6667, 0.8567),
                vec![vbox((3, 9), (-3.0, 0.0, 0.0), (3.0, 0.0, 3.0))],
            ),
            Vec3::new(0.2182, 0.3491, 0.0),
        ),
        rotated(
            vpart(
                "left_wing",
                Some(0),
                Vec3::new(1.0, -0.6667, 0.8567),
                vec![mirrored(vbox((-3, 9), (0.0, 0.0, 0.0), (3.0, 0.0, 3.0)))],
            ),
            Vec3::new(0.2182, -0.3491, 0.0),
        ),
        legs("front_legs", 1.8567, 0),
        legs("middle_legs", 2.8567, 1),
        legs("back_legs", 3.8567, 2),
    ];
    bake_model(parts, 32, 32)
}

/// Vanilla `BeeModel.setupAnim` (with `AdultBeeModel.bobUpAndDown`'s
/// antennae, which the baby mesh lacks), shared by both meshes.
pub fn compute_bee_anim(model: &BakedEntityModel, age_in_ticks: f32, s: &BeeAnims) -> PartAnim {
    let idx = |name: &str| part_index(&model.parts, name);
    let antennae = find_part(&model.parts, "left_antenna").map(|left| (left, idx("right_antenna")));
    let (bone, stinger) = (idx("bone"), idx("stinger"));
    let (right_wing, left_wing) = (idx("right_wing"), idx("left_wing"));
    let (front_leg, mid_leg, back_leg) = (idx("front_legs"), idx("middle_legs"), idx("back_legs"));

    let mut rot: Vec<Vec3> = model.parts.iter().map(|p| p.default_rotation).collect();
    let mut bone_y = 0.0;
    if !s.is_on_ground {
        let speed = age_in_ticks * 120.32113 * (PI / 180.0);
        rot[right_wing].y = 0.0;
        rot[right_wing].z = speed.cos() * PI * 0.15;
        rot[left_wing].x = rot[right_wing].x;
        rot[left_wing].y = rot[right_wing].y;
        rot[left_wing].z = -rot[right_wing].z;
        rot[front_leg].x = 0.785_398_2;
        rot[mid_leg].x = 0.785_398_2;
        rot[back_leg].x = 0.785_398_2;
    }
    if !s.is_angry && !s.is_on_ground {
        // `bobUpAndDown(speed, ageInTicks)`.
        let speed = (age_in_ticks * 0.18).cos();
        rot[bone].x = 0.1 + speed * PI * 0.025;
        bone_y -= speed * 0.9;
        rot[front_leg].x = -speed * PI * 0.1 + 0.392_699_1;
        rot[back_leg].x = -speed * PI * 0.05 + 0.785_398_2;
        if let Some((left, right)) = antennae {
            rot[left].x = speed * PI * 0.03;
            rot[right].x = speed * PI * 0.03;
        }
    }
    if s.roll_amount > 0.0 {
        rot[bone].x = rot_lerp_rad(s.roll_amount, rot[bone].x, 3.091_592_8);
    }

    PartAnim {
        rotation: rot.into_iter().enumerate().collect(),
        translation: vec![(bone, Vec3::new(0.0, bone_y, 0.0))],
        // `stinger.visible = hasStinger`.
        scale: if s.has_stinger {
            Vec::new()
        } else {
            vec![(stinger, Vec3::ZERO)]
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Grounded bees keep the rest pose; a stung bee hides its stinger.
    #[test]
    fn grounded_bee_rests() {
        let model = bake_bee_model();
        let s = BeeAnims {
            is_on_ground: true,
            has_stinger: false,
            ..Default::default()
        };
        let anim = compute_bee_anim(&model, 37.0, &s);
        assert_eq!(
            anim.rotation_of(&model, "right_wing"),
            Vec3::new(0.0, -0.2618, 0.0)
        );
        assert_eq!(anim.rotation_of(&model, "front_legs"), Vec3::ZERO);
        let stinger = part_index(&model.parts, "stinger");
        assert_eq!(anim.scale, vec![(stinger, Vec3::ZERO)]);
    }

    /// A full roll lands the bone on `3.0915928` whatever the bob angle.
    #[test]
    fn full_roll_flips_the_bone() {
        let model = bake_baby_bee_model();
        let s = BeeAnims {
            roll_amount: 1.0,
            ..Default::default()
        };
        let anim = compute_bee_anim(&model, 12.5, &s);
        assert!((anim.rotation_of(&model, "bone").x - 3.091_592_8).abs() < 1e-5);
        assert!(anim.scale.is_empty());
    }
}
