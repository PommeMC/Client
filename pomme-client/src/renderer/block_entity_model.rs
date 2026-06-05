use glam::Vec3;

use super::entity_model::{BakedEntityModel, EntityPart, ModelCube, bake_model};

/// Shulker box, closed state. Matches vanilla `ShulkerModel`: a 16x12x16 lid
/// stacked on a 16x8x16 base, with the lid's bottom flush against the base's
/// top. Texture is 64x64 `entity/shulker/shulker_<color>.png`.
pub fn bake_shulker_box_model() -> BakedEntityModel {
    let base = EntityPart {
        name: "base".into(),
        offset: Vec3::new(0.0, 8.0, 0.0),
        default_rotation: Vec3::ZERO,
        cubes: vec![ModelCube {
            origin: Vec3::new(-8.0, 8.0, -8.0),
            size: Vec3::new(16.0, 8.0, 16.0),
            tex_offset: (0, 28),
            deformation: 0.0,
            mirror: false,
        }],
        parent: None,
    };
    let lid = EntityPart {
        name: "lid".into(),
        offset: Vec3::new(0.0, 24.0, 0.0),
        default_rotation: Vec3::ZERO,
        cubes: vec![ModelCube {
            origin: Vec3::new(-8.0, -16.0, -8.0),
            size: Vec3::new(16.0, 12.0, 16.0),
            tex_offset: (0, 0),
            deformation: 0.0,
            mirror: false,
        }],
        parent: None,
    };
    bake_model(vec![lid, base], 64, 64)
}

/// Standing sign, matching vanilla `SignRenderer`/`SignModel`: 24x12x2 board
/// raised above ground, with a 2x14x2 post hanging from its center.
/// Texture is 64x32 `entity/signs/<wood>.png`.
pub fn bake_sign_model() -> BakedEntityModel {
    let board = EntityPart {
        name: "sign".into(),
        offset: Vec3::new(0.0, 24.0, 0.0),
        default_rotation: Vec3::ZERO,
        cubes: vec![ModelCube {
            origin: Vec3::new(-12.0, -14.0, -1.0),
            size: Vec3::new(24.0, 12.0, 2.0),
            tex_offset: (0, 0),
            deformation: 0.0,
            mirror: false,
        }],
        parent: None,
    };
    let stick = EntityPart {
        name: "stick".into(),
        offset: Vec3::new(0.0, 24.0, 0.0),
        default_rotation: Vec3::ZERO,
        cubes: vec![ModelCube {
            origin: Vec3::new(-1.0, -2.0, -1.0),
            size: Vec3::new(2.0, 14.0, 2.0),
            tex_offset: (0, 14),
            deformation: 0.0,
            mirror: false,
        }],
        parent: None,
    };
    bake_model(vec![board, stick], 64, 32)
}

/// Single-chest model, matching vanilla `ChestRenderer` geometry.
/// Texture is 64x64 `entity/chest/normal.png`. Closed (lid not rotated).
pub fn bake_chest_model() -> BakedEntityModel {
    let lid = EntityPart {
        name: "lid".into(),
        offset: Vec3::new(0.0, 9.0, 1.0),
        default_rotation: Vec3::ZERO,
        cubes: vec![
            ModelCube {
                origin: Vec3::new(-7.0, 0.0, -15.0),
                size: Vec3::new(14.0, 5.0, 14.0),
                tex_offset: (0, 0),
                deformation: 0.0,
                mirror: false,
            },
            ModelCube {
                origin: Vec3::new(-1.0, -2.0, -16.0),
                size: Vec3::new(2.0, 4.0, 1.0),
                tex_offset: (0, 0),
                deformation: 0.0,
                mirror: false,
            },
        ],
        parent: None,
    };
    let bottom = EntityPart {
        name: "bottom".into(),
        offset: Vec3::new(0.0, 0.0, 0.0),
        default_rotation: Vec3::ZERO,
        cubes: vec![ModelCube {
            origin: Vec3::new(-7.0, 0.0, -7.0),
            size: Vec3::new(14.0, 10.0, 14.0),
            tex_offset: (0, 19),
            deformation: 0.0,
            mirror: false,
        }],
        parent: None,
    };
    bake_model(vec![lid, bottom], 64, 64)
}
