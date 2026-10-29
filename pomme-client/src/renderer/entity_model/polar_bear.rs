//! Polar bear: `PolarBearModel` (adult, `MeshTransformer.scaling(1.2)`) and
//! the `BabyPolarBearModel` mesh, which `PolarBearRenderer` wraps in a plain
//! `PolarBearModel`, so both run the same `setupAnim`.

use std::f32::consts::PI;

use glam::Vec3;

use super::{
    BakedEntityModel, PartAnim, bake_model, bake_root_scaled, compute_quadruped_anim, mirrored,
    rotated, vbox, vpart,
};

/// Vanilla `PolarBearModel.createBodyLayer`, 128x64, scaled 1.2 from the
/// root.
pub fn bake_polar_bear_model() -> BakedEntityModel {
    let hind_leg = vbox((50, 22), (-2.0, 0.0, -2.0), (4.0, 10.0, 8.0));
    let front_leg = vbox((50, 40), (-2.0, 0.0, -2.0), (4.0, 10.0, 6.0));
    let parts = vec![
        vpart(
            "head",
            None,
            Vec3::new(0.0, 10.0, -16.0),
            vec![
                vbox((0, 0), (-3.5, -3.0, -3.0), (7.0, 7.0, 7.0)),
                // mouth, right_ear, left_ear
                vbox((0, 44), (-2.5, 1.0, -6.0), (5.0, 3.0, 3.0)),
                vbox((26, 0), (-4.5, -4.0, -1.0), (2.0, 2.0, 1.0)),
                mirrored(vbox((26, 0), (2.5, -4.0, -1.0), (2.0, 2.0, 1.0))),
            ],
        ),
        rotated(
            vpart(
                "body",
                None,
                Vec3::new(-2.0, 9.0, 12.0),
                vec![
                    vbox((0, 19), (-5.0, -13.0, -7.0), (14.0, 14.0, 11.0)),
                    vbox((39, 0), (-4.0, -25.0, -7.0), (12.0, 12.0, 10.0)),
                ],
            ),
            Vec3::new(PI / 2.0, 0.0, 0.0),
        ),
        vpart(
            "right_hind_leg",
            None,
            Vec3::new(-4.5, 14.0, 6.0),
            vec![hind_leg],
        ),
        vpart(
            "left_hind_leg",
            None,
            Vec3::new(4.5, 14.0, 6.0),
            vec![hind_leg],
        ),
        vpart(
            "right_front_leg",
            None,
            Vec3::new(-3.5, 14.0, -8.0),
            vec![front_leg],
        ),
        vpart(
            "left_front_leg",
            None,
            Vec3::new(3.5, 14.0, -8.0),
            vec![front_leg],
        ),
    ];
    bake_root_scaled(parts, 1.2, 128, 64)
}

/// Vanilla `BabyPolarBearModel.createBodyLayer`, 64x64, unscaled.
pub fn bake_baby_polar_bear_model() -> BakedEntityModel {
    let leg = |name: &str, x: f32, z: f32, uv: (i32, i32)| {
        vpart(
            name,
            None,
            Vec3::new(x, 21.5, z),
            vec![vbox(uv, (-1.5, -0.5, -1.5), (3.0, 3.0, 3.0))],
        )
    };
    let parts = vec![
        vpart(
            "body",
            None,
            Vec3::new(0.0, 17.5, 0.0),
            vec![vbox((0, 9), (-4.0, -3.5, -6.0), (8.0, 7.0, 12.0))],
        ),
        vpart(
            "head",
            None,
            Vec3::new(0.0, 18.625, -5.75),
            vec![
                vbox((0, 0), (-3.0, -2.625, -4.25), (6.0, 5.0, 4.0)),
                vbox((20, 3), (-2.0, 0.375, -6.25), (4.0, 2.0, 2.0)),
                vbox((20, 0), (-4.0, -3.625, -2.75), (2.0, 2.0, 1.0)),
                vbox((26, 0), (2.0, -3.625, -2.75), (2.0, 2.0, 1.0)),
            ],
        ),
        leg("right_hind_leg", -2.5, 4.5, (0, 34)),
        leg("left_hind_leg", 2.5, 4.5, (12, 34)),
        leg("right_front_leg", -2.5, -4.5, (0, 28)),
        leg("left_front_leg", 2.5, -4.5, (12, 28)),
    ];
    bake_model(parts, 64, 64)
}

/// Vanilla `PolarBearModel.setupAnim`: the quadruped look and leg swing, then
/// the stand-up pose scaled by `standScale²`. `stand_scale` is
/// `PolarBearRenderState.standScale` (0..1), `age_scale` its `ageScale`.
pub fn compute_polar_bear_anim(
    model: &BakedEntityModel,
    head_x_rot_deg: f32,
    local_head_y_rot_deg: f32,
    walk_pos: f32,
    walk_speed: f32,
    stand_scale: f32,
    age_scale: f32,
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
    let s = stand_scale * stand_scale;
    if s == 0.0 {
        return anim;
    }
    for (i, part) in model.parts.iter().enumerate() {
        // (xRot delta, offset delta); the left front leg copies the right's
        // y/z, which share their rest values, so the deltas match.
        let (x_rot, offset) = match part.name.as_str() {
            "body" => (-s * PI * 0.35, Vec3::new(0.0, s * age_scale * 2.0, 0.0)),
            "right_front_leg" | "left_front_leg" => (
                -s * PI * 0.45,
                Vec3::new(0.0, -s * age_scale * 20.0, s * age_scale * 4.0),
            ),
            "head" => (s * PI * 0.15, Vec3::new(0.0, -s * 24.0, s * 13.0)),
            _ => continue,
        };
        anim.rotation_mut(model, i).x += x_rot;
        anim.translation.push((i, offset));
    }
    anim
}
