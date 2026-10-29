//! Axolotl: `AdultAxolotlModel` (procedural, factor-blended poses),
//! `BabyAxolotlModel` (keyframed) and the `BabyAxolotlAnimation` tables,
//! converted mechanically from the decompiled definitions (`scaleVec` values
//! stored as `v - 1`). The unused `baby_axolotl_dash` is omitted.

use std::f32::consts::{FRAC_PI_2, FRAC_PI_4, PI};

use glam::Vec3;

use super::{
    BakedEntityModel, KfAnim, KfChannel, KfDeltas, KfTarget, Kfi, ModelCube, ModelPose, PartAnim,
    apply_kf_anim, apply_kf_walk, bake_model, deformed, kf, part_index, rotated, vbox, vpart,
};

/// Vanilla `AdultAxolotlModel`'s `CubeDeformation(0.001f)`.
fn fudged(cube: ModelCube) -> ModelCube {
    deformed(cube, 0.001)
}

/// Vanilla `AdultAxolotlModel.createBodyLayer`, 64x64.
pub fn bake_axolotl_model() -> BakedEntityModel {
    let left_leg = fudged(vbox((2, 13), (-1.0, 0.0, 0.0), (3.0, 5.0, 0.0)));
    let right_leg = fudged(vbox((2, 13), (-2.0, 0.0, 0.0), (3.0, 5.0, 0.0)));
    let parts = vec![
        vpart(
            "body",
            None,
            Vec3::new(0.0, 19.5, 5.0),
            vec![
                vbox((0, 11), (-4.0, -2.0, -9.0), (8.0, 4.0, 10.0)),
                vbox((2, 17), (0.0, -3.0, -8.0), (0.0, 5.0, 9.0)),
            ],
        ),
        vpart(
            "head",
            Some(0),
            Vec3::new(0.0, 0.0, -9.0),
            vec![fudged(vbox((0, 1), (-4.0, -3.0, -5.0), (8.0, 5.0, 5.0)))],
        ),
        vpart(
            "top_gills",
            Some(1),
            Vec3::new(0.0, -3.0, -1.0),
            vec![fudged(vbox((3, 37), (-4.0, -3.0, 0.0), (8.0, 3.0, 0.0)))],
        ),
        vpart(
            "left_gills",
            Some(1),
            Vec3::new(-4.0, 0.0, -1.0),
            vec![fudged(vbox((0, 40), (-3.0, -5.0, 0.0), (3.0, 7.0, 0.0)))],
        ),
        vpart(
            "right_gills",
            Some(1),
            Vec3::new(4.0, 0.0, -1.0),
            vec![fudged(vbox((11, 40), (0.0, -5.0, 0.0), (3.0, 7.0, 0.0)))],
        ),
        vpart(
            "right_hind_leg",
            Some(0),
            Vec3::new(-3.5, 1.0, -1.0),
            vec![right_leg],
        ),
        vpart(
            "left_hind_leg",
            Some(0),
            Vec3::new(3.5, 1.0, -1.0),
            vec![left_leg],
        ),
        vpart(
            "right_front_leg",
            Some(0),
            Vec3::new(-3.5, 1.0, -8.0),
            vec![right_leg],
        ),
        vpart(
            "left_front_leg",
            Some(0),
            Vec3::new(3.5, 1.0, -8.0),
            vec![left_leg],
        ),
        vpart(
            "tail",
            Some(0),
            Vec3::new(0.0, 0.0, 1.0),
            vec![vbox((2, 19), (0.0, -3.0, 0.0), (0.0, 5.0, 12.0))],
        ),
    ];
    bake_model(parts, 64, 64)
}

/// Vanilla `BabyAxolotlModel.createBodyLayer`, 32x32. The right hind leg's
/// cube hangs off a rotated `right_leg_r1` child. The `1.5708` rotations are
/// vanilla's rounded literals, not `FRAC_PI_2`.
#[allow(clippy::approx_constant)]
pub fn bake_baby_axolotl_model() -> BakedEntityModel {
    let leg = |name: &str, x: f32, z: f32, uv: (i32, i32), origin_x: f32| {
        vpart(
            name,
            Some(1),
            Vec3::new(x, 0.25, z),
            vec![vbox(uv, (origin_x, 0.0, -0.5), (3.0, 0.0, 1.0))],
        )
    };
    let parts = vec![
        vpart("root", None, Vec3::new(0.0, 24.0, 0.0), vec![]),
        vpart(
            "body",
            Some(0),
            Vec3::new(0.0, -1.25, 1.75),
            vec![
                vbox((0, 0), (-2.0, -0.75, -2.75), (4.0, 2.0, 6.0)),
                vbox((0, 12), (0.0, -1.75, -2.75), (0.0, 3.0, 5.0)),
            ],
        ),
        leg("right_front_leg", -2.0, -1.25, (20, 16), -3.0),
        rotated(
            vpart(
                "right_hind_leg",
                Some(1),
                Vec3::new(-2.0, 0.25, 1.75),
                vec![],
            ),
            Vec3::new(0.0, 1.5708, 1.5708),
        ),
        rotated(
            vpart(
                "right_leg_r1",
                Some(3),
                Vec3::ZERO,
                vec![vbox((20, 14), (0.0, 0.0, -0.5), (3.0, 0.0, 1.0))],
            ),
            Vec3::new(-1.5708, 0.0, 1.5708),
        ),
        leg("left_front_leg", 2.0, -1.25, (20, 13), 0.0),
        leg("left_hind_leg", 2.0, 1.75, (20, 14), 0.0),
        vpart(
            "tail",
            Some(1),
            Vec3::new(0.0, -0.25, 3.25),
            vec![vbox((10, 9), (0.0, -1.5, -1.0), (0.0, 3.0, 8.0))],
        ),
        vpart(
            "head",
            Some(1),
            Vec3::new(0.0, 0.25, -2.75),
            vec![vbox((0, 8), (-3.0, -2.0, -4.0), (6.0, 3.0, 4.0))],
        ),
        vpart(
            "left_gills",
            Some(8),
            Vec3::new(3.0, -0.5, -2.0),
            vec![vbox((20, 8), (0.0, -3.5, 0.0), (3.0, 5.0, 0.0))],
        ),
        vpart(
            "right_gills",
            Some(8),
            Vec3::new(-3.0, -0.5, -2.0),
            vec![vbox((20, 3), (-3.0, -3.5, 0.0), (3.0, 5.0, 0.0))],
        ),
        vpart(
            "top_gills",
            Some(8),
            Vec3::new(0.0, -2.0, -2.0),
            vec![vbox((20, 0), (-3.0, -3.0, 0.0), (6.0, 3.0, 0.0))],
        ),
    ];
    bake_model(parts, 32, 32)
}

/// Vanilla `AxolotlRenderState`'s animation inputs: the adult's four eased
/// `BinaryAnimator` factors and the baby's `AnimationState` clocks (seconds
/// since start; `None` = stopped). `walkUnderWaterAnimationState` is
/// extracted by vanilla but never read by `BabyAxolotlModel`, so it isn't
/// carried.
#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub struct AxolotlAnims {
    pub playing_dead_factor: f32,
    pub in_water_factor: f32,
    pub on_ground_factor: f32,
    pub moving_factor: f32,
    pub swim: Option<f32>,
    pub walk: Option<f32>,
    pub idle_on_ground: Option<f32>,
    pub idle_under_water: Option<f32>,
    pub idle_under_water_on_ground: Option<f32>,
    pub play_dead: Option<f32>,
}

/// Vanilla `AdultAxolotlModel.setupAnim` / `BabyAxolotlModel.setupAnim`.
#[allow(clippy::too_many_arguments)]
pub fn compute_axolotl_anim(
    model: &BakedEntityModel,
    is_baby: bool,
    x_rot_deg: f32,
    local_head_y_rot_deg: f32,
    age_in_ticks: f32,
    walk_pos: f32,
    walk_speed: f32,
    a: &AxolotlAnims,
) -> PartAnim {
    if is_baby {
        return compute_baby_axolotl_anim(model, walk_pos, walk_speed, a);
    }
    let idx = |name| part_index(&model.parts, name);
    let p = AdultParts {
        body: idx("body"),
        head: idx("head"),
        top_gills: idx("top_gills"),
        left_gills: idx("left_gills"),
        right_gills: idx("right_gills"),
        left_hind_leg: idx("left_hind_leg"),
        right_hind_leg: idx("right_hind_leg"),
        left_front_leg: idx("left_front_leg"),
        right_front_leg: idx("right_front_leg"),
        tail: idx("tail"),
    };
    let mut m = ModelPose::new(model);
    let moving = a.moving_factor;
    let not_moving = 1.0 - moving;
    let mirrored_legs = 1.0 - a.on_ground_factor.min(moving);
    m.rot[p.body].y += local_head_y_rot_deg * (PI / 180.0);
    swimming(
        &mut m,
        &p,
        age_in_ticks,
        x_rot_deg,
        moving.min(a.in_water_factor),
    );
    water_hovering(&mut m, &p, age_in_ticks, not_moving.min(a.in_water_factor));
    ground_crawling(&mut m, &p, age_in_ticks, moving.min(a.on_ground_factor));
    lay_still_on_ground(&mut m, &p, age_in_ticks, not_moving.min(a.on_ground_factor));
    play_dead(&mut m, &p, a.playing_dead_factor);
    mirror_leg_rotations(&mut m, &p, mirrored_legs);
    m.emit(model)
}

/// `BabyAxolotlModel.setupAnim`. Vanilla applies `WALK_FLOOR_UNDERWATER`
/// on the floor walk's `AnimationState`, not its own, so it plays alongside
/// the floor walk and never under water.
fn compute_baby_axolotl_anim(
    model: &BakedEntityModel,
    walk_pos: f32,
    walk_speed: f32,
    a: &AxolotlAnims,
) -> PartAnim {
    let mut d = KfDeltas::new(model);
    if a.walk.is_some() {
        apply_kf_walk(
            &AXOLOTL_WALK_FLOOR,
            model,
            walk_pos,
            walk_speed,
            15.0,
            30.0,
            &mut d,
        );
    }
    apply_kf_anim(&BABY_AXOLOTL_SWIM, model, a.swim, &mut d);
    apply_kf_anim(&WALK_FLOOR_UNDERWATER, model, a.walk, &mut d);
    apply_kf_anim(&BABY_AXOLOTL_IDLE_FLOOR, model, a.idle_on_ground, &mut d);
    apply_kf_anim(&IDLE_UNDERWATER, model, a.idle_under_water, &mut d);
    apply_kf_anim(
        &IDLE_FLOOR_UNDERWATER,
        model,
        a.idle_under_water_on_ground,
        &mut d,
    );
    apply_kf_anim(&BABY_AXOLOTL_PLAY_DEAD, model, a.play_dead, &mut d);
    d.emit(model)
}

/// `AdultAxolotlModel`'s posed parts.
struct AdultParts {
    body: usize,
    head: usize,
    top_gills: usize,
    left_gills: usize,
    right_gills: usize,
    left_hind_leg: usize,
    right_hind_leg: usize,
    left_front_leg: usize,
    right_front_leg: usize,
    tail: usize,
}

/// The on-ground poses spread all three gills by the same angle.
fn flare_gills(m: &mut ModelPose, p: &AdultParts, gill: f32) {
    m.rot[p.top_gills].x += gill;
    m.rot[p.left_gills].y -= gill;
    m.rot[p.right_gills].y += gill;
}

fn lay_still_on_ground(m: &mut ModelPose, p: &AdultParts, age_in_ticks: f32, factor: f32) {
    if factor <= 1.0e-5 {
        return;
    }
    let speed = age_in_ticks * 0.09;
    let sine = speed.sin();
    let cosine = speed.cos();
    let movement = sine * sine - 2.0 * sine;
    let movement2 = cosine * cosine - 3.0 * sine;
    m.rot[p.head].x += -0.09 * movement * factor;
    m.rot[p.head].z += -0.2 * factor;
    m.rot[p.tail].y += (-0.1 + 0.1 * movement) * factor;
    flare_gills(m, p, (0.6 + 0.05 * movement2) * factor);
    m.rot[p.left_hind_leg].x += 1.1 * factor;
    m.rot[p.left_hind_leg].y += 1.0 * factor;
    m.rot[p.left_front_leg].x += 0.8 * factor;
    m.rot[p.left_front_leg].y += 2.3 * factor;
    m.rot[p.left_front_leg].z -= 0.5 * factor;
}

fn ground_crawling(m: &mut ModelPose, p: &AdultParts, age_in_ticks: f32, factor: f32) {
    if factor <= 1.0e-5 {
        return;
    }
    let speed = age_in_ticks * 0.11;
    let cosine = speed.cos();
    let hind_leg_y_sway = (cosine * cosine - 2.0 * cosine) / 5.0;
    let front_leg_y_sway = 0.7 * cosine;
    let head_and_tail_y = 0.09 * cosine * factor;
    m.rot[p.head].y += head_and_tail_y;
    m.rot[p.tail].y += head_and_tail_y;
    flare_gills(
        m,
        p,
        (0.6 - 0.08 * (cosine * cosine + 2.0 * speed.sin())) * factor,
    );
    let hind_leg_x = 0.9424779 * factor;
    let front_leg_x = 1.0995574 * factor;
    m.rot[p.left_hind_leg].x += hind_leg_x;
    m.rot[p.left_hind_leg].y += (1.5 - hind_leg_y_sway) * factor;
    m.rot[p.left_hind_leg].z += -0.1 * factor;
    m.rot[p.left_front_leg].x += front_leg_x;
    m.rot[p.left_front_leg].y += (FRAC_PI_2 - front_leg_y_sway) * factor;
    m.rot[p.right_hind_leg].x += hind_leg_x;
    m.rot[p.right_hind_leg].y += (-1.0 - hind_leg_y_sway) * factor;
    m.rot[p.right_front_leg].x += front_leg_x;
    m.rot[p.right_front_leg].y += (-FRAC_PI_2 - front_leg_y_sway) * factor;
}

fn water_hovering(m: &mut ModelPose, p: &AdultParts, age_in_ticks: f32, factor: f32) {
    if factor <= 1.0e-5 {
        return;
    }
    let speed = age_in_ticks * 0.075;
    let cosine = speed.cos();
    let sine = speed.sin() * 0.15;
    let body_x = (-0.15 + 0.075 * cosine) * factor;
    m.rot[p.body].x += body_x;
    m.pos[p.body].y -= sine * factor;
    m.rot[p.head].x -= body_x;
    m.rot[p.top_gills].x += 0.2 * cosine * factor;
    let gill_y = (-0.3 * cosine - 0.19) * factor;
    m.rot[p.left_gills].y += gill_y;
    m.rot[p.right_gills].y -= gill_y;
    m.rot[p.left_hind_leg].x += (2.3561945 - cosine * 0.11) * factor;
    m.rot[p.left_hind_leg].y += 0.47123894 * factor;
    m.rot[p.left_hind_leg].z += 1.7278761 * factor;
    m.rot[p.left_front_leg].x += (FRAC_PI_4 - cosine * 0.2) * factor;
    m.rot[p.left_front_leg].y += 2.042035 * factor;
    m.rot[p.tail].y += 0.5 * cosine * factor;
}

fn swimming(m: &mut ModelPose, p: &AdultParts, age_in_ticks: f32, x_rot_deg: f32, factor: f32) {
    if factor <= 1.0e-5 {
        return;
    }
    let speed = age_in_ticks * 0.33;
    let sine = speed.sin();
    let cosine = speed.cos();
    let body_sway = 0.13 * sine;
    m.rot[p.body].x += (x_rot_deg * (PI / 180.0) + body_sway) * factor;
    m.rot[p.head].x -= body_sway * 1.8 * factor;
    m.pos[p.body].y -= 0.45 * cosine * factor;
    m.rot[p.top_gills].x += (-0.5 * sine - 0.8) * factor;
    let gill_y = (0.3 * sine + 0.9) * factor;
    m.rot[p.left_gills].y += gill_y;
    m.rot[p.right_gills].y -= gill_y;
    m.rot[p.tail].y += 0.3 * (speed * 0.9).cos() * factor;
    m.rot[p.left_hind_leg].x += 1.8849558 * factor;
    m.rot[p.left_hind_leg].y += -0.4 * sine * factor;
    m.rot[p.left_hind_leg].z += FRAC_PI_2 * factor;
    m.rot[p.left_front_leg].x += 1.8849558 * factor;
    m.rot[p.left_front_leg].y += (-0.2 * cosine - 0.1) * factor;
    m.rot[p.left_front_leg].z += FRAC_PI_2 * factor;
}

fn play_dead(m: &mut ModelPose, p: &AdultParts, factor: f32) {
    if factor <= 1.0e-5 {
        return;
    }
    m.rot[p.left_hind_leg].x += 1.4137167 * factor;
    m.rot[p.left_hind_leg].y += 1.0995574 * factor;
    m.rot[p.left_hind_leg].z += FRAC_PI_4 * factor;
    m.rot[p.left_front_leg].x += FRAC_PI_4 * factor;
    m.rot[p.left_front_leg].y += 2.042035 * factor;
    m.rot[p.body].x += -0.15 * factor;
    m.rot[p.body].z += 0.35 * factor;
}

/// The right legs take on the left legs' pose, y and z negated.
fn mirror_leg_rotations(m: &mut ModelPose, p: &AdultParts, factor: f32) {
    if factor <= 1.0e-5 {
        return;
    }
    let mirror = Vec3::new(1.0, -1.0, -1.0) * factor;
    for (right, left) in [
        (p.right_hind_leg, p.left_hind_leg),
        (p.right_front_leg, p.left_front_leg),
    ] {
        let mirrored = m.rot[left] * mirror;
        m.rot[right] += mirrored;
    }
}

static BABY_AXOLOTL_IDLE_FLOOR: KfAnim = {
    use Kfi::{Cat, Lin};
    KfAnim {
        length: 2.88,
        looping: true,
        channels: &[
            KfChannel {
                part: "body",
                target: KfTarget::Rot,
                frames: &[kf(0.0, 0.0, 0.0, 0.0, Cat), kf(2.8, 0.0, 0.0, 0.0, Cat)],
            },
            KfChannel {
                part: "right_front_leg",
                target: KfTarget::Rot,
                frames: &[kf(0.0, 160.0, -10.0, -37.5, Cat)],
            },
            KfChannel {
                part: "right_front_leg",
                target: KfTarget::Pos,
                frames: &[kf(0.0, 0.2, 0.0, 0.0, Lin)],
            },
            KfChannel {
                part: "right_hind_leg",
                target: KfTarget::Rot,
                frames: &[kf(1.72, 360.0, 0.0, -45.0, Cat)],
            },
            KfChannel {
                part: "left_front_leg",
                target: KfTarget::Rot,
                frames: &[kf(0.0, 160.0, 10.0, 37.5, Cat)],
            },
            KfChannel {
                part: "left_front_leg",
                target: KfTarget::Pos,
                frames: &[kf(0.0, -0.2, 0.0, 0.0, Lin)],
            },
            KfChannel {
                part: "left_hind_leg",
                target: KfTarget::Rot,
                frames: &[kf(0.0, 0.0, 0.0, 37.5, Cat)],
            },
            KfChannel {
                part: "tail",
                target: KfTarget::Rot,
                frames: &[
                    kf(0.0, 0.0, 10.0, 0.0, Cat),
                    kf(0.72, 0.0, -14.0, 0.0, Lin),
                    kf(0.96, 0.0, -21.0, 0.0, Lin),
                    kf(1.4, 0.0, -25.0, 0.0, Cat),
                    kf(2.04, 0.0, -16.75, 0.0, Cat),
                    kf(2.84, 0.0, 10.0, 0.0, Cat),
                ],
            },
            KfChannel {
                part: "head",
                target: KfTarget::Rot,
                frames: &[
                    kf(0.04, 0.0, 0.0, -6.0, Cat),
                    kf(1.32, -6.45, -1.45, -6.5, Lin),
                    kf(1.48, -6.45, -1.45, -6.5, Lin),
                    kf(2.84, 0.0, 0.0, -6.0, Cat),
                ],
            },
            KfChannel {
                part: "left_gills",
                target: KfTarget::Rot,
                frames: &[
                    kf(0.0, 0.0, 38.0, 0.0, Cat),
                    kf(1.28, 0.0, 45.5, 0.0, Cat),
                    kf(1.64, 0.0, 45.5, 0.0, Cat),
                    kf(2.8, 0.0, 38.0, 0.0, Cat),
                ],
            },
            KfChannel {
                part: "right_gills",
                target: KfTarget::Rot,
                frames: &[
                    kf(0.04, 0.0, -50.0, 0.0, Cat),
                    kf(1.32, 0.0, -59.0, 0.0, Cat),
                    kf(1.6, 0.0, -59.0, 0.0, Cat),
                    kf(2.84, 0.0, -50.0, 0.0, Cat),
                ],
            },
            KfChannel {
                part: "top_gills",
                target: KfTarget::Rot,
                frames: &[
                    kf(0.0, 33.0, 0.0, 0.0, Cat),
                    kf(1.28, 47.4, 0.0, 0.0, Cat),
                    kf(1.64, 47.4, 0.0, 0.0, Cat),
                    kf(2.8, 33.0, 0.0, 0.0, Cat),
                ],
            },
        ],
    }
};

static IDLE_FLOOR_UNDERWATER: KfAnim = {
    use Kfi::{Cat, Lin};
    KfAnim {
        length: 5.12,
        looping: true,
        channels: &[
            KfChannel {
                part: "body",
                target: KfTarget::Rot,
                frames: &[kf(0.0, 0.0, 0.0, 0.0, Cat), kf(5.12, 0.0, 0.0, 0.0, Cat)],
            },
            KfChannel {
                part: "body",
                target: KfTarget::Pos,
                frames: &[kf(0.0, 0.0, 1.5, 0.0, Cat), kf(5.12, 0.0, 1.5, 0.0, Cat)],
            },
            KfChannel {
                part: "right_front_leg",
                target: KfTarget::Rot,
                frames: &[kf(5.04, 190.0, -15.0, -30.0, Cat)],
            },
            KfChannel {
                part: "right_front_leg",
                target: KfTarget::Pos,
                frames: &[kf(0.0, 0.3, 0.0, 0.5, Cat), kf(5.12, 0.3, 0.0, 0.5, Cat)],
            },
            KfChannel {
                part: "right_hind_leg",
                target: KfTarget::Rot,
                frames: &[kf(0.0, 360.0, 0.0, -30.0, Cat)],
            },
            KfChannel {
                part: "right_hind_leg",
                target: KfTarget::Pos,
                frames: &[kf(0.0, 0.18, 0.0, 0.0, Lin), kf(5.12, 0.18, 0.0, 0.0, Lin)],
            },
            KfChannel {
                part: "left_front_leg",
                target: KfTarget::Rot,
                frames: &[kf(5.04, 190.0, 18.0, 30.0, Cat)],
            },
            KfChannel {
                part: "left_front_leg",
                target: KfTarget::Pos,
                frames: &[kf(0.0, -0.1, 0.0, 0.5, Cat), kf(5.12, -0.1, 0.0, 0.5, Cat)],
            },
            KfChannel {
                part: "left_hind_leg",
                target: KfTarget::Rot,
                frames: &[kf(5.04, 180.0, 0.0, 30.0, Cat)],
            },
            KfChannel {
                part: "left_hind_leg",
                target: KfTarget::Pos,
                frames: &[kf(0.0, -0.2, 0.0, 0.0, Lin), kf(5.12, -0.2, 0.0, 0.0, Lin)],
            },
            KfChannel {
                part: "tail",
                target: KfTarget::Rot,
                frames: &[
                    kf(0.04, 0.0, -12.0, 0.0, Cat),
                    kf(2.56, 0.0, 12.0, 0.0, Cat),
                    kf(5.12, 0.0, -12.0, 0.0, Cat),
                ],
            },
            KfChannel {
                part: "left_gills",
                target: KfTarget::Rot,
                frames: &[
                    kf(0.0, 0.0, 10.0, 0.0, Cat),
                    kf(3.12, 0.0, -16.0, 0.0, Cat),
                    kf(5.12, 0.0, 10.0, 0.0, Cat),
                ],
            },
            KfChannel {
                part: "right_gills",
                target: KfTarget::Rot,
                frames: &[
                    kf(0.0, 0.0, -16.0, 0.0, Cat),
                    kf(2.08, 0.0, 12.0, 0.0, Cat),
                    kf(5.12, 0.0, -16.0, 0.0, Cat),
                ],
            },
            KfChannel {
                part: "top_gills",
                target: KfTarget::Rot,
                frames: &[
                    kf(0.0, 10.0, 0.0, 0.0, Cat),
                    kf(2.56, -16.0, 0.0, 0.0, Cat),
                    kf(5.12, 10.0, 0.0, 0.0, Cat),
                ],
            },
        ],
    }
};

static IDLE_UNDERWATER: KfAnim = {
    use Kfi::Cat;
    KfAnim {
        length: 5.2,
        looping: true,
        channels: &[
            KfChannel {
                part: "body",
                target: KfTarget::Rot,
                frames: &[
                    kf(0.0, -8.9, 0.0, 0.0, Cat),
                    kf(2.6, -2.6, 0.0, 0.0, Cat),
                    kf(5.2, -8.9, 0.0, 0.0, Cat),
                ],
            },
            KfChannel {
                part: "body",
                target: KfTarget::Pos,
                frames: &[
                    kf(0.0, 0.0, 0.0, 0.0, Cat),
                    kf(2.6, 0.0, -0.5, 0.0, Cat),
                    kf(5.2, 0.0, 0.0, 0.0, Cat),
                ],
            },
            KfChannel {
                part: "right_front_leg",
                target: KfTarget::Rot,
                frames: &[
                    kf(0.0, -5.0, -20.0, -55.0, Cat),
                    kf(2.6, 1.0, -20.0, -65.0, Cat),
                    kf(5.2, -5.0, -20.0, -55.0, Cat),
                ],
            },
            KfChannel {
                part: "right_front_leg",
                target: KfTarget::Pos,
                frames: &[kf(0.0, 0.15, -0.3391, 0.0876, Cat)],
            },
            KfChannel {
                part: "right_hind_leg",
                target: KfTarget::Rot,
                frames: &[
                    kf(0.0, 60.0, -60.0, 15.0, Cat),
                    kf(2.6, 75.0, -60.0, 0.0, Cat),
                    kf(5.2, 60.0, -60.0, 15.0, Cat),
                ],
            },
            KfChannel {
                part: "right_hind_leg",
                target: KfTarget::Pos,
                frames: &[kf(5.2, 0.0, -0.4, 0.0, Cat)],
            },
            KfChannel {
                part: "left_front_leg",
                target: KfTarget::Rot,
                frames: &[
                    kf(0.0, 0.0, 20.0, 50.0, Cat),
                    kf(2.6, 0.0, 20.0, 60.0, Cat),
                    kf(5.2, 0.0, 20.0, 50.0, Cat),
                ],
            },
            KfChannel {
                part: "left_front_leg",
                target: KfTarget::Pos,
                frames: &[kf(2.68, -0.15, -0.4, 0.3, Cat)],
            },
            KfChannel {
                part: "left_hind_leg",
                target: KfTarget::Rot,
                frames: &[
                    kf(0.0, -135.0, -50.0, -330.0, Cat),
                    kf(2.6, -160.0, -60.0, -295.0, Cat),
                    kf(5.2, -135.0, -50.0, -330.0, Cat),
                ],
            },
            KfChannel {
                part: "left_hind_leg",
                target: KfTarget::Pos,
                frames: &[kf(5.2, 0.0, -0.5, 0.0, Cat)],
            },
            KfChannel {
                part: "tail",
                target: KfTarget::Rot,
                frames: &[
                    kf(0.0, 0.0, 15.0, 0.0, Cat),
                    kf(2.6, 0.0, -32.0, 0.0, Cat),
                    kf(5.2, 0.0, 15.0, 0.0, Cat),
                ],
            },
            KfChannel {
                part: "head",
                target: KfTarget::Rot,
                frames: &[
                    kf(0.0, 11.7, 0.0, 0.0, Cat),
                    kf(2.6, 2.4, 0.0, 0.0, Cat),
                    kf(5.2, 11.7, 0.0, 0.0, Cat),
                ],
            },
            KfChannel {
                part: "left_gills",
                target: KfTarget::Rot,
                frames: &[
                    kf(0.0, 0.0, -25.6, 0.0, Cat),
                    kf(3.12, 0.0, 16.0, 0.0, Cat),
                    kf(5.2, 0.0, -25.6, 0.0, Cat),
                ],
            },
            KfChannel {
                part: "right_gills",
                target: KfTarget::Rot,
                frames: &[
                    kf(0.0, 0.0, 23.0, 0.0, Cat),
                    kf(2.08, 0.0, -26.0, 0.0, Cat),
                    kf(5.2, 0.0, 23.0, 0.0, Cat),
                ],
            },
            KfChannel {
                part: "top_gills",
                target: KfTarget::Rot,
                frames: &[
                    kf(0.0, -19.2, 0.0, 0.0, Cat),
                    kf(2.6, 12.3, 0.0, 0.0, Cat),
                    kf(5.2, -19.2, 0.0, 0.0, Cat),
                ],
            },
        ],
    }
};

static BABY_AXOLOTL_SWIM: KfAnim = {
    use Kfi::{Cat, Lin};
    KfAnim {
        length: 1.0,
        looping: true,
        channels: &[
            KfChannel {
                part: "body",
                target: KfTarget::Rot,
                frames: &[
                    kf(0.0, 8.0, 0.0, 0.0, Cat),
                    kf(0.24, 12.0, 0.0, 0.0, Cat),
                    kf(0.68, -7.5, 0.0, 0.0, Cat),
                    kf(1.0, 8.0, 0.0, 0.0, Lin),
                ],
            },
            KfChannel {
                part: "body",
                target: KfTarget::Pos,
                frames: &[
                    kf(0.0, 0.0, 0.0, 0.0, Cat),
                    kf(0.52, 0.0, -1.0, 0.0, Cat),
                    kf(0.96, 0.0, -0.05, 0.0, Cat),
                ],
            },
            KfChannel {
                part: "right_front_leg",
                target: KfTarget::Rot,
                frames: &[
                    kf(0.0, 310.0, 70.0, 400.0, Cat),
                    kf(0.24, 330.0, 70.0, 420.0, Cat),
                    kf(0.52, 290.0, 65.0, 384.0, Cat),
                    kf(0.76, 290.0, 70.0, 380.0, Cat),
                    kf(0.96, 310.0, 70.0, 400.0, Cat),
                ],
            },
            KfChannel {
                part: "right_hind_leg",
                target: KfTarget::Rot,
                frames: &[
                    kf(0.0, 105.0, -95.0, 180.0, Cat),
                    kf(0.28, 105.0, -85.0, 180.0, Cat),
                    kf(0.52, 105.0, -80.0, 180.0, Cat),
                    kf(0.76, 105.0, -95.0, 180.0, Cat),
                    kf(0.96, 105.0, -95.0, 180.0, Cat),
                ],
            },
            KfChannel {
                part: "left_front_leg",
                target: KfTarget::Rot,
                frames: &[
                    kf(0.0, -50.0, -70.0, -40.0, Cat),
                    kf(0.52, -60.0, -68.0, -30.0, Cat),
                    kf(0.76, -55.0, -70.0, -30.0, Cat),
                    kf(0.96, -50.0, -70.0, -40.0, Cat),
                ],
            },
            KfChannel {
                part: "left_hind_leg",
                target: KfTarget::Rot,
                frames: &[
                    kf(0.0, 70.0, -80.0, 15.0, Cat),
                    kf(0.28, 130.0, -80.0, -45.0, Cat),
                    kf(0.52, 120.0, -70.0, -35.0, Cat),
                    kf(0.76, 80.0, -70.0, 5.0, Cat),
                    kf(0.96, 70.0, -80.0, 15.0, Cat),
                ],
            },
            KfChannel {
                part: "tail",
                target: KfTarget::Rot,
                frames: &[
                    kf(0.0, 0.0, 15.0, 0.0, Cat),
                    kf(0.52, 0.0, -15.0, 0.0, Cat),
                    kf(0.96, 0.0, 13.36, 0.0, Cat),
                ],
            },
            KfChannel {
                part: "head",
                target: KfTarget::Rot,
                frames: &[
                    kf(0.0, -7.5, 0.0, 0.0, Cat),
                    kf(0.28, -13.9, 0.0, 0.0, Cat),
                    kf(0.72, 14.23, 0.0, 0.0, Cat),
                    kf(1.0, -7.5, 0.0, 0.0, Cat),
                ],
            },
            KfChannel {
                part: "left_gills",
                target: KfTarget::Rot,
                frames: &[
                    kf(0.0, 0.0, -72.0, 0.0, Cat),
                    kf(0.2, 0.0, -79.9, 0.0, Cat),
                    kf(0.64, 0.0, -38.1, 0.0, Cat),
                    kf(0.96, 0.0, -72.0, 0.0, Cat),
                ],
            },
            KfChannel {
                part: "right_gills",
                target: KfTarget::Rot,
                frames: &[
                    kf(0.0, 0.0, 72.0, 0.0, Cat),
                    kf(0.2, 0.0, 86.5, 0.0, Cat),
                    kf(0.64, 0.0, 26.7, 0.0, Cat),
                    kf(0.96, 0.0, 72.0, 0.0, Cat),
                ],
            },
            KfChannel {
                part: "top_gills",
                target: KfTarget::Rot,
                frames: &[
                    kf(0.0, -57.0, 0.0, 0.0, Cat),
                    kf(0.2, -68.7, 0.0, 0.0, Cat),
                    kf(0.64, -24.2, 0.0, 0.0, Cat),
                    kf(0.96, -57.0, 0.0, 0.0, Cat),
                ],
            },
        ],
    }
};

static AXOLOTL_WALK_FLOOR: KfAnim = {
    use Kfi::{Cat, Lin};
    KfAnim {
        length: 2.72,
        looping: true,
        channels: &[
            KfChannel {
                part: "body",
                target: KfTarget::Rot,
                frames: &[kf(0.0, 0.0, 0.0, 0.0, Lin), kf(2.72, 0.0, 0.0, 0.0, Lin)],
            },
            KfChannel {
                part: "right_front_leg",
                target: KfTarget::Rot,
                frames: &[
                    kf(0.0, 2.5, 10.0, -30.0, Cat),
                    kf(1.32, 2.5, -22.5, -30.0, Cat),
                    kf(2.72, 2.5, 10.0, -30.0, Cat),
                ],
            },
            KfChannel {
                part: "right_front_leg",
                target: KfTarget::Pos,
                frames: &[kf(1.92, 0.5, 0.0, 0.8, Cat)],
            },
            KfChannel {
                part: "right_hind_leg",
                target: KfTarget::Rot,
                frames: &[
                    kf(0.0, 260.0, -10.0, 230.0, Cat),
                    kf(1.32, 90.0, -20.0, 50.0, Cat),
                    kf(2.72, 260.0, -10.0, 230.0, Cat),
                ],
            },
            KfChannel {
                part: "right_hind_leg",
                target: KfTarget::Pos,
                frames: &[kf(0.0, 0.5, 0.0, 0.0, Cat), kf(2.72, 0.5, 0.0, 0.0, Cat)],
            },
            KfChannel {
                part: "left_front_leg",
                target: KfTarget::Rot,
                frames: &[
                    kf(0.0, -2.5, 32.5, 30.0, Cat),
                    kf(1.32, -2.5, -12.0, 30.0, Cat),
                    kf(2.72, -2.5, 32.5, 30.0, Cat),
                ],
            },
            KfChannel {
                part: "left_front_leg",
                target: KfTarget::Pos,
                frames: &[kf(0.0, -0.5, 0.0, 0.7, Cat)],
            },
            KfChannel {
                part: "left_hind_leg",
                target: KfTarget::Rot,
                frames: &[
                    kf(0.0, -2.5, -30.0, 30.0, Cat),
                    kf(1.32, 2.5, 12.0, 30.0, Cat),
                    kf(2.72, -2.5, -30.0, 30.0, Cat),
                ],
            },
            KfChannel {
                part: "left_hind_leg",
                target: KfTarget::Pos,
                frames: &[kf(0.0, -0.5, 0.0, 0.0, Cat), kf(2.72, -0.5, 0.0, 0.0, Cat)],
            },
            KfChannel {
                part: "tail",
                target: KfTarget::Rot,
                frames: &[
                    kf(0.0, 0.0, -10.0, 0.0, Cat),
                    kf(1.32, 0.0, 10.0, 0.0, Cat),
                    kf(2.72, 0.0, -10.0, 0.0, Cat),
                ],
            },
            KfChannel {
                part: "head",
                target: KfTarget::Rot,
                frames: &[
                    kf(0.0, 0.0, -7.5, 0.0, Cat),
                    kf(1.32, 0.0, 6.7, 0.0, Cat),
                    kf(2.72, 0.0, -7.5, 0.0, Cat),
                ],
            },
            KfChannel {
                part: "left_gills",
                target: KfTarget::Rot,
                frames: &[
                    kf(0.0, 0.0, 38.1, 0.0, Cat),
                    kf(1.08, 0.0, 45.6, 0.0, Cat),
                    kf(1.44, 0.0, 45.6, 0.0, Cat),
                    kf(2.72, 0.0, 38.1, 0.0, Cat),
                ],
            },
            KfChannel {
                part: "right_gills",
                target: KfTarget::Rot,
                frames: &[
                    kf(0.0, 0.0, -50.0, 0.0, Cat),
                    kf(1.08, 0.0, -59.0, 0.0, Cat),
                    kf(1.36, 0.0, -59.0, 0.0, Cat),
                    kf(2.72, 0.0, -50.0, 0.0, Cat),
                ],
            },
            KfChannel {
                part: "top_gills",
                target: KfTarget::Rot,
                frames: &[
                    kf(0.0, 33.0, 0.0, 0.0, Cat),
                    kf(1.08, 47.4, 0.0, 0.0, Cat),
                    kf(1.44, 47.4, 0.0, 0.0, Cat),
                    kf(2.72, 33.0, 0.0, 0.0, Cat),
                ],
            },
        ],
    }
};

static WALK_FLOOR_UNDERWATER: KfAnim = {
    use Kfi::Cat;
    KfAnim {
        length: 2.04,
        looping: true,
        channels: &[
            KfChannel {
                part: "body",
                target: KfTarget::Rot,
                frames: &[
                    kf(0.0, 0.0, -5.0, 6.0, Cat),
                    kf(1.04, 0.0, 5.0, -4.0, Cat),
                    kf(2.04, 0.0, -5.0, 6.0, Cat),
                ],
            },
            KfChannel {
                part: "body",
                target: KfTarget::Pos,
                frames: &[kf(0.0, 0.0, 1.5, 0.0, Cat), kf(2.04, 0.0, 1.5, 0.0, Cat)],
            },
            KfChannel {
                part: "right_front_leg",
                target: KfTarget::Rot,
                frames: &[
                    kf(0.0, 0.0, -15.0, -30.0, Cat),
                    kf(0.72, 0.0, 9.0, -30.0, Cat),
                    kf(1.12, 0.0, 13.0, -24.0, Cat),
                    kf(1.44, 0.0, 9.0, -30.0, Cat),
                    kf(2.04, 0.0, -15.0, -30.0, Cat),
                ],
            },
            KfChannel {
                part: "right_front_leg",
                target: KfTarget::Pos,
                frames: &[
                    kf(0.0, 0.0, 0.0, 0.0, Cat),
                    kf(0.96, 0.4, 0.0, 0.0, Cat),
                    kf(2.04, 0.0, 0.0, 0.0, Cat),
                ],
            },
            KfChannel {
                part: "right_hind_leg",
                target: KfTarget::Rot,
                frames: &[
                    kf(0.0, 95.0, -20.0, 70.0, Cat),
                    kf(0.68, 255.0, -20.0, 230.0, Cat),
                    kf(1.12, 280.0, -25.0, 255.0, Cat),
                    kf(1.52, 255.0, -20.0, 230.0, Cat),
                    kf(2.04, 95.0, -20.0, 70.0, Cat),
                ],
            },
            KfChannel {
                part: "right_hind_leg",
                target: KfTarget::Pos,
                frames: &[
                    kf(0.0, 0.5, 0.0, 0.0, Cat),
                    kf(0.96, 0.5, 0.0, 0.0, Cat),
                    kf(2.04, 0.5, 0.0, 0.0, Cat),
                ],
            },
            KfChannel {
                part: "left_front_leg",
                target: KfTarget::Rot,
                frames: &[
                    kf(0.0, -5.0, -20.0, 30.0, Cat),
                    kf(0.72, 0.0, 10.0, 25.0, Cat),
                    kf(1.44, 0.0, 10.0, 25.0, Cat),
                    kf(2.04, -5.0, -20.0, 30.0, Cat),
                ],
            },
            KfChannel {
                part: "left_front_leg",
                target: KfTarget::Pos,
                frames: &[
                    kf(0.0, -0.5, 0.0, 0.0, Cat),
                    kf(0.96, 0.0, 0.0, 0.0, Cat),
                    kf(2.04, -0.5, 0.0, 0.0, Cat),
                ],
            },
            KfChannel {
                part: "left_hind_leg",
                target: KfTarget::Rot,
                frames: &[
                    kf(0.0, 5.0, 15.0, 25.0, Cat),
                    kf(0.72, -10.0, -20.0, 35.0, Cat),
                    kf(1.44, -10.0, -20.0, 35.0, Cat),
                    kf(2.04, 5.0, 15.0, 25.0, Cat),
                ],
            },
            KfChannel {
                part: "left_hind_leg",
                target: KfTarget::Pos,
                frames: &[
                    kf(0.0, -0.7, 0.0, 0.0, Cat),
                    kf(0.96, -0.4, 0.0, 0.0, Cat),
                    kf(2.04, -0.7, 0.0, 0.0, Cat),
                ],
            },
            KfChannel {
                part: "tail",
                target: KfTarget::Rot,
                frames: &[
                    kf(0.0, 0.0, 16.5, 0.0, Cat),
                    kf(0.56, 0.0, 1.12, 0.0, Cat),
                    kf(1.44, 0.0, 17.5, 0.0, Cat),
                    kf(2.04, 0.0, 16.5, 0.0, Cat),
                ],
            },
            KfChannel {
                part: "head",
                target: KfTarget::Rot,
                frames: &[
                    kf(0.0, 0.0, 4.0, -5.8, Cat),
                    kf(1.04, 0.0, -4.0, 5.0, Cat),
                    kf(2.04, 0.0, 4.0, -5.8, Cat),
                ],
            },
            KfChannel {
                part: "left_gills",
                target: KfTarget::Rot,
                frames: &[
                    kf(0.0, 0.0, -60.0, 0.0, Cat),
                    kf(1.24, 0.0, -45.0, 0.0, Cat),
                    kf(2.04, 0.0, -60.0, 0.0, Cat),
                ],
            },
            KfChannel {
                part: "right_gills",
                target: KfTarget::Rot,
                frames: &[
                    kf(0.0, 0.0, 38.0, 0.0, Cat),
                    kf(0.64, 0.0, 53.0, 0.0, Cat),
                    kf(2.04, 0.0, 38.0, 0.0, Cat),
                ],
            },
            KfChannel {
                part: "top_gills",
                target: KfTarget::Rot,
                frames: &[
                    kf(0.0, -34.0, 0.0, 0.0, Cat),
                    kf(1.04, -41.5, 0.0, 0.0, Cat),
                    kf(2.04, -34.0, 0.0, 0.0, Cat),
                ],
            },
        ],
    }
};

static BABY_AXOLOTL_PLAY_DEAD: KfAnim = {
    use Kfi::{Cat, Lin};
    KfAnim {
        length: 0.04,
        looping: false,
        channels: &[
            KfChannel {
                part: "body",
                target: KfTarget::Rot,
                frames: &[kf(0.0, 0.0, 0.0, 30.0, Cat)],
            },
            KfChannel {
                part: "body",
                target: KfTarget::Pos,
                frames: &[kf(0.0, 0.0, 0.0, 0.0, Lin)],
            },
            KfChannel {
                part: "body",
                target: KfTarget::Scale,
                frames: &[kf(0.0, 0.0, 0.0, 0.0, Lin)],
            },
            KfChannel {
                part: "right_front_leg",
                target: KfTarget::Rot,
                frames: &[kf(0.0, 202.35, -30.33, -40.82, Cat)],
            },
            KfChannel {
                part: "right_front_leg",
                target: KfTarget::Pos,
                frames: &[kf(0.0, 0.2, 0.0, 0.0, Lin)],
            },
            KfChannel {
                part: "right_front_leg",
                target: KfTarget::Scale,
                frames: &[kf(0.0, 0.0, 0.0, 0.0, Lin)],
            },
            KfChannel {
                part: "right_hind_leg",
                target: KfTarget::Rot,
                frames: &[kf(0.0, 418.0, -50.0, 35.8, Cat)],
            },
            KfChannel {
                part: "right_hind_leg",
                target: KfTarget::Pos,
                frames: &[kf(0.0, 0.0, 0.0, 0.0, Lin)],
            },
            KfChannel {
                part: "right_hind_leg",
                target: KfTarget::Scale,
                frames: &[kf(0.0, 0.0, 0.0, 0.0, Lin)],
            },
            KfChannel {
                part: "left_front_leg",
                target: KfTarget::Rot,
                frames: &[kf(0.0, 177.0, 22.76, 15.9, Cat)],
            },
            KfChannel {
                part: "left_front_leg",
                target: KfTarget::Pos,
                frames: &[kf(0.0, -0.2, 0.0, 0.0, Lin)],
            },
            KfChannel {
                part: "left_front_leg",
                target: KfTarget::Scale,
                frames: &[kf(0.0, 0.0, 0.0, 0.0, Lin)],
            },
            KfChannel {
                part: "left_hind_leg",
                target: KfTarget::Rot,
                frames: &[kf(0.0, 16.4287, -37.6467, 16.9822, Cat)],
            },
            KfChannel {
                part: "left_hind_leg",
                target: KfTarget::Pos,
                frames: &[kf(0.0, 0.0, 0.0, 0.0, Lin)],
            },
            KfChannel {
                part: "left_hind_leg",
                target: KfTarget::Scale,
                frames: &[kf(0.0, 0.0, 0.0, 0.0, Lin)],
            },
            KfChannel {
                part: "tail",
                target: KfTarget::Rot,
                frames: &[kf(0.0, 0.0, -18.67, 0.0, Lin)],
            },
            KfChannel {
                part: "tail",
                target: KfTarget::Pos,
                frames: &[kf(0.0, 0.0, 0.0, 0.0, Lin)],
            },
            KfChannel {
                part: "tail",
                target: KfTarget::Scale,
                frames: &[kf(0.0, 0.0, 0.0, 0.0, Lin)],
            },
            KfChannel {
                part: "head",
                target: KfTarget::Rot,
                frames: &[kf(0.0, -4.21, -0.95, -6.33, Lin)],
            },
            KfChannel {
                part: "head",
                target: KfTarget::Pos,
                frames: &[kf(0.0, 0.0, 0.0, 0.0, Lin)],
            },
            KfChannel {
                part: "head",
                target: KfTarget::Scale,
                frames: &[kf(0.0, 0.0, 0.0, 0.0, Lin)],
            },
            KfChannel {
                part: "left_gills",
                target: KfTarget::Rot,
                frames: &[kf(0.0, 0.0, 43.21, 0.0, Cat)],
            },
            KfChannel {
                part: "left_gills",
                target: KfTarget::Pos,
                frames: &[kf(0.0, 0.0, 0.0, 0.0, Lin)],
            },
            KfChannel {
                part: "left_gills",
                target: KfTarget::Scale,
                frames: &[kf(0.0, 0.0, 0.0, 0.0, Lin)],
            },
            KfChannel {
                part: "right_gills",
                target: KfTarget::Rot,
                frames: &[kf(0.0, 0.0, -55.87, 0.0, Cat)],
            },
            KfChannel {
                part: "right_gills",
                target: KfTarget::Pos,
                frames: &[kf(0.0, 0.0, 0.0, 0.0, Lin)],
            },
            KfChannel {
                part: "right_gills",
                target: KfTarget::Scale,
                frames: &[kf(0.0, 0.0, 0.0, 0.0, Lin)],
            },
            KfChannel {
                part: "top_gills",
                target: KfTarget::Rot,
                frames: &[kf(0.0, 43.0, 0.0, 0.0, Cat)],
            },
            KfChannel {
                part: "top_gills",
                target: KfTarget::Pos,
                frames: &[kf(0.0, 0.0, 0.0, 0.0, Lin)],
            },
            KfChannel {
                part: "top_gills",
                target: KfTarget::Scale,
                frames: &[kf(0.0, 0.0, 0.0, 0.0, Lin)],
            },
            KfChannel {
                part: "root",
                target: KfTarget::Rot,
                frames: &[kf(0.0, 0.0, 0.0, 0.0, Lin)],
            },
            KfChannel {
                part: "root",
                target: KfTarget::Pos,
                frames: &[kf(0.0, 0.0, 0.0, 0.0, Lin)],
            },
            KfChannel {
                part: "root",
                target: KfTarget::Scale,
                frames: &[kf(0.0, 0.0, 0.0, 0.0, Lin)],
            },
        ],
    }
};
