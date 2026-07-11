use glam::Vec3;

use super::chunk::mesher::{ChunkVertex, PACKED_WHITE_SHIFTED, pack_light_tint, pack_uv};
use super::entity_model::{
    BakedEntityModel, EntityPart, ModelConvention, ModelCube, bake_model,
    generate_cube_vertices_faces,
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

const FACE_DOWN: u8 = 1 << 0;
const FACE_UP: u8 = 1 << 1;
const FACE_WEST: u8 = 1 << 2;
const FACE_NORTH: u8 = 1 << 3;
const FACE_EAST: u8 = 1 << 4;
const FACE_SOUTH: u8 = 1 << 5;
const FACE_ALL: u8 = 0x3F;

/// Emit one vanilla `ModelPart.Cube` in literal y-up part-local space with the
/// exact vanilla box unwrap (unlike `generate_cube_vertices`, which negates Y
/// and lays UVs out for the entity convention). `origin`/`size` are in model
/// pixels; `faces` masks which quads are emitted (double-chest halves cull the
/// seam face).
fn emit_vanilla_cube(
    origin: Vec3,
    size: Vec3,
    tex_offset: (u32, u32),
    tex_w: u32,
    tex_h: u32,
    faces: u8,
    vertices: &mut Vec<ChunkVertex>,
) {
    let (w, h, d) = (size.x, size.y, size.z);
    let (x0, y0, z0) = (origin.x, origin.y, origin.z);
    let (x1, y1, z1) = (x0 + w, y0 + h, z0 + d);

    let t0 = [x0, y0, z0];
    let t1 = [x1, y0, z0];
    let t2 = [x1, y1, z0];
    let t3 = [x0, y1, z0];
    let l0 = [x0, y0, z1];
    let l1 = [x1, y0, z1];
    let l2 = [x1, y1, z1];
    let l3 = [x0, y1, z1];

    let u0 = tex_offset.0 as f32;
    let v0 = tex_offset.1 as f32;
    let u1 = u0 + d;
    let u2 = u1 + w;
    let u22 = u2 + w;
    let u3 = u2 + d;
    let u4 = u3 + w;
    let v1 = v0 + d;
    let v2 = v1 + h;

    // Vertex order and per-corner UVs match vanilla's Cube constructor; the UP
    // face's v runs reversed there too.
    let quads = [
        (
            FACE_DOWN,
            [(l1, u2, v0), (l0, u1, v0), (t0, u1, v1), (t1, u2, v1)],
        ),
        (
            FACE_UP,
            [(t2, u22, v1), (t3, u2, v1), (l3, u2, v0), (l2, u22, v0)],
        ),
        (
            FACE_WEST,
            [(t0, u1, v1), (l0, u0, v1), (l3, u0, v2), (t3, u1, v2)],
        ),
        (
            FACE_NORTH,
            [(t1, u2, v1), (t0, u1, v1), (t3, u1, v2), (t2, u2, v2)],
        ),
        (
            FACE_EAST,
            [(l1, u3, v1), (t1, u2, v1), (t2, u2, v2), (l2, u3, v2)],
        ),
        (
            FACE_SOUTH,
            [(l0, u4, v1), (l1, u3, v1), (l2, u3, v2), (l3, u4, v2)],
        ),
    ];

    for (face, corners) in quads {
        if faces & face == 0 {
            continue;
        }
        for &i in &[0usize, 1, 2, 0, 2, 3] {
            let (pos, u, v) = corners[i];
            vertices.push(ChunkVertex {
                position: [pos[0] / 16.0, pos[1] / 16.0, pos[2] / 16.0],
                tex_coords: pack_uv(u / tex_w as f32, v / tex_h as f32),
                // TODO: full-bright; vanilla samples the lightmap at the block
                // (pending lighting support in the entity pipeline).
                light_tint: pack_light_tint(1.0, PACKED_WHITE_SHIFTED),
            });
        }
    }
}

/// One chest layer as parts [bottom, lid, lock], matching vanilla `ChestModel`
/// (single/double-left/double-right differ only in body/lock x extents and the
/// culled seam face). Texture 64x64; lid and lock pivot at offset (0, 9, 1).
fn bake_chest_layer(
    body_x0: f32,
    body_w: f32,
    lock_x0: f32,
    lock_w: f32,
    faces: u8,
) -> BakedEntityModel {
    let cubes = [
        (
            "bottom",
            Vec3::ZERO,
            Vec3::new(body_x0, 0.0, 1.0),
            Vec3::new(body_w, 10.0, 14.0),
            (0, 19),
        ),
        (
            "lid",
            Vec3::new(0.0, 9.0, 1.0),
            Vec3::new(body_x0, 0.0, 0.0),
            Vec3::new(body_w, 5.0, 14.0),
            (0, 0),
        ),
        (
            "lock",
            Vec3::new(0.0, 9.0, 1.0),
            Vec3::new(lock_x0, -2.0, 14.0),
            Vec3::new(lock_w, 4.0, 1.0),
            (0, 0),
        ),
    ];

    let mut vertices = Vec::new();
    let mut part_ranges = Vec::new();
    let mut parts = Vec::new();
    for (name, offset, origin, size, tex_offset) in cubes {
        let start = vertices.len() as u32;
        emit_vanilla_cube(origin, size, tex_offset, 64, 64, faces, &mut vertices);
        part_ranges.push((start, vertices.len() as u32 - start));
        parts.push(EntityPart {
            name: name.into(),
            offset,
            default_rotation: Vec3::ZERO,
            cubes: Vec::new(),
            parent: None,
        });
    }
    BakedEntityModel::new(parts, vertices, part_ranges).with_convention(ModelConvention::BlockYUp)
}

/// Chest models in variant order [single, double-left, double-right], from
/// vanilla `ChestModel::createSingleBodyLayer` / `createDoubleBodyLeftLayer` /
/// `createDoubleBodyRightLayer`.
// TODO: copper chest variants (26.2) are not rendered yet.
pub fn bake_chest_models() -> Vec<BakedEntityModel> {
    vec![
        bake_chest_layer(1.0, 14.0, 7.0, 2.0, FACE_ALL),
        bake_chest_layer(0.0, 15.0, 0.0, 1.0, FACE_ALL & !FACE_WEST),
        bake_chest_layer(1.0, 15.0, 15.0, 1.0, FACE_ALL & !FACE_EAST),
    ]
}
