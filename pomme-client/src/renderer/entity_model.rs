use glam::{Mat4, Vec3};

use super::chunk::mesher::ChunkVertex;

pub struct ModelCube {
    pub origin: Vec3,
    pub size: Vec3,
    pub tex_offset: (u32, u32),
}

pub struct EntityPart {
    pub name: String,
    pub offset: Vec3,
    pub default_rotation: Vec3,
    pub cubes: Vec<ModelCube>,
    pub parent: Option<usize>,
}

pub struct BakedEntityModel {
    pub parts: Vec<EntityPart>,
    pub vertices: Vec<ChunkVertex>,
    pub part_ranges: Vec<(u32, u32)>,
}

impl BakedEntityModel {
    pub fn compute_part_transforms(&self, rotations: &[(usize, Vec3)]) -> Vec<Mat4> {
        let mut transforms = Vec::with_capacity(self.parts.len());

        for part in &self.parts {
            let mut rot = part.default_rotation;
            for &(idx, r) in rotations {
                if idx == transforms.len() {
                    rot = r;
                    break;
                }
            }

            let offset = Vec3::new(part.offset.x, -(part.offset.y - 24.0), part.offset.z) / 16.0;

            let local = Mat4::from_translation(offset)
                * Mat4::from_rotation_x(-rot.x)
                * Mat4::from_rotation_y(-rot.y)
                * Mat4::from_rotation_z(rot.z);

            let transform = if let Some(parent_idx) = part.parent {
                transforms[parent_idx] * local
            } else {
                local
            };

            transforms.push(transform);
        }

        transforms
    }
}

fn bake_model(parts: Vec<EntityPart>, tex_w: u32, tex_h: u32) -> BakedEntityModel {
    let mut vertices = Vec::new();
    let mut part_ranges = Vec::new();

    for part in &parts {
        let start = vertices.len() as u32;
        for cube in &part.cubes {
            generate_cube_vertices(cube, tex_w, tex_h, &mut vertices);
        }
        let count = vertices.len() as u32 - start;
        part_ranges.push((start, count));
    }

    BakedEntityModel {
        parts,
        vertices,
        part_ranges,
    }
}

pub fn bake_pig_model() -> BakedEntityModel {
    let parts = vec![
        EntityPart {
            name: "head".into(),
            offset: Vec3::new(0.0, 12.0, -6.0),
            default_rotation: Vec3::ZERO,
            cubes: vec![
                ModelCube {
                    origin: Vec3::new(-4.0, -4.0, -8.0),
                    size: Vec3::new(8.0, 8.0, 8.0),
                    tex_offset: (0, 0),
                },
                ModelCube {
                    origin: Vec3::new(-2.0, 0.0, -9.0),
                    size: Vec3::new(4.0, 3.0, 1.0),
                    tex_offset: (16, 16),
                },
            ],
            parent: None,
        },
        EntityPart {
            name: "body".into(),
            offset: Vec3::new(0.0, 11.0, 2.0),
            default_rotation: Vec3::new(std::f32::consts::FRAC_PI_2, 0.0, 0.0),
            cubes: vec![ModelCube {
                origin: Vec3::new(-5.0, -10.0, -7.0),
                size: Vec3::new(10.0, 16.0, 8.0),
                tex_offset: (28, 8),
            }],
            parent: None,
        },
        EntityPart {
            name: "right_hind_leg".into(),
            offset: Vec3::new(-3.0, 18.0, 7.0),
            default_rotation: Vec3::ZERO,
            cubes: vec![ModelCube {
                origin: Vec3::new(-2.0, 0.0, -2.0),
                size: Vec3::new(4.0, 6.0, 4.0),
                tex_offset: (0, 16),
            }],
            parent: None,
        },
        EntityPart {
            name: "left_hind_leg".into(),
            offset: Vec3::new(3.0, 18.0, 7.0),
            default_rotation: Vec3::ZERO,
            cubes: vec![ModelCube {
                origin: Vec3::new(-2.0, 0.0, -2.0),
                size: Vec3::new(4.0, 6.0, 4.0),
                tex_offset: (0, 16),
            }],
            parent: None,
        },
        EntityPart {
            name: "right_front_leg".into(),
            offset: Vec3::new(-3.0, 18.0, -5.0),
            default_rotation: Vec3::ZERO,
            cubes: vec![ModelCube {
                origin: Vec3::new(-2.0, 0.0, -2.0),
                size: Vec3::new(4.0, 6.0, 4.0),
                tex_offset: (0, 16),
            }],
            parent: None,
        },
        EntityPart {
            name: "left_front_leg".into(),
            offset: Vec3::new(3.0, 18.0, -5.0),
            default_rotation: Vec3::ZERO,
            cubes: vec![ModelCube {
                origin: Vec3::new(-2.0, 0.0, -2.0),
                size: Vec3::new(4.0, 6.0, 4.0),
                tex_offset: (0, 16),
            }],
            parent: None,
        },
    ];

    bake_model(parts, 64, 64)
}

pub fn bake_baby_pig_model() -> BakedEntityModel {
    let parts = vec![
        EntityPart {
            name: "head".into(),
            offset: Vec3::new(0.0, 19.0, -2.0),
            default_rotation: Vec3::ZERO,
            cubes: vec![
                ModelCube {
                    origin: Vec3::new(-3.5, -5.0, -5.0),
                    size: Vec3::new(7.0, 6.0, 6.0),
                    tex_offset: (0, 15),
                },
                ModelCube {
                    origin: Vec3::new(-1.5, -1.975, -6.0),
                    size: Vec3::new(3.0, 2.0, 1.0),
                    tex_offset: (6, 27),
                },
            ],
            parent: None,
        },
        EntityPart {
            name: "body".into(),
            offset: Vec3::new(0.0, 19.0, 0.5),
            default_rotation: Vec3::ZERO,
            cubes: vec![ModelCube {
                origin: Vec3::new(-3.5, -3.0, -4.5),
                size: Vec3::new(7.0, 6.0, 9.0),
                tex_offset: (0, 0),
            }],
            parent: None,
        },
        EntityPart {
            name: "right_hind_leg".into(),
            offset: Vec3::new(-2.5, 22.0, 4.0),
            default_rotation: Vec3::ZERO,
            cubes: vec![ModelCube {
                origin: Vec3::new(-1.0, 0.0, -1.0),
                size: Vec3::new(2.0, 2.0, 2.0),
                tex_offset: (23, 4),
            }],
            parent: None,
        },
        EntityPart {
            name: "left_hind_leg".into(),
            offset: Vec3::new(2.5, 22.0, 4.0),
            default_rotation: Vec3::ZERO,
            cubes: vec![ModelCube {
                origin: Vec3::new(-1.0, 0.0, -1.0),
                size: Vec3::new(2.0, 2.0, 2.0),
                tex_offset: (0, 4),
            }],
            parent: None,
        },
        EntityPart {
            name: "right_front_leg".into(),
            offset: Vec3::new(-2.5, 22.0, -3.0),
            default_rotation: Vec3::ZERO,
            cubes: vec![ModelCube {
                origin: Vec3::new(-1.0, 0.0, -1.0),
                size: Vec3::new(2.0, 2.0, 2.0),
                tex_offset: (23, 0),
            }],
            parent: None,
        },
        EntityPart {
            name: "left_front_leg".into(),
            offset: Vec3::new(2.5, 22.0, -3.0),
            default_rotation: Vec3::ZERO,
            cubes: vec![ModelCube {
                origin: Vec3::new(-1.0, 0.0, -1.0),
                size: Vec3::new(2.0, 2.0, 2.0),
                tex_offset: (0, 0),
            }],
            parent: None,
        },
    ];

    bake_model(parts, 32, 32)
}

pub fn bake_player_model() -> BakedEntityModel {
    let parts = vec![
        EntityPart {
            name: "head".into(),
            offset: Vec3::new(0.0, 0.0, 0.0),
            default_rotation: Vec3::ZERO,
            cubes: vec![ModelCube {
                origin: Vec3::new(-4.0, -8.0, -4.0),
                size: Vec3::new(8.0, 8.0, 8.0),
                tex_offset: (0, 0),
            }],
            parent: None,
        },
        EntityPart {
            name: "body".into(),
            offset: Vec3::new(0.0, 0.0, 0.0),
            default_rotation: Vec3::ZERO,
            cubes: vec![ModelCube {
                origin: Vec3::new(-4.0, 0.0, -2.0),
                size: Vec3::new(8.0, 12.0, 4.0),
                tex_offset: (16, 16),
            }],
            parent: None,
        },
        EntityPart {
            name: "right_arm".into(),
            offset: Vec3::new(-5.0, 2.0, 0.0),
            default_rotation: Vec3::ZERO,
            cubes: vec![ModelCube {
                origin: Vec3::new(-3.0, -2.0, -2.0),
                size: Vec3::new(4.0, 12.0, 4.0),
                tex_offset: (40, 16),
            }],
            parent: None,
        },
        EntityPart {
            name: "left_arm".into(),
            offset: Vec3::new(5.0, 2.0, 0.0),
            default_rotation: Vec3::ZERO,
            cubes: vec![ModelCube {
                origin: Vec3::new(-1.0, -2.0, -2.0),
                size: Vec3::new(4.0, 12.0, 4.0),
                tex_offset: (32, 48),
            }],
            parent: None,
        },
        EntityPart {
            name: "right_leg".into(),
            offset: Vec3::new(-1.9, 12.0, 0.0),
            default_rotation: Vec3::ZERO,
            cubes: vec![ModelCube {
                origin: Vec3::new(-2.0, 0.0, -2.0),
                size: Vec3::new(4.0, 12.0, 4.0),
                tex_offset: (0, 16),
            }],
            parent: None,
        },
        EntityPart {
            name: "left_leg".into(),
            offset: Vec3::new(1.9, 12.0, 0.0),
            default_rotation: Vec3::ZERO,
            cubes: vec![ModelCube {
                origin: Vec3::new(-2.0, 0.0, -2.0),
                size: Vec3::new(4.0, 12.0, 4.0),
                tex_offset: (16, 48),
            }],
            parent: None,
        },
    ];

    bake_model(parts, 64, 64)
}

pub fn compute_humanoid_anim(
    model: &BakedEntityModel,
    head_pitch: f32,
    head_yaw: f32,
    walk_pos: f32,
    walk_speed: f32,
) -> Vec<(usize, Vec3)> {
    let mut rotations = Vec::new();

    for (i, part) in model.parts.iter().enumerate() {
        let rot = match part.name.as_str() {
            "head" => Vec3::new(head_pitch.to_radians(), head_yaw.to_radians(), 0.0),
            "right_arm" => Vec3::new(
                (walk_pos * 0.6662 + std::f32::consts::PI).cos() * 2.0 * walk_speed * 0.5,
                0.0,
                0.0,
            ),
            "left_arm" => Vec3::new((walk_pos * 0.6662).cos() * 2.0 * walk_speed * 0.5, 0.0, 0.0),
            "right_leg" => Vec3::new((walk_pos * 0.6662).cos() * 1.4 * walk_speed, 0.0, 0.0),
            "left_leg" => Vec3::new(
                (walk_pos * 0.6662 + std::f32::consts::PI).cos() * 1.4 * walk_speed,
                0.0,
                0.0,
            ),
            _ => continue,
        };
        rotations.push((i, rot));
    }

    rotations
}

pub fn compute_quadruped_anim(
    model: &BakedEntityModel,
    head_pitch: f32,
    head_yaw: f32,
    walk_pos: f32,
    walk_speed: f32,
) -> Vec<(usize, Vec3)> {
    let mut rotations = Vec::new();

    for (i, part) in model.parts.iter().enumerate() {
        let rot = match part.name.as_str() {
            "head" => Vec3::new(head_pitch.to_radians(), head_yaw.to_radians(), 0.0),
            "right_hind_leg" => Vec3::new((walk_pos * 0.6662).cos() * 1.4 * walk_speed, 0.0, 0.0),
            "left_hind_leg" => Vec3::new(
                (walk_pos * 0.6662 + std::f32::consts::PI).cos() * 1.4 * walk_speed,
                0.0,
                0.0,
            ),
            "right_front_leg" => Vec3::new(
                (walk_pos * 0.6662 + std::f32::consts::PI).cos() * 1.4 * walk_speed,
                0.0,
                0.0,
            ),
            "left_front_leg" => Vec3::new((walk_pos * 0.6662).cos() * 1.4 * walk_speed, 0.0, 0.0),
            _ => continue,
        };
        rotations.push((i, rot));
    }

    rotations
}

fn generate_cube_vertices(
    cube: &ModelCube,
    tex_w: u32,
    tex_h: u32,
    vertices: &mut Vec<ChunkVertex>,
) {
    let tw = tex_w as f32;
    let th = tex_h as f32;
    let u0 = cube.tex_offset.0 as f32;
    let v0 = cube.tex_offset.1 as f32;
    let w = cube.size.x;
    let h = cube.size.y;
    let d = cube.size.z;

    let x0 = cube.origin.x / 16.0;
    let y0 = cube.origin.y / 16.0;
    let x1 = x0 + w / 16.0;
    let y1 = y0 + h / 16.0;
    let z0 = cube.origin.z / 16.0;
    let z1 = z0 + d / 16.0;

    let yb = -y1;
    let yt = -y0;

    struct Face {
        positions: [[f32; 3]; 4],
        uv: [f32; 4],
    }

    let faces = [
        Face {
            positions: [[x1, yb, z0], [x0, yb, z0], [x0, yt, z0], [x1, yt, z0]],
            uv: [u0 + d, v0 + d, u0 + d + w, v0 + d + h],
        },
        Face {
            positions: [[x0, yb, z1], [x1, yb, z1], [x1, yt, z1], [x0, yt, z1]],
            uv: [u0 + d + w + d, v0 + d, u0 + d + w + d + w, v0 + d + h],
        },
        Face {
            positions: [[x0, yt, z0], [x0, yt, z1], [x1, yt, z1], [x1, yt, z0]],
            uv: [u0 + d, v0, u0 + d + w, v0 + d],
        },
        Face {
            positions: [[x0, yb, z1], [x0, yb, z0], [x1, yb, z0], [x1, yb, z1]],
            uv: [u0 + d + w, v0, u0 + d + w + w, v0 + d],
        },
        Face {
            positions: [[x0, yb, z1], [x0, yb, z0], [x0, yt, z0], [x0, yt, z1]],
            uv: [u0, v0 + d, u0 + d, v0 + d + h],
        },
        Face {
            positions: [[x1, yb, z0], [x1, yb, z1], [x1, yt, z1], [x1, yt, z0]],
            uv: [u0 + d + w, v0 + d, u0 + d + w + d, v0 + d + h],
        },
    ];

    for face in &faces {
        let u_min = face.uv[0] / tw;
        let v_min = face.uv[1] / th;
        let u_max = face.uv[2] / tw;
        let v_max = face.uv[3] / th;

        let uvs = [
            [u_min, v_max],
            [u_max, v_max],
            [u_max, v_min],
            [u_min, v_min],
        ];

        for &i in &[0usize, 1, 2, 0, 2, 3] {
            vertices.push(ChunkVertex {
                position: face.positions[i],
                tex_coords: crate::renderer::chunk::mesher::pack_uv(uvs[i][0], uvs[i][1]),
                light_tint: crate::renderer::chunk::mesher::pack_light_tint(
                    1.0,
                    crate::renderer::chunk::mesher::PACKED_WHITE_SHIFTED,
                ),
            });
        }
    }
}
