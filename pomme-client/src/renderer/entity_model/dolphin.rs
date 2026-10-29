//! Dolphin: `DolphinModel`, `BabyDolphinModel` and `DolphinModel.setupAnim`,
//! which both meshes share.

use std::f32::consts::FRAC_PI_3;

use glam::Vec3;

use super::{BakedEntityModel, PartAnim, bake_model, mirrored, part_index, rotated, vbox, vpart};

/// Vanilla `DolphinModel.createBodyLayer`, 64x64.
pub fn bake_dolphin_model() -> BakedEntityModel {
    // Vanilla's `1.0471976f` / `2.0943952f`.
    const FIN_X_ROT: f32 = FRAC_PI_3;
    const FIN_Z_ROT: f32 = 2.0 * FRAC_PI_3;
    let parts = vec![
        vpart(
            "body",
            None,
            Vec3::new(0.0, 22.0, -5.0),
            vec![vbox((22, 0), (-4.0, -7.0, 0.0), (8.0, 7.0, 13.0))],
        ),
        rotated(
            vpart(
                "back_fin",
                Some(0),
                Vec3::ZERO,
                vec![vbox((51, 0), (-0.5, 0.0, 8.0), (1.0, 4.0, 5.0))],
            ),
            Vec3::new(FIN_X_ROT, 0.0, 0.0),
        ),
        rotated(
            vpart(
                "left_fin",
                Some(0),
                Vec3::new(2.0, -2.0, 4.0),
                vec![mirrored(vbox((48, 20), (-0.5, -4.0, 0.0), (1.0, 4.0, 7.0)))],
            ),
            Vec3::new(FIN_X_ROT, 0.0, FIN_Z_ROT),
        ),
        rotated(
            vpart(
                "right_fin",
                Some(0),
                Vec3::new(-2.0, -2.0, 4.0),
                vec![vbox((48, 20), (-0.5, -4.0, 0.0), (1.0, 4.0, 7.0))],
            ),
            Vec3::new(FIN_X_ROT, 0.0, -FIN_Z_ROT),
        ),
        rotated(
            vpart(
                "tail",
                Some(0),
                Vec3::new(0.0, -2.5, 11.0),
                vec![vbox((0, 19), (-2.0, -2.5, 0.0), (4.0, 5.0, 11.0))],
            ),
            Vec3::new(-0.10471976, 0.0, 0.0),
        ),
        vpart(
            "tail_fin",
            Some(4),
            Vec3::new(0.0, 0.0, 9.0),
            vec![vbox((19, 20), (-5.0, -0.5, 0.0), (10.0, 1.0, 6.0))],
        ),
        vpart(
            "head",
            Some(0),
            Vec3::new(0.0, -4.0, -3.0),
            vec![vbox((0, 0), (-4.0, -3.0, -3.0), (8.0, 7.0, 6.0))],
        ),
        vpart(
            "nose",
            Some(6),
            Vec3::ZERO,
            vec![vbox((0, 13), (-1.0, 2.0, -7.0), (2.0, 2.0, 4.0))],
        ),
    ];
    bake_model(parts, 64, 64)
}

/// Vanilla `BabyDolphinModel.createBodyLayer`, 64x64. `DolphinRenderer`
/// bakes this layer as-is; `DolphinModel.BABY_TRANSFORMER` is unused.
pub fn bake_baby_dolphin_model() -> BakedEntityModel {
    const FIN_X_ROT: f32 = 0.8727;
    const FIN_Z_ROT: f32 = 1.7017;
    let parts = vec![
        vpart(
            "body",
            None,
            Vec3::new(0.0, 21.5, 0.0),
            vec![vbox((20, 0), (-3.0, -2.5, -4.0), (6.0, 5.0, 8.0))],
        ),
        vpart(
            "head",
            Some(0),
            Vec3::new(0.0, 1.0, -4.0),
            vec![vbox((0, 0), (-3.0, -3.5, -4.0), (6.0, 5.0, 4.0))],
        ),
        vpart(
            "nose",
            Some(1),
            Vec3::new(0.0, 0.5, -4.0),
            vec![vbox((0, 9), (-1.0, -1.0, -2.0), (2.0, 2.0, 2.0))],
        ),
        rotated(
            vpart(
                "left_fin",
                Some(0),
                Vec3::new(1.8, 0.85, -2.6),
                vec![vbox((34, 18), (-0.5, -1.5, -0.5), (1.0, 3.0, 6.0))],
            ),
            Vec3::new(FIN_X_ROT, 0.0, FIN_Z_ROT),
        ),
        rotated(
            vpart(
                "right_fin",
                Some(0),
                Vec3::new(-1.8, 0.85, -2.6),
                vec![mirrored(vbox(
                    (48, 18),
                    (-0.5, -1.5, -0.5),
                    (1.0, 3.0, 6.0),
                ))],
            ),
            Vec3::new(FIN_X_ROT, 0.0, -FIN_Z_ROT),
        ),
        vpart(
            "tail",
            Some(0),
            Vec3::new(0.0, 1.0, 4.0),
            vec![vbox((0, 13), (-2.0, -1.5, 0.0), (4.0, 3.0, 7.0))],
        ),
        vpart(
            "tail_fin",
            Some(5),
            Vec3::new(0.0, 0.0, 6.0),
            vec![vbox((22, 13), (-4.0, -0.5, -1.0), (8.0, 1.0, 4.0))],
        ),
        rotated(
            vpart(
                "back_fin",
                Some(0),
                Vec3::new(0.0, -1.0, -2.7),
                vec![vbox((42, 0), (-0.5, -1.0, 1.0), (1.0, 3.0, 4.0))],
            ),
            Vec3::new(FIN_X_ROT, 0.0, 0.0),
        ),
    ];
    bake_model(parts, 64, 64)
}

/// Vanilla `DolphinModel.setupAnim`: the whole body takes the entity pitch and
/// head yaw; a swimming dolphin (`DolphinRenderState.isMoving`) also beats its
/// body and tail.
pub fn compute_dolphin_anim(
    model: &BakedEntityModel,
    x_rot_deg: f32,
    local_head_y_rot_deg: f32,
    age_in_ticks: f32,
    moving: bool,
) -> PartAnim {
    let mut anim = PartAnim::default();
    let mut body = Vec3::new(
        x_rot_deg.to_radians(),
        local_head_y_rot_deg.to_radians(),
        0.0,
    );
    if moving {
        let beat = (age_in_ticks * 0.3).cos();
        body.x += -0.05 - 0.05 * beat;
        for (name, amplitude) in [("tail", -0.1), ("tail_fin", -0.2)] {
            anim.rotation_mut(model, part_index(&model.parts, name)).x = amplitude * beat;
        }
    }
    anim.rotation.push((part_index(&model.parts, "body"), body));
    anim
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn idle_dolphin_keeps_its_tail_rest_pose() {
        let model = bake_dolphin_model();
        let anim = compute_dolphin_anim(&model, 30.0, -10.0, 7.0, false);
        assert_eq!(anim.rotation.len(), 1);
        let (i, rot) = anim.rotation[0];
        assert_eq!(model.parts[i].name, "body");
        assert_eq!(rot, Vec3::new(30f32.to_radians(), -10f32.to_radians(), 0.0));
    }

    #[test]
    fn swimming_dolphin_beats_body_and_tail() {
        let model = bake_baby_dolphin_model();
        let anim = compute_dolphin_anim(&model, 0.0, 0.0, 0.0, true);
        let rot = |name| anim.rotation_of(&model, name);
        assert_eq!(rot("body").x, -0.1);
        assert_eq!(rot("tail").x, -0.1);
        assert_eq!(rot("tail_fin").x, -0.2);
    }
}
