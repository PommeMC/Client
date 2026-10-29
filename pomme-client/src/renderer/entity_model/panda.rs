//! Panda: `PandaModel`/`BabyPandaModel` meshes, `PandaModel.setupAnim` (on
//! top of `QuadrupedModel`'s) and `PandaRenderer.setupRotations`.

// Vanilla's radian literals are kept verbatim, not swapped for `consts`.
#![allow(clippy::approx_constant)]

use glam::{Mat4, Vec3};

use super::{
    BakedEntityModel, ModelPose, PartAnim, bake_model, lerp, part_index, rot_lerp_rad, rotated,
    vbox, vpart, walk_swing,
};

/// Vanilla `PandaModel.createBodyLayer`, 64x64. The nose and ears are named
/// head cubes, not parts.
pub fn bake_panda_model() -> BakedEntityModel {
    let leg = |name, x: f32, z: f32| {
        vpart(
            name,
            None,
            Vec3::new(x, 15.0, z),
            vec![vbox((40, 0), (-3.0, 0.0, -3.0), (6.0, 9.0, 6.0))],
        )
    };
    let parts = vec![
        vpart(
            "head",
            None,
            Vec3::new(0.0, 11.5, -17.0),
            vec![
                vbox((0, 6), (-6.5, -5.0, -4.0), (13.0, 10.0, 9.0)),
                vbox((45, 16), (-3.5, 0.0, -6.0), (7.0, 5.0, 2.0)),
                vbox((52, 25), (3.5, -8.0, -1.0), (5.0, 4.0, 1.0)),
                vbox((52, 25), (-8.5, -8.0, -1.0), (5.0, 4.0, 1.0)),
            ],
        ),
        rotated(
            vpart(
                "body",
                None,
                Vec3::new(0.0, 10.0, 0.0),
                vec![vbox((0, 25), (-9.5, -13.0, -6.5), (19.0, 26.0, 13.0))],
            ),
            Vec3::new(1.5707964, 0.0, 0.0),
        ),
        leg("right_hind_leg", -5.5, 9.0),
        leg("left_hind_leg", 5.5, 9.0),
        leg("right_front_leg", -5.5, -9.0),
        leg("left_front_leg", 5.5, -9.0),
    ];
    bake_model(parts, 64, 64)
}

/// Vanilla `BabyPandaModel.createBodyLayer`, 64x64.
pub fn bake_baby_panda_model() -> BakedEntityModel {
    let leg = |name, tex, x: f32, z: f32| {
        vpart(
            name,
            None,
            Vec3::new(x, 22.0, z),
            vec![vbox(tex, (-1.5, 0.0, -1.5), (3.0, 2.0, 3.0))],
        )
    };
    let parts = vec![
        vpart(
            "body",
            None,
            Vec3::new(0.0, 18.5, 2.5),
            vec![vbox((0, 11), (-4.5, -3.5, -5.5), (9.0, 7.0, 11.0))],
        ),
        vpart(
            "head",
            None,
            Vec3::new(0.0, 19.0, -3.0),
            vec![
                vbox((0, 0), (-3.5, -3.0, -5.0), (7.0, 6.0, 5.0)),
                vbox((24, 6), (-2.0, 1.0, -6.0), (4.0, 2.0, 1.0)),
                vbox((24, 0), (-4.5, -4.0, -3.5), (3.0, 3.0, 1.0)),
                vbox((33, 0), (1.5, -4.0, -3.5), (3.0, 3.0, 1.0)),
            ],
        ),
        leg("right_hind_leg", (0, 34), -3.0, 6.5),
        leg("left_hind_leg", (12, 34), 3.0, 6.5),
        leg("right_front_leg", (0, 29), -3.0, -1.5),
        leg("left_front_leg", (12, 29), 3.0, -1.5),
    ];
    bake_model(parts, 64, 64)
}

/// The `PandaRenderState` fields `PandaModel.setupAnim` and
/// `PandaRenderer.setupRotations` read.
#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub struct PandaAnims {
    pub is_unhappy: bool,
    pub is_sneezing: bool,
    pub sneeze_time: i32,
    pub is_eating: bool,
    pub is_scared: bool,
    pub sit_amount: f32,
    pub lie_on_back_amount: f32,
    /// Always 0 for babies (`extractRenderState`).
    pub roll_amount: f32,
    /// `rollCounter + partialTicks` while rolling, else 0.
    pub roll_time: f32,
}

/// Vanilla `PandaModel.setupAnim` (`QuadrupedModel.setupAnim` first), with
/// `BabyPandaModel.animateSitting` for the baby.
#[allow(clippy::too_many_arguments)]
pub fn compute_panda_anim(
    model: &BakedEntityModel,
    is_baby: bool,
    head_x_rot_deg: f32,
    local_head_y_rot_deg: f32,
    walk_pos: f32,
    walk_speed: f32,
    age_in_ticks: f32,
    s: &PandaAnims,
) -> PartAnim {
    use std::f32::consts::FRAC_PI_2;
    let idx = |name| part_index(&model.parts, name);
    let (head, body) = (idx("head"), idx("body"));
    let (rh, lh, rf, lf) = (
        idx("right_hind_leg"),
        idx("left_hind_leg"),
        idx("right_front_leg"),
        idx("left_front_leg"),
    );
    let age = age_in_ticks;
    let mut m = ModelPose::new(model);

    // QuadrupedModel.setupAnim
    m.rot[head].x = head_x_rot_deg.to_radians();
    m.rot[head].y = local_head_y_rot_deg.to_radians();
    m.swing_legs([rh, lh, rf, lf], |phase| {
        walk_swing(walk_pos, walk_speed, phase)
    });

    if s.is_unhappy {
        m.rot[head].y = 0.35 * (0.6 * age).sin();
        m.rot[head].z = 0.35 * (0.6 * age).sin();
        m.rot[rf].x = -0.75 * (0.3 * age).sin();
        m.rot[lf].x = 0.75 * (0.3 * age).sin();
    } else {
        m.rot[head].z = 0.0;
    }
    if s.is_sneezing {
        if s.sneeze_time < 15 {
            m.rot[head].x = -0.7853982 * s.sneeze_time as f32 / 14.0;
        } else if s.sneeze_time < 20 {
            // Integer division in vanilla: 0 for the whole 15..19 span.
            let internal_sneeze_pos = ((s.sneeze_time - 15) / 5) as f32;
            m.rot[head].x = -0.7853982 + 0.7853982 * internal_sneeze_pos;
        }
    }
    if s.sit_amount > 0.0 {
        let sit = s.sit_amount;
        if is_baby {
            m.rot[body].x = rot_lerp_rad(sit, m.rot[body].x, 0.17453292);
            m.pos[body].z = lerp(sit, m.pos[body].z, -1.5);
            m.pos[head].z = lerp(sit, m.pos[head].z, -11.5);
            m.pos[head].y = lerp(sit, m.pos[head].y, 17.5);
            m.pos[rf].z = lerp(sit, m.pos[rf].z, -5.0);
            m.pos[lf].z = lerp(sit, m.pos[lf].z, -5.0);
            m.pos[rh].z = lerp(sit, m.pos[rh].z, 3.0);
            m.pos[lh].z = lerp(sit, m.pos[lh].z, 3.0);
        } else {
            m.rot[body].x = rot_lerp_rad(sit, m.rot[body].x, 1.7407963);
            m.rot[head].x = rot_lerp_rad(sit, m.rot[head].x, FRAC_PI_2);
        }
        m.rot[rf].z = -0.27079642;
        m.rot[lf].z = 0.27079642;
        m.rot[rh].z = 0.5707964;
        m.rot[lh].z = -0.5707964;
        if s.is_eating {
            m.rot[head].x = FRAC_PI_2 + 0.2 * (age * 0.6).sin();
            m.rot[rf].x = -0.4 - 0.2 * (age * 0.6).sin();
            m.rot[lf].x = -0.4 - 0.2 * (age * 0.6).sin();
        }
        if s.is_scared {
            m.rot[head].x = 2.1707964;
            m.rot[rf].x = -0.9;
            m.rot[lf].x = -0.9;
        }
    } else {
        m.rot[rh].z = 0.0;
        m.rot[lh].z = 0.0;
        m.rot[rf].z = 0.0;
        m.rot[lf].z = 0.0;
    }
    if s.lie_on_back_amount > 0.0 {
        m.rot[rh].x = -0.6 * (age * 0.15).sin();
        m.rot[lh].x = 0.6 * (age * 0.15).sin();
        m.rot[rf].x = 0.3 * (age * 0.25).sin();
        m.rot[lf].x = -0.3 * (age * 0.25).sin();
        m.rot[head].x = rot_lerp_rad(s.lie_on_back_amount, m.rot[head].x, FRAC_PI_2);
    }
    if s.roll_amount > 0.0 {
        m.rot[head].x = rot_lerp_rad(s.roll_amount, m.rot[head].x, 2.0561945);
        m.rot[rh].x = -0.5 * (age * 0.5).sin();
        m.rot[lh].x = 0.5 * (age * 0.5).sin();
        m.rot[rf].x = 0.5 * (age * 0.5).sin();
        m.rot[lf].x = -0.5 * (age * 0.5).sin();
    }
    m.emit(model)
}

/// `PandaRenderer.getAngle`.
fn roll_angle(this: f32, next: f32, next_roll_pos: i32, transition: f32, threshold: f32) -> f32 {
    if (next_roll_pos as f32) < threshold {
        lerp(transition, this, next)
    } else {
        this
    }
}

/// The `PandaRenderer.setupRotations` additions after the living-entity
/// ones: the forward roll, the sit-up pitch (and scared shake) and the
/// lie-on-back flip. `x_rot_deg` is the render state's `xRot`. `None` when
/// none applies.
pub fn panda_body_transform(
    s: &PandaAnims,
    is_baby: bool,
    x_rot_deg: f32,
    age_in_ticks: f32,
) -> Option<Mat4> {
    let translate = |y: f32, z: f32| Mat4::from_translation(Vec3::new(0.0, y, z));
    let rot_x = |deg: f32| Mat4::from_rotation_x(deg.to_radians());
    let mut m = Mat4::IDENTITY;
    let mut any = false;
    if s.roll_time > 0.0 {
        let transition = s.roll_time - s.roll_time.floor();
        let roll_pos = s.roll_time.floor() as i32;
        let next_roll_pos = roll_pos + 1;
        let y = if is_baby { 0.3 } else { 0.8 };
        let pos = roll_pos as f32;
        let next = next_roll_pos as f32;
        let step = if pos < 8.0 {
            let angle = roll_angle(
                90.0 * pos / 7.0,
                90.0 * next / 7.0,
                next_roll_pos,
                transition,
                8.0,
            );
            Some(((y + 0.2) * (angle / 90.0), angle))
        } else if pos < 16.0 {
            let internal = (pos - 8.0) / 7.0;
            let angle = roll_angle(
                90.0 + 90.0 * internal,
                90.0 + 90.0 * (next - 8.0) / 7.0,
                next_roll_pos,
                transition,
                16.0,
            );
            Some((y + 0.2 + (y - 0.2) * (angle - 90.0) / 90.0, angle))
        } else if pos < 24.0 {
            let internal = (pos - 16.0) / 7.0;
            let angle = roll_angle(
                180.0 + 90.0 * internal,
                180.0 + 90.0 * (next - 16.0) / 7.0,
                next_roll_pos,
                transition,
                24.0,
            );
            Some((y + y * (270.0 - angle) / 90.0, angle))
        } else if roll_pos < 32 {
            let internal = (pos - 24.0) / 7.0;
            let angle = roll_angle(
                270.0 + 90.0 * internal,
                270.0 + 90.0 * (next - 24.0) / 7.0,
                next_roll_pos,
                transition,
                32.0,
            );
            Some((y * ((360.0 - angle) / 90.0), angle))
        } else {
            None
        };
        if let Some((ty, angle)) = step {
            m = m * translate(ty, 0.0) * rot_x(-angle);
            any = true;
        }
    }
    let sit = s.sit_amount;
    if sit > 0.0 {
        m = m
            * translate(0.8 * sit, 0.0)
            * rot_x(lerp(sit, x_rot_deg, x_rot_deg + 90.0))
            * translate(-sit, 0.0);
        if s.is_scared {
            let shake_rot = (f64::from(age_in_ticks * 1.25).cos()
                * std::f64::consts::PI
                * f64::from(0.05f32)) as f32;
            m *= Mat4::from_rotation_y(shake_rot.to_radians());
            if is_baby {
                m *= translate(0.8, 0.55);
            }
        }
        any = true;
    }
    let lie = s.lie_on_back_amount;
    if lie > 0.0 {
        let y = if is_baby { 0.5 } else { 1.3 };
        m = m * translate(y * lie, 0.0) * rot_x(lerp(lie, x_rot_deg, x_rot_deg + 180.0));
        any = true;
    }
    any.then_some(m)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The sneeze ramps the head down over 14 ticks, then (vanilla's integer
    /// division) holds it fully down until the flag clears.
    #[test]
    fn sneeze_pitches_the_head() {
        let model = bake_panda_model();
        let pitch = |t| {
            let s = PandaAnims {
                is_sneezing: true,
                sneeze_time: t,
                ..Default::default()
            };
            let anim = compute_panda_anim(&model, false, 0.0, 0.0, 0.0, 0.0, 0.0, &s);
            anim.rotation_of(&model, "head").x
        };
        assert_eq!(pitch(7), -0.7853982 * 7.0 / 14.0);
        assert_eq!(pitch(18), -0.7853982);
        assert_eq!(pitch(20), 0.0);
    }

    /// Full sit: body pitched to 1.7407963, both rest rotations replaced.
    #[test]
    fn sitting_pitches_the_body_and_spreads_the_legs() {
        let model = bake_panda_model();
        let s = PandaAnims {
            sit_amount: 1.0,
            ..Default::default()
        };
        let anim = compute_panda_anim(&model, false, 0.0, 0.0, 0.0, 0.0, 0.0, &s);
        let rot = |name| anim.rotation_of(&model, name);
        assert!((rot("body").x - 1.7407963).abs() < 1e-6);
        assert_eq!(rot("right_hind_leg").z, 0.5707964);
        assert!(panda_body_transform(&s, false, 0.0, 0.0).is_some());
        assert!(panda_body_transform(&PandaAnims::default(), false, 0.0, 0.0).is_none());
    }
}
