//! Fox: `AdultFoxModel`/`BabyFoxModel` meshes, `FoxModel.setupAnim` with the
//! adult/baby pose overrides, and the `FoxBabyAnimation` walk.

// Vanilla's radian literals are kept verbatim, not swapped for `consts`.
#![allow(clippy::approx_constant)]

use std::sync::atomic::{AtomicU32, Ordering};

use glam::Vec3;

use super::{
    BakedEntityModel, KfAnim, KfChannel, KfDeltas, KfTarget, Kfi, ModelPose, PartAnim,
    apply_kf_walk, bake_model, deformed, kf, part_index, rotated, vbox, vpart, walk_swing,
};

/// Vanilla `AdultFoxModel.createBodyLayer`, 48x32.
pub fn bake_fox_model() -> BakedEntityModel {
    let fudge = |tex| deformed(vbox(tex, (2.0, 0.5, -1.0), (2.0, 6.0, 2.0)), 0.001);
    let (left_leg, right_leg) = (fudge((4, 24)), fudge((13, 24)));
    let parts = vec![
        vpart(
            "head",
            None,
            Vec3::new(-1.0, 16.5, -3.0),
            vec![vbox((1, 5), (-3.0, -2.0, -5.0), (8.0, 6.0, 6.0))],
        ),
        vpart(
            "right_ear",
            Some(0),
            Vec3::ZERO,
            vec![vbox((8, 1), (-3.0, -4.0, -4.0), (2.0, 2.0, 1.0))],
        ),
        vpart(
            "left_ear",
            Some(0),
            Vec3::ZERO,
            vec![vbox((15, 1), (3.0, -4.0, -4.0), (2.0, 2.0, 1.0))],
        ),
        vpart(
            "nose",
            Some(0),
            Vec3::ZERO,
            vec![vbox((6, 18), (-1.0, 2.01, -8.0), (4.0, 2.0, 3.0))],
        ),
        rotated(
            vpart(
                "body",
                None,
                Vec3::new(0.0, 16.0, -6.0),
                vec![vbox((24, 15), (-3.0, 3.999, -3.5), (6.0, 11.0, 6.0))],
            ),
            Vec3::new(1.5707964, 0.0, 0.0),
        ),
        vpart(
            "right_hind_leg",
            None,
            Vec3::new(-5.0, 17.5, 7.0),
            vec![right_leg],
        ),
        vpart(
            "left_hind_leg",
            None,
            Vec3::new(-1.0, 17.5, 7.0),
            vec![left_leg],
        ),
        vpart(
            "right_front_leg",
            None,
            Vec3::new(-5.0, 17.5, 0.0),
            vec![right_leg],
        ),
        vpart(
            "left_front_leg",
            None,
            Vec3::new(-1.0, 17.5, 0.0),
            vec![left_leg],
        ),
        rotated(
            vpart(
                "tail",
                Some(4),
                Vec3::new(-4.0, 15.0, -1.0),
                vec![vbox((30, 0), (2.0, 0.0, -1.0), (4.0, 9.0, 5.0))],
            ),
            Vec3::new(-0.05235988, 0.0, 0.0),
        ),
    ];
    bake_model(parts, 48, 32)
}

/// Vanilla `BabyFoxModel.createBodyLayer`, 32x32. The ears are head cubes,
/// not parts.
pub fn bake_baby_fox_model() -> BakedEntityModel {
    let leg = |name, tex, x: f32, z: f32| {
        vpart(
            name,
            None,
            Vec3::new(x, 22.0, z),
            vec![vbox(tex, (-1.0, 0.0, -1.0), (2.0, 2.0, 2.0))],
        )
    };
    let parts = vec![
        vpart(
            "head",
            None,
            Vec3::new(0.0, 18.125, 0.125),
            vec![
                vbox((0, 0), (-3.0, -2.125, -5.125), (6.0, 5.0, 5.0)),
                vbox((18, 20), (-1.0, 0.875, -7.125), (2.0, 2.0, 2.0)),
                vbox((22, 8), (-3.0, -4.125, -4.125), (2.0, 2.0, 1.0)),
                vbox((22, 11), (1.0, -4.125, -4.125), (2.0, 2.0, 1.0)),
            ],
        ),
        leg("right_hind_leg", (22, 4), -1.5, 4.0),
        leg("left_hind_leg", (22, 0), 1.5, 4.0),
        leg("right_front_leg", (22, 4), -1.5, 0.0),
        leg("left_front_leg", (22, 0), 1.5, 0.0),
        vpart(
            "body",
            None,
            Vec3::new(0.0, 20.0, 2.0),
            vec![vbox((0, 10), (-2.5, -2.0, -3.0), (5.0, 4.0, 6.0))],
        ),
        vpart(
            "tail",
            Some(5),
            Vec3::new(0.0, -0.5, 3.0),
            vec![vbox((0, 20), (-1.5, -1.48, -1.0), (3.0, 3.0, 6.0))],
        ),
    ];
    bake_model(parts, 32, 32)
}

/// The `FoxRenderState` fields `FoxModel.setupAnim` reads.
#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub struct FoxAnims {
    /// `getHeadRollAngle`, radians.
    pub head_roll_angle: f32,
    pub crouch_amount: f32,
    pub is_crouching: bool,
    pub is_sleeping: bool,
    pub is_sitting: bool,
    pub is_faceplanted: bool,
    pub is_pouncing: bool,
    /// `FoxModel.legMotionPos` after this fox's frame advanced it (see
    /// [`advance_fox_leg_motion_pos`]); read only while faceplanted.
    pub leg_motion_pos: f32,
}

/// `FoxModel.legMotionPos`: a field on the model instance, so one counter is
/// shared by every faceplanted fox drawn with that model.
static LEG_MOTION_POS: [AtomicU32; 2] = [AtomicU32::new(0), AtomicU32::new(0)];

/// The `legMotionPos += 0.67` of a faceplanted fox's `setupAnim`, which runs
/// once per fox per rendered frame: call it once per frame from render-state
/// extraction, not from the (possibly repeated) anim compute.
pub fn advance_fox_leg_motion_pos(is_baby: bool) -> f32 {
    let cell = &LEG_MOTION_POS[is_baby as usize];
    let next = f32::from_bits(cell.load(Ordering::Relaxed)) + 0.67;
    cell.store(next.to_bits(), Ordering::Relaxed);
    next
}

struct FoxParts {
    head: usize,
    body: usize,
    tail: usize,
    right_hind_leg: usize,
    left_hind_leg: usize,
    right_front_leg: usize,
    left_front_leg: usize,
}

/// Vanilla `FoxModel.setupAnim` with the `AdultFoxModel`/`BabyFoxModel`
/// overrides.
#[allow(clippy::too_many_arguments)]
pub fn compute_fox_anim(
    model: &BakedEntityModel,
    is_baby: bool,
    head_x_rot_deg: f32,
    local_head_y_rot_deg: f32,
    walk_pos: f32,
    walk_speed: f32,
    age_in_ticks: f32,
    s: &FoxAnims,
) -> PartAnim {
    let idx = |name| part_index(&model.parts, name);
    let p = FoxParts {
        head: idx("head"),
        body: idx("body"),
        tail: idx("tail"),
        right_hind_leg: idx("right_hind_leg"),
        left_hind_leg: idx("left_hind_leg"),
        right_front_leg: idx("right_front_leg"),
        left_front_leg: idx("left_front_leg"),
    };
    let legs = [
        p.right_hind_leg,
        p.left_hind_leg,
        p.right_front_leg,
        p.left_front_leg,
    ];
    let age_scale = if is_baby { 0.5 } else { 1.0 };
    let mut m = ModelPose::new(model);

    // setWalkingPose
    m.rot[p.head].z = s.head_roll_angle;
    for leg in legs {
        m.visible[leg] = true;
    }
    if is_baby {
        let mut d = KfDeltas::new(model);
        apply_kf_walk(
            &FOX_BABY_WALK,
            model,
            walk_pos,
            walk_speed,
            1.0,
            2.5,
            &mut d,
        );
        m.add_kf(&d);
    } else {
        m.swing_legs(legs, |phase| walk_swing(walk_pos, walk_speed, phase));
    }

    if s.is_crouching {
        set_crouching_pose(&mut m, &p, is_baby, age_scale, age_in_ticks, s);
    } else if s.is_sleeping {
        for leg in legs {
            m.visible[leg] = false;
        }
        set_sleeping_pose(&mut m, &p, is_baby);
    } else if s.is_sitting {
        m.rot[p.head].x = 0.0;
        m.rot[p.head].y = 0.0;
        set_sitting_pose(&mut m, &p, is_baby, age_scale);
    }
    // Only the adult overrides `setPouncingPose`.
    if s.is_pouncing && !is_baby {
        let crouch = s.crouch_amount / 2.0;
        m.pos[p.body].y -= crouch;
        m.pos[p.head].y -= crouch;
    }
    if !(s.is_sleeping || s.is_faceplanted || s.is_crouching) {
        m.rot[p.head].x = head_x_rot_deg.to_radians();
        m.rot[p.head].y = local_head_y_rot_deg.to_radians();
    }
    if s.is_sleeping {
        m.rot[p.head].x = 0.0;
        m.rot[p.head].y = -2.0943952;
        m.rot[p.head].z = (age_in_ticks * 0.027).cos() / 22.0;
    }
    if s.is_faceplanted {
        m.swing_legs(legs, |phase| {
            (s.leg_motion_pos * 0.4662 + phase).cos() * 0.1
        });
    }
    m.emit(model)
}

fn set_crouching_pose(
    m: &mut ModelPose,
    p: &FoxParts,
    is_baby: bool,
    age_scale: f32,
    age_in_ticks: f32,
    s: &FoxAnims,
) {
    m.rot[p.body].x += 0.10471976;
    m.pos[p.head].y += s.crouch_amount * age_scale;
    let wiggle = age_in_ticks.cos() * 0.05;
    m.rot[p.body].y = wiggle;
    m.rot[p.right_hind_leg].z = wiggle;
    m.rot[p.left_hind_leg].z = wiggle;
    m.rot[p.right_front_leg].z = wiggle / 2.0;
    m.rot[p.left_front_leg].z = wiggle / 2.0;
    m.pos[p.body].y += if is_baby {
        s.crouch_amount / 6.0
    } else {
        s.crouch_amount
    };
}

fn set_sleeping_pose(m: &mut ModelPose, p: &FoxParts, is_baby: bool) {
    if is_baby {
        m.rot[p.body].z = -1.5707964;
        m.rot[p.body].x = -0.17453292;
        m.pos[p.body].y += 1.5;
        m.pos[p.body].z -= 1.5;
        m.pos[p.body].x -= 1.5;
        // Vanilla assigns -2.6179938 first, then overwrites it.
        m.rot[p.tail].x = -2.1816616;
        m.pos[p.tail].x -= 0.7;
        m.pos[p.tail].z += 0.6;
        m.pos[p.tail].y += 0.9;
        m.pos[p.head].x -= 2.0;
        m.pos[p.head].y += 2.8;
        m.pos[p.head].z -= 4.0;
    } else {
        m.rot[p.body].z = -1.5707964;
        m.pos[p.body].y += 5.0;
        m.rot[p.tail].x = -2.6179938;
        m.pos[p.head].x += 2.0;
        m.pos[p.head].y += 2.99;
    }
    m.rot[p.head].y = -2.0943952;
    m.rot[p.head].z = 0.0;
}

fn set_sitting_pose(m: &mut ModelPose, p: &FoxParts, is_baby: bool, age_scale: f32) {
    if is_baby {
        m.rot[p.body].x = -0.959931;
        m.pos[p.body].z -= 4.5 * age_scale;
        m.pos[p.body].y += 3.0 * age_scale;
        m.pos[p.tail].y -= 0.6;
        m.pos[p.tail].z -= 2.0 * age_scale;
        m.rot[p.tail].x = 0.95993114;
        m.pos[p.head].y -= 0.75;
        m.rot[p.right_front_leg].x = -0.2617994;
        m.rot[p.left_front_leg].x = -0.2617994;
        m.pos[p.right_front_leg].z -= 1.5;
        m.pos[p.left_front_leg].z -= 1.5;
        m.pos[p.right_front_leg].x += 0.01;
        m.pos[p.left_front_leg].x -= 0.01;
        m.pos[p.right_hind_leg].z -= 3.75;
        m.pos[p.left_hind_leg].z -= 3.75;
        m.pos[p.right_hind_leg].x += 0.01;
        m.pos[p.left_hind_leg].x -= 0.01;
    } else {
        m.rot[p.body].x = 0.5235988;
        m.pos[p.body].y -= 7.0;
        m.pos[p.body].z += 3.0;
        m.rot[p.tail].x = 0.7853982;
        m.pos[p.head].y -= 6.5;
        m.pos[p.head].z += 2.75;
        m.rot[p.right_front_leg].x = -0.2617994;
        m.rot[p.left_front_leg].x = -0.2617994;
        m.rot[p.right_hind_leg].x = -1.3089969;
        m.pos[p.right_hind_leg].y += 4.0;
        m.pos[p.right_hind_leg].z -= 0.25;
        m.rot[p.left_hind_leg].x = -1.3089969;
        m.pos[p.left_hind_leg].y += 4.0;
        m.pos[p.left_hind_leg].z -= 0.25;
        m.pos[p.tail].z -= 1.0;
    }
}

/// `FoxBabyAnimation.FOX_BABY_WALK` (0.5s, looping); all-zero channels
/// omitted.
static FOX_BABY_WALK: KfAnim = {
    use Kfi::{Cat, Lin};
    KfAnim {
        length: 0.5,
        looping: true,
        channels: &[
            KfChannel {
                part: "head",
                target: KfTarget::Pos,
                frames: &[kf(0.0, 0.0, -1.025, 0.0, Lin)],
            },
            KfChannel {
                part: "right_hind_leg",
                target: KfTarget::Rot,
                frames: &[
                    kf(0.0, -35.0, 0.0, 0.0, Cat),
                    kf(0.25, 35.0, 0.0, 0.0, Lin),
                    kf(0.5, -35.0, 0.0, 0.0, Cat),
                ],
            },
            KfChannel {
                part: "right_hind_leg",
                target: KfTarget::Pos,
                frames: &[
                    kf(0.0, 0.05, 0.6, -0.02, Lin),
                    kf(0.25, 0.05, 0.6, -0.02, Lin),
                    kf(0.5, 0.05, 0.6, -0.02, Lin),
                ],
            },
            KfChannel {
                part: "right_hind_leg",
                target: KfTarget::Scale,
                frames: &[kf(0.0, 0.0, 0.15, 0.0, Lin)],
            },
            KfChannel {
                part: "left_hind_leg",
                target: KfTarget::Rot,
                frames: &[
                    kf(0.0, 35.0, 0.0, 0.0, Cat),
                    kf(0.25, -35.0, 0.0, 0.0, Lin),
                    kf(0.5, 35.0, 0.0, 0.0, Cat),
                ],
            },
            KfChannel {
                part: "left_hind_leg",
                target: KfTarget::Pos,
                frames: &[
                    kf(0.0, -0.05, 0.6, -0.02, Lin),
                    kf(0.25, -0.05, 0.6, -0.02, Lin),
                    kf(0.5, -0.05, 0.6, -0.02, Lin),
                ],
            },
            KfChannel {
                part: "left_hind_leg",
                target: KfTarget::Scale,
                frames: &[kf(0.0, 0.0, 0.15, 0.0, Lin)],
            },
            KfChannel {
                part: "right_front_leg",
                target: KfTarget::Rot,
                frames: &[
                    kf(0.0, 35.0, 0.0, 0.0, Cat),
                    kf(0.25, -35.0, 0.0, 0.0, Lin),
                    kf(0.5, 35.0, 0.0, 0.0, Cat),
                ],
            },
            KfChannel {
                part: "right_front_leg",
                target: KfTarget::Pos,
                frames: &[kf(0.0, 0.05, 0.6, -0.4, Lin)],
            },
            KfChannel {
                part: "right_front_leg",
                target: KfTarget::Scale,
                frames: &[kf(0.0, 0.0, 0.15, 0.0, Lin)],
            },
            KfChannel {
                part: "left_front_leg",
                target: KfTarget::Rot,
                frames: &[
                    kf(0.0, -35.0, 0.0, 0.0, Cat),
                    kf(0.25, 35.0, 0.0, 0.0, Lin),
                    kf(0.5, -35.0, 0.0, 0.0, Cat),
                ],
            },
            KfChannel {
                part: "left_front_leg",
                target: KfTarget::Pos,
                frames: &[kf(0.0, -0.05, 0.6, -0.4, Lin)],
            },
            KfChannel {
                part: "left_front_leg",
                target: KfTarget::Scale,
                frames: &[kf(0.0, 0.0, 0.15, 0.0, Lin)],
            },
            KfChannel {
                part: "tail",
                target: KfTarget::Rot,
                frames: &[kf(0.0, -2.5, 0.0, 0.0, Cat)],
            },
            KfChannel {
                part: "tail",
                target: KfTarget::Pos,
                frames: &[kf(0.0, 0.0, -0.05, 0.0, Cat)],
            },
        ],
    }
};

#[cfg(test)]
mod tests {
    use super::*;

    /// `setSleepingPose` hides the legs and rolls the body onto its side;
    /// `setWalkingPose` shows them again on the next call.
    #[test]
    fn sleeping_hides_legs_and_rolls_the_body() {
        let model = bake_fox_model();
        let sleeping = FoxAnims {
            is_sleeping: true,
            ..Default::default()
        };
        let anim = compute_fox_anim(&model, false, 0.0, 0.0, 0.0, 0.0, 0.0, &sleeping);
        let leg = part_index(&model.parts, "left_front_leg");
        assert!(anim.scale.contains(&(leg, Vec3::ZERO)));
        assert_eq!(
            anim.rotation_of(&model, "body"),
            Vec3::new(1.5707964, 0.0, -1.5707964)
        );
        assert_eq!(
            anim.translation_of(&model, "body"),
            Vec3::new(0.0, 5.0, 0.0)
        );

        let awake = compute_fox_anim(&model, false, 0.0, 0.0, 0.0, 0.0, 0.0, &FoxAnims::default());
        assert!(awake.scale.is_empty());
    }

    /// Sitting zeroes the head look, but the later look assignment wins
    /// (sitting isn't one of its exclusions).
    #[test]
    fn sitting_keeps_the_head_look() {
        let model = bake_baby_fox_model();
        let sitting = FoxAnims {
            is_sitting: true,
            ..Default::default()
        };
        let anim = compute_fox_anim(&model, true, 30.0, 0.0, 0.0, 0.0, 0.0, &sitting);
        assert_eq!(anim.rotation_of(&model, "head").x, 30.0f32.to_radians());
    }
}
