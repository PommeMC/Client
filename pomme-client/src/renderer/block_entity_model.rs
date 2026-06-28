use glam::Vec3;

use super::entity_model::{
    BakedEntityModel, EntityPart, ModelCube, bake_model, generate_cube_vertices_faces,
};

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

/// Standing sign, matching vanilla `block/template_sign_rot_0`: a 16x8x1.33
/// board (one block wide, centered) raised on a 1.33x9.33x1.33 post. Geometry
/// and UVs are in block-model units (16 = one block); UVs are in 0-16 space so
/// the model bakes against a 16x16 reference even though the texture
/// (`block/<wood>_sign.png`) is 32x32. Face order: -Z, +Z, +Y, -Y, -X, +X.
pub fn bake_sign_model() -> BakedEntityModel {
    const BOARD_UVS: [[f32; 4]; 6] = [
        [0.0, 8.0, 12.0, 14.0],  // -Z (back)
        [0.0, 1.0, 12.0, 7.0],   // +Z (front)
        [0.0, 0.0, 12.0, 1.0],   // +Y (top)
        [0.0, 14.0, 12.0, 15.0], // -Y (bottom)
        [12.0, 8.0, 13.0, 14.0], // -X
        [12.0, 1.0, 13.0, 7.0],  // +X
    ];
    // The post's top is hidden under the board, so its +Y face reuses the
    // bottom rect rather than claiming texture vanilla never assigns it.
    const POST_UVS: [[f32; 4]; 6] = [
        [14.0, 8.0, 15.0, 15.0],  // -Z
        [14.0, 0.0, 15.0, 7.0],   // +Z
        [14.0, 15.0, 15.0, 16.0], // +Y (hidden)
        [14.0, 15.0, 15.0, 16.0], // -Y
        [15.0, 8.0, 16.0, 15.0],  // -X
        [15.0, 0.0, 16.0, 7.0],   // +X
    ];

    let board = ModelCube {
        origin: Vec3::new(-8.0, -52.0 / 3.0, -2.0 / 3.0),
        size: Vec3::new(16.0, 8.0, 4.0 / 3.0),
        tex_offset: (0, 0),
        deformation: 0.0,
        mirror: false,
    };
    let post = ModelCube {
        origin: Vec3::new(-2.0 / 3.0, -28.0 / 3.0, -2.0 / 3.0),
        size: Vec3::new(4.0 / 3.0, 28.0 / 3.0, 4.0 / 3.0),
        tex_offset: (0, 0),
        deformation: 0.0,
        mirror: false,
    };

    let mut vertices = Vec::new();
    let mut part_ranges = Vec::new();
    let mut parts = Vec::new();
    for (name, cube, uvs) in [("sign", board, &BOARD_UVS), ("stick", post, &POST_UVS)] {
        let start = vertices.len() as u32;
        generate_cube_vertices_faces(&cube, uvs, 16, 16, &mut vertices);
        part_ranges.push((start, vertices.len() as u32 - start));
        parts.push(EntityPart {
            name: name.into(),
            offset: Vec3::new(0.0, 24.0, 0.0),
            default_rotation: Vec3::ZERO,
            // Vertices were emitted above with explicit UVs, so no cubes to bake.
            cubes: Vec::new(),
            parent: None,
        });
    }
    BakedEntityModel::new(parts, vertices, part_ranges)
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
