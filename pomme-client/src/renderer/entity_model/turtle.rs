//! Turtle: `AdultTurtleModel`, `BabyTurtleModel` and the shared
//! `TurtleModel.setupAnim` (on top of `QuadrupedModel.setupAnim`).

use std::f32::consts::FRAC_PI_2;

use glam::Vec3;

use super::{
    BakedEntityModel, EntityPart, PartAnim, bake_model, compute_quadruped_anim, part_index,
    rotated, vbox, vpart,
};

/// `TurtleRenderState` flags the model reads.
#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub struct TurtleAnims {
    /// `!isInWater() && onGround()`.
    pub on_land: bool,
    pub laying_egg: bool,
    /// `!isBaby() && hasEgg()`.
    pub has_egg: bool,
}

/// `vpart` lying on its back, `PartPose.offsetAndRotation(.., PI/2, 0, 0)`.
fn flat(part: EntityPart) -> EntityPart {
    rotated(part, Vec3::new(FRAC_PI_2, 0.0, 0.0))
}

/// Vanilla `AdultTurtleModel.createBodyLayer`, 128x64.
pub fn bake_turtle_model() -> BakedEntityModel {
    let body_pivot = Vec3::new(0.0, 11.0, -10.0);
    let parts = vec![
        vpart(
            "head",
            None,
            Vec3::new(0.0, 19.0, -10.0),
            vec![vbox((3, 0), (-3.0, -1.0, -3.0), (6.0, 5.0, 6.0))],
        ),
        flat(vpart(
            "body",
            None,
            body_pivot,
            vec![
                // "shell", then "belly".
                vbox((7, 37), (-9.5, 3.0, -10.0), (19.0, 20.0, 6.0)),
                vbox((31, 1), (-5.5, 3.0, -13.0), (11.0, 18.0, 3.0)),
            ],
        )),
        flat(vpart(
            "egg_belly",
            None,
            body_pivot,
            vec![vbox((70, 33), (-4.5, 3.0, -14.0), (9.0, 18.0, 1.0))],
        )),
        vpart(
            "right_hind_leg",
            None,
            Vec3::new(-3.5, 22.0, 11.0),
            vec![vbox((1, 23), (-2.0, 0.0, 0.0), (4.0, 1.0, 10.0))],
        ),
        vpart(
            "left_hind_leg",
            None,
            Vec3::new(3.5, 22.0, 11.0),
            vec![vbox((1, 12), (-2.0, 0.0, 0.0), (4.0, 1.0, 10.0))],
        ),
        vpart(
            "right_front_leg",
            None,
            Vec3::new(-5.0, 21.0, -4.0),
            vec![vbox((27, 30), (-13.0, 0.0, -2.0), (13.0, 1.0, 5.0))],
        ),
        vpart(
            "left_front_leg",
            None,
            Vec3::new(5.0, 21.0, -4.0),
            vec![vbox((27, 24), (0.0, 0.0, -2.0), (13.0, 1.0, 5.0))],
        ),
    ];
    bake_model(parts, 128, 64)
}

/// Vanilla `BabyTurtleModel.createBodyLayer`, 16x16 (drawn backface-culled,
/// `entityCutoutCull`: the legs are zero-height planes).
pub fn bake_baby_turtle_model() -> BakedEntityModel {
    let parts = vec![
        vpart(
            "body",
            None,
            Vec3::new(0.0, 22.9, 1.0),
            vec![vbox((0, 0), (-2.0, -1.0, -2.0), (4.0, 2.0, 4.0))],
        ),
        vpart(
            "head",
            None,
            Vec3::new(0.0, 22.9, -1.0),
            vec![vbox((0, 6), (-1.5, -2.0, -3.0), (3.0, 3.0, 3.0))],
        ),
        vpart(
            "right_hind_leg",
            None,
            Vec3::new(-2.0, 23.9, 2.5),
            vec![vbox((-1, 0), (-2.0, 0.0, -0.5), (2.0, 0.0, 1.0))],
        ),
        vpart(
            "left_hind_leg",
            None,
            Vec3::new(2.0, 23.9, 2.5),
            vec![vbox((-1, 1), (0.0, 0.0, -0.5), (2.0, 0.0, 1.0))],
        ),
        vpart(
            "right_front_leg",
            None,
            Vec3::new(-2.0, 23.9, -0.5),
            vec![vbox((8, 6), (-2.0, 0.0, -0.5), (2.0, 0.0, 1.0))],
        ),
        vpart(
            "left_front_leg",
            None,
            Vec3::new(2.0, 23.9, -0.5),
            vec![vbox((8, 7), (0.0, 0.0, -0.5), (2.0, 0.0, 1.0))],
        ),
    ];
    bake_model(parts, 16, 16)
}

/// Vanilla `TurtleModel.setupAnim` over `QuadrupedModel.setupAnim`, plus
/// `AdultTurtleModel.setupAnim`'s egg belly. On land the legs paddle sideways
/// (yaw) on top of the quadruped stride; in water they stroke.
pub fn compute_turtle_anim(
    model: &BakedEntityModel,
    head_x_rot_deg: f32,
    local_head_y_rot_deg: f32,
    walk_pos: f32,
    walk_speed: f32,
    t: &TurtleAnims,
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
    let (lay_egg, lay_egg_amplitude) = if t.laying_egg { (4.0, 2.0) } else { (1.0, 1.0) };
    let swing_pos = walk_pos * 5.0;
    let front_swing = (lay_egg * swing_pos).cos();
    let hind_swing = swing_pos.cos();
    let swing = (walk_pos * 0.6662 * 0.6).cos() * (0.5 * walk_speed);
    let idx = |name| part_index(&model.parts, name);
    let (rh, lh, rf, lf) = (
        idx("right_hind_leg"),
        idx("left_hind_leg"),
        idx("right_front_leg"),
        idx("left_front_leg"),
    );
    if t.on_land {
        anim.rotation_mut(model, rf).y = -front_swing * 8.0 * walk_speed * lay_egg_amplitude;
        anim.rotation_mut(model, lf).y = front_swing * 8.0 * walk_speed * lay_egg_amplitude;
        anim.rotation_mut(model, rh).y = -hind_swing * 3.0 * walk_speed;
        anim.rotation_mut(model, lh).y = hind_swing * 3.0 * walk_speed;
    } else {
        anim.rotation_mut(model, rh).x = swing;
        anim.rotation_mut(model, lh).x = -swing;
        anim.rotation_mut(model, rf).z = -swing;
        anim.rotation_mut(model, lf).z = swing;
    }
    if let Some(egg_belly) = model.parts.iter().position(|p| p.name == "egg_belly") {
        if t.has_egg {
            // `root.y -= 1`: every part hangs off the root.
            for (i, part) in model.parts.iter().enumerate() {
                if part.parent.is_none() {
                    anim.translation.push((i, Vec3::new(0.0, -1.0, 0.0)));
                }
            }
        } else {
            // `eggBelly.visible = false`.
            anim.scale.push((egg_belly, Vec3::ZERO));
        }
    }
    anim
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_rot(model: &BakedEntityModel, anim: &PartAnim, name: &str, expected: Vec3) {
        let rot = anim.rotation_of(model, name);
        assert!(
            rot.abs_diff_eq(expected, 1e-6),
            "{name}: {rot} != {expected}"
        );
    }

    #[test]
    fn egg_belly_shows_and_lifts_only_with_an_egg() {
        let model = bake_turtle_model();
        let egg = part_index(&model.parts, "egg_belly");
        let plain = compute_turtle_anim(&model, 0.0, 0.0, 0.0, 0.0, &TurtleAnims::default());
        assert_eq!(plain.scale, [(egg, Vec3::ZERO)]);
        assert!(plain.translation.is_empty());

        let gravid = TurtleAnims {
            has_egg: true,
            ..Default::default()
        };
        let lifted = compute_turtle_anim(&model, 0.0, 0.0, 0.0, 0.0, &gravid);
        assert!(lifted.scale.is_empty());
        assert_eq!(lifted.translation.len(), model.parts.len());

        // The baby mesh has no egg belly to toggle.
        let baby = bake_baby_turtle_model();
        let anim = compute_turtle_anim(&baby, 0.0, 0.0, 0.0, 0.0, &gravid);
        assert!(anim.scale.is_empty() && anim.translation.is_empty());
    }

    #[test]
    fn land_legs_yaw_over_the_stride_and_water_legs_stroke() {
        let model = bake_turtle_model();
        let land = TurtleAnims {
            on_land: true,
            ..Default::default()
        };
        let anim = compute_turtle_anim(&model, 0.0, 0.0, 0.0, 0.5, &land);
        assert_rot(&model, &anim, "right_front_leg", Vec3::new(-0.7, -4.0, 0.0));
        assert_rot(&model, &anim, "left_hind_leg", Vec3::new(-0.7, 1.5, 0.0));

        let anim = compute_turtle_anim(&model, 0.0, 0.0, 0.0, 0.5, &TurtleAnims::default());
        assert_rot(&model, &anim, "right_hind_leg", Vec3::new(0.25, 0.0, 0.0));
        assert_rot(&model, &anim, "left_front_leg", Vec3::new(0.7, 0.0, 0.25));
        assert_rot(
            &model,
            &anim,
            "right_front_leg",
            Vec3::new(-0.7, 0.0, -0.25),
        );
    }
}
