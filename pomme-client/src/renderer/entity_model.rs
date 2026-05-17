use glam::{Mat4, Vec3};

use super::chunk::mesher::ChunkVertex;

#[derive(Clone, Copy)]
pub struct ModelCube {
    pub origin: Vec3,
    pub size: Vec3,
    pub tex_offset: (u32, u32),
    pub deformation: f32,
    pub mirror: bool,
}

fn quadruped_legs(
    leg_x: f32,
    leg_y: f32,
    front_z: f32,
    hind_z: f32,
    right_cube: ModelCube,
    left_cube: ModelCube,
) -> [EntityPart; 4] {
    let leg = |name: &str, x: f32, z: f32, cube: ModelCube| EntityPart {
        name: name.into(),
        offset: Vec3::new(x, leg_y, z),
        default_rotation: Vec3::ZERO,
        cubes: vec![cube],
        parent: None,
    };
    [
        leg("right_hind_leg", -leg_x, hind_z, right_cube),
        leg("left_hind_leg", leg_x, hind_z, left_cube),
        leg("right_front_leg", -leg_x, front_z, right_cube),
        leg("left_front_leg", leg_x, front_z, left_cube),
    ]
}

#[derive(Clone)]
pub struct EntityPart {
    pub name: String,
    pub offset: Vec3,
    pub default_rotation: Vec3,
    pub cubes: Vec<ModelCube>,
    pub parent: Option<usize>,
}

#[derive(Clone)]
pub struct BakedEntityModel {
    pub parts: Vec<EntityPart>,
    pub vertices: Vec<ChunkVertex>,
    pub part_ranges: Vec<(u32, u32)>,
}

#[derive(Default)]
pub struct PartAnim {
    pub rotation: Vec<(usize, Vec3)>,
    pub translation: Vec<(usize, Vec3)>,
}

impl BakedEntityModel {
    pub fn compute_part_transforms(&self, anim: &PartAnim) -> Vec<Mat4> {
        let mut transforms = Vec::with_capacity(self.parts.len());

        for part in &self.parts {
            let mut rot = part.default_rotation;
            for &(idx, r) in &anim.rotation {
                if idx == transforms.len() {
                    rot = r;
                    break;
                }
            }
            let mut extra_translation = Vec3::ZERO;
            for &(idx, t) in &anim.translation {
                if idx == transforms.len() {
                    extra_translation = t;
                    break;
                }
            }

            let offset_x = part.offset.x + extra_translation.x;
            let offset_y = -(part.offset.y + extra_translation.y - 24.0);
            let offset_z = part.offset.z + extra_translation.z;
            let offset = Vec3::new(offset_x, offset_y, offset_z) / 16.0;

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
    let mut parts = vec![
        EntityPart {
            name: "head".into(),
            offset: Vec3::new(0.0, 12.0, -6.0),
            default_rotation: Vec3::ZERO,
            cubes: vec![
                ModelCube {
                    origin: Vec3::new(-4.0, -4.0, -8.0),
                    size: Vec3::new(8.0, 8.0, 8.0),
                    tex_offset: (0, 0),
                    deformation: 0.0,
                    mirror: false,
                },
                ModelCube {
                    origin: Vec3::new(-2.0, 0.0, -9.0),
                    size: Vec3::new(4.0, 3.0, 1.0),
                    tex_offset: (16, 16),
                    deformation: 0.0,
                    mirror: false,
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
                deformation: 0.0,
                mirror: false,
            }],
            parent: None,
        },
    ];
    let pig_leg = ModelCube {
        origin: Vec3::new(-2.0, 0.0, -2.0),
        size: Vec3::new(4.0, 6.0, 4.0),
        tex_offset: (0, 16),
        deformation: 0.0,
        mirror: false,
    };
    parts.extend(quadruped_legs(3.0, 18.0, -5.0, 7.0, pig_leg, pig_leg));
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
                    deformation: 0.0,
                    mirror: false,
                },
                ModelCube {
                    origin: Vec3::new(-1.5, -1.975, -6.0),
                    size: Vec3::new(3.0, 2.0, 1.0),
                    tex_offset: (6, 27),
                    deformation: 0.0,
                    mirror: false,
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
                deformation: 0.0,
                mirror: false,
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
                deformation: 0.0,
                mirror: false,
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
                deformation: 0.0,
                mirror: false,
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
                deformation: 0.0,
                mirror: false,
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
                deformation: 0.0,
                mirror: false,
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
                deformation: 0.0,
                mirror: false,
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
                deformation: 0.0,
                mirror: false,
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
                deformation: 0.0,
                mirror: false,
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
                deformation: 0.0,
                mirror: false,
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
                deformation: 0.0,
                mirror: false,
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
                deformation: 0.0,
                mirror: false,
            }],
            parent: None,
        },
    ];

    bake_model(parts, 64, 64)
}

pub fn bake_cow_model() -> BakedEntityModel {
    let mut parts = vec![
        EntityPart {
            name: "head".into(),
            offset: Vec3::new(0.0, 4.0, -8.0),
            default_rotation: Vec3::ZERO,
            cubes: vec![
                ModelCube {
                    origin: Vec3::new(-4.0, -4.0, -6.0),
                    size: Vec3::new(8.0, 8.0, 6.0),
                    tex_offset: (0, 0),
                    deformation: 0.0,
                    mirror: false,
                },
                ModelCube {
                    origin: Vec3::new(-3.0, 1.0, -7.0),
                    size: Vec3::new(6.0, 3.0, 1.0),
                    tex_offset: (1, 33),
                    deformation: 0.0,
                    mirror: false,
                },
                ModelCube {
                    origin: Vec3::new(-5.0, -5.0, -5.0),
                    size: Vec3::new(1.0, 3.0, 1.0),
                    tex_offset: (22, 0),
                    deformation: 0.0,
                    mirror: false,
                },
                ModelCube {
                    origin: Vec3::new(4.0, -5.0, -5.0),
                    size: Vec3::new(1.0, 3.0, 1.0),
                    tex_offset: (22, 0),
                    deformation: 0.0,
                    mirror: false,
                },
            ],
            parent: None,
        },
        EntityPart {
            name: "body".into(),
            offset: Vec3::new(0.0, 5.0, 2.0),
            default_rotation: Vec3::new(std::f32::consts::FRAC_PI_2, 0.0, 0.0),
            cubes: vec![
                ModelCube {
                    origin: Vec3::new(-6.0, -10.0, -7.0),
                    size: Vec3::new(12.0, 18.0, 10.0),
                    tex_offset: (18, 4),
                    deformation: 0.0,
                    mirror: false,
                },
                ModelCube {
                    origin: Vec3::new(-2.0, 2.0, -8.0),
                    size: Vec3::new(4.0, 6.0, 1.0),
                    tex_offset: (52, 0),
                    deformation: 0.0,
                    mirror: false,
                },
            ],
            parent: None,
        },
    ];
    let cow_leg_right = ModelCube {
        origin: Vec3::new(-2.0, 0.0, -2.0),
        size: Vec3::new(4.0, 12.0, 4.0),
        tex_offset: (0, 16),
        deformation: 0.0,
        mirror: false,
    };
    let cow_leg_left = ModelCube {
        mirror: true,
        ..cow_leg_right
    };
    parts.extend(quadruped_legs(
        4.0,
        12.0,
        -5.0,
        7.0,
        cow_leg_right,
        cow_leg_left,
    ));
    bake_model(parts, 64, 64)
}

pub fn bake_baby_cow_model() -> BakedEntityModel {
    let parts = vec![
        EntityPart {
            name: "head".into(),
            offset: Vec3::new(0.0, 13.569, -5.1667),
            default_rotation: Vec3::ZERO,
            cubes: vec![
                ModelCube {
                    origin: Vec3::new(-3.0, -4.569, -4.8333),
                    size: Vec3::new(6.0, 6.0, 5.0),
                    tex_offset: (0, 18),
                    deformation: 0.0,
                    mirror: false,
                },
                ModelCube {
                    origin: Vec3::new(3.0, -5.569, -3.8333),
                    size: Vec3::new(1.0, 2.0, 1.0),
                    tex_offset: (8, 29),
                    deformation: 0.0,
                    mirror: false,
                },
                ModelCube {
                    origin: Vec3::new(-4.0, -5.569, -3.8333),
                    size: Vec3::new(1.0, 2.0, 1.0),
                    tex_offset: (4, 29),
                    deformation: 0.0,
                    mirror: true,
                },
                ModelCube {
                    origin: Vec3::new(-2.0, -1.569, -5.8333),
                    size: Vec3::new(4.0, 3.0, 1.0),
                    tex_offset: (12, 29),
                    deformation: 0.0,
                    mirror: false,
                },
            ],
            parent: None,
        },
        EntityPart {
            name: "body".into(),
            offset: Vec3::new(3.0, 19.0, -5.0),
            default_rotation: Vec3::ZERO,
            cubes: vec![ModelCube {
                origin: Vec3::new(-7.0, -7.0, -1.0),
                size: Vec3::new(8.0, 6.0, 12.0),
                tex_offset: (0, 0),
                deformation: 0.0,
                mirror: false,
            }],
            parent: None,
        },
        EntityPart {
            name: "right_front_leg".into(),
            offset: Vec3::new(-2.5, 18.0, -3.5),
            default_rotation: Vec3::ZERO,
            cubes: vec![ModelCube {
                origin: Vec3::new(-1.5, 0.0, -1.5),
                size: Vec3::new(3.0, 6.0, 3.0),
                tex_offset: (22, 18),
                deformation: 0.0,
                mirror: false,
            }],
            parent: None,
        },
        EntityPart {
            name: "left_front_leg".into(),
            offset: Vec3::new(2.5, 18.0, -3.5),
            default_rotation: Vec3::ZERO,
            cubes: vec![ModelCube {
                origin: Vec3::new(-1.5, 0.0, -1.5),
                size: Vec3::new(3.0, 6.0, 3.0),
                tex_offset: (34, 18),
                deformation: 0.0,
                mirror: false,
            }],
            parent: None,
        },
        EntityPart {
            name: "right_hind_leg".into(),
            offset: Vec3::new(-2.5, 18.0, 3.5),
            default_rotation: Vec3::ZERO,
            cubes: vec![ModelCube {
                origin: Vec3::new(-1.5, 0.0, -1.5),
                size: Vec3::new(3.0, 6.0, 3.0),
                tex_offset: (22, 27),
                deformation: 0.0,
                mirror: false,
            }],
            parent: None,
        },
        EntityPart {
            name: "left_hind_leg".into(),
            offset: Vec3::new(2.5, 18.0, 3.5),
            default_rotation: Vec3::ZERO,
            cubes: vec![ModelCube {
                origin: Vec3::new(-1.5, 0.0, -1.5),
                size: Vec3::new(3.0, 6.0, 3.0),
                tex_offset: (34, 27),
                deformation: 0.0,
                mirror: false,
            }],
            parent: None,
        },
    ];

    bake_model(parts, 64, 64)
}

pub fn bake_sheep_model() -> BakedEntityModel {
    let mut parts = vec![
        EntityPart {
            name: "head".into(),
            offset: Vec3::new(0.0, 6.0, -8.0),
            default_rotation: Vec3::ZERO,
            cubes: vec![ModelCube {
                origin: Vec3::new(-3.0, -4.0, -6.0),
                size: Vec3::new(6.0, 6.0, 8.0),
                tex_offset: (0, 0),
                deformation: 0.0,
                mirror: false,
            }],
            parent: None,
        },
        EntityPart {
            name: "body".into(),
            offset: Vec3::new(0.0, 5.0, 2.0),
            default_rotation: Vec3::new(std::f32::consts::FRAC_PI_2, 0.0, 0.0),
            cubes: vec![ModelCube {
                origin: Vec3::new(-4.0, -10.0, -7.0),
                size: Vec3::new(8.0, 16.0, 6.0),
                tex_offset: (28, 8),
                deformation: 0.0,
                mirror: false,
            }],
            parent: None,
        },
    ];
    let sheep_leg_right = ModelCube {
        origin: Vec3::new(-2.0, 0.0, -2.0),
        size: Vec3::new(4.0, 12.0, 4.0),
        tex_offset: (0, 16),
        deformation: 0.0,
        mirror: false,
    };
    let sheep_leg_left = ModelCube {
        mirror: true,
        ..sheep_leg_right
    };
    parts.extend(quadruped_legs(
        3.0,
        12.0,
        -5.0,
        7.0,
        sheep_leg_right,
        sheep_leg_left,
    ));
    bake_model(parts, 64, 32)
}

pub fn bake_baby_sheep_model() -> BakedEntityModel {
    let parts = vec![
        EntityPart {
            name: "head".into(),
            offset: Vec3::new(0.0, 15.5, -2.5),
            default_rotation: Vec3::ZERO,
            cubes: vec![ModelCube {
                origin: Vec3::new(-2.5, -4.5, -3.5),
                size: Vec3::new(5.0, 5.0, 5.0),
                tex_offset: (0, 0),
                deformation: 0.0,
                mirror: false,
            }],
            parent: None,
        },
        EntityPart {
            name: "body".into(),
            offset: Vec3::new(0.0, 17.0, 0.5),
            default_rotation: Vec3::ZERO,
            cubes: vec![ModelCube {
                origin: Vec3::new(-3.0, -2.0, -4.5),
                size: Vec3::new(6.0, 4.0, 9.0),
                tex_offset: (0, 10),
                deformation: 0.0,
                mirror: false,
            }],
            parent: None,
        },
        EntityPart {
            name: "right_hind_leg".into(),
            offset: Vec3::new(-2.0, 19.0, 3.0),
            default_rotation: Vec3::ZERO,
            cubes: vec![ModelCube {
                origin: Vec3::new(-1.0, 0.0, -1.0),
                size: Vec3::new(2.0, 5.0, 2.0),
                tex_offset: (0, 23),
                deformation: 0.0,
                mirror: false,
            }],
            parent: None,
        },
        EntityPart {
            name: "left_hind_leg".into(),
            offset: Vec3::new(2.0, 19.0, 3.0),
            default_rotation: Vec3::ZERO,
            cubes: vec![ModelCube {
                origin: Vec3::new(-1.0, 0.0, -1.0),
                size: Vec3::new(2.0, 5.0, 2.0),
                tex_offset: (24, 12),
                deformation: 0.0,
                mirror: false,
            }],
            parent: None,
        },
        EntityPart {
            name: "right_front_leg".into(),
            offset: Vec3::new(-2.0, 19.0, -2.0),
            default_rotation: Vec3::ZERO,
            cubes: vec![ModelCube {
                origin: Vec3::new(-1.0, 0.0, -1.0),
                size: Vec3::new(2.0, 5.0, 2.0),
                tex_offset: (8, 23),
                deformation: 0.0,
                mirror: false,
            }],
            parent: None,
        },
        EntityPart {
            name: "left_front_leg".into(),
            offset: Vec3::new(2.0, 19.0, -2.0),
            default_rotation: Vec3::ZERO,
            cubes: vec![ModelCube {
                origin: Vec3::new(-1.0, 0.0, -1.0),
                size: Vec3::new(2.0, 5.0, 2.0),
                tex_offset: (24, 5),
                deformation: 0.0,
                mirror: false,
            }],
            parent: None,
        },
    ];

    bake_model(parts, 64, 32)
}

pub fn bake_sheep_wool_model() -> BakedEntityModel {
    let mut parts = vec![
        EntityPart {
            name: "head".into(),
            offset: Vec3::new(0.0, 6.0, -8.0),
            default_rotation: Vec3::ZERO,
            cubes: vec![ModelCube {
                origin: Vec3::new(-3.0, -4.0, -4.0),
                size: Vec3::new(6.0, 6.0, 6.0),
                tex_offset: (0, 0),
                deformation: 0.6,
                mirror: false,
            }],
            parent: None,
        },
        EntityPart {
            name: "body".into(),
            offset: Vec3::new(0.0, 5.0, 2.0),
            default_rotation: Vec3::new(std::f32::consts::FRAC_PI_2, 0.0, 0.0),
            cubes: vec![ModelCube {
                origin: Vec3::new(-4.0, -10.0, -7.0),
                size: Vec3::new(8.0, 16.0, 6.0),
                tex_offset: (28, 8),
                deformation: 1.75,
                mirror: false,
            }],
            parent: None,
        },
    ];
    let wool_leg_right = ModelCube {
        origin: Vec3::new(-2.0, 0.0, -2.0),
        size: Vec3::new(4.0, 6.0, 4.0),
        tex_offset: (0, 16),
        deformation: 0.5,
        mirror: false,
    };
    let wool_leg_left = ModelCube {
        mirror: true,
        ..wool_leg_right
    };
    parts.extend(quadruped_legs(
        3.0,
        12.0,
        -5.0,
        7.0,
        wool_leg_right,
        wool_leg_left,
    ));
    bake_model(parts, 64, 32)
}

pub fn bake_sheep_wool_undercoat_model() -> BakedEntityModel {
    bake_sheep_model()
}

pub fn bake_baby_sheep_wool_model() -> BakedEntityModel {
    bake_baby_sheep_model()
}

pub fn compute_humanoid_anim(
    model: &BakedEntityModel,
    head_pitch: f32,
    head_yaw: f32,
    walk_pos: f32,
    walk_speed: f32,
) -> PartAnim {
    let mut anim = PartAnim::default();

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
        anim.rotation.push((i, rot));
    }

    anim
}

pub fn compute_quadruped_anim(
    model: &BakedEntityModel,
    head_pitch: f32,
    head_yaw: f32,
    walk_pos: f32,
    walk_speed: f32,
    head_y_offset: f32,
    head_x_rot_override: Option<f32>,
) -> PartAnim {
    let mut anim = PartAnim::default();

    for (i, part) in model.parts.iter().enumerate() {
        let rot = match part.name.as_str() {
            "head" => Vec3::new(
                head_x_rot_override.unwrap_or_else(|| head_pitch.to_radians()),
                head_yaw.to_radians(),
                0.0,
            ),
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
        if head_y_offset != 0.0 && part.name == "head" {
            anim.translation
                .push((i, Vec3::new(0.0, head_y_offset, 0.0)));
        }
        anim.rotation.push((i, rot));
    }

    anim
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

    let inf = cube.deformation;
    let x0 = (cube.origin.x - inf) / 16.0;
    let y0 = (cube.origin.y - inf) / 16.0;
    let z0 = (cube.origin.z - inf) / 16.0;
    let x1 = (cube.origin.x + w + inf) / 16.0;
    let y1 = (cube.origin.y + h + inf) / 16.0;
    let z1 = (cube.origin.z + d + inf) / 16.0;

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

    // Indices 4 (-X) and 5 (+X) are the side faces. When mirror is set, vanilla's
    // minX/maxX swap effectively exchanges their UV regions; every face also has
    // its U flipped.
    for (idx, face) in faces.iter().enumerate() {
        let uv_source = match (cube.mirror, idx) {
            (true, 4) => &faces[5].uv,
            (true, 5) => &faces[4].uv,
            _ => &face.uv,
        };
        let v_min = uv_source[1] / th;
        let v_max = uv_source[3] / th;
        let (u_min, u_max) = if cube.mirror {
            (uv_source[2] / tw, uv_source[0] / tw)
        } else {
            (uv_source[0] / tw, uv_source[2] / tw)
        };

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
