//! Llama and trader llama meshes: vanilla `LlamaModel` and `BabyLlamaModel`.
//! Both models' `setupAnim` is the quadruped head look and leg swing
//! (`compute_quadruped_anim`) plus the chest toggle, which is a separate
//! bake here (an empty chest part draws nothing).

use std::f32::consts::FRAC_PI_2;

use glam::Vec3;

use super::{
    BakedEntityModel, EntityPart, bake_model, inflate, quadruped_legs, rotated, vbox, vpart,
};

/// Vanilla `LlamaModel.createBodyLayer(NONE)`, 128x64.
fn llama_parts(chest: bool) -> Vec<EntityPart> {
    let chest_part = |name: &str, x: f32, uv: (i32, i32)| {
        rotated(
            vpart(
                name,
                None,
                Vec3::new(x, 3.0, 3.0),
                if chest {
                    vec![vbox(uv, (-3.0, 0.0, 0.0), (8.0, 8.0, 3.0))]
                } else {
                    vec![]
                },
            ),
            Vec3::new(0.0, FRAC_PI_2, 0.0),
        )
    };
    let leg = vbox((29, 29), (-2.0, 0.0, -2.0), (4.0, 14.0, 4.0));
    let mut parts = vec![
        vpart(
            "head",
            None,
            Vec3::new(0.0, 7.0, -6.0),
            vec![
                vbox((0, 0), (-2.0, -14.0, -10.0), (4.0, 4.0, 9.0)),
                // neck
                vbox((0, 14), (-4.0, -16.0, -6.0), (8.0, 18.0, 6.0)),
                // ears
                vbox((17, 0), (-4.0, -19.0, -4.0), (3.0, 3.0, 2.0)),
                vbox((17, 0), (1.0, -19.0, -4.0), (3.0, 3.0, 2.0)),
            ],
        ),
        rotated(
            vpart(
                "body",
                None,
                Vec3::new(0.0, 5.0, 2.0),
                vec![vbox((29, 0), (-6.0, -10.0, -7.0), (12.0, 18.0, 10.0))],
            ),
            Vec3::new(FRAC_PI_2, 0.0, 0.0),
        ),
        chest_part("right_chest", -8.5, (45, 28)),
        chest_part("left_chest", 5.5, (45, 41)),
    ];
    parts.extend(quadruped_legs(3.5, 10.0, -5.0, 6.0, leg, leg));
    parts
}

/// Vanilla `BabyLlamaModel.createBodyLayer(NONE)`, 64x64. Its chest parts
/// are left out: `LlamaRenderer` clears `hasChest` for babies, so they never
/// show.
fn baby_llama_parts() -> Vec<EntityPart> {
    let leg = |name: &str, x: f32, z: f32, uv: (i32, i32), origin_x: f32| {
        vpart(
            name,
            None,
            Vec3::new(x, 16.5, z),
            vec![vbox(uv, (origin_x, -0.5, -1.5), (3.0, 8.0, 3.0))],
        )
    };
    vec![
        vpart(
            "head",
            None,
            Vec3::new(0.0, 12.0, -4.0),
            vec![
                vbox((0, 0), (-3.0, -9.0, -4.0), (6.0, 11.0, 4.0)),
                vbox((0, 15), (-1.5, -7.0, -7.0), (3.0, 3.0, 3.0)),
                vbox((20, 4), (0.5, -11.0, -3.0), (2.0, 2.0, 2.0)),
                vbox((20, 0), (-2.5, -11.0, -3.0), (2.0, 2.0, 2.0)),
            ],
        ),
        leg("right_hind_leg", -2.5, 4.5, (0, 45), -1.4),
        leg("left_hind_leg", 2.5, 4.5, (12, 45), -1.6),
        leg("right_front_leg", -2.5, -3.5, (0, 34), -1.4),
        leg("left_front_leg", 2.5, -3.5, (12, 34), -1.6),
        vpart(
            "body",
            None,
            Vec3::new(0.0, 14.0, 2.5),
            vec![vbox((0, 15), (-4.0, -3.0, -8.5), (8.0, 6.0, 13.0))],
        ),
    ]
}

pub fn bake_llama_model(chest: bool) -> BakedEntityModel {
    bake_model(llama_parts(chest), 128, 64)
}

/// Vanilla `LLAMA_DECOR` (`LlamaModel` at `CubeDeformation(0.5)`) for
/// `LlamaDecorLayer`, whose `setupAnim` shows its chests with the base's.
pub fn bake_llama_decor_model(chest: bool) -> BakedEntityModel {
    let mut parts = llama_parts(chest);
    inflate(&mut parts, 0.5);
    bake_model(parts, 128, 64)
}

pub fn bake_baby_llama_model() -> BakedEntityModel {
    bake_model(baby_llama_parts(), 64, 64)
}

/// Vanilla `LLAMA_BABY_DECOR` (`BabyLlamaModel` at `CubeDeformation(0.2)`).
pub fn bake_baby_llama_decor_model() -> BakedEntityModel {
    let mut parts = baby_llama_parts();
    inflate(&mut parts, 0.2);
    bake_model(parts, 64, 64)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `compute_quadruped_anim` drives these parts by name, and the chest
    /// toggle only empties the chest parts.
    #[test]
    fn llama_meshes_have_the_quadruped_parts() {
        for parts in [llama_parts(false), llama_parts(true), baby_llama_parts()] {
            for name in [
                "head",
                "body",
                "right_hind_leg",
                "left_hind_leg",
                "right_front_leg",
                "left_front_leg",
            ] {
                super::super::part_index(&parts, name);
            }
        }
        let chest_cubes = |parts: &[EntityPart]| {
            parts
                .iter()
                .filter(|p| p.name.ends_with("_chest"))
                .map(|p| p.cubes.len())
                .sum::<usize>()
        };
        assert_eq!(chest_cubes(&llama_parts(false)), 0);
        assert_eq!(chest_cubes(&llama_parts(true)), 2);
    }
}
