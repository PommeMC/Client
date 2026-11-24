//! Wither: `WitherBossModel`, the body and its `WitherArmorLayer` mesh.

use std::f32::consts::PI;

use glam::Vec3;

use super::{BakedEntityModel, PartAnim, bake_model, deformed, part_index, rotated, vbox, vpart};

const RIBCAGE_X_ROT: f32 = 0.20420352;

/// The tail's pivot, hung off the end of a ribcage pitched `ribcage_x_rot`.
fn tail_pivot(ribcage_x_rot: f32) -> Vec3 {
    Vec3::new(
        -2.0,
        6.9 + ribcage_x_rot.cos() * 10.0,
        -0.5 + ribcage_x_rot.sin() * 10.0,
    )
}

/// Vanilla `WitherBossModel.createBodyLayer(g)`, 64x64: 0 for the body, 0.5
/// for the armor layer.
fn wither_model(g: f32) -> BakedEntityModel {
    let rib = |y| deformed(vbox((24, 22), (-4.0, y, 0.5), (11.0, 2.0, 2.0)), g);
    let side_head = |name, x| {
        vpart(
            name,
            None,
            Vec3::new(x, 4.0, 0.0),
            vec![deformed(
                vbox((32, 0), (-4.0, -4.0, -4.0), (6.0, 6.0, 6.0)),
                g,
            )],
        )
    };
    let parts = vec![
        vpart(
            "shoulders",
            None,
            Vec3::ZERO,
            vec![deformed(
                vbox((0, 16), (-10.0, 3.9, -0.5), (20.0, 3.0, 3.0)),
                g,
            )],
        ),
        rotated(
            vpart(
                "ribcage",
                None,
                Vec3::new(-2.0, 6.9, -0.5),
                vec![
                    deformed(vbox((0, 22), (0.0, 0.0, 0.0), (3.0, 10.0, 3.0)), g),
                    rib(1.5),
                    rib(4.0),
                    rib(6.5),
                ],
            ),
            Vec3::new(RIBCAGE_X_ROT, 0.0, 0.0),
        ),
        rotated(
            vpart(
                "tail",
                None,
                tail_pivot(RIBCAGE_X_ROT),
                vec![deformed(
                    vbox((12, 22), (0.0, 0.0, 0.0), (3.0, 6.0, 3.0)),
                    g,
                )],
            ),
            Vec3::new(0.83252203, 0.0, 0.0),
        ),
        vpart(
            "center_head",
            None,
            Vec3::ZERO,
            vec![deformed(
                vbox((0, 0), (-4.0, -4.0, -4.0), (8.0, 8.0, 8.0)),
                g,
            )],
        ),
        side_head("right_head", -8.0),
        side_head("left_head", 10.0),
    ];
    bake_model(parts, 64, 64)
}

pub fn bake_wither_model() -> BakedEntityModel {
    wither_model(0.0)
}

/// `ModelLayers.WITHER_ARMOR`: `INNER_ARMOR_DEFORMATION`.
pub fn bake_wither_armor_model() -> BakedEntityModel {
    wither_model(0.5)
}

/// `WitherRenderState`'s side head rotations, in degrees.
#[derive(Clone, Copy, Default)]
pub struct WitherAnims {
    pub x_head_rots: [f32; 2],
    pub y_head_rots: [f32; 2],
}

/// Vanilla `WitherBossModel.setupAnim`: the side heads turn to their own
/// targets, the ribcage and tail breathe, and the center head looks.
pub fn compute_wither_anim(
    model: &BakedEntityModel,
    head_x_rot_deg: f32,
    local_head_y_rot_deg: f32,
    body_y_rot_deg: f32,
    age_in_ticks: f32,
    wither: &WitherAnims,
) -> PartAnim {
    let look = |x_deg: f32, y_deg: f32| Vec3::new(x_deg.to_radians(), y_deg.to_radians(), 0.0);
    let mut anim = PartAnim::default();
    for (i, name) in ["right_head", "left_head"].into_iter().enumerate() {
        let head = part_index(&model.parts, name);
        let y_deg = wither.y_head_rots[i] - body_y_rot_deg;
        anim.rotation
            .push((head, look(wither.x_head_rots[i], y_deg)));
    }
    let breath = (age_in_ticks * 0.1).cos();
    let ribcage_x_rot = (0.065 + 0.05 * breath) * PI;
    let ribcage = part_index(&model.parts, "ribcage");
    anim.rotation
        .push((ribcage, Vec3::new(ribcage_x_rot, 0.0, 0.0)));
    let tail = part_index(&model.parts, "tail");
    anim.rotation
        .push((tail, Vec3::new((0.265 + 0.1 * breath) * PI, 0.0, 0.0)));
    anim.translation
        .push((tail, tail_pivot(ribcage_x_rot) - tail_pivot(RIBCAGE_X_ROT)));
    let center = part_index(&model.parts, "center_head");
    anim.rotation
        .push((center, look(head_x_rot_deg, local_head_y_rot_deg)));
    anim
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_tail_hangs_off_the_breathing_ribcage() {
        let model = bake_wither_model();
        // cos(0) = 1: the ribcage pitches 0.115 pi.
        let anim = compute_wither_anim(&model, 0.0, 0.0, 0.0, 0.0, &WitherAnims::default());
        let ribcage_x_rot = anim.rotation_of(&model, "ribcage").x;
        assert!((ribcage_x_rot - 0.115 * PI).abs() < 1e-6);
        let tail = part_index(&model.parts, "tail");
        let pivot = model.parts[tail].offset + anim.translation_of(&model, "tail");
        assert!((pivot - tail_pivot(ribcage_x_rot)).length() < 1e-5);
        assert!((anim.rotation_of(&model, "tail").x - 0.365 * PI).abs() < 1e-6);
    }

    #[test]
    fn side_heads_turn_relative_to_the_body() {
        let model = bake_wither_model();
        let wither = WitherAnims {
            x_head_rots: [10.0, -20.0],
            y_head_rots: [100.0, 60.0],
        };
        let anim = compute_wither_anim(&model, 5.0, 15.0, 90.0, 0.0, &wither);
        let right = anim.rotation_of(&model, "right_head");
        assert_eq!((right.x, right.y), (10f32.to_radians(), 10f32.to_radians()));
        let left = anim.rotation_of(&model, "left_head");
        assert_eq!(
            (left.x, left.y),
            ((-20f32).to_radians(), (-30f32).to_radians())
        );
        let center = anim.rotation_of(&model, "center_head");
        assert_eq!(
            (center.x, center.y),
            (5f32.to_radians(), 15f32.to_radians())
        );
    }
}
