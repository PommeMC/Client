//! Parrot: `ParrotModel` mesh and `setupAnim`.

// Vanilla's radian literals are kept verbatim, not swapped for `consts`.
#![allow(clippy::approx_constant)]

use std::f32::consts::PI;

use glam::Vec3;

use super::{BakedEntityModel, PartAnim, bake_model, part_index, rotated, vbox, vpart};

/// Vanilla `ParrotModel.Pose`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ParrotPose {
    #[default]
    Flying,
    Standing,
    Sitting,
    Party,
    // TODO: `OnShoulder`, for `ParrotOnShoulderLayer` (needs the player's
    // shoulder-parrot entity data).
}

/// `ParrotRenderState`'s animation inputs
/// (`ParrotRenderer.extractRenderState`).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ParrotAnims {
    /// `(sin(flap) + 1) * flapSpeed`, both lerped.
    pub flap_angle: f32,
    pub pose: ParrotPose,
}

/// Vanilla `ParrotModel.createBodyLayer`, 32x32.
pub fn bake_parrot_model() -> BakedEntityModel {
    let part = |name, parent, offset: (f32, f32, f32), rot: (f32, f32, f32), cube| {
        rotated(vpart(name, parent, offset.into(), vec![cube]), rot.into())
    };
    let wing = vbox((19, 8), (-0.5, 0.0, -1.5), (1.0, 5.0, 3.0));
    let leg = vbox((14, 18), (-0.5, 0.0, -0.5), (1.0, 2.0, 1.0));
    let parts = vec![
        part(
            "body",
            None,
            (0.0, 16.5, -3.0),
            (0.4937, 0.0, 0.0),
            vbox((2, 8), (-1.5, 0.0, -1.5), (3.0, 6.0, 3.0)),
        ),
        part(
            "tail",
            None,
            (0.0, 21.07, 1.16),
            (1.015, 0.0, 0.0),
            vbox((22, 1), (-1.5, -1.0, -1.0), (3.0, 4.0, 1.0)),
        ),
        part(
            "left_wing",
            None,
            (1.5, 16.94, -2.76),
            (-0.6981, -PI, 0.0),
            wing,
        ),
        part(
            "right_wing",
            None,
            (-1.5, 16.94, -2.76),
            (-0.6981, -PI, 0.0),
            wing,
        ),
        part(
            "head",
            None,
            (0.0, 15.69, -2.76),
            (0.0, 0.0, 0.0),
            vbox((2, 2), (-1.0, -1.5, -1.0), (2.0, 3.0, 2.0)),
        ),
        part(
            "head2",
            Some(4),
            (0.0, -2.0, -1.0),
            (0.0, 0.0, 0.0),
            vbox((10, 0), (-1.0, -0.5, -2.0), (2.0, 1.0, 4.0)),
        ),
        part(
            "beak1",
            Some(4),
            (0.0, -0.5, -1.5),
            (0.0, 0.0, 0.0),
            vbox((11, 7), (-0.5, -1.0, -0.5), (1.0, 2.0, 1.0)),
        ),
        part(
            "beak2",
            Some(4),
            (0.0, -1.75, -2.45),
            (0.0, 0.0, 0.0),
            vbox((16, 7), (-0.5, 0.0, -0.5), (1.0, 2.0, 1.0)),
        ),
        part(
            "feather",
            Some(4),
            (0.0, -2.15, 0.15),
            (-0.2214, 0.0, 0.0),
            vbox((2, 18), (0.0, -4.0, -2.0), (0.0, 5.0, 4.0)),
        ),
        part(
            "left_leg",
            None,
            (1.0, 22.0, -1.05),
            (-0.0299, 0.0, 0.0),
            leg,
        ),
        part(
            "right_leg",
            None,
            (-1.0, 22.0, -1.05),
            (-0.0299, 0.0, 0.0),
            leg,
        ),
    ];
    bake_model(parts, 32, 32)
}

/// Vanilla `ParrotModel.setupAnim` (with `prepare`).
pub fn compute_parrot_anim(
    model: &BakedEntityModel,
    head_x_rot_deg: f32,
    local_head_y_rot_deg: f32,
    walk_pos: f32,
    walk_speed: f32,
    age_in_ticks: f32,
    s: &ParrotAnims,
) -> PartAnim {
    let idx = |name: &str| part_index(&model.parts, name);
    let (body, tail, head) = (idx("body"), idx("tail"), idx("head"));
    let (left_wing, right_wing) = (idx("left_wing"), idx("right_wing"));
    let (left_leg, right_leg) = (idx("left_leg"), idx("right_leg"));

    let mut rot: Vec<Vec3> = model.parts.iter().map(|p| p.default_rotation).collect();
    let mut pos = vec![Vec3::ZERO; model.parts.len()];

    // `prepare(pose)`.
    match s.pose {
        ParrotPose::Flying => {
            rot[left_leg].x += 0.698_131_7;
            rot[right_leg].x += 0.698_131_7;
        }
        ParrotPose::Sitting => {
            pos[head].y += 1.9;
            rot[tail].x += 0.523_598_8;
            pos[tail].y += 1.9;
            pos[body].y += 1.9;
            rot[left_wing].z = -0.0873;
            pos[left_wing].y += 1.9;
            rot[right_wing].z = 0.0873;
            pos[right_wing].y += 1.9;
            pos[left_leg].y += 1.9;
            pos[right_leg].y += 1.9;
            rot[left_leg].x += 1.570_796_4;
            rot[right_leg].x += 1.570_796_4;
        }
        ParrotPose::Party => {
            rot[left_leg].z = -0.349_065_84;
            rot[right_leg].z = 0.349_065_84;
        }
        ParrotPose::Standing => {}
    }

    rot[head].x = head_x_rot_deg.to_radians();
    rot[head].y = local_head_y_rot_deg.to_radians();
    match s.pose {
        ParrotPose::Sitting => {}
        ParrotPose::Party => {
            let x_pos = age_in_ticks.cos();
            let y_pos = age_in_ticks.sin();
            let shift = Vec3::new(x_pos, y_pos, 0.0);
            pos[head] += shift;
            rot[head] = Vec3::new(0.0, 0.0, age_in_ticks.sin() * 0.4);
            pos[body] += shift;
            rot[left_wing].z = -0.0873 - s.flap_angle;
            pos[left_wing] += shift;
            rot[right_wing].z = 0.0873 + s.flap_angle;
            pos[right_wing] += shift;
            pos[tail] += shift;
        }
        ParrotPose::Standing | ParrotPose::Flying => {
            if s.pose == ParrotPose::Standing {
                rot[left_leg].x += (walk_pos * 0.6662).cos() * 1.4 * walk_speed;
                rot[right_leg].x += (walk_pos * 0.6662 + PI).cos() * 1.4 * walk_speed;
            }
            let bobbing_body = s.flap_angle * 0.3;
            pos[head].y += bobbing_body;
            rot[tail].x += (walk_pos * 0.6662).cos() * 0.3 * walk_speed;
            pos[tail].y += bobbing_body;
            pos[body].y += bobbing_body;
            rot[left_wing].z = -0.0873 - s.flap_angle;
            pos[left_wing].y += bobbing_body;
            rot[right_wing].z = 0.0873 + s.flap_angle;
            pos[right_wing].y += bobbing_body;
            pos[left_leg].y += bobbing_body;
            pos[right_leg].y += bobbing_body;
        }
    }

    PartAnim {
        rotation: rot.into_iter().enumerate().collect(),
        translation: pos.into_iter().enumerate().collect(),
        scale: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Sitting drops every root part 1.9 and folds the legs; the flap
    /// amplitude doesn't reach the wings.
    #[test]
    fn sitting_ignores_the_flap() {
        let model = bake_parrot_model();
        let s = ParrotAnims {
            flap_angle: 0.8,
            pose: ParrotPose::Sitting,
        };
        let anim = compute_parrot_anim(&model, 0.0, 0.0, 0.0, 0.0, 10.0, &s);
        assert_eq!(anim.translation_of(&model, "body").y, 1.9);
        assert_eq!(anim.rotation_of(&model, "left_wing").z, -0.0873);
        assert!((anim.rotation_of(&model, "left_leg").x - (1.570_796_4 - 0.0299)).abs() < 1e-6);
    }

    /// Flying tucks the legs 40 degrees and bobs with the flap.
    #[test]
    fn flying_bobs_with_the_flap() {
        let model = bake_parrot_model();
        let s = ParrotAnims {
            flap_angle: 1.0,
            pose: ParrotPose::Flying,
        };
        let anim = compute_parrot_anim(&model, 0.0, 0.0, 0.0, 0.0, 10.0, &s);
        assert!((anim.translation_of(&model, "head").y - 0.3).abs() < 1e-6);
        assert!((anim.rotation_of(&model, "right_wing").z - 1.0873).abs() < 1e-6);
        assert!((anim.rotation_of(&model, "right_leg").x - (0.698_131_7 - 0.0299)).abs() < 1e-6);
    }
}
