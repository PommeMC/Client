//! Ender dragon: `EnderDragonModel`, 256x256, and its `setupAnim`, which
//! strings the neck and tail along the dragon's flight history.

use std::f32::consts::{PI, TAU};

use glam::Vec3;

use super::{
    BakedEntityModel, EntityPart, PartAnim, bake_model, mirrored, part_index, rotated, vbox, vpart,
};
use crate::entity::ender_dragon::FlightSample;
use crate::entity::wrap_degrees;
use crate::physics::movement::{mth_cos, mth_sin};

const NECK_NAMES: [&str; 5] = ["neck0", "neck1", "neck2", "neck3", "neck4"];
const TAIL_NAMES: [&str; 12] = [
    "tail0", "tail1", "tail2", "tail3", "tail4", "tail5", "tail6", "tail7", "tail8", "tail9",
    "tail10", "tail11",
];
/// `[side][part]`: the wing and wing tip, then `poseLimbs`' six.
const LIMB_NAMES: [[&str; 8]; 2] = [
    [
        "left_wing",
        "left_wing_tip",
        "left_hind_leg",
        "left_hind_leg_tip",
        "left_hind_foot",
        "left_front_leg",
        "left_front_leg_tip",
        "left_front_foot",
    ],
    [
        "right_wing",
        "right_wing_tip",
        "right_hind_leg",
        "right_hind_leg_tip",
        "right_hind_foot",
        "right_front_leg",
        "right_front_leg_tip",
        "right_front_foot",
    ],
];
/// The flight history the model reads: delays 0 to 23.
pub const DRAGON_HISTORY: usize = 24;

/// Adds one side's wing and legs under `body`, named by its `LIMB_NAMES`
/// row: `x` is 1 for the left side, -1 for the right.
fn push_side(parts: &mut Vec<EntityPart>, body: usize, names: &[&str; 8], x: f32) {
    let [
        wing_name,
        tip_name,
        hind_leg,
        hind_leg_tip,
        hind_foot,
        front_leg,
        front_leg_tip,
        front_foot,
    ] = *names;
    // The left wing reaches +x and is `.mirror()`ed; the right reaches -x.
    let wing_box = |uv, (y, z), size| {
        let cube = vbox(uv, (if x > 0.0 { 0.0 } else { -56.0 }, y, z), size);
        if x > 0.0 { mirrored(cube) } else { cube }
    };
    let wing = parts.len();
    parts.push(vpart(
        wing_name,
        Some(body),
        Vec3::new(12.0 * x, -15.0, -6.0),
        vec![
            wing_box((112, 88), (-4.0, -4.0), (56.0, 8.0, 8.0)),
            wing_box((-56, 88), (0.0, 2.0), (56.0, 0.0, 56.0)),
        ],
    ));
    parts.push(vpart(
        tip_name,
        Some(wing),
        Vec3::new(56.0 * x, 0.0, 0.0),
        vec![
            wing_box((112, 136), (-2.0, -2.0), (56.0, 4.0, 4.0)),
            wing_box((-56, 144), (0.0, 2.0), (56.0, 0.0, 56.0)),
        ],
    ));
    // One leg segment under `parent`, at rest pitched `x_rot`.
    let mut segment = |parent, name: &str, offset, x_rot, cube| {
        parts.push(rotated(
            vpart(name, Some(parent), offset, vec![cube]),
            Vec3::new(x_rot, 0.0, 0.0),
        ));
        parts.len() - 1
    };
    let leg = segment(
        body,
        front_leg,
        Vec3::new(12.0 * x, 0.0, -6.0),
        1.3,
        vbox((112, 104), (-4.0, -4.0, -4.0), (8.0, 24.0, 8.0)),
    );
    let tip = segment(
        leg,
        front_leg_tip,
        Vec3::new(0.0, 20.0, -1.0),
        -0.5,
        vbox((226, 138), (-3.0, -1.0, -3.0), (6.0, 24.0, 6.0)),
    );
    segment(
        tip,
        front_foot,
        Vec3::new(0.0, 23.0, 0.0),
        0.75,
        vbox((144, 104), (-4.0, 0.0, -12.0), (8.0, 4.0, 16.0)),
    );
    let leg = segment(
        body,
        hind_leg,
        Vec3::new(16.0 * x, -4.0, 34.0),
        1.0,
        vbox((0, 0), (-8.0, -4.0, -8.0), (16.0, 32.0, 16.0)),
    );
    let tip = segment(
        leg,
        hind_leg_tip,
        Vec3::new(0.0, 32.0, -4.0),
        0.5,
        vbox((196, 0), (-6.0, -2.0, 0.0), (12.0, 32.0, 12.0)),
    );
    segment(
        tip,
        hind_foot,
        Vec3::new(0.0, 31.0, 4.0),
        0.75,
        vbox((112, 0), (-9.0, 0.0, -20.0), (18.0, 6.0, 24.0)),
    );
}

/// Vanilla `EnderDragonModel.createBodyLayer`, under a cubeless `root` for
/// `setupAnim`'s root pose.
pub fn bake_ender_dragon_model() -> BakedEntityModel {
    let root = 0;
    let mut parts = vec![
        vpart("root", None, Vec3::ZERO, vec![]),
        vpart(
            "head",
            Some(root),
            Vec3::new(0.0, 20.0, -62.0),
            vec![
                vbox((176, 44), (-6.0, -1.0, -24.0), (12.0, 5.0, 16.0)),
                vbox((112, 30), (-8.0, -8.0, -10.0), (16.0, 16.0, 16.0)),
                mirrored(vbox((0, 0), (-5.0, -12.0, -4.0), (2.0, 4.0, 6.0))),
                mirrored(vbox((112, 0), (-5.0, -3.0, -22.0), (2.0, 2.0, 4.0))),
                mirrored(vbox((0, 0), (3.0, -12.0, -4.0), (2.0, 4.0, 6.0))),
                mirrored(vbox((112, 0), (3.0, -3.0, -22.0), (2.0, 2.0, 4.0))),
            ],
        ),
    ];
    parts.push(vpart(
        "jaw",
        Some(1),
        Vec3::new(0.0, 4.0, -8.0),
        vec![vbox((176, 65), (-6.0, 0.0, -16.0), (12.0, 4.0, 16.0))],
    ));
    let spine = || {
        vec![
            vbox((192, 104), (-5.0, -5.0, -5.0), (10.0, 10.0, 10.0)),
            vbox((48, 0), (-1.0, -9.0, -3.0), (2.0, 4.0, 6.0)),
        ]
    };
    for (i, name) in NECK_NAMES.into_iter().enumerate() {
        parts.push(vpart(
            name,
            Some(root),
            Vec3::new(0.0, 20.0, -12.0 - i as f32 * 10.0),
            spine(),
        ));
    }
    for (i, name) in TAIL_NAMES.into_iter().enumerate() {
        parts.push(vpart(
            name,
            Some(root),
            Vec3::new(0.0, 10.0, 60.0 + i as f32 * 10.0),
            spine(),
        ));
    }
    let body = parts.len();
    let scale = |z| vbox((220, 53), (-1.0, -22.0, z), (2.0, 6.0, 12.0));
    parts.push(vpart(
        "body",
        Some(root),
        Vec3::new(0.0, 20.0, 8.0),
        vec![
            vbox((0, 0), (-12.0, -16.0, -16.0), (24.0, 24.0, 64.0)),
            scale(-10.0),
            scale(10.0),
            scale(30.0),
        ],
    ));
    push_side(&mut parts, body, &LIMB_NAMES[0], 1.0);
    push_side(&mut parts, body, &LIMB_NAMES[1], -1.0);
    bake_model(parts, 256, 256)
}

/// `EnderDragonRenderState`'s fields the model reads.
#[derive(Clone, Copy, Default)]
pub struct EnderDragonAnims {
    pub flap_time: f32,
    /// `getHistoricalPos(delay)` for delays 0 to 23.
    pub history: [FlightSample; DRAGON_HISTORY],
    pub is_landing_or_taking_off: bool,
    pub is_sitting: bool,
    /// Squared distance to the top of the exit portal's podium.
    pub distance_to_egg: f64,
}

impl EnderDragonAnims {
    /// `EnderDragonRenderState.getHeadPartYOffset`.
    fn head_part_y_offset(&self, part: usize, body: FlightSample, part_pos: FlightSample) -> f32 {
        (if self.is_landing_or_taking_off {
            part as f64 / (self.distance_to_egg / 4.0).max(1.0)
        } else if self.is_sitting {
            part as f64
        } else if part == 6 {
            0.0
        } else {
            part_pos.y - body.y
        }) as f32
    }
}

/// Where the next neck or tail segment hangs: 10 along this one's pitch and
/// yaw.
fn segment_step(x_rot: f32, y_rot: f32) -> Vec3 {
    Vec3::new(
        -mth_sin(y_rot) * mth_cos(x_rot),
        mth_sin(x_rot),
        -mth_cos(y_rot) * mth_cos(x_rot),
    ) * 10.0
}

/// Vanilla `EnderDragonModel.setupAnim`.
pub fn compute_ender_dragon_anim(model: &BakedEntityModel, dragon: &EnderDragonAnims) -> PartAnim {
    let deg = PI / 180.0;
    let index = |name| part_index(&model.parts, name);
    let rest = |name| model.parts[index(name)].offset;
    let history = |delay: usize| dragon.history[delay];
    let mut anim = PartAnim::default();
    let mut pose = |name, offset: Vec3, rotation: Vec3| {
        let i = index(name);
        anim.rotation.push((i, rotation));
        anim.translation.push((i, offset - rest(name)));
    };

    let flap = dragon.flap_time * TAU;
    let bounce = mth_sin(flap - 1.0) + 1.0;
    let bounce = (bounce * bounce + bounce * 2.0) * 0.05;
    pose(
        "root",
        Vec3::new(0.0, (bounce - 2.0) * 16.0, -48.0),
        Vec3::new(bounce * 2.0 * deg, 0.0, 0.0),
    );
    pose(
        "jaw",
        rest("jaw"),
        Vec3::new((mth_sin(flap) + 1.0) * 0.2, 0.0, 0.0),
    );

    let start = history(6);
    let rot2 = wrap_degrees(history(5).y_rot - history(10).y_rot);
    let rot = wrap_degrees(history(5).y_rot + rot2 / 2.0);
    let mut at = rest("neck0");
    for (i, name) in NECK_NAMES.into_iter().enumerate() {
        let point = history(5 - i);
        let y_rot = wrap_degrees(point.y_rot - start.y_rot) * deg * 1.5;
        let x_rot = mth_cos(i as f32 * 0.45 + flap) * 0.15
            + dragon.head_part_y_offset(i, start, point) * deg * 1.5 * 5.0;
        let z_rot = -wrap_degrees(point.y_rot - rot) * deg * 1.5;
        pose(name, at, Vec3::new(x_rot, y_rot, z_rot));
        at += segment_step(x_rot, y_rot);
    }
    let current = history(0);
    pose(
        "head",
        at,
        Vec3::new(
            wrap_degrees(dragon.head_part_y_offset(6, start, current)) * deg * 1.5 * 5.0,
            wrap_degrees(current.y_rot - start.y_rot) * deg,
            -wrap_degrees(current.y_rot - rot) * deg,
        ),
    );
    pose("body", rest("body"), Vec3::new(0.0, 0.0, -rot2 * 1.5 * deg));

    let wing = Vec3::new(
        0.125 - mth_cos(flap) * 0.2,
        -0.25,
        -(mth_sin(flap) + 0.125) * 0.8,
    );
    let wing_tip_z = (mth_sin(flap + 2.0) + 0.5) * 0.75;
    // `poseLimbs`, in `LIMB_NAMES` order.
    let b = bounce * 0.1;
    let limb_x_rots = [1.0 + b, 0.5 + b, 0.75 + b, 1.3 + b, -0.5 - b, 0.75 + b];
    for (names, sign) in LIMB_NAMES.iter().zip([1.0, -1.0]) {
        let [wing_name, tip_name, limbs @ ..] = names;
        pose(
            wing_name,
            rest(wing_name),
            Vec3::new(wing.x, wing.y * sign, wing.z * sign),
        );
        pose(
            tip_name,
            rest(tip_name),
            Vec3::new(0.0, 0.0, wing_tip_z * sign),
        );
        for (name, x_rot) in limbs.iter().zip(limb_x_rots) {
            pose(name, rest(name), Vec3::new(x_rot, 0.0, 0.0));
        }
    }

    let start = history(11);
    let mut at = rest("tail0");
    let mut tail_x_rot = 0.0;
    for (i, name) in TAIL_NAMES.into_iter().enumerate() {
        let point = history(12 + i);
        tail_x_rot += mth_sin(i as f32 * 0.45 + flap) * 0.05;
        let y_rot = (wrap_degrees(point.y_rot - start.y_rot) * 1.5 + 180.0) * deg;
        let x_rot = tail_x_rot + (point.y - start.y) as f32 * deg * 1.5 * 5.0;
        let z_rot = wrap_degrees(point.y_rot - rot) * deg * 1.5;
        pose(name, at, Vec3::new(x_rot, y_rot, z_rot));
        at += segment_step(x_rot, y_rot);
    }
    anim
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A climbing, turning flight: every sample differs.
    fn turning_flight() -> EnderDragonAnims {
        EnderDragonAnims {
            flap_time: 0.3,
            history: std::array::from_fn(|delay| FlightSample {
                y: 64.0 - delay as f64 * 0.4,
                y_rot: delay as f32 * 7.0,
            }),
            ..Default::default()
        }
    }

    fn pivot(model: &BakedEntityModel, anim: &PartAnim, name: &str) -> Vec3 {
        model.parts[part_index(&model.parts, name)].offset + anim.translation_of(model, name)
    }

    #[test]
    fn neck_and_tail_segments_stay_ten_apart() {
        let model = bake_ender_dragon_model();
        let anim = compute_ender_dragon_anim(&model, &turning_flight());
        for chain in [&NECK_NAMES[..], &TAIL_NAMES[..]] {
            for pair in chain.windows(2) {
                let gap = pivot(&model, &anim, pair[1]) - pivot(&model, &anim, pair[0]);
                assert!((gap.length() - 10.0).abs() < 1e-3, "{pair:?}: {gap}");
            }
        }
        // The head hangs off the last neck segment.
        let gap = pivot(&model, &anim, "head") - pivot(&model, &anim, "neck4");
        assert!((gap.length() - 10.0).abs() < 1e-3);
    }

    #[test]
    fn a_turn_bends_the_neck_toward_the_newer_yaw() {
        let model = bake_ender_dragon_model();
        let anim = compute_ender_dragon_anim(&model, &turning_flight());
        // neck0 follows delay 5, measured from delay 6: 7 degrees, times 1.5.
        let neck0 = anim.rotation_of(&model, "neck0");
        assert!((neck0.y - (-7.0f32 * 1.5).to_radians()).abs() < 1e-5);
    }

    #[test]
    fn the_wings_mirror_each_other() {
        let model = bake_ender_dragon_model();
        let anim = compute_ender_dragon_anim(&model, &turning_flight());
        let left = anim.rotation_of(&model, "left_wing");
        let right = anim.rotation_of(&model, "right_wing");
        assert_eq!((left.x, left.y, left.z), (right.x, -right.y, -right.z));
        assert_eq!(
            anim.rotation_of(&model, "left_wing_tip").z,
            -anim.rotation_of(&model, "right_wing_tip").z
        );
    }

    #[test]
    fn a_landing_dragon_dips_its_head_by_distance() {
        let model = bake_ender_dragon_model();
        let far = EnderDragonAnims {
            is_landing_or_taking_off: true,
            distance_to_egg: 400.0,
            ..turning_flight()
        };
        // Part 6 over max(400 / 4, 1): 0.06 degrees, times 1.5 and 5.
        let head = compute_ender_dragon_anim(&model, &far).rotation_of(&model, "head");
        assert!((head.x - (0.06f32 * 7.5).to_radians()).abs() < 1e-6);
    }
}
