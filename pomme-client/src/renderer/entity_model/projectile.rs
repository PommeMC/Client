//! Arrows and the thrown trident: `ArrowModel` and `TridentModel`. Neither
//! renderer flips the model (`ArrowRenderer` and `ThrownTridentRenderer`
//! submit it straight after their two rotations), so both bake in vanilla's
//! literal y-up space.

use std::f32::consts::FRAC_PI_4;

use glam::Vec3;

use super::{
    BakedEntityModel, EntityPart, ModelCube, PartAnim, bake_model_y_up, mirrored, vbox, vpart,
};

/// `CubeListBuilder.addBox`'s texture scale: `part`'s V divisor scaled by
/// `v_scale`.
fn scale_part_v(model: &mut BakedEntityModel, part: usize, v_scale: f32) {
    let (start, count) = model.part_ranges[part];
    for vertex in &mut model.vertices[start as usize..(start + count) as usize] {
        vertex.tex_coords[1] = (f32::from(vertex.tex_coords[1]) / v_scale).round() as u16;
    }
}

/// Vanilla `ArrowModel.createBodyLayer`, 32x32: a fletching square and two
/// crossed shaft planes under a root `PartPose.scaled(0.9)`.
pub fn bake_arrow_model() -> BakedEntityModel {
    let cross = vbox((0, 0), (-12.0, -2.0, 0.0), (16.0, 4.0, 0.0));
    let parts = vec![
        vpart("root", None, Vec3::ZERO, vec![]),
        EntityPart {
            default_rotation: Vec3::new(FRAC_PI_4, 0.0, 0.0),
            ..vpart(
                "back",
                Some(0),
                Vec3::new(-11.0, 0.0, 0.0),
                vec![vbox((0, 0), (0.0, -2.5, -2.5), (0.0, 5.0, 5.0))],
            )
        },
        EntityPart {
            default_rotation: Vec3::new(FRAC_PI_4, 0.0, 0.0),
            ..vpart("cross_1", Some(0), Vec3::ZERO, vec![cross])
        },
        EntityPart {
            default_rotation: Vec3::new(3.0 * FRAC_PI_4, 0.0, 0.0),
            ..vpart("cross_2", Some(0), Vec3::ZERO, vec![cross])
        },
    ];
    let mut model = bake_model_y_up(parts, 32, 32);
    // The planes' `addBox(..., 1.0f, 0.8f)`.
    scale_part_v(&mut model, 2, 0.8);
    scale_part_v(&mut model, 3, 0.8);
    // `mesh.transformed(pose -> pose.scaled(0.9))` and `back`'s
    // `withScale(0.8)`.
    model.part_scales[0] = 0.9;
    model.part_scales[1] = 0.8;
    model
}

/// Vanilla `ArrowModel.setupAnim`: a landed arrow quivers about the root's
/// z for `shake` ticks.
pub fn compute_arrow_anim(model: &BakedEntityModel, shake: f32) -> PartAnim {
    let mut anim = PartAnim::default();
    if shake > 0.0 {
        let pow = -(shake * 3.0).sin() * shake;
        anim.rotation_mut(model, 0).z += pow.to_radians();
    }
    anim
}

/// Vanilla `TridentModel.createLayer`, 32x32.
pub fn bake_trident_model() -> BakedEntityModel {
    let child = |name: &str, cube: ModelCube| vpart(name, Some(0), Vec3::ZERO, vec![cube]);
    let parts = vec![
        vpart(
            "pole",
            None,
            Vec3::ZERO,
            vec![vbox((0, 6), (-0.5, 2.0, -0.5), (1.0, 25.0, 1.0))],
        ),
        child("base", vbox((4, 0), (-1.5, 0.0, -0.5), (3.0, 2.0, 1.0))),
        child(
            "left_spike",
            vbox((4, 3), (-2.5, -3.0, -0.5), (1.0, 4.0, 1.0)),
        ),
        child(
            "middle_spike",
            vbox((0, 0), (-0.5, -4.0, -0.5), (1.0, 4.0, 1.0)),
        ),
        child(
            "right_spike",
            mirrored(vbox((4, 3), (1.5, -3.0, -0.5), (1.0, 4.0, 1.0))),
        ),
    ];
    bake_model_y_up(parts, 32, 32)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The shaft planes' UVs divide by 25.6, not 32: their 4-texel V span
    /// reaches 4/25.6 of the sheet.
    #[test]
    fn arrow_planes_scale_their_v() {
        let model = bake_arrow_model();
        let (start, count) = model.part_ranges[2];
        let max_v = model.vertices[start as usize..(start + count) as usize]
            .iter()
            .map(|v| f32::from(v.tex_coords[1]) / 65535.0)
            .fold(0.0, f32::max);
        assert!((max_v - 4.0 / 25.6).abs() < 1e-3, "{max_v}");
    }

    #[test]
    fn arrow_quivers_only_while_shaking() {
        let model = bake_arrow_model();
        assert!(compute_arrow_anim(&model, 0.0).rotation.is_empty());
        let anim = compute_arrow_anim(&model, 3.5);
        let expected = (-(3.5f32 * 3.0).sin() * 3.5).to_radians();
        assert_eq!(anim.rotation, vec![(0, Vec3::new(0.0, 0.0, expected))]);
    }
}
