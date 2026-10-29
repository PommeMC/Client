//! Armadillo: `AdultArmadilloModel`/`BabyArmadilloModel` meshes,
//! `ArmadilloModel.setupAnim`, and the `ArmadilloAnimation` /
//! `BabyArmadilloAnimation` keyframe tables (converted mechanically from the
//! decompiled definitions; all-zero channels omitted).

// Vanilla's radian literals are kept verbatim, not swapped for `consts`.
#![allow(clippy::approx_constant)]

use glam::Vec3;

use super::{
    BakedEntityModel, KfAnim, KfChannel, KfDeltas, KfTarget, Kfi, PartAnim, apply_kf_anim,
    apply_kf_walk, bake_model, deformed, head_rotation, kf, mirrored, part_index, rotated, vbox,
    vpart,
};

/// The body's own cubes, split off as a child at the body's pivot so
/// `body.skipDraw` can hide them while the head (also a body child) still
/// draws. Pomme-only; no keyframe channel targets it.
const BODY_SHELL: &str = "body_shell";

/// Vanilla `AdultArmadilloModel.createBodyLayer`, 64x64.
pub fn bake_armadillo_model() -> BakedEntityModel {
    let leg = |name: &str, x: f32, z: f32, uv: (i32, i32)| {
        vpart(
            name,
            None,
            Vec3::new(x, 21.0, z),
            vec![vbox(uv, (-1.0, 0.0, -1.0), (2.0, 3.0, 2.0))],
        )
    };
    let parts = vec![
        vpart("body", None, Vec3::new(0.0, 21.0, 4.0), vec![]),
        vpart(
            BODY_SHELL,
            Some(0),
            Vec3::ZERO,
            vec![
                deformed(vbox((0, 20), (-4.0, -7.0, -10.0), (8.0, 8.0, 12.0)), 0.3),
                vbox((0, 40), (-4.0, -7.0, -10.0), (8.0, 8.0, 12.0)),
            ],
        ),
        rotated(
            vpart(
                "tail",
                Some(0),
                Vec3::new(0.0, -3.0, 1.0),
                vec![vbox((44, 53), (-0.5, -0.0865, 0.0933), (1.0, 6.0, 1.0))],
            ),
            Vec3::new(0.5061, 0.0, 0.0),
        ),
        vpart("head", Some(0), Vec3::new(0.0, -2.0, -11.0), vec![]),
        rotated(
            vpart(
                "head_cube",
                Some(3),
                Vec3::ZERO,
                vec![vbox((43, 15), (-1.5, -1.0, -1.0), (3.0, 5.0, 2.0))],
            ),
            Vec3::new(-0.3927, 0.0, 0.0),
        ),
        vpart("right_ear", Some(3), Vec3::new(-1.0, -1.0, 0.0), vec![]),
        rotated(
            vpart(
                "right_ear_cube",
                Some(5),
                Vec3::new(-0.5, 0.0, -0.6),
                vec![vbox((43, 10), (-2.0, -3.0, 0.0), (2.0, 5.0, 0.0))],
            ),
            Vec3::new(0.1886, -0.3864, -0.0718),
        ),
        vpart("left_ear", Some(3), Vec3::new(1.0, -2.0, 0.0), vec![]),
        rotated(
            vpart(
                "left_ear_cube",
                Some(7),
                Vec3::new(0.5, 1.0, -0.6),
                vec![vbox((47, 10), (0.0, -3.0, 0.0), (2.0, 5.0, 0.0))],
            ),
            Vec3::new(0.1886, 0.3864, 0.0718),
        ),
        leg("right_hind_leg", -2.0, 4.0, (51, 31)),
        leg("left_hind_leg", 2.0, 4.0, (42, 31)),
        leg("right_front_leg", -2.0, -4.0, (51, 43)),
        leg("left_front_leg", 2.0, -4.0, (42, 43)),
        vpart(
            "cube",
            None,
            Vec3::new(0.0, 24.0, 0.0),
            vec![vbox((0, 0), (-5.0, -10.0, -6.0), (10.0, 10.0, 10.0))],
        ),
    ];
    bake_model(parts, 64, 64)
}

/// Vanilla `BabyArmadilloModel.createBodyLayer`, 64x64. The names are
/// vanilla's: the tail cube is called `right_ear_cube`, the ears hang off
/// `head_cube`, and the right front leg sits at +x.
pub fn bake_baby_armadillo_model() -> BakedEntityModel {
    let parts = vec![
        vpart("body", None, Vec3::new(0.0, 20.0, 0.5), vec![]),
        vpart(
            BODY_SHELL,
            Some(0),
            Vec3::ZERO,
            vec![
                deformed(vbox((0, 0), (-2.5, -2.0, -3.5), (5.0, 4.0, 7.0)), 0.3),
                vbox((0, 11), (-2.5, -2.0, -3.0), (5.0, 4.0, 6.0)),
            ],
        ),
        vpart("tail", Some(0), Vec3::new(0.0, 0.0, 3.4), vec![]),
        rotated(
            vpart(
                "right_ear_cube",
                Some(2),
                Vec3::new(0.0, 1.5, 1.0),
                vec![vbox((22, 11), (-0.5, -0.5, -2.0), (1.0, 1.0, 4.0))],
            ),
            Vec3::new(-1.0472, 0.0, 0.0),
        ),
        vpart("head", Some(0), Vec3::new(0.0, 0.0, -3.2), vec![]),
        rotated(
            vpart(
                "head_cube",
                Some(4),
                Vec3::ZERO,
                vec![vbox((20, 17), (-1.0, -2.0, -4.0), (2.0, 2.0, 4.0))],
            ),
            Vec3::new(0.7417649, 0.0, 0.0),
        ),
        rotated(
            vpart(
                "right_ear",
                Some(5),
                Vec3::new(-1.0, -2.0, -0.3),
                vec![mirrored(vbox((28, 8), (-1.8, -2.0, 0.0), (2.0, 3.0, 0.0)))],
            ),
            Vec3::new(-0.4363, -0.1134, 0.0524),
        ),
        rotated(
            vpart(
                "left_ear",
                Some(5),
                Vec3::new(1.0, -2.0, -0.3),
                vec![vbox((28, 8), (-0.2, -2.0, 0.0), (2.0, 3.0, 0.0))],
            ),
            Vec3::new(-0.4363, 0.1134, -0.0524),
        ),
        vpart(
            "right_hind_leg",
            None,
            Vec3::new(-1.5, 22.0, 2.5),
            vec![mirrored(vbox((20, 27), (-1.0, 0.0, -1.0), (2.0, 2.0, 2.0)))],
        ),
        vpart(
            "left_hind_leg",
            None,
            Vec3::new(1.5, 22.0, 2.5),
            vec![vbox((20, 27), (-1.0, 0.0, -1.0), (2.0, 2.0, 2.0))],
        ),
        vpart(
            "right_front_leg",
            None,
            Vec3::new(1.5, 22.0, -1.5),
            vec![vbox((20, 23), (-1.0, 0.0, -1.0), (2.0, 2.0, 2.0))],
        ),
        vpart(
            "left_front_leg",
            None,
            Vec3::new(-1.5, 22.0, -1.5),
            vec![mirrored(vbox((24, 0), (-1.0, 0.0, -1.0), (2.0, 2.0, 2.0)))],
        ),
        vpart(
            "cube",
            None,
            Vec3::new(0.0, 20.7, 0.5),
            vec![deformed(
                vbox((0, 25), (-3.0, -3.0, -3.0), (6.0, 6.0, 6.0)),
                0.3,
            )],
        ),
    ];
    bake_model(parts, 64, 64)
}

/// `ArmadilloRenderState`: the shell flag and the three `AnimationState`
/// clocks (seconds since start; `None` = stopped).
#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub struct ArmadilloAnims {
    pub hiding_in_shell: bool,
    pub roll_out: Option<f32>,
    pub roll_up: Option<f32>,
    pub peek: Option<f32>,
}

/// One mesh's `ArmadilloModel` constructor arguments.
struct ArmadilloTables {
    walk: &'static KfAnim,
    roll_out: &'static KfAnim,
    roll_up: &'static KfAnim,
    peek: &'static KfAnim,
}

static ADULT: ArmadilloTables = ArmadilloTables {
    walk: &ARMADILLO_WALK,
    roll_out: &ARMADILLO_ROLL_OUT,
    roll_up: &ARMADILLO_ROLL_UP,
    peek: &ARMADILLO_PEEK,
};

static BABY: ArmadilloTables = ArmadilloTables {
    walk: &ARMADILLO_BABY_WALK,
    roll_out: &ARMADILLO_BABY_ROLL_OUT,
    roll_up: &ARMADILLO_BABY_ROLL_UP,
    peek: &ARMADILLO_BABY_PEEK,
};

/// Vanilla `ArmadilloModel.setupAnim`, shared by the adult and baby meshes.
/// Hidden parts (`visible = false`, `skipDraw`) collapse to a zero scale.
pub fn compute_armadillo_anim(
    model: &BakedEntityModel,
    head_x_rot_deg: f32,
    local_head_y_rot_deg: f32,
    walk_pos: f32,
    walk_speed: f32,
    is_baby: bool,
    a: &ArmadilloAnims,
) -> PartAnim {
    let t = if is_baby { &BABY } else { &ADULT };
    let mut d = KfDeltas::new(model);
    if !a.hiding_in_shell {
        d.set_rot(
            model,
            part_index(&model.parts, "head"),
            head_rotation(
                head_x_rot_deg.clamp(-22.5, 25.0),
                local_head_y_rot_deg.clamp(-32.5, 32.5),
            ),
        );
        apply_kf_walk(t.walk, model, walk_pos, walk_speed, 16.5, 2.5, &mut d);
    }
    apply_kf_anim(t.roll_out, model, a.roll_out, &mut d);
    apply_kf_anim(t.roll_up, model, a.roll_up, &mut d);
    apply_kf_anim(t.peek, model, a.peek, &mut d);
    let mut anim = d.emit(model);
    let hidden: &[&str] = if a.hiding_in_shell {
        &[BODY_SHELL, "left_hind_leg", "right_hind_leg", "tail"]
    } else {
        &["cube"]
    };
    for name in hidden {
        anim.scale
            .push((part_index(&model.parts, name), Vec3::ZERO));
    }
    anim
}

static ARMADILLO_ROLL_UP: KfAnim = {
    use Kfi::Lin;
    KfAnim {
        length: 0.5,
        looping: false,
        channels: &[
            KfChannel {
                part: "body",
                target: KfTarget::Pos,
                frames: &[
                    kf(0.0, 0.0, 0.0, 0.0, Lin),
                    kf(0.1667, 0.0, 5.0, 0.0, Lin),
                    kf(0.2083, 0.0, 6.0, -1.0, Lin),
                    kf(0.25, 0.0, 6.0, -1.0, Lin),
                    kf(0.375, 0.0, -1.0, -1.0, Lin),
                ],
            },
            KfChannel {
                part: "tail",
                target: KfTarget::Pos,
                frames: &[
                    kf(0.0, 0.0, 0.0, 0.0, Lin),
                    kf(0.1667, 0.0, 0.0, 0.0, Lin),
                    kf(0.2083, 0.0, 0.0, -2.0, Lin),
                    kf(0.25, 0.0, 0.0, -2.0, Lin),
                ],
            },
            KfChannel {
                part: "head",
                target: KfTarget::Rot,
                frames: &[
                    kf(0.0, 0.0, 0.0, 0.0, Lin),
                    kf(0.1667, 17.5, 0.0, 0.0, Lin),
                    kf(0.25, -72.5, 0.0, 0.0, Lin),
                ],
            },
            KfChannel {
                part: "head",
                target: KfTarget::Pos,
                frames: &[
                    kf(0.0, 0.0, 0.0, 0.0, Lin),
                    kf(0.1667, 0.0, -1.0, 0.0, Lin),
                    kf(0.2083, 0.0, 2.0, 1.0, Lin),
                    kf(0.25, 0.0, 2.0, 1.0, Lin),
                    kf(0.2917, 0.0, 2.0, 6.0, Lin),
                    kf(0.375, 0.0, 2.0, 7.0, Lin),
                ],
            },
            KfChannel {
                part: "right_hind_leg",
                target: KfTarget::Rot,
                frames: &[
                    kf(0.0, 0.0, 0.0, 0.0, Lin),
                    kf(0.1667, -35.0, 0.0, 0.0, Lin),
                ],
            },
            KfChannel {
                part: "right_hind_leg",
                target: KfTarget::Pos,
                frames: &[
                    kf(0.0, 0.0, 0.0, 0.0, Lin),
                    kf(0.1667, 0.0, 5.0, -2.0, Lin),
                    kf(0.1875, 0.0, 8.0, -3.0, Lin),
                    kf(0.2083, 0.0, 8.0, -2.0, Lin),
                    kf(0.25, 0.0, 8.0, -2.0, Lin),
                    kf(0.375, 1.0, 3.0, -6.0, Lin),
                ],
            },
            KfChannel {
                part: "left_hind_leg",
                target: KfTarget::Rot,
                frames: &[
                    kf(0.0, 0.0, 0.0, 0.0, Lin),
                    kf(0.1667, -45.0, 0.0, 0.0, Lin),
                ],
            },
            KfChannel {
                part: "left_hind_leg",
                target: KfTarget::Pos,
                frames: &[
                    kf(0.0, 0.0, 0.0, 0.0, Lin),
                    kf(0.1667, 0.0, 5.0, -2.0, Lin),
                    kf(0.1875, 0.0, 8.0, -3.0, Lin),
                    kf(0.2083, 0.0, 8.0, -2.0, Lin),
                    kf(0.25, 0.0, 8.0, -2.0, Lin),
                    kf(0.375, -1.0, 3.0, -6.0, Lin),
                ],
            },
            KfChannel {
                part: "right_front_leg",
                target: KfTarget::Rot,
                frames: &[
                    kf(0.0, 0.0, 0.0, 0.0, Lin),
                    kf(0.1667, -27.5, 0.0, 0.0, Lin),
                    kf(0.2083, -32.5, 0.0, 0.0, Lin),
                    kf(0.25, -85.0, 0.0, 0.0, Lin),
                ],
            },
            KfChannel {
                part: "right_front_leg",
                target: KfTarget::Pos,
                frames: &[
                    kf(0.0, 0.0, 0.0, 0.0, Lin),
                    kf(0.1667, 0.0, 5.0, 0.0, Lin),
                    kf(0.1875, -0.5, 11.5, 0.5, Lin),
                    kf(0.2083, -1.0, 9.0, -1.0, Lin),
                    kf(0.25, -1.0, 9.0, -1.0, Lin),
                    kf(0.375, -1.0, 2.0, 3.0, Lin),
                ],
            },
            KfChannel {
                part: "left_front_leg",
                target: KfTarget::Rot,
                frames: &[
                    kf(0.0, 0.0, 0.0, 0.0, Lin),
                    kf(0.1667, -12.5, 0.0, 0.0, Lin),
                    kf(0.2083, -35.0, 0.0, 0.0, Lin),
                    kf(0.25, -85.0, 0.0, 0.0, Lin),
                ],
            },
            KfChannel {
                part: "left_front_leg",
                target: KfTarget::Pos,
                frames: &[
                    kf(0.0, 0.0, 0.0, 0.0, Lin),
                    kf(0.1667, 0.0, 5.0, 0.0, Lin),
                    kf(0.1875, 0.5, 11.5, 0.5, Lin),
                    kf(0.2083, 1.0, 9.0, -1.0, Lin),
                    kf(0.25, 1.0, 9.0, -1.0, Lin),
                    kf(0.375, 1.0, 2.0, 3.0, Lin),
                ],
            },
            KfChannel {
                part: "cube",
                target: KfTarget::Rot,
                frames: &[
                    kf(0.0, 0.0, 0.0, 0.0, Lin),
                    kf(0.1667, 0.0, 0.0, 0.0, Lin),
                    kf(0.4167, -2.5, 0.0, 0.0, Lin),
                    kf(0.4583, 5.0, 0.0, 0.0, Lin),
                    kf(0.5, 0.0, 0.0, 0.0, Lin),
                ],
            },
            KfChannel {
                part: "cube",
                target: KfTarget::Pos,
                frames: &[
                    kf(0.0, 0.0, 3.0, 0.0, Lin),
                    kf(0.1667, 0.0, 8.0, 0.0, Lin),
                    kf(0.2083, 0.0, 7.0, 0.0, Lin),
                    kf(0.25, 0.0, 7.0, 0.0, Lin),
                    kf(0.375, 0.0, 0.0, 0.0, Lin),
                    kf(0.4167, 0.0, 1.0, 0.0, Lin),
                    kf(0.4583, 0.0, 0.6, 0.0, Lin),
                    kf(0.5, 0.0, 0.0, 0.0, Lin),
                ],
            },
        ],
    }
};

static ARMADILLO_WALK: KfAnim = {
    use Kfi::{Cat, Lin};
    KfAnim {
        length: 1.4583,
        looping: true,
        channels: &[
            KfChannel {
                part: "body",
                target: KfTarget::Rot,
                frames: &[
                    kf(0.0, 0.0, 0.0, 0.0, Cat),
                    kf(0.25, 0.0, 0.0, 4.6, Cat),
                    kf(0.2917, 0.0, 0.0, 6.81, Cat),
                    kf(0.5, 0.0, 0.0, 0.0, Cat),
                    kf(0.7083, 0.0, 0.0, 0.0, Cat),
                    kf(0.9583, 0.0, 0.0, -4.6, Cat),
                    kf(1.0, 0.0, 0.0, -6.89, Cat),
                    kf(1.25, 0.0, 0.0, 0.0, Cat),
                    kf(1.4583, 0.0, 0.0, 0.0, Cat),
                ],
            },
            KfChannel {
                part: "body",
                target: KfTarget::Pos,
                frames: &[
                    kf(0.0, 0.0, 0.0, 0.0, Cat),
                    kf(0.25, 0.0, -0.2, 0.0, Cat),
                    kf(0.5, 0.0, 0.0, 0.0, Cat),
                    kf(0.7083, 0.0, 0.0, 0.0, Cat),
                    kf(0.9583, 0.0, -0.2, 0.0, Cat),
                    kf(1.25, 0.0, 0.0, 0.0, Cat),
                    kf(1.4583, 0.0, 0.0, 0.0, Cat),
                ],
            },
            KfChannel {
                part: "tail",
                target: KfTarget::Rot,
                frames: &[
                    kf(0.0, 0.0, 0.0, 0.0, Lin),
                    kf(0.5, -9.17, 0.0, 0.0, Lin),
                    kf(0.75, 5.0, 0.0, 0.0, Lin),
                    kf(1.2083, -8.24, 0.0, 0.0, Lin),
                    kf(1.4583, 0.0, 0.0, 0.0, Lin),
                ],
            },
            KfChannel {
                part: "right_hind_leg",
                target: KfTarget::Rot,
                frames: &[
                    kf(0.0, -50.0, 0.0, 0.0, Lin),
                    kf(0.25, 0.0, 0.0, 0.0, Lin),
                    kf(0.5, 0.0, 0.0, 0.0, Lin),
                    kf(0.75, 50.0, 0.0, 0.0, Lin),
                    kf(1.0, 50.0, 0.0, 0.0, Lin),
                    kf(1.2917, -20.0, 0.0, 0.0, Lin),
                    kf(1.4583, -50.0, 0.0, 0.0, Lin),
                ],
            },
            KfChannel {
                part: "right_hind_leg",
                target: KfTarget::Pos,
                frames: &[
                    kf(0.0, 0.0, 0.0, 0.0, Lin),
                    kf(0.25, 0.0, 0.0, -0.5, Lin),
                    kf(0.5, 0.0, 0.0, 0.0, Lin),
                    kf(1.0, 0.0, 0.0, -0.5, Lin),
                    kf(1.2917, 0.0, 1.0, -0.18, Lin),
                    kf(1.4583, 0.0, 0.0, 0.0, Lin),
                ],
            },
            KfChannel {
                part: "left_hind_leg",
                target: KfTarget::Rot,
                frames: &[
                    kf(0.0, 50.0, 0.0, 0.0, Lin),
                    kf(0.25, 50.0, 0.0, 0.0, Lin),
                    kf(0.5417, -20.0, 0.0, 0.0, Lin),
                    kf(0.7083, -50.0, 0.0, 0.0, Lin),
                    kf(0.9583, 0.0, 0.0, 0.0, Lin),
                    kf(1.2083, 0.0, 0.0, 0.0, Lin),
                    kf(1.4583, 50.0, 0.0, 0.0, Lin),
                ],
            },
            KfChannel {
                part: "left_hind_leg",
                target: KfTarget::Pos,
                frames: &[
                    kf(0.0, 0.0, 0.0, -0.25, Lin),
                    kf(0.25, 0.0, 0.0, -0.5, Lin),
                    kf(0.5417, 0.0, 1.0, -0.18, Lin),
                    kf(0.7083, 0.0, 0.0, 0.0, Lin),
                    kf(0.9583, 0.0, 0.0, -0.5, Lin),
                    kf(1.2083, 0.0, 0.0, 0.0, Lin),
                    kf(1.4583, 0.0, 0.0, -0.25, Lin),
                ],
            },
            KfChannel {
                part: "right_front_leg",
                target: KfTarget::Rot,
                frames: &[
                    kf(0.0, 50.0, 0.0, 0.0, Lin),
                    kf(0.2917, 50.0, 0.0, 0.0, Lin),
                    kf(0.5417, -20.0, 0.0, 0.0, Lin),
                    kf(0.7083, -50.0, 0.0, 0.0, Lin),
                    kf(0.9583, 0.0, 0.0, 0.0, Lin),
                    kf(1.2083, 0.0, 0.0, 0.0, Lin),
                    kf(1.4583, 50.0, 0.0, 0.0, Lin),
                ],
            },
            KfChannel {
                part: "right_front_leg",
                target: KfTarget::Pos,
                frames: &[
                    kf(0.0, 0.0, 0.0, -0.25, Lin),
                    kf(0.25, 0.0, 0.0, -0.5, Lin),
                    kf(0.5417, 0.0, 1.0, -0.18, Lin),
                    kf(0.7083, 0.0, 0.0, 0.0, Lin),
                    kf(0.9583, 0.0, 0.0, -0.5, Lin),
                    kf(1.2083, 0.0, 0.0, 0.0, Lin),
                    kf(1.4583, 0.0, 0.0, -0.25, Lin),
                ],
            },
            KfChannel {
                part: "left_front_leg",
                target: KfTarget::Rot,
                frames: &[
                    kf(0.0, -50.0, 0.0, 0.0, Lin),
                    kf(0.25, 0.0, 0.0, 0.0, Lin),
                    kf(0.5, 0.0, 0.0, 0.0, Lin),
                    kf(0.75, 50.0, 0.0, 0.0, Lin),
                    kf(1.0, 50.0, 0.0, 0.0, Lin),
                    kf(1.2917, -20.0, 0.0, 0.0, Lin),
                    kf(1.4583, -50.0, 0.0, 0.0, Lin),
                ],
            },
            KfChannel {
                part: "left_front_leg",
                target: KfTarget::Pos,
                frames: &[
                    kf(0.0, 0.0, 0.0, 0.0, Lin),
                    kf(0.25, 0.0, 0.0, -0.5, Lin),
                    kf(0.5, 0.0, 0.0, 0.0, Lin),
                    kf(1.0, 0.0, 0.0, -0.5, Lin),
                    kf(1.2917, 0.0, 1.0, -0.18, Lin),
                    kf(1.4583, 0.0, 0.0, 0.0, Lin),
                ],
            },
            KfChannel {
                part: "head",
                target: KfTarget::Rot,
                frames: &[
                    kf(0.0, 0.0, 0.0, 0.0, Lin),
                    kf(0.25, 0.0, 0.0, -2.5, Lin),
                    kf(0.5, 0.0, 0.0, 0.0, Lin),
                    kf(0.7083, 0.0, 0.0, 0.0, Lin),
                    kf(1.0, 0.0, 0.0, 2.5, Lin),
                    kf(1.25, 0.0, 0.0, 0.0, Lin),
                    kf(1.4583, 0.0, 0.0, 0.0, Lin),
                ],
            },
        ],
    }
};

static ARMADILLO_PEEK: KfAnim = {
    use Kfi::{Cat, Lin};
    KfAnim {
        length: 2.5,
        looping: false,
        channels: &[
            KfChannel {
                part: "head",
                target: KfTarget::Rot,
                frames: &[
                    kf(0.0, -70.0, 0.0, 0.0, Lin),
                    kf(0.15, -65.0, 0.0, 0.0, Lin),
                    kf(0.4, -50.0, 0.0, 0.0, Lin),
                    kf(0.5, 0.0, 0.0, 0.0, Lin),
                    kf(0.7, 0.0, 0.0, 0.0, Lin),
                    kf(0.9, -7.5, 0.0, 45.0, Cat),
                    kf(1.15, -7.5, 0.0, 45.0, Lin),
                    kf(1.3, -0.8639, -1.4959, -39.1287, Cat),
                    kf(1.6, -0.8639, -1.4959, -39.1287, Lin),
                    kf(1.75, 0.0, 0.0, 0.0, Lin),
                    kf(1.8, -25.0, 0.0, 0.0, Lin),
                    kf(1.85, -70.0, 0.0, 0.0, Lin),
                ],
            },
            KfChannel {
                part: "head",
                target: KfTarget::Pos,
                frames: &[
                    kf(0.0, 0.0, 1.0, 7.0, Lin),
                    kf(0.05, 0.0, 1.0, 4.0, Lin),
                    kf(0.15, 0.0, 1.0, 4.0, Lin),
                    kf(0.25, 0.0, 1.0, 5.0, Lin),
                    kf(0.35, 0.0, 1.0, 0.2, Lin),
                    kf(0.4, 0.0, 1.0, 0.2, Lin),
                    kf(0.5, 0.0, 2.1, 1.2, Lin),
                    kf(0.6, 0.0, 1.1, 0.2, Lin),
                    kf(0.7, 0.0, 1.1, 0.2, Lin),
                    kf(1.75, 0.0, 1.1, 0.2, Lin),
                    kf(1.8, 0.0, 0.1, 0.2, Lin),
                    kf(1.95, 0.0, 0.1, 5.2, Lin),
                    kf(2.0, 0.0, 0.1, 7.2, Lin),
                    kf(2.15, 0.0, 0.1, 8.2, Lin),
                    kf(2.3, 0.0, 0.1, 5.2, Lin),
                ],
            },
            KfChannel {
                part: "right_hind_leg",
                target: KfTarget::Pos,
                frames: &[kf(0.0, 0.0, 3.0, -2.0, Lin)],
            },
            KfChannel {
                part: "left_hind_leg",
                target: KfTarget::Pos,
                frames: &[kf(0.0, 0.0, 3.0, -2.0, Lin)],
            },
            KfChannel {
                part: "right_front_leg",
                target: KfTarget::Rot,
                frames: &[
                    kf(0.0, -90.0, 0.0, 0.0, Lin),
                    kf(0.5833, -90.0, 0.0, 0.0, Cat),
                    kf(0.8333, -45.0, 0.0, 0.0, Cat),
                    kf(1.0, -45.0, 0.0, 0.0, Cat),
                    kf(1.75, -45.0, 0.0, 0.0, Cat),
                    kf(1.8, -90.0, 0.0, 0.0, Cat),
                    kf(1.95, -90.0, 0.0, 0.0, Cat),
                ],
            },
            KfChannel {
                part: "right_front_leg",
                target: KfTarget::Pos,
                frames: &[
                    kf(0.0, -1.0, 2.0, 2.0, Lin),
                    kf(0.5833, -1.0, 2.0, 2.0, Cat),
                    kf(0.6667, -1.0, 2.0, -2.0, Cat),
                    kf(0.8333, -1.0, 2.0, -1.0, Cat),
                    kf(1.0, -1.0, 2.0, -1.0, Cat),
                    kf(1.75, -1.0, 2.0, -1.0, Cat),
                    kf(1.95, -1.0, 2.0, -1.0, Cat),
                    kf(2.0, -1.0, 2.0, 3.0, Cat),
                    kf(2.15, -1.0, 3.0, 4.0, Cat),
                ],
            },
            KfChannel {
                part: "left_front_leg",
                target: KfTarget::Rot,
                frames: &[
                    kf(0.0, -90.0, 0.0, 0.0, Lin),
                    kf(0.6667, -90.0, 0.0, 0.0, Cat),
                    kf(0.8333, -45.0, 0.0, 0.0, Cat),
                    kf(1.0, -45.0, 0.0, 0.0, Cat),
                    kf(1.75, -45.0, 0.0, 0.0, Cat),
                    kf(1.8, -90.0, 0.0, 0.0, Cat),
                    kf(1.95, -90.0, 0.0, 0.0, Cat),
                ],
            },
            KfChannel {
                part: "left_front_leg",
                target: KfTarget::Pos,
                frames: &[
                    kf(0.0, 1.0, 2.0, 2.0, Lin),
                    kf(0.6667, 1.0, 2.0, 2.0, Cat),
                    kf(0.75, 1.0, 2.0, -2.0, Cat),
                    kf(0.8333, 1.0, 2.0, -1.0, Cat),
                    kf(1.0, 1.0, 2.0, -1.0, Cat),
                    kf(1.75, 1.0, 2.0, -1.0, Cat),
                    kf(1.95, 1.0, 2.0, -1.0, Cat),
                    kf(2.0, 1.0, 2.0, 3.0, Cat),
                    kf(2.15, 1.0, 3.0, 4.0, Cat),
                ],
            },
            KfChannel {
                part: "cube",
                target: KfTarget::Rot,
                frames: &[
                    kf(0.0, 0.0, 0.0, 0.0, Lin),
                    kf(0.25, 0.0, 0.0, 0.0, Lin),
                    kf(0.35, 15.0, 0.0, 0.0, Lin),
                    kf(0.5, -7.5, 0.0, 0.0, Lin),
                    kf(0.6, 0.0, 0.0, 0.0, Lin),
                    kf(2.0, 0.0, 0.0, 0.0, Lin),
                    kf(2.05, -17.5, 0.0, 0.0, Lin),
                    kf(2.15, -25.0, 0.0, 0.0, Lin),
                    kf(2.25, 0.0, 0.0, 0.0, Lin),
                    kf(2.3, 12.5, 0.0, 0.0, Lin),
                    kf(2.5, 0.0, 0.0, 0.0, Lin),
                ],
            },
            KfChannel {
                part: "cube",
                target: KfTarget::Pos,
                frames: &[
                    kf(0.25, 0.0, 0.0, 0.0, Lin),
                    kf(0.35, 0.0, 1.6, 0.0, Lin),
                    kf(0.5, 0.0, 0.5, 0.0, Lin),
                    kf(0.6, 0.0, 0.0, 0.0, Lin),
                    kf(2.0, 0.0, 0.0, 0.0, Lin),
                    kf(2.05, 0.0, 1.2, 0.0, Lin),
                    kf(2.15, 0.0, 1.7, 0.0, Lin),
                    kf(2.25, 0.0, 0.0, 0.0, Lin),
                    kf(2.3, 0.0, 1.3, 0.0, Lin),
                    kf(2.5, 0.0, 0.0, 0.0, Lin),
                ],
            },
        ],
    }
};

static ARMADILLO_ROLL_OUT: KfAnim = {
    use Kfi::{Cat, Lin};
    KfAnim {
        length: 1.5,
        looping: false,
        channels: &[
            KfChannel {
                part: "head",
                target: KfTarget::Rot,
                frames: &[
                    kf(0.1, -50.0, 0.0, 0.0, Lin),
                    kf(0.15, 0.0, 0.0, 0.0, Lin),
                    kf(0.25, 0.0, 0.0, 0.0, Lin),
                    kf(0.4, 0.0, 0.0, 0.0, Lin),
                    kf(0.65, 0.0, 0.0, 0.0, Lin),
                    kf(0.7, 0.0, 0.0, 0.0, Lin),
                    kf(0.85, -2.5, 0.0, 0.0, Lin),
                    kf(0.9, 0.0, 0.0, 0.0, Lin),
                    kf(0.95, -7.5, 0.0, 0.0, Lin),
                    kf(1.05, 0.0, 0.0, 0.0, Lin),
                    kf(1.1, 7.5, 0.0, 0.0, Lin),
                ],
            },
            KfChannel {
                part: "head",
                target: KfTarget::Pos,
                frames: &[
                    kf(0.0, 0.0, 1.0, 5.0, Lin),
                    kf(0.05, 0.0, 1.0, 0.2, Lin),
                    kf(0.1, 0.0, 1.0, 0.2, Lin),
                    kf(0.15, 0.0, 2.1, 1.2, Lin),
                    kf(0.25, 0.0, 1.03, 0.13, Lin),
                    kf(0.4, 0.0, 1.03, 0.13, Lin),
                    kf(0.65, 0.0, 1.03, 0.13, Lin),
                    kf(0.7, 0.0, 1.1, 0.2, Lin),
                    kf(0.75, 0.0, 4.1, 2.2, Lin),
                    kf(0.85, 0.0, 5.1, 3.2, Lin),
                    kf(0.9, 0.0, 0.1, 0.2, Lin),
                    kf(0.95, 0.0, 0.9, -0.8, Lin),
                    kf(1.05, 0.0, 0.9, 0.0, Lin),
                    kf(1.1, 0.0, 2.6, 0.2, Lin),
                    kf(1.15, 0.0, 2.4, 0.2, Lin),
                    kf(1.2, 0.0, 0.0, 0.2, Lin),
                    kf(1.25, 0.0, 0.0, 0.2, Lin),
                    kf(1.3, 0.0, 0.0, 0.2, Lin),
                ],
            },
            KfChannel {
                part: "right_hind_leg",
                target: KfTarget::Rot,
                frames: &[
                    kf(1.1, 0.0, 0.0, 0.0, Lin),
                    kf(1.3, 0.0, 0.0, 30.0, Lin),
                    kf(1.4, 0.0, 0.0, 0.0, Lin),
                    kf(1.45, 0.0, 0.0, 0.0, Lin),
                    kf(1.5, 0.0, 0.0, 0.0, Lin),
                ],
            },
            KfChannel {
                part: "right_hind_leg",
                target: KfTarget::Pos,
                frames: &[
                    kf(1.1, 0.0, 3.0, -2.0, Lin),
                    kf(1.2, 0.0, 8.0, -2.0, Lin),
                    kf(1.3, -1.0, 3.0, 0.0, Lin),
                    kf(1.4, 0.0, 0.0, 0.0, Lin),
                    kf(1.45, 0.0, 0.0, 0.0, Lin),
                    kf(1.5, 0.0, 0.0, 0.0, Lin),
                ],
            },
            KfChannel {
                part: "left_hind_leg",
                target: KfTarget::Rot,
                frames: &[
                    kf(1.1, 0.0, 0.0, 0.0, Lin),
                    kf(1.3, 0.0, 0.0, -30.0, Lin),
                    kf(1.4, 0.0, 0.0, 0.0, Lin),
                    kf(1.45, 0.0, 0.0, 0.0, Lin),
                    kf(1.5, 0.0, 0.0, 0.0, Lin),
                ],
            },
            KfChannel {
                part: "left_hind_leg",
                target: KfTarget::Pos,
                frames: &[
                    kf(1.1, 0.0, 3.0, -2.0, Lin),
                    kf(1.2, 0.0, 8.0, -2.0, Lin),
                    kf(1.3, 1.0, 3.0, 0.0, Lin),
                    kf(1.35, 1.0, 3.0, 0.0, Lin),
                    kf(1.4, 0.0, 0.0, 0.0, Lin),
                    kf(1.45, 0.0, 0.0, 0.0, Lin),
                    kf(1.5, 0.0, 0.0, 0.0, Lin),
                ],
            },
            KfChannel {
                part: "right_front_leg",
                target: KfTarget::Rot,
                frames: &[
                    kf(0.0, -90.0, 0.0, 0.0, Cat),
                    kf(0.05, -90.0, 0.0, 0.0, Cat),
                    kf(0.25, -45.0, 0.0, 0.0, Lin),
                    kf(0.55, -45.0, 0.0, 0.0, Lin),
                    kf(0.6, -92.5, 0.0, 0.0, Cat),
                    kf(1.1, -90.0, 0.0, 0.0, Cat),
                    kf(1.3, 0.0, 0.0, 30.0, Cat),
                    kf(1.4, 0.0, 0.0, 0.0, Cat),
                    kf(1.45, 0.0, 0.0, 0.0, Lin),
                    kf(1.5, 0.0, 0.0, 0.0, Cat),
                ],
            },
            KfChannel {
                part: "right_front_leg",
                target: KfTarget::Pos,
                frames: &[
                    kf(0.0, -1.0, 2.0, 2.0, Lin),
                    kf(0.05, -1.0, 2.0, 2.0, Cat),
                    kf(0.25, -1.0, 2.0, -1.0, Lin),
                    kf(0.55, -1.0, 2.0, -1.0, Lin),
                    kf(0.7, -1.0, 2.0, 2.63, Cat),
                    kf(1.1, -1.0, 2.0, 2.0, Lin),
                    kf(1.2, -1.0, 7.0, 2.0, Lin),
                    kf(1.3, -1.0, 3.0, 0.0, Lin),
                    kf(1.4, 0.0, 0.0, 0.0, Lin),
                    kf(1.45, 0.0, 0.0, 0.0, Lin),
                    kf(1.5, 0.0, 0.0, 0.0, Lin),
                ],
            },
            KfChannel {
                part: "left_front_leg",
                target: KfTarget::Rot,
                frames: &[
                    kf(0.0, -90.0, 0.0, 0.0, Cat),
                    kf(0.05, -90.0, 0.0, 0.0, Cat),
                    kf(0.25, -45.0, 0.0, 0.0, Lin),
                    kf(0.55, -45.0, 0.0, 0.0, Lin),
                    kf(0.6, -87.5, 0.0, 0.0, Cat),
                    kf(1.1, -90.0, 0.0, 0.0, Cat),
                    kf(1.3, 0.0, 0.0, -30.0, Cat),
                    kf(1.4, 0.0, 0.0, 0.0, Cat),
                    kf(1.45, 0.0, 0.0, 0.0, Lin),
                    kf(1.5, 0.0, 0.0, 0.0, Cat),
                ],
            },
            KfChannel {
                part: "left_front_leg",
                target: KfTarget::Pos,
                frames: &[
                    kf(0.0, 1.0, 2.0, 2.0, Cat),
                    kf(0.05, 1.0, 2.0, 2.0, Cat),
                    kf(0.15, 1.0, 2.0, 0.0, Cat),
                    kf(0.25, 1.0, 2.0, -1.0, Lin),
                    kf(0.55, 1.0, 2.0, -1.0, Lin),
                    kf(0.7, 1.0, 2.0, 1.88, Cat),
                    kf(0.75, 1.0, 2.0, 2.67, Cat),
                    kf(1.1, 1.0, 2.0, 2.0, Cat),
                    kf(1.2, 1.0, 8.0, 2.0, Cat),
                    kf(1.25, 1.06, 5.06, 1.0, Cat),
                    kf(1.3, 1.0, 3.0, 0.0, Cat),
                    kf(1.4, 0.0, 0.0, 0.0, Cat),
                    kf(1.45, 0.0, 0.0, 0.0, Lin),
                    kf(1.5, 0.0, 0.0, 0.0, Cat),
                ],
            },
            KfChannel {
                part: "cube",
                target: KfTarget::Rot,
                frames: &[
                    kf(0.0, 0.0, 0.0, 0.0, Lin),
                    kf(0.05, 15.0, 0.0, 0.0, Lin),
                    kf(0.15, -7.5, 0.0, 0.0, Lin),
                    kf(0.25, 0.0, 0.0, 0.0, Lin),
                    kf(0.7, 0.0, 0.0, 0.0, Lin),
                    kf(0.75, -17.5, 0.0, 0.0, Lin),
                    kf(0.85, -25.0, 0.0, 0.0, Lin),
                    kf(0.9, 0.0, 0.0, 0.0, Lin),
                    kf(0.95, 12.5, 0.0, 0.0, Lin),
                    kf(1.05, 0.0, 0.0, 0.0, Lin),
                    kf(1.1, 0.0, 0.0, 0.0, Lin),
                ],
            },
            KfChannel {
                part: "cube",
                target: KfTarget::Pos,
                frames: &[
                    kf(0.0, 0.0, 0.0, 0.0, Lin),
                    kf(0.05, 0.0, 1.6, 0.0, Lin),
                    kf(0.15, 0.0, 0.5, 0.0, Lin),
                    kf(0.25, 0.0, 0.0, 0.0, Lin),
                    kf(0.7, 0.0, 0.0, 0.0, Lin),
                    kf(0.75, 0.0, 1.2, 0.0, Lin),
                    kf(0.85, 0.0, 1.7, 0.0, Lin),
                    kf(0.9, 0.0, 0.0, 0.0, Lin),
                    kf(0.95, 0.0, 1.3, 0.0, Lin),
                    kf(1.05, 0.0, 0.0, 0.0, Lin),
                    kf(1.2, 0.0, 5.0, 0.0, Lin),
                    kf(1.25, 0.0, 8.0, 0.0, Lin),
                    kf(1.5, 0.0, 1.0, 0.0, Lin),
                ],
            },
            KfChannel {
                part: "body",
                target: KfTarget::Pos,
                frames: &[
                    kf(1.1, 0.0, 0.0, 0.0, Lin),
                    kf(1.2, 0.0, 4.0, 0.0, Lin),
                    kf(1.25, 0.0, 5.0, 0.0, Lin),
                    kf(1.3, 0.0, 4.0, 0.0, Lin),
                    kf(1.4, 0.0, -1.0, 0.0, Lin),
                    kf(1.5, 0.0, 0.0, 0.0, Lin),
                ],
            },
        ],
    }
};

static ARMADILLO_BABY_ROLL_UP: KfAnim = {
    use Kfi::Lin;
    KfAnim {
        length: 0.5,
        looping: false,
        channels: &[
            KfChannel {
                part: "body",
                target: KfTarget::Pos,
                frames: &[
                    kf(0.0, 0.0, 0.0, 0.0, Lin),
                    kf(0.15, 0.0, 5.0, 0.0, Lin),
                    kf(0.2, 0.0, 6.0, -1.0, Lin),
                    kf(0.25, 0.0, 6.0, -1.0, Lin),
                    kf(0.4, 0.0, -1.0, -1.0, Lin),
                ],
            },
            KfChannel {
                part: "tail",
                target: KfTarget::Pos,
                frames: &[
                    kf(0.0, 0.0, 0.0, 0.0, Lin),
                    kf(0.15, 0.0, 0.0, 0.0, Lin),
                    kf(0.2, 0.0, 1.0, -3.0, Lin),
                    kf(0.25, 0.0, 1.0, -3.0, Lin),
                    kf(0.4, 0.0, 2.0, -3.0, Lin),
                ],
            },
            KfChannel {
                part: "head",
                target: KfTarget::Rot,
                frames: &[
                    kf(0.0, 0.0, 0.0, 0.0, Lin),
                    kf(0.15, 17.5, 0.0, 0.0, Lin),
                    kf(0.25, -72.5, 0.0, 0.0, Lin),
                ],
            },
            KfChannel {
                part: "head",
                target: KfTarget::Pos,
                frames: &[
                    kf(0.0, 0.0, 0.0, 0.0, Lin),
                    kf(0.15, 0.0, -1.0, 0.0, Lin),
                    kf(0.2, 0.0, 0.0, 1.0, Lin),
                    kf(0.25, 0.0, 0.0, 1.0, Lin),
                    kf(0.3, 0.0, 0.0, 5.0, Lin),
                    kf(0.4, 0.0, 0.0, 5.0, Lin),
                    kf(0.5, 0.0, -0.25, 5.0, Lin),
                ],
            },
            KfChannel {
                part: "right_hind_leg",
                target: KfTarget::Rot,
                frames: &[kf(0.0, 0.0, 0.0, 0.0, Lin), kf(0.15, -35.0, 0.0, 0.0, Lin)],
            },
            KfChannel {
                part: "right_hind_leg",
                target: KfTarget::Pos,
                frames: &[
                    kf(0.0, 0.0, 0.0, 0.0, Lin),
                    kf(0.15, 0.0, 5.0, -2.0, Lin),
                    kf(0.2, 0.0, 8.0, -2.0, Lin),
                    kf(0.25, 0.0, 8.0, -2.0, Lin),
                    kf(0.4, 1.0, 3.0, -2.0, Lin),
                ],
            },
            KfChannel {
                part: "left_hind_leg",
                target: KfTarget::Rot,
                frames: &[kf(0.0, 0.0, 0.0, 0.0, Lin), kf(0.15, -45.0, 0.0, 0.0, Lin)],
            },
            KfChannel {
                part: "left_hind_leg",
                target: KfTarget::Pos,
                frames: &[
                    kf(0.0, 0.0, 0.0, 0.0, Lin),
                    kf(0.15, 0.0, 5.0, -2.0, Lin),
                    kf(0.2, 0.0, 8.0, -2.0, Lin),
                    kf(0.25, 0.0, 8.0, -2.0, Lin),
                    kf(0.4, -1.0, 3.0, -3.0, Lin),
                ],
            },
            KfChannel {
                part: "right_front_leg",
                target: KfTarget::Rot,
                frames: &[
                    kf(0.0, 0.0, 0.0, 0.0, Lin),
                    kf(0.15, -27.5, 0.0, 0.0, Lin),
                    kf(0.2, -32.5, 0.0, 0.0, Lin),
                    kf(0.25, -85.0, 0.0, 0.0, Lin),
                ],
            },
            KfChannel {
                part: "right_front_leg",
                target: KfTarget::Pos,
                frames: &[
                    kf(0.0, 0.0, 0.0, 0.0, Lin),
                    kf(0.15, 0.0, 5.0, 0.0, Lin),
                    kf(0.2, -1.0, 8.0, -1.0, Lin),
                    kf(0.25, -1.0, 9.0, -1.0, Lin),
                    kf(0.4, -1.0, 2.0, 3.0, Lin),
                ],
            },
            KfChannel {
                part: "left_front_leg",
                target: KfTarget::Rot,
                frames: &[
                    kf(0.0, 0.0, 0.0, 0.0, Lin),
                    kf(0.15, -12.5, 0.0, 0.0, Lin),
                    kf(0.2, -35.0, 0.0, 0.0, Lin),
                    kf(0.25, -85.0, 0.0, 0.0, Lin),
                ],
            },
            KfChannel {
                part: "left_front_leg",
                target: KfTarget::Pos,
                frames: &[
                    kf(0.0, 0.0, 0.0, 0.0, Lin),
                    kf(0.15, 0.0, 5.0, 0.0, Lin),
                    kf(0.2, 1.0, 8.0, -1.0, Lin),
                    kf(0.25, 1.0, 9.0, -1.0, Lin),
                    kf(0.4, 1.0, 2.0, 3.0, Lin),
                ],
            },
            KfChannel {
                part: "cube",
                target: KfTarget::Rot,
                frames: &[
                    kf(0.0, 0.0, 0.0, 0.0, Lin),
                    kf(0.15, 0.0, 0.0, 0.0, Lin),
                    kf(0.4, -2.5, 0.0, 0.0, Lin),
                    kf(0.45, 5.0, 0.0, 0.0, Lin),
                    kf(0.5, 0.0, 0.0, 0.0, Lin),
                ],
            },
            KfChannel {
                part: "cube",
                target: KfTarget::Pos,
                frames: &[
                    kf(0.0, 0.0, 3.0, 0.0, Lin),
                    kf(0.15, 0.0, 8.0, 0.0, Lin),
                    kf(0.2, 0.0, 7.0, 0.0, Lin),
                    kf(0.25, 0.0, 7.0, 0.0, Lin),
                    kf(0.4, 0.0, 1.0, 0.0, Lin),
                    kf(0.45, 0.0, 0.6, 0.0, Lin),
                    kf(0.5, 0.0, 0.0, 0.0, Lin),
                ],
            },
        ],
    }
};

static ARMADILLO_BABY_WALK: KfAnim = {
    use Kfi::{Cat, Lin};
    KfAnim {
        length: 1.4583,
        looping: true,
        channels: &[
            KfChannel {
                part: "body",
                target: KfTarget::Rot,
                frames: &[
                    kf(0.0, 0.0, 0.0, 0.0, Cat),
                    kf(0.25, 0.0, 0.0, 4.6, Cat),
                    kf(0.3, 0.0, 0.0, 6.81, Cat),
                    kf(0.5, 0.0, 0.0, 0.0, Cat),
                    kf(0.7, 0.0, 0.0, 0.0, Cat),
                    kf(0.95, 0.0, 0.0, -4.6, Cat),
                    kf(1.0, 0.0, 0.0, -6.89, Cat),
                    kf(1.25, 0.0, 0.0, 0.0, Cat),
                    kf(1.45, 0.0, 0.0, 0.0, Cat),
                ],
            },
            KfChannel {
                part: "body",
                target: KfTarget::Pos,
                frames: &[
                    kf(0.0, 0.0, 0.0, 0.0, Cat),
                    kf(0.25, 0.0, -0.2, 0.0, Cat),
                    kf(0.5, 0.0, 0.0, 0.0, Cat),
                    kf(0.7, 0.0, 0.0, 0.0, Cat),
                    kf(0.95, 0.0, -0.2, 0.0, Cat),
                    kf(1.25, 0.0, 0.0, 0.0, Cat),
                    kf(1.45, 0.0, 0.0, 0.0, Cat),
                ],
            },
            KfChannel {
                part: "tail",
                target: KfTarget::Rot,
                frames: &[
                    kf(0.0, 0.0, 0.0, 0.0, Lin),
                    kf(0.5, -9.17, 0.0, 0.0, Lin),
                    kf(0.75, 5.0, 0.0, 0.0, Lin),
                    kf(1.2, -8.24, 0.0, 0.0, Lin),
                    kf(1.45, 0.0, 0.0, 0.0, Lin),
                ],
            },
            KfChannel {
                part: "right_hind_leg",
                target: KfTarget::Rot,
                frames: &[
                    kf(0.0, -50.0, 0.0, 0.0, Lin),
                    kf(0.25, 0.0, 0.0, 0.0, Lin),
                    kf(0.5, 0.0, 0.0, 0.0, Lin),
                    kf(0.75, 50.0, 0.0, 0.0, Lin),
                    kf(1.0, 50.0, 0.0, 0.0, Lin),
                    kf(1.3, -20.0, 0.0, 0.0, Lin),
                    kf(1.45, -50.0, 0.0, 0.0, Lin),
                ],
            },
            KfChannel {
                part: "right_hind_leg",
                target: KfTarget::Pos,
                frames: &[
                    kf(0.0, 0.0, 0.0, 0.0, Lin),
                    kf(0.25, 0.0, 0.0, -0.5, Lin),
                    kf(0.5, 0.0, 0.0, 0.0, Lin),
                    kf(1.0, 0.0, 0.0, -0.5, Lin),
                    kf(1.3, 0.0, 1.0, -0.18, Lin),
                    kf(1.45, 0.0, 0.0, 0.0, Lin),
                ],
            },
            KfChannel {
                part: "left_hind_leg",
                target: KfTarget::Rot,
                frames: &[
                    kf(0.0, 50.0, 0.0, 0.0, Lin),
                    kf(0.25, 50.0, 0.0, 0.0, Lin),
                    kf(0.55, -20.0, 0.0, 0.0, Lin),
                    kf(0.7, -50.0, 0.0, 0.0, Lin),
                    kf(0.95, 0.0, 0.0, 0.0, Lin),
                    kf(1.2, 0.0, 0.0, 0.0, Lin),
                    kf(1.45, 50.0, 0.0, 0.0, Lin),
                ],
            },
            KfChannel {
                part: "left_hind_leg",
                target: KfTarget::Pos,
                frames: &[
                    kf(0.0, 0.0, 0.0, -0.25, Lin),
                    kf(0.25, 0.0, 0.0, -0.5, Lin),
                    kf(0.55, 0.0, 1.0, -0.18, Lin),
                    kf(0.7, 0.0, 0.0, 0.0, Lin),
                    kf(0.95, 0.0, 0.0, -0.5, Lin),
                    kf(1.2, 0.0, 0.0, 0.0, Lin),
                    kf(1.45, 0.0, 0.0, -0.25, Lin),
                ],
            },
            KfChannel {
                part: "right_front_leg",
                target: KfTarget::Rot,
                frames: &[
                    kf(0.0, 50.0, 0.0, 0.0, Lin),
                    kf(0.3, 50.0, 0.0, 0.0, Lin),
                    kf(0.55, -20.0, 0.0, 0.0, Lin),
                    kf(0.7, -50.0, 0.0, 0.0, Lin),
                    kf(0.95, 0.0, 0.0, 0.0, Lin),
                    kf(1.2, 0.0, 0.0, 0.0, Lin),
                    kf(1.45, 50.0, 0.0, 0.0, Lin),
                ],
            },
            KfChannel {
                part: "right_front_leg",
                target: KfTarget::Pos,
                frames: &[
                    kf(0.0, 0.0, 0.0, -0.25, Lin),
                    kf(0.25, 0.0, 0.0, -0.5, Lin),
                    kf(0.55, 0.0, 1.0, -0.18, Lin),
                    kf(0.7, 0.0, 0.0, 0.0, Lin),
                    kf(0.95, 0.0, 0.0, -0.5, Lin),
                    kf(1.2, 0.0, 0.0, 0.0, Lin),
                    kf(1.45, 0.0, 0.0, -0.25, Lin),
                ],
            },
            KfChannel {
                part: "left_front_leg",
                target: KfTarget::Rot,
                frames: &[
                    kf(0.0, -50.0, 0.0, 0.0, Lin),
                    kf(0.25, 0.0, 0.0, 0.0, Lin),
                    kf(0.5, 0.0, 0.0, 0.0, Lin),
                    kf(0.75, 50.0, 0.0, 0.0, Lin),
                    kf(1.0, 50.0, 0.0, 0.0, Lin),
                    kf(1.3, -20.0, 0.0, 0.0, Lin),
                    kf(1.45, -50.0, 0.0, 0.0, Lin),
                ],
            },
            KfChannel {
                part: "left_front_leg",
                target: KfTarget::Pos,
                frames: &[
                    kf(0.0, 0.0, 0.0, 0.0, Lin),
                    kf(0.25, 0.0, 0.0, -0.5, Lin),
                    kf(0.5, 0.0, 0.0, 0.0, Lin),
                    kf(1.0, 0.0, 0.0, -0.5, Lin),
                    kf(1.3, 0.0, 1.0, -0.18, Lin),
                    kf(1.45, 0.0, 0.0, 0.0, Lin),
                ],
            },
            KfChannel {
                part: "head",
                target: KfTarget::Rot,
                frames: &[
                    kf(0.0, 0.0, 0.0, 0.0, Lin),
                    kf(0.25, 0.0, 0.0, -2.5, Lin),
                    kf(0.5, 0.0, 0.0, 0.0, Lin),
                    kf(0.7, 0.0, 0.0, 0.0, Lin),
                    kf(1.0, 0.0, 0.0, 2.5, Lin),
                    kf(1.25, 0.0, 0.0, 0.0, Lin),
                    kf(1.45, 0.0, 0.0, 0.0, Lin),
                ],
            },
        ],
    }
};

static ARMADILLO_BABY_ROLL_OUT: KfAnim = {
    use Kfi::{Cat, Lin};
    KfAnim {
        length: 1.5,
        looping: false,
        channels: &[
            KfChannel {
                part: "head",
                target: KfTarget::Rot,
                frames: &[
                    kf(0.1, -50.0, 0.0, 0.0, Lin),
                    kf(0.15, 0.0, 0.0, 0.0, Lin),
                    kf(0.25, 0.0, 0.0, 0.0, Lin),
                    kf(0.4, 0.0, 0.0, 0.0, Lin),
                    kf(0.65, 0.0, 0.0, 0.0, Lin),
                    kf(0.7, 0.0, 0.0, 0.0, Lin),
                    kf(0.85, -2.5, 0.0, 0.0, Lin),
                    kf(0.9, 0.0, 0.0, 0.0, Lin),
                    kf(0.95, -7.5, 0.0, 0.0, Lin),
                    kf(1.05, 0.0, 0.0, 0.0, Lin),
                    kf(1.1, 7.5, 0.0, 0.0, Lin),
                ],
            },
            KfChannel {
                part: "head",
                target: KfTarget::Pos,
                frames: &[
                    kf(0.0, 0.0, -1.9, 5.0, Lin),
                    kf(0.05, 0.0, -1.0, 0.2, Lin),
                    kf(0.1, 0.0, 0.0, 0.2, Lin),
                    kf(0.15, 0.0, 2.1, 1.2, Lin),
                    kf(0.25, 0.0, 0.03, 0.13, Lin),
                    kf(0.4, 0.0, 0.03, 0.13, Lin),
                    kf(0.65, 0.0, 0.03, 0.13, Lin),
                    kf(0.7, 0.0, 0.1, 0.2, Lin),
                    kf(0.75, 0.0, 3.1, 2.2, Lin),
                    kf(0.85, 0.0, 4.1, 2.2, Lin),
                    kf(0.9, 0.0, 0.1, 0.2, Lin),
                    kf(0.95, 0.0, 0.9, -0.8, Lin),
                    kf(1.05, 0.0, 0.9, 0.0, Lin),
                    kf(1.1, 0.0, 2.6, 0.2, Lin),
                    kf(1.15, 0.0, 2.4, 0.2, Lin),
                    kf(1.2, 0.0, 0.0, 0.2, Lin),
                    kf(1.25, 0.0, 0.0, 0.2, Lin),
                    kf(1.3, 0.0, 0.0, 0.2, Lin),
                ],
            },
            KfChannel {
                part: "right_hind_leg",
                target: KfTarget::Rot,
                frames: &[
                    kf(1.1, 0.0, 0.0, 0.0, Lin),
                    kf(1.3, 0.0, 0.0, 30.0, Lin),
                    kf(1.4, 0.0, 0.0, 0.0, Lin),
                    kf(1.45, 0.0, 0.0, 0.0, Lin),
                    kf(1.5, 0.0, 0.0, 0.0, Lin),
                ],
            },
            KfChannel {
                part: "right_hind_leg",
                target: KfTarget::Pos,
                frames: &[
                    kf(1.1, 0.0, 3.0, -2.0, Lin),
                    kf(1.2, 0.0, 8.0, -2.0, Lin),
                    kf(1.3, -1.0, 3.0, 0.0, Lin),
                    kf(1.4, 0.0, 0.0, 0.0, Lin),
                    kf(1.45, 0.0, 0.0, 0.0, Lin),
                    kf(1.5, 0.0, 0.0, 0.0, Lin),
                ],
            },
            KfChannel {
                part: "left_hind_leg",
                target: KfTarget::Rot,
                frames: &[
                    kf(1.1, 0.0, 0.0, 0.0, Lin),
                    kf(1.3, 0.0, 0.0, -30.0, Lin),
                    kf(1.4, 0.0, 0.0, 0.0, Lin),
                    kf(1.45, 0.0, 0.0, 0.0, Lin),
                    kf(1.5, 0.0, 0.0, 0.0, Lin),
                ],
            },
            KfChannel {
                part: "left_hind_leg",
                target: KfTarget::Pos,
                frames: &[
                    kf(1.1, 0.0, 3.0, -2.0, Lin),
                    kf(1.2, 0.0, 8.0, -2.0, Lin),
                    kf(1.3, 1.0, 3.0, 0.0, Lin),
                    kf(1.35, 1.0, 3.0, 0.0, Lin),
                    kf(1.4, 0.0, 0.0, 0.0, Lin),
                    kf(1.45, 0.0, 0.0, 0.0, Lin),
                    kf(1.5, 0.0, 0.0, 0.0, Lin),
                ],
            },
            KfChannel {
                part: "right_front_leg",
                target: KfTarget::Rot,
                frames: &[
                    kf(0.0, -90.0, 0.0, 0.0, Cat),
                    kf(0.05, -90.0, 0.0, 0.0, Cat),
                    kf(0.25, -45.0, 0.0, 0.0, Lin),
                    kf(0.55, -45.0, 0.0, 0.0, Lin),
                    kf(0.6, -92.5, 0.0, 0.0, Cat),
                    kf(1.1, -90.0, 0.0, 0.0, Cat),
                    kf(1.3, 0.0, 0.0, 30.0, Cat),
                    kf(1.4, 0.0, 0.0, 0.0, Cat),
                    kf(1.45, 0.0, 0.0, 0.0, Lin),
                    kf(1.5, 0.0, 0.0, 0.0, Cat),
                ],
            },
            KfChannel {
                part: "right_front_leg",
                target: KfTarget::Pos,
                frames: &[
                    kf(0.0, -1.0, 2.0, 2.0, Lin),
                    kf(0.05, -1.0, 2.0, 2.0, Cat),
                    kf(0.25, 0.0, 2.0, -1.0, Lin),
                    kf(0.55, 0.0, 2.0, -1.0, Lin),
                    kf(0.7, -1.0, 2.0, 2.63, Cat),
                    kf(1.1, -1.0, 2.0, 2.0, Lin),
                    kf(1.2, -1.0, 7.0, 2.0, Lin),
                    kf(1.3, -1.0, 3.0, 0.0, Lin),
                    kf(1.4, 0.0, 0.0, 0.0, Lin),
                    kf(1.45, 0.0, 0.0, 0.0, Lin),
                    kf(1.5, 0.0, 0.0, 0.0, Lin),
                ],
            },
            KfChannel {
                part: "left_front_leg",
                target: KfTarget::Rot,
                frames: &[
                    kf(0.0, -90.0, 0.0, 0.0, Cat),
                    kf(0.05, -90.0, 0.0, 0.0, Cat),
                    kf(0.25, -45.0, 0.0, 0.0, Lin),
                    kf(0.55, -45.0, 0.0, 0.0, Lin),
                    kf(0.6, -87.5, 0.0, 0.0, Cat),
                    kf(1.1, -90.0, 0.0, 0.0, Cat),
                    kf(1.3, 0.0, 0.0, -30.0, Cat),
                    kf(1.4, 0.0, 0.0, 0.0, Cat),
                    kf(1.45, 0.0, 0.0, 0.0, Lin),
                    kf(1.5, 0.0, 0.0, 0.0, Cat),
                ],
            },
            KfChannel {
                part: "left_front_leg",
                target: KfTarget::Pos,
                frames: &[
                    kf(0.0, 1.0, 2.0, 2.0, Cat),
                    kf(0.05, 1.0, 2.0, 2.0, Cat),
                    kf(0.15, 1.0, 2.0, 0.0, Cat),
                    kf(0.25, 0.0, 2.0, -1.0, Lin),
                    kf(0.55, 0.0, 2.0, -1.0, Lin),
                    kf(0.7, 1.0, 2.0, 1.88, Cat),
                    kf(0.75, 1.0, 2.0, 2.67, Cat),
                    kf(1.1, 1.0, 2.0, 2.0, Cat),
                    kf(1.2, 1.0, 8.0, 2.0, Cat),
                    kf(1.25, 1.06, 5.06, 1.0, Cat),
                    kf(1.3, 1.0, 3.0, 0.0, Cat),
                    kf(1.4, 0.0, 0.0, 0.0, Cat),
                    kf(1.45, 0.0, 0.0, 0.0, Lin),
                    kf(1.5, 0.0, 0.0, 0.0, Cat),
                ],
            },
            KfChannel {
                part: "body",
                target: KfTarget::Pos,
                frames: &[
                    kf(1.1, 0.0, 0.0, 0.0, Lin),
                    kf(1.2, 0.0, 4.0, 0.0, Lin),
                    kf(1.25, 0.0, 5.0, 0.0, Lin),
                    kf(1.3, 0.0, 4.0, 0.0, Lin),
                    kf(1.4, 0.0, -1.0, 0.0, Lin),
                    kf(1.5, 0.0, 0.0, 0.0, Lin),
                ],
            },
            KfChannel {
                part: "left_ear",
                target: KfTarget::Pos,
                frames: &[
                    kf(0.0, 0.0, 0.0, 0.0, Lin),
                    kf(0.15, 0.0, -1.4129, 0.0617, Lin),
                    kf(0.25, 0.0, 0.0, 0.0, Lin),
                ],
            },
            KfChannel {
                part: "right_ear",
                target: KfTarget::Pos,
                frames: &[
                    kf(0.0, 0.0, 0.0, 0.0, Lin),
                    kf(0.15, 0.0, -1.4129, 0.0617, Lin),
                    kf(0.25, 0.0, 0.0, 0.0, Lin),
                ],
            },
            KfChannel {
                part: "cube",
                target: KfTarget::Rot,
                frames: &[
                    kf(0.0, 0.0, 0.0, 0.0, Lin),
                    kf(0.05, 15.0, 0.0, 0.0, Lin),
                    kf(0.15, -7.5, 0.0, 0.0, Lin),
                    kf(0.25, 0.0, 0.0, 0.0, Lin),
                    kf(0.7, 0.0, 0.0, 0.0, Lin),
                    kf(0.75, -17.5, 0.0, 0.0, Lin),
                    kf(0.85, -25.0, 0.0, 0.0, Lin),
                    kf(0.9, 0.0, 0.0, 0.0, Lin),
                    kf(0.95, 12.5, 0.0, 0.0, Lin),
                    kf(1.05, 0.0, 0.0, 0.0, Lin),
                    kf(1.1, 0.0, 0.0, 0.0, Lin),
                ],
            },
            KfChannel {
                part: "cube",
                target: KfTarget::Pos,
                frames: &[
                    kf(0.0, 0.0, 0.0, 0.0, Lin),
                    kf(0.05, 0.0, 1.6, 0.0, Lin),
                    kf(0.15, 0.0, 0.5, 0.0, Lin),
                    kf(0.25, 0.0, 0.0, 0.0, Lin),
                    kf(0.7, 0.0, 0.0, 0.0, Lin),
                    kf(0.75, 0.0, 1.2, 0.0, Lin),
                    kf(0.85, 0.0, 1.7, 0.0, Lin),
                    kf(0.9, 0.0, 0.0, 0.0, Lin),
                    kf(0.95, 0.0, 1.3, 0.0, Lin),
                    kf(1.05, 0.0, 0.0, 0.0, Lin),
                    kf(1.2, 0.0, 5.0, 0.0, Lin),
                    kf(1.25, 0.0, 8.0, 0.0, Lin),
                    kf(1.5, 0.0, 1.0, 0.0, Lin),
                ],
            },
        ],
    }
};

static ARMADILLO_BABY_PEEK: KfAnim = {
    use Kfi::{Cat, Lin};
    KfAnim {
        length: 2.5,
        looping: false,
        channels: &[
            KfChannel {
                part: "head",
                target: KfTarget::Rot,
                frames: &[
                    kf(0.0, -70.0, 0.0, 0.0, Lin),
                    kf(0.15, -65.0, 0.0, 0.0, Lin),
                    kf(0.4, -50.0, 0.0, 0.0, Lin),
                    kf(0.5, 0.0, 0.0, 0.0, Lin),
                    kf(0.7, 0.0, 0.0, 0.0, Lin),
                    kf(0.9, -7.5, 0.0, 45.0, Cat),
                    kf(1.15, -7.5, 0.0, 45.0, Lin),
                    kf(1.3, -0.8639, -1.4959, -39.1287, Cat),
                    kf(1.6, -0.8639, -1.4959, -39.1287, Lin),
                    kf(1.75, 0.0, 0.0, 0.0, Lin),
                    kf(1.8, -25.0, 0.0, 0.0, Lin),
                    kf(1.85, -70.0, 0.0, 0.0, Lin),
                ],
            },
            KfChannel {
                part: "head",
                target: KfTarget::Pos,
                frames: &[
                    kf(0.0, 0.0, -1.25, 4.0, Lin),
                    kf(0.1, 0.0, -1.0, 4.0, Lin),
                    kf(0.15, 0.0, -1.0, 4.0, Lin),
                    kf(0.25, 0.0, -1.0, 5.0, Lin),
                    kf(0.35, 0.0, 0.0, 0.2, Lin),
                    kf(0.4, 0.0, 0.0, 0.2, Lin),
                    kf(0.45, 0.0, 0.05, 0.7, Lin),
                    kf(0.5, 0.0, 1.1, 1.2, Lin),
                    kf(0.6, 0.0, 0.1, 0.8, Lin),
                    kf(0.7, 0.0, 0.1, 0.7, Lin),
                    kf(1.75, 0.0, 0.1, 0.7, Lin),
                    kf(1.8, 0.0, 0.1, 0.2, Lin),
                    kf(1.9, 0.0, -1.2, 3.53, Lin),
                    kf(1.95, 0.0, -1.2, 3.53, Lin),
                    kf(2.0, 0.0, -1.2, 3.53, Lin),
                    kf(2.15, 0.0, -0.9, 4.2, Lin),
                    kf(2.3, 0.0, -0.9, 3.4, Lin),
                    kf(2.5, 0.0, -1.9, 4.1, Lin),
                ],
            },
            KfChannel {
                part: "right_hind_leg",
                target: KfTarget::Pos,
                frames: &[kf(0.0, 0.0, 3.0, -2.0, Lin)],
            },
            KfChannel {
                part: "left_hind_leg",
                target: KfTarget::Pos,
                frames: &[kf(0.0, 0.0, 3.0, -2.0, Lin)],
            },
            KfChannel {
                part: "right_front_leg",
                target: KfTarget::Rot,
                frames: &[
                    kf(0.0, -90.0, 0.0, 0.0, Lin),
                    kf(0.6, -90.0, 0.0, 0.0, Cat),
                    kf(0.85, -45.0, 0.0, 0.0, Cat),
                    kf(1.0, -45.0, 0.0, 0.0, Cat),
                    kf(1.75, -45.0, 0.0, 0.0, Cat),
                    kf(1.8, -90.0, 0.0, 0.0, Cat),
                    kf(1.95, -90.0, 0.0, 0.0, Cat),
                ],
            },
            KfChannel {
                part: "right_front_leg",
                target: KfTarget::Pos,
                frames: &[
                    kf(0.0, -1.0, 2.0, 2.0, Lin),
                    kf(0.6, -1.0, 2.0, 2.0, Cat),
                    kf(0.65, -1.0, 2.0, 1.0, Cat),
                    kf(0.85, 0.0, 1.0, -0.5, Cat),
                    kf(1.0, 0.0, 1.0, -0.5, Cat),
                    kf(1.75, 0.0, 1.0, -0.5, Cat),
                    kf(1.95, 0.0, 1.0, 3.0, Cat),
                    kf(2.0, -1.0, 2.0, 3.0, Cat),
                    kf(2.15, -1.0, 3.0, 4.0, Cat),
                ],
            },
            KfChannel {
                part: "left_front_leg",
                target: KfTarget::Rot,
                frames: &[
                    kf(0.0, -90.0, 0.0, 0.0, Lin),
                    kf(0.65, -90.0, 0.0, 0.0, Cat),
                    kf(0.85, -45.0, 0.0, 0.0, Cat),
                    kf(1.0, -45.0, 0.0, 0.0, Cat),
                    kf(1.75, -45.0, 0.0, 0.0, Cat),
                    kf(1.8, -90.0, 0.0, 0.0, Cat),
                    kf(1.95, -90.0, 0.0, 0.0, Cat),
                ],
            },
            KfChannel {
                part: "left_front_leg",
                target: KfTarget::Pos,
                frames: &[
                    kf(0.0, 1.0, 2.0, 2.0, Lin),
                    kf(0.65, 1.0, 2.0, 2.0, Cat),
                    kf(0.85, 0.0, 1.0, -0.4, Cat),
                    kf(1.0, 0.0, 1.0, -0.4, Cat),
                    kf(1.75, 0.0, 1.0, -0.4, Cat),
                    kf(1.95, 1.0, 1.0, 2.0, Cat),
                    kf(2.0, 1.0, 2.0, 3.0, Cat),
                    kf(2.15, 1.0, 3.0, 4.0, Cat),
                ],
            },
            KfChannel {
                part: "left_ear",
                target: KfTarget::Rot,
                frames: &[
                    kf(0.0, 0.0, 0.0, 0.0, Lin),
                    kf(0.4, 0.0, 0.0, 0.0, Lin),
                    kf(0.5, -15.0, 0.0, 0.0, Lin),
                    kf(0.6, 0.0, 0.0, 0.0, Lin),
                    kf(1.8, 0.0, 0.0, 0.0, Lin),
                    kf(1.85, -10.0, 0.0, 0.0, Lin),
                    kf(2.5, 0.0, 0.0, 0.0, Lin),
                ],
            },
            KfChannel {
                part: "left_ear",
                target: KfTarget::Pos,
                frames: &[
                    kf(0.0, 0.0, 0.0, 0.0, Lin),
                    kf(0.4, 0.0, -1.13, 1.05, Lin),
                    kf(0.5, 0.0, -1.4129, 0.0617, Lin),
                    kf(0.6, 0.0, 0.0, 0.0, Lin),
                ],
            },
            KfChannel {
                part: "right_ear",
                target: KfTarget::Rot,
                frames: &[
                    kf(0.0, 0.0, 0.0, 0.0, Lin),
                    kf(0.4, 0.0, 0.0, 0.0, Lin),
                    kf(0.5, -15.0, 0.0, 0.0, Lin),
                    kf(0.6, 0.0, 0.0, 0.0, Lin),
                    kf(1.8, 0.0, 0.0, 0.0, Lin),
                    kf(1.85, -10.0, 0.0, 0.0, Lin),
                    kf(2.5, 0.0, 0.0, 0.0, Lin),
                ],
            },
            KfChannel {
                part: "right_ear",
                target: KfTarget::Pos,
                frames: &[
                    kf(0.0, 0.0, 0.0, 0.0, Lin),
                    kf(0.4, 0.0, -1.13, 1.05, Lin),
                    kf(0.5, 0.0, -1.4129, 0.0617, Lin),
                    kf(0.6, 0.0, 0.0, 0.0, Lin),
                ],
            },
            KfChannel {
                part: "cube",
                target: KfTarget::Rot,
                frames: &[
                    kf(0.0, 0.0, 0.0, 0.0, Lin),
                    kf(0.25, 0.0, 0.0, 0.0, Lin),
                    kf(0.35, 15.0, 0.0, 0.0, Lin),
                    kf(0.5, -7.5, 0.0, 0.0, Lin),
                    kf(0.6, 0.0, 0.0, 0.0, Lin),
                    kf(2.0, 0.0, 0.0, 0.0, Lin),
                    kf(2.05, -17.5, 0.0, 0.0, Lin),
                    kf(2.15, -25.0, 0.0, 0.0, Lin),
                    kf(2.25, 0.0, 0.0, 0.0, Lin),
                    kf(2.3, 12.5, 0.0, 0.0, Lin),
                    kf(2.5, 0.0, 0.0, 0.0, Lin),
                ],
            },
            KfChannel {
                part: "cube",
                target: KfTarget::Pos,
                frames: &[
                    kf(0.25, 0.0, 0.0, 0.0, Lin),
                    kf(0.35, 0.0, 1.6, 0.0, Lin),
                    kf(0.5, 0.0, 0.5, 0.0, Lin),
                    kf(0.6, 0.0, 0.0, 0.0, Lin),
                    kf(2.0, 0.0, 0.0, 0.0, Lin),
                    kf(2.05, 0.0, 1.2, 0.0, Lin),
                    kf(2.15, 0.0, 1.7, 0.0, Lin),
                    kf(2.25, 0.0, 0.0, 0.0, Lin),
                    kf(2.3, 0.0, 1.3, 0.0, Lin),
                    kf(2.5, 0.0, 0.0, 0.0, Lin),
                ],
            },
        ],
    }
};

#[cfg(test)]
mod tests {
    use super::*;

    /// `KeyframeAnimation.bake` throws on a channel naming a part the model
    /// lacks; every table must resolve against its own mesh.
    #[test]
    fn every_channel_names_a_part_of_its_mesh() {
        for (model, t) in [
            (bake_armadillo_model(), &ADULT),
            (bake_baby_armadillo_model(), &BABY),
        ] {
            for anim in [t.walk, t.roll_out, t.roll_up, t.peek] {
                for ch in anim.channels {
                    part_index(&model.parts, ch.part);
                }
            }
        }
    }

    /// In the shell the cube replaces the body's own cubes, hind legs and
    /// tail; out of it only the cube hides.
    #[test]
    fn shell_swaps_body_for_cube() {
        let model = bake_armadillo_model();
        let hidden = |hiding_in_shell| {
            let a = ArmadilloAnims {
                hiding_in_shell,
                ..Default::default()
            };
            let anim = compute_armadillo_anim(&model, 0.0, 0.0, 0.0, 0.0, false, &a);
            let mut names: Vec<_> = anim
                .scale
                .iter()
                .filter(|(_, s)| *s == Vec3::ZERO)
                .map(|&(i, _)| model.parts[i].name.as_str())
                .collect();
            names.sort_unstable();
            names
        };
        assert_eq!(
            hidden(true),
            [BODY_SHELL, "left_hind_leg", "right_hind_leg", "tail"]
        );
        assert_eq!(hidden(false), ["cube"]);
    }
}
