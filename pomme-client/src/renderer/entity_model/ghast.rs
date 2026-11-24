//! Ghast and happy ghast: `GhastModel`, `HappyGhastModel` (also the ropes
//! layer's mesh) and `HappyGhastHarnessModel`, with their `setupAnim`s.

// Vanilla's radian literals are kept verbatim, not swapped for `consts`.
#![allow(clippy::approx_constant)]

use glam::Vec3;

use super::{BakedEntityModel, PartAnim, bake_root_scaled, deformed, find_part, vbox, vpart};
use crate::physics::movement::mth_sin;
use crate::util::JavaRandom;

/// `PartNames.tentacle(i)`.
const TENTACLES: [&str; 9] = [
    "tentacle0",
    "tentacle1",
    "tentacle2",
    "tentacle3",
    "tentacle4",
    "tentacle5",
    "tentacle6",
    "tentacle7",
    "tentacle8",
];

const HAPPY_GHAST_SCALE: f32 = 4.0;
/// `HappyGhastModel.BABY_TRANSFORMER`; `MeshTransformer.scaling`s compose
/// into one of their product.
const BABY_TRANSFORMER: f32 = 0.2375;

/// Vanilla `GhastModel.createBodyLayer`, 64x32, scaled 4.5. The tentacle
/// lengths come from a `RandomSource` seeded 1660.
pub fn bake_ghast_model() -> BakedEntityModel {
    let mut parts = vec![vpart(
        "body",
        None,
        Vec3::new(0.0, 17.6, 0.0),
        vec![vbox((0, 0), (-8.0, -8.0, -8.0), (16.0, 16.0, 16.0))],
    )];
    let mut random = JavaRandom::new(1660);
    for (i, name) in (0..).zip(TENTACLES) {
        let xo = (((i % 3) as f32 - (i / 3 % 2) as f32 * 0.5 + 0.25) / 2.0 * 2.0 - 1.0) * 5.0;
        let yo = ((i / 3) as f32 / 2.0 * 2.0 - 1.0) * 5.0;
        let len = (random.next_int(7) + 8) as f32;
        parts.push(vpart(
            name,
            None,
            Vec3::new(xo, 24.6, yo),
            vec![vbox((0, 0), (-1.0, 0.0, -1.0), (2.0, len, 2.0))],
        ));
    }
    bake_root_scaled(parts, 4.5, 64, 32)
}

/// Vanilla `HappyGhastModel.createBodyLayer(isBaby, deformation)`, 64x64:
/// the tentacles hang off the body, and the baby adds an inner body. `g` is
/// the ropes layer's 0.2 `CubeDeformation`, 0 for the body itself.
fn happy_ghast_model(baby: bool, g: f32) -> BakedEntityModel {
    let mut parts = vec![vpart(
        "body",
        None,
        Vec3::new(0.0, 16.0, 0.0),
        vec![deformed(
            vbox((0, 0), (-8.0, -8.0, -8.0), (16.0, 16.0, 16.0)),
            g,
        )],
    )];
    if baby {
        parts.push(vpart(
            "inner_body",
            Some(0),
            Vec3::new(0.0, 8.0, 0.0),
            vec![deformed(
                vbox((0, 32), (-8.0, -16.0, -8.0), (16.0, 16.0, 16.0)),
                g - 0.5,
            )],
        ));
    }
    // (x, z, length) per tentacle.
    const TENTACLE_POSES: [(f32, f32, f32); 9] = [
        (-3.75, -5.0, 5.0),
        (1.25, -5.0, 7.0),
        (6.25, -5.0, 4.0),
        (-6.25, 0.0, 5.0),
        (-1.25, 0.0, 5.0),
        (3.75, 0.0, 7.0),
        (-3.75, 5.0, 8.0),
        (1.25, 5.0, 8.0),
        (6.25, 5.0, 5.0),
    ];
    for (name, (x, z, len)) in TENTACLES.into_iter().zip(TENTACLE_POSES) {
        parts.push(vpart(
            name,
            Some(0),
            Vec3::new(x, 7.0, z),
            vec![deformed(
                vbox((0, 0), (-1.0, 0.0, -1.0), (2.0, len, 2.0)),
                g,
            )],
        ));
    }
    let scale = HAPPY_GHAST_SCALE * if baby { BABY_TRANSFORMER } else { 1.0 };
    bake_root_scaled(parts, scale, 64, 64)
}

pub fn bake_happy_ghast_model() -> BakedEntityModel {
    happy_ghast_model(false, 0.0)
}

pub fn bake_baby_happy_ghast_model() -> BakedEntityModel {
    happy_ghast_model(true, 0.0)
}

pub fn bake_happy_ghast_ropes_model(baby: bool) -> BakedEntityModel {
    happy_ghast_model(baby, 0.2)
}

/// Vanilla `HappyGhastHarnessModel.createHarnessLayer(baby)`, 64x64.
/// `ModelLayers.HAPPY_GHAST_BABY_HARNESS` applies `BABY_TRANSFORMER` again
/// on top of the one `createHarnessLayer(true)` already applied, so the
/// baby's harness shrinks twice and sits hidden inside its body.
pub fn bake_happy_ghast_harness_model(baby: bool) -> BakedEntityModel {
    let parts = vec![
        vpart(
            "harness",
            None,
            Vec3::new(0.0, 24.0, 0.0),
            vec![vbox((0, 0), (-8.0, -16.0, -8.0), (16.0, 16.0, 16.0))],
        ),
        vpart(
            "goggles",
            None,
            Vec3::new(0.0, 14.0, -5.5),
            vec![deformed(
                vbox((0, 32), (-8.0, -2.5, -2.5), (16.0, 5.0, 5.0)),
                0.15,
            )],
        ),
    ];
    let baby_scale = BABY_TRANSFORMER * BABY_TRANSFORMER;
    let scale = HAPPY_GHAST_SCALE * if baby { baby_scale } else { 1.0 };
    bake_root_scaled(parts, scale, 64, 64)
}

/// `HappyGhastRenderState`'s fields the models read.
#[derive(Clone, Copy, Default)]
pub struct HappyGhastAnims {
    /// `!bodyItem.isEmpty()`.
    pub has_body_item: bool,
    pub is_ridden: bool,
}

/// Vanilla `GhastModel.animateTentacles`, plus `HappyGhastModel.setupAnim`'s
/// body squeeze under a body item and `HappyGhastHarnessModel.setupAnim`'s
/// goggles, on whichever of those parts `model` has.
pub fn compute_ghast_anim(
    model: &BakedEntityModel,
    age_in_ticks: f32,
    happy: &HappyGhastAnims,
) -> PartAnim {
    let mut anim = PartAnim::default();
    for (i, name) in TENTACLES.into_iter().enumerate() {
        if let Some(idx) = find_part(&model.parts, name) {
            anim.rotation_mut(model, idx).x = 0.2 * mth_sin(age_in_ticks * 0.3 + i as f32) + 0.4;
        }
    }
    if happy.has_body_item
        && let Some(body) = find_part(&model.parts, "body")
    {
        anim.scale.push((body, Vec3::splat(0.9375)));
    }
    if let Some(goggles) = find_part(&model.parts, "goggles")
        && !happy.is_ridden
    {
        anim.rotation_mut(model, goggles).x = -0.7854;
        // y 9 against the rest pose's 14.
        anim.translation.push((goggles, Vec3::new(0.0, -5.0, 0.0)));
    }
    anim
}

#[cfg(test)]
mod tests {
    use super::super::EntityPart;
    use super::*;

    fn tentacle(model: &BakedEntityModel, i: usize) -> &EntityPart {
        &model.parts[find_part(&model.parts, TENTACLES[i]).unwrap()]
    }

    /// The lengths `new SingleThreadedRandomSource(1660)` draws, checked
    /// against the game's own `nextInt(7) + 8` sequence.
    #[test]
    fn ghast_tentacle_lengths_follow_the_seeded_random() {
        let model = bake_ghast_model();
        let lens: Vec<f32> = (0..9)
            .map(|i| tentacle(&model, i).cubes[0].size.y)
            .collect();
        assert_eq!(lens, [8.0, 13.0, 9.0, 11.0, 11.0, 10.0, 12.0, 9.0, 12.0]);
    }

    #[test]
    fn ghast_tentacles_sit_on_a_staggered_grid() {
        let model = bake_ghast_model();
        assert_eq!(tentacle(&model, 0).offset, Vec3::new(-3.75, 24.6, -5.0));
        assert_eq!(tentacle(&model, 4).offset, Vec3::new(-1.25, 24.6, 0.0));
        assert_eq!(tentacle(&model, 8).offset, Vec3::new(6.25, 24.6, 5.0));
    }

    #[test]
    fn a_body_item_squeezes_the_happy_ghast_and_a_rider_lowers_its_goggles() {
        let body = bake_happy_ghast_model();
        let worn = HappyGhastAnims {
            has_body_item: true,
            is_ridden: false,
        };
        let anim = compute_ghast_anim(&body, 0.0, &worn);
        let body_idx = find_part(&body.parts, "body").unwrap();
        assert_eq!(anim.scale, vec![(body_idx, Vec3::splat(0.9375))]);
        assert!(
            compute_ghast_anim(&body, 0.0, &HappyGhastAnims::default())
                .scale
                .is_empty()
        );

        let harness = bake_happy_ghast_harness_model(false);
        let idle = compute_ghast_anim(&harness, 0.0, &worn);
        assert_eq!(idle.rotation_of(&harness, "goggles").x, -0.7854);
        assert_eq!(idle.translation_of(&harness, "goggles").y, -5.0);
        let ridden = HappyGhastAnims {
            is_ridden: true,
            ..worn
        };
        let ridden = compute_ghast_anim(&harness, 0.0, &ridden);
        assert_eq!(ridden.rotation_of(&harness, "goggles"), Vec3::ZERO);
        assert_eq!(ridden.translation_of(&harness, "goggles"), Vec3::ZERO);
    }

    #[test]
    fn tentacles_swing_with_age() {
        let model = bake_ghast_model();
        let anim = compute_ghast_anim(&model, 10.0, &HappyGhastAnims::default());
        let x = anim.rotation_of(&model, "tentacle2").x;
        assert_eq!(x, 0.2 * mth_sin(3.0 + 2.0) + 0.4);
    }
}
