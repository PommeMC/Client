//! Goat: `GoatModel`/`BabyGoatModel` meshes and their `setupAnim` over
//! `QuadrupedModel`'s.

// Vanilla's radian literals are kept verbatim, not swapped for `consts`.
#![allow(clippy::approx_constant)]

use glam::Vec3;

use super::{
    BakedEntityModel, PartAnim, bake_model, compute_quadruped_anim, mirrored, part_index, rotated,
    vbox, vpart,
};

/// Vanilla `GoatModel.createBodyLayer`, 64x64. The head's `.mirror()` carries
/// over to the goatee (`CubeListBuilder` keeps the flag).
pub fn bake_goat_model() -> BakedEntityModel {
    let parts = vec![
        vpart(
            "head",
            None,
            Vec3::new(1.0, 14.0, 0.0),
            vec![
                vbox((2, 61), (-6.0, -11.0, -10.0), (3.0, 2.0, 1.0)),
                mirrored(vbox((2, 61), (2.0, -11.0, -10.0), (3.0, 2.0, 1.0))),
                mirrored(vbox((23, 52), (-0.5, -3.0, -14.0), (0.0, 7.0, 5.0))),
            ],
        ),
        vpart(
            "left_horn",
            Some(0),
            Vec3::ZERO,
            vec![vbox((12, 55), (-0.01, -16.0, -10.0), (2.0, 7.0, 2.0))],
        ),
        vpart(
            "right_horn",
            Some(0),
            Vec3::ZERO,
            vec![vbox((12, 55), (-2.99, -16.0, -10.0), (2.0, 7.0, 2.0))],
        ),
        rotated(
            vpart(
                "nose",
                Some(0),
                Vec3::new(0.0, -8.0, -8.0),
                vec![vbox((34, 46), (-3.0, -4.0, -8.0), (5.0, 7.0, 10.0))],
            ),
            Vec3::new(0.9599, 0.0, 0.0),
        ),
        vpart(
            "body",
            None,
            Vec3::new(0.0, 24.0, 0.0),
            vec![
                vbox((1, 1), (-4.0, -17.0, -7.0), (9.0, 11.0, 16.0)),
                vbox((0, 28), (-5.0, -18.0, -8.0), (11.0, 14.0, 11.0)),
            ],
        ),
        vpart(
            "left_hind_leg",
            None,
            Vec3::new(1.0, 14.0, 4.0),
            vec![vbox((36, 29), (0.0, 4.0, 0.0), (3.0, 6.0, 3.0))],
        ),
        vpart(
            "right_hind_leg",
            None,
            Vec3::new(-3.0, 14.0, 4.0),
            vec![vbox((49, 29), (0.0, 4.0, 0.0), (3.0, 6.0, 3.0))],
        ),
        vpart(
            "left_front_leg",
            None,
            Vec3::new(1.0, 14.0, -6.0),
            vec![vbox((49, 2), (0.0, 0.0, 0.0), (3.0, 10.0, 3.0))],
        ),
        vpart(
            "right_front_leg",
            None,
            Vec3::new(-3.0, 14.0, -6.0),
            vec![vbox((35, 2), (0.0, 0.0, 0.0), (3.0, 10.0, 3.0))],
        ),
    ];
    bake_model(parts, 64, 64)
}

/// Vanilla `BabyGoatModel.createBodyLayer`, 64x64.
pub fn bake_baby_goat_model() -> BakedEntityModel {
    let leg = |name: &str, x: f32, z: f32, tex: (i32, i32)| {
        vpart(
            name,
            None,
            Vec3::new(x, 19.5, z),
            vec![vbox(tex, (-1.0, -0.5, -1.0), (2.0, 5.0, 2.0))],
        )
    };
    let horn_rot = Vec3::new(-0.3926991, 0.0, 0.0);
    let parts = vec![
        leg("left_hind_leg", 1.5, 3.0, (29, 12)),
        leg("right_hind_leg", -1.5, 3.0, (21, 12)),
        leg("right_front_leg", -1.5, -2.0, (21, 5)),
        leg("left_front_leg", 1.5, -2.0, (29, 5)),
        vpart(
            "body",
            None,
            Vec3::new(0.0, 17.8, 0.0),
            vec![
                vbox((0, 10), (-3.0, -2.3, -4.5), (6.0, 5.0, 9.0)),
                vbox((0, 24), (-2.5, -2.2, -4.0), (5.0, 4.0, 8.0)),
            ],
        ),
        rotated(
            vpart(
                "head",
                None,
                Vec3::new(0.0, 15.5, -3.0),
                vec![vbox((0, 0), (-2.0, -3.8126, -5.1548), (4.0, 4.0, 6.0))],
            ),
            Vec3::new(0.4363, 0.0, 0.0),
        ),
        rotated(
            vpart(
                "right_horn",
                Some(5),
                Vec3::new(-1.5, -1.5, -1.0),
                vec![mirrored(vbox((24, 0), (0.0, -4.5, 0.0), (1.0, 2.0, 1.0)))],
            ),
            horn_rot,
        ),
        rotated(
            vpart(
                "left_horn",
                Some(5),
                Vec3::new(-1.5, -1.5, -1.0),
                vec![mirrored(vbox((24, 0), (2.0, -4.5, 0.0), (1.0, 2.0, 1.0)))],
            ),
            horn_rot,
        ),
        rotated(
            vpart(
                "right_ear",
                Some(5),
                Vec3::new(-1.7, -2.3126, 0.1452),
                vec![mirrored(vbox((0, 12), (-2.0, -0.5, -0.5), (2.0, 1.0, 1.0)))],
            ),
            Vec3::new(0.0, -0.5236, 0.0),
        ),
        rotated(
            vpart(
                "left_ear",
                Some(5),
                Vec3::new(1.7, -2.3126, 0.1452),
                vec![vbox((0, 12), (0.0, -0.5, -0.5), (2.0, 1.0, 1.0))],
            ),
            Vec3::new(0.0, 0.5236, 0.0),
        ),
        vpart(
            "HeadMain",
            Some(5),
            Vec3::new(0.0, -1.3126, -1.1548),
            vec![vbox((0, 0), (-2.0, -2.5, -4.0), (4.0, 4.0, 6.0))],
        ),
    ];
    bake_model(parts, 64, 64)
}

/// `GoatRenderState` (vanilla defaults: both horns present).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GoatAnims {
    pub has_left_horn: bool,
    pub has_right_horn: bool,
    /// `Goat.getRammingXHeadRot`, radians.
    pub ramming_x_head_rot: f32,
}

impl Default for GoatAnims {
    fn default() -> Self {
        Self {
            has_left_horn: true,
            has_right_horn: true,
            ramming_x_head_rot: 0.0,
        }
    }
}

/// `BabyGoatModel.setupAnim`'s head pitch while not ramming (22.5 degrees).
const BABY_HEAD_X_ROT: f32 = 0.3926991;

/// Vanilla `GoatModel.setupAnim` (and `BabyGoatModel`'s override) over
/// `QuadrupedModel.setupAnim`. A missing horn collapses to a zero scale.
pub fn compute_goat_anim(
    model: &BakedEntityModel,
    head_x_rot_deg: f32,
    local_head_y_rot_deg: f32,
    walk_pos: f32,
    walk_speed: f32,
    is_baby: bool,
    g: &GoatAnims,
) -> PartAnim {
    let mut anim = compute_quadruped_anim(
        model,
        head_x_rot_deg,
        local_head_y_rot_deg,
        walk_pos,
        walk_speed,
        0.0,
        None,
    );
    let head_x = if g.ramming_x_head_rot != 0.0 {
        Some(g.ramming_x_head_rot)
    } else if is_baby {
        Some(BABY_HEAD_X_ROT)
    } else {
        None
    };
    if let Some(head_x) = head_x {
        let head = part_index(&model.parts, "head");
        anim.rotation_mut(model, head).x = head_x;
    }
    for (name, present) in [
        ("left_horn", g.has_left_horn),
        ("right_horn", g.has_right_horn),
    ] {
        if !present {
            anim.scale
                .push((part_index(&model.parts, name), Vec3::ZERO));
        }
    }
    anim
}

#[cfg(test)]
mod tests {
    use super::*;

    fn head_x(is_baby: bool, ramming: f32) -> f32 {
        let model = if is_baby {
            bake_baby_goat_model()
        } else {
            bake_goat_model()
        };
        let g = GoatAnims {
            ramming_x_head_rot: ramming,
            ..Default::default()
        };
        let anim = compute_goat_anim(&model, 10.0, 0.0, 0.0, 0.0, is_baby, &g);
        anim.rotation_of(&model, "head").x
    }

    /// Ramming overrides the look pitch; a baby not ramming pitches a fixed
    /// 22.5 degrees whatever it looks at.
    #[test]
    fn ramming_and_baby_override_head_pitch() {
        assert_eq!(head_x(false, 0.0), 10f32.to_radians());
        assert_eq!(head_x(false, 0.3), 0.3);
        assert_eq!(head_x(true, 0.0), BABY_HEAD_X_ROT);
        assert_eq!(head_x(true, 0.6), 0.6);
    }
}
