use glam::{Mat4, Quat, Vec3};

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

/// Coordinate space a model's parts and vertices were authored in.
#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub enum ModelConvention {
    /// Vanilla entity convention: cube Y negated at bake, pivots at
    /// `(24 - y)/16`, euler signs (-x, -y, +z). All mob models use this.
    #[default]
    EntityYDown,
    /// Vanilla block-entity literal space: y-up, coords/16 relative to the
    /// block's min corner, pivots at `offset/16`, vanilla ZYX euler with
    /// unmodified signs. Used by the chest models.
    BlockYUp,
}

#[derive(Clone)]
pub struct BakedEntityModel {
    pub parts: Vec<EntityPart>,
    pub vertices: Vec<ChunkVertex>,
    pub part_ranges: Vec<(u32, u32)>,
    pub convention: ModelConvention,
    /// Per-part scale (parallel to `parts`), applied about each part's pivot at
    /// transform time. Scales geometry only, never UVs — used for baby mobs
    /// whose head and body shrink by different factors. Default 1.0 for
    /// every part.
    pub part_scales: Vec<f32>,
}

#[derive(Default)]
pub struct PartAnim {
    pub rotation: Vec<(usize, Vec3)>,
    /// Quaternion rotation override; takes precedence over `rotation` for a
    /// part. Used where the engine's fixed euler order can't reproduce
    /// vanilla's composition (e.g. spider legs with combined yaw + tilt).
    pub rotation_quat: Vec<(usize, Quat)>,
    pub translation: Vec<(usize, Vec3)>,
}

impl BakedEntityModel {
    /// Assemble baked parts, defaulting every part's scale to 1.0.
    pub(crate) fn new(
        parts: Vec<EntityPart>,
        vertices: Vec<ChunkVertex>,
        part_ranges: Vec<(u32, u32)>,
    ) -> Self {
        let part_scales = vec![1.0; parts.len()];
        Self {
            parts,
            vertices,
            part_ranges,
            convention: ModelConvention::default(),
            part_scales,
        }
    }

    pub(crate) fn with_convention(mut self, convention: ModelConvention) -> Self {
        self.convention = convention;
        self
    }

    pub fn compute_part_transforms(&self, anim: &PartAnim) -> Vec<Mat4> {
        let mut transforms = Vec::with_capacity(self.parts.len());

        for (i, part) in self.parts.iter().enumerate() {
            let mut quat_rot = None;
            for &(idx, q) in &anim.rotation_quat {
                if idx == i {
                    quat_rot = Some(q);
                    break;
                }
            }
            let mut rot = part.default_rotation;
            for &(idx, r) in &anim.rotation {
                if idx == i {
                    rot = r;
                    break;
                }
            }
            let mut extra_translation = Vec3::ZERO;
            for &(idx, t) in &anim.translation {
                if idx == i {
                    extra_translation = t;
                    break;
                }
            }

            let offset = match self.convention {
                ModelConvention::EntityYDown => Vec3::new(
                    part.offset.x + extra_translation.x,
                    -(part.offset.y + extra_translation.y - 24.0),
                    part.offset.z + extra_translation.z,
                ),
                ModelConvention::BlockYUp => part.offset + extra_translation,
            } / 16.0;

            // A quaternion override expresses the exact render-space orientation
            // directly; otherwise use the per-axis euler product: the y-down
            // convention needs the engine's mixed signs (-x, -y, +z), y-up
            // matches vanilla's `translateAndRotate` ZYX order verbatim.
            let rot_mat = match (quat_rot, self.convention) {
                (Some(q), _) => Mat4::from_quat(q),
                (None, ModelConvention::EntityYDown) => {
                    Mat4::from_rotation_x(-rot.x)
                        * Mat4::from_rotation_y(-rot.y)
                        * Mat4::from_rotation_z(rot.z)
                }
                (None, ModelConvention::BlockYUp) => {
                    Mat4::from_rotation_z(rot.z)
                        * Mat4::from_rotation_y(rot.y)
                        * Mat4::from_rotation_x(rot.x)
                }
            };
            let scale = self.part_scales.get(i).copied().unwrap_or(1.0);

            let local =
                Mat4::from_translation(offset) * rot_mat * Mat4::from_scale(Vec3::splat(scale));

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

pub fn bake_model(parts: Vec<EntityPart>, tex_w: u32, tex_h: u32) -> BakedEntityModel {
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

    BakedEntityModel::new(parts, vertices, part_ranges)
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

/// `slim` is the 3px-wide-arm (Alex) layout; same texture offsets and pivots,
/// only the arm boxes differ.
// TODO: jacket/sleeve/pants overlay layers (only the hat is modeled here).
// TODO: default-skin-by-UUID selection for players without a fetched skin.
pub fn bake_player_model(slim: bool) -> BakedEntityModel {
    let arm_w = if slim { 3.0 } else { 4.0 };
    let right_arm_ox = if slim { -2.0 } else { -3.0 };
    let parts = vec![
        EntityPart {
            name: "head".into(),
            offset: Vec3::new(0.0, 0.0, 0.0),
            default_rotation: Vec3::ZERO,
            cubes: vec![
                ModelCube {
                    origin: Vec3::new(-4.0, -8.0, -4.0),
                    size: Vec3::new(8.0, 8.0, 8.0),
                    tex_offset: (0, 0),
                    deformation: 0.0,
                    mirror: false,
                },
                // Hat / headwear outer layer.
                ModelCube {
                    origin: Vec3::new(-4.0, -8.0, -4.0),
                    size: Vec3::new(8.0, 8.0, 8.0),
                    tex_offset: (32, 0),
                    deformation: 0.5,
                    mirror: false,
                },
            ],
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
                origin: Vec3::new(right_arm_ox, -2.0, -2.0),
                size: Vec3::new(arm_w, 12.0, 4.0),
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
                size: Vec3::new(arm_w, 12.0, 4.0),
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

/// Legacy single-arm humanoid layout (head/body/right_arm/left_arm/right_leg/
/// left_leg), shared by zombie and skeleton. `limb` builds the four limb cubes
/// so zombie (4-wide) and skeleton (2-wide thin) can differ while keeping
/// identical part names/order (required for shared animation indexing). `tex_h`
/// lets zombie use a 64-tall sheet and skeleton a 32-tall one.
fn humanoid_parts(
    arm_cube_right: ModelCube,
    leg_cube_right: ModelCube,
    right_leg_x: f32,
) -> Vec<EntityPart> {
    // Pomme's `mirror` flag only flips UVs, so mirror the geometry origin across
    // x=0 too (vanilla `.mirror()` does both, e.g. arm origin -3 -> -1).
    let mirror_x = |c: ModelCube| ModelCube {
        origin: Vec3::new(-(c.origin.x + c.size.x), c.origin.y, c.origin.z),
        mirror: true,
        ..c
    };
    let arm_cube_left = mirror_x(arm_cube_right);
    let leg_cube_left = mirror_x(leg_cube_right);
    vec![
        EntityPart {
            name: "head".into(),
            offset: Vec3::new(0.0, 0.0, 0.0),
            default_rotation: Vec3::ZERO,
            cubes: vec![
                ModelCube {
                    origin: Vec3::new(-4.0, -8.0, -4.0),
                    size: Vec3::new(8.0, 8.0, 8.0),
                    tex_offset: (0, 0),
                    deformation: 0.0,
                    mirror: false,
                },
                // Hat / headwear outer layer (vanilla `HumanoidModel` head child).
                ModelCube {
                    origin: Vec3::new(-4.0, -8.0, -4.0),
                    size: Vec3::new(8.0, 8.0, 8.0),
                    tex_offset: (32, 0),
                    deformation: 0.5,
                    mirror: false,
                },
            ],
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
            cubes: vec![arm_cube_right],
            parent: None,
        },
        EntityPart {
            name: "left_arm".into(),
            offset: Vec3::new(5.0, 2.0, 0.0),
            default_rotation: Vec3::ZERO,
            cubes: vec![arm_cube_left],
            parent: None,
        },
        EntityPart {
            name: "right_leg".into(),
            offset: Vec3::new(-right_leg_x, 12.0, 0.0),
            default_rotation: Vec3::ZERO,
            cubes: vec![leg_cube_right],
            parent: None,
        },
        EntityPart {
            name: "left_leg".into(),
            offset: Vec3::new(right_leg_x, 12.0, 0.0),
            default_rotation: Vec3::ZERO,
            cubes: vec![leg_cube_left],
            parent: None,
        },
    ]
}

/// Zombie mesh parts: `HumanoidModel.createMesh` layout with 4×12×4 limbs.
fn zombie_parts() -> Vec<EntityPart> {
    let arm = ModelCube {
        origin: Vec3::new(-3.0, -2.0, -2.0),
        size: Vec3::new(4.0, 12.0, 4.0),
        tex_offset: (40, 16),
        deformation: 0.0,
        mirror: false,
    };
    let leg = ModelCube {
        origin: Vec3::new(-2.0, 0.0, -2.0),
        size: Vec3::new(4.0, 12.0, 4.0),
        tex_offset: (0, 16),
        deformation: 0.0,
        mirror: false,
    };
    humanoid_parts(arm, leg, 1.9)
}

pub fn bake_zombie_model() -> BakedEntityModel {
    bake_model(zombie_parts(), 64, 64)
}

/// Baby zombie: adult mesh transformed per vanilla `BabyModelTransform` — head
/// scales 0.75, body/limbs 0.5, each pivot shifted so the feet stay grounded.
/// Per-part scaling is geometry-only, so UVs are untouched.
pub fn bake_baby_zombie_model() -> BakedEntityModel {
    const HEAD_SCALE: f32 = 0.75;
    const BODY_SCALE: f32 = 0.5;
    let mut parts = zombie_parts();
    let mut scales = Vec::with_capacity(parts.len());
    for part in &mut parts {
        let (scale, lift) = if part.name == "head" {
            (HEAD_SCALE, 16.0)
        } else {
            (BODY_SCALE, 24.0)
        };
        part.offset = (part.offset + Vec3::new(0.0, lift, 0.0)) * scale;
        scales.push(scale);
    }
    let mut model = bake_model(parts, 64, 64);
    model.part_scales = scales;
    model
}

/// Skeleton: humanoid layout with thin 2×12×2 limbs, 64×32 sheet
/// (`SkeletonModel.createDefaultSkeletonMesh`).
pub fn bake_skeleton_model() -> BakedEntityModel {
    let arm = ModelCube {
        origin: Vec3::new(-1.0, -2.0, -1.0),
        size: Vec3::new(2.0, 12.0, 2.0),
        tex_offset: (40, 16),
        deformation: 0.0,
        mirror: false,
    };
    let leg = ModelCube {
        origin: Vec3::new(-1.0, 0.0, -1.0),
        size: Vec3::new(2.0, 12.0, 2.0),
        tex_offset: (0, 16),
        deformation: 0.0,
        mirror: false,
    };
    bake_model(humanoid_parts(arm, leg, 2.0), 64, 32)
}

/// Creeper: head + upright body + four legs, animated as a quadruped
/// (`CreeperModel.createBodyLayer`). 64×32 sheet.
pub fn bake_creeper_model() -> BakedEntityModel {
    let mut parts = vec![
        EntityPart {
            name: "head".into(),
            offset: Vec3::new(0.0, 6.0, 0.0),
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
            offset: Vec3::new(0.0, 6.0, 0.0),
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
    ];
    let leg = ModelCube {
        origin: Vec3::new(-2.0, 0.0, -2.0),
        size: Vec3::new(4.0, 6.0, 4.0),
        tex_offset: (0, 16),
        deformation: 0.0,
        mirror: false,
    };
    parts.extend(quadruped_legs(2.0, 18.0, -4.0, 4.0, leg, leg));
    bake_model(parts, 64, 32)
}

/// Spider: head, two body segments, and eight legs with per-leg base rotations.
/// `SpiderModel.createSpiderBodyLayer`, 64×32 sheet.
pub fn bake_spider_model() -> BakedEntityModel {
    bake_model(spider_parts(), 64, 32)
}

fn spider_parts() -> Vec<EntityPart> {
    use std::f32::consts::{FRAC_PI_4, FRAC_PI_8};
    // Vanilla middle-leg z-rotation; not a named constant.
    const MID_Z_ROT: f32 = 0.58119464;
    let right_leg = ModelCube {
        origin: Vec3::new(-15.0, -1.0, -1.0),
        size: Vec3::new(16.0, 2.0, 2.0),
        tex_offset: (18, 0),
        deformation: 0.0,
        mirror: false,
    };
    let left_leg = ModelCube {
        origin: Vec3::new(-1.0, -1.0, -1.0),
        size: Vec3::new(16.0, 2.0, 2.0),
        tex_offset: (18, 0),
        deformation: 0.0,
        mirror: true,
    };
    // Base leg poses (x, z) and rotations (yRot, zRot) from vanilla PartPose.
    let leg = |name: &str, x: f32, z: f32, y_rot: f32, z_rot: f32, cube: ModelCube| EntityPart {
        name: name.into(),
        offset: Vec3::new(x, 15.0, z),
        default_rotation: Vec3::new(0.0, y_rot, z_rot),
        cubes: vec![cube],
        parent: None,
    };
    vec![
        EntityPart {
            name: "head".into(),
            offset: Vec3::new(0.0, 15.0, -3.0),
            default_rotation: Vec3::ZERO,
            cubes: vec![ModelCube {
                origin: Vec3::new(-4.0, -4.0, -8.0),
                size: Vec3::new(8.0, 8.0, 8.0),
                tex_offset: (32, 4),
                deformation: 0.0,
                mirror: false,
            }],
            parent: None,
        },
        EntityPart {
            name: "body0".into(),
            offset: Vec3::new(0.0, 15.0, 0.0),
            default_rotation: Vec3::ZERO,
            cubes: vec![ModelCube {
                origin: Vec3::new(-3.0, -3.0, -3.0),
                size: Vec3::new(6.0, 6.0, 6.0),
                tex_offset: (0, 0),
                deformation: 0.0,
                mirror: false,
            }],
            parent: None,
        },
        EntityPart {
            name: "body1".into(),
            offset: Vec3::new(0.0, 15.0, 9.0),
            default_rotation: Vec3::ZERO,
            cubes: vec![ModelCube {
                origin: Vec3::new(-5.0, -4.0, -6.0),
                size: Vec3::new(10.0, 8.0, 12.0),
                tex_offset: (0, 12),
                deformation: 0.0,
                mirror: false,
            }],
            parent: None,
        },
        leg(
            "right_hind_leg",
            -4.0,
            2.0,
            FRAC_PI_4,
            -FRAC_PI_4,
            right_leg,
        ),
        leg("left_hind_leg", 4.0, 2.0, -FRAC_PI_4, FRAC_PI_4, left_leg),
        leg(
            "right_middle_hind_leg",
            -4.0,
            1.0,
            FRAC_PI_8,
            -MID_Z_ROT,
            right_leg,
        ),
        leg(
            "left_middle_hind_leg",
            4.0,
            1.0,
            -FRAC_PI_8,
            MID_Z_ROT,
            left_leg,
        ),
        leg(
            "right_middle_front_leg",
            -4.0,
            0.0,
            -FRAC_PI_8,
            -MID_Z_ROT,
            right_leg,
        ),
        leg(
            "left_middle_front_leg",
            4.0,
            0.0,
            FRAC_PI_8,
            MID_Z_ROT,
            left_leg,
        ),
        leg(
            "right_front_leg",
            -4.0,
            -1.0,
            -FRAC_PI_4,
            -FRAC_PI_4,
            right_leg,
        ),
        leg("left_front_leg", 4.0, -1.0, FRAC_PI_4, FRAC_PI_4, left_leg),
    ]
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
    head_x_rot_deg: f32,
    local_head_y_rot_deg: f32,
    walk_pos: f32,
    walk_speed: f32,
    is_crouching: bool,
) -> PartAnim {
    let mut anim = PartAnim::default();
    let crouch_arm_rot = if is_crouching { 0.4 } else { 0.0 };

    for (i, part) in model.parts.iter().enumerate() {
        let rot = match part.name.as_str() {
            "head" => {
                let rot = Quat::from_rotation_y(local_head_y_rot_deg.to_radians())
                    * Quat::from_rotation_x(head_x_rot_deg.to_radians());
                let (x, y, z) = rot.to_euler(glam::EulerRot::XYZ);
                Vec3::new(x, y, z)
            }
            "body" if is_crouching => Vec3::new(0.5, 0.0, 0.0),
            "right_arm" => Vec3::new(
                (walk_pos * 0.6662 + std::f32::consts::PI).cos() * 2.0 * walk_speed * 0.5
                    + crouch_arm_rot,
                0.0,
                0.0,
            ),
            "left_arm" => Vec3::new(
                (walk_pos * 0.6662).cos() * 2.0 * walk_speed * 0.5 + crouch_arm_rot,
                0.0,
                0.0,
            ),
            "right_leg" => Vec3::new((walk_pos * 0.6662).cos() * 1.4 * walk_speed, 0.0, 0.0),
            "left_leg" => Vec3::new(
                (walk_pos * 0.6662 + std::f32::consts::PI).cos() * 1.4 * walk_speed,
                0.0,
                0.0,
            ),
            _ => continue,
        };
        if is_crouching {
            let translation = match part.name.as_str() {
                "head" => Vec3::new(0.0, 4.2, 0.0),
                "body" | "right_arm" | "left_arm" => Vec3::new(0.0, 3.2, 0.0),
                "right_leg" | "left_leg" => Vec3::new(0.0, 0.0, 4.0),
                _ => Vec3::ZERO,
            };
            if translation != Vec3::ZERO {
                anim.translation.push((i, translation));
            }
        }
        anim.rotation.push((i, rot));
    }

    anim
}

pub fn compute_quadruped_anim(
    model: &BakedEntityModel,
    head_x_rot_deg: f32,
    local_head_y_rot_deg: f32,
    walk_pos: f32,
    walk_speed: f32,
    head_y_offset: f32,
    head_x_rot_deg_override: Option<f32>,
) -> PartAnim {
    let mut anim = PartAnim::default();

    for (i, part) in model.parts.iter().enumerate() {
        let rot = match part.name.as_str() {
            "head" => {
                let rot = Quat::from_rotation_y(local_head_y_rot_deg.to_radians())
                    * Quat::from_rotation_x(
                        head_x_rot_deg_override
                            .unwrap_or(head_x_rot_deg)
                            .to_radians(),
                    );
                let (x, y, z) = rot.to_euler(glam::EulerRot::XYZ);
                Vec3::new(x, y, z)
            }
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

fn head_rotation(head_x_rot_deg: f32, local_head_y_rot_deg: f32) -> Vec3 {
    let rot = Quat::from_rotation_y(local_head_y_rot_deg.to_radians())
        * Quat::from_rotation_x(head_x_rot_deg.to_radians());
    let (x, y, z) = rot.to_euler(glam::EulerRot::XYZ);
    Vec3::new(x, y, z)
}

/// Vanilla `AnimationUtils.bobModelPart`: a gentle idle sway added to undead
/// arms. Returns the (xRot, zRot) delta; `side` is +1.0 for the right arm, -1.0
/// left.
fn bob_arm(age_in_ticks: f32, side: f32) -> (f32, f32) {
    let z = side * ((age_in_ticks * 0.09).cos() * 0.05 + 0.05);
    let x = side * (age_in_ticks * 0.067).sin() * 0.05;
    (x, z)
}

/// Zombie: humanoid head/body/legs, but arms held out forward (the classic
/// zombie pose) and raised higher when aggressive, plus the
/// `AnimationUtils.animateZombieArms` attack swing driven by `attack_time`.
// TODO: hardcodes vanilla's `raiseArms = true` path; a baby zombie holding an item
// needs the `raiseArms = false` path. Implement once held items are rendered.
#[allow(clippy::too_many_arguments)]
pub fn compute_zombie_anim(
    model: &BakedEntityModel,
    head_x_rot_deg: f32,
    local_head_y_rot_deg: f32,
    walk_pos: f32,
    walk_speed: f32,
    aggressive: bool,
    age_in_ticks: f32,
    attack_time: f32,
) -> PartAnim {
    use std::f32::consts::PI;
    let mut anim = PartAnim::default();
    let arm_drop = if aggressive { -PI / 1.5 } else { -PI / 2.25 };
    // Vanilla `AnimationUtils.animateZombieArms` attack swing (0 at both endpoints,
    // peaks mid-swing). Added on top of the held-out idle pose.
    let attack_y = (attack_time * PI).sin();
    let attack_x = ((1.0 - (1.0 - attack_time) * (1.0 - attack_time)) * PI).sin();
    let arm_swing_x = attack_y * 1.2 - attack_x * 0.4;

    for (i, part) in model.parts.iter().enumerate() {
        let rot = match part.name.as_str() {
            "head" => head_rotation(head_x_rot_deg, local_head_y_rot_deg),
            "right_arm" => {
                let (bx, bz) = bob_arm(age_in_ticks, 1.0);
                Vec3::new(arm_drop + arm_swing_x + bx, -0.1 + attack_y * 0.6, bz)
            }
            "left_arm" => {
                let (bx, bz) = bob_arm(age_in_ticks, -1.0);
                Vec3::new(arm_drop + arm_swing_x + bx, 0.1 - attack_y * 0.6, bz)
            }
            "right_leg" => Vec3::new((walk_pos * 0.6662).cos() * 1.4 * walk_speed, 0.0, 0.0),
            "left_leg" => Vec3::new((walk_pos * 0.6662 + PI).cos() * 1.4 * walk_speed, 0.0, 0.0),
            _ => continue,
        };
        anim.rotation.push((i, rot));
    }

    anim
}

/// Skeleton: standard humanoid limb swing; when aggressive the arms take the
/// vanilla `HumanoidModel` `BOW_AND_ARROW` aim pose tracking the head (no held
/// bow item is rendered). `age_in_ticks` is currently unused but kept for
/// parity with the other humanoid anims.
// TODO: aim pose is hardcoded right-handed and gated on `aggressive` alone;
// vanilla keys it off the main arm and a held bow. Implement once held items
// are rendered.
pub fn compute_skeleton_anim(
    model: &BakedEntityModel,
    head_x_rot_deg: f32,
    local_head_y_rot_deg: f32,
    walk_pos: f32,
    walk_speed: f32,
    aggressive: bool,
    _age_in_ticks: f32,
) -> PartAnim {
    use std::f32::consts::{FRAC_PI_2, PI};
    let mut anim = PartAnim::default();
    let head_x = head_x_rot_deg.to_radians();
    let head_y = local_head_y_rot_deg.to_radians();

    for (i, part) in model.parts.iter().enumerate() {
        let rot = match part.name.as_str() {
            "head" => head_rotation(head_x_rot_deg, local_head_y_rot_deg),
            // `poseRightArm` BOW_AND_ARROW (right-handed): both arms point where the
            // head looks, the off hand swung slightly inward.
            "right_arm" if aggressive => Vec3::new(-FRAC_PI_2 + head_x, -0.1 + head_y, 0.0),
            "left_arm" if aggressive => Vec3::new(-FRAC_PI_2 + head_x, 0.1 + head_y + 0.4, 0.0),
            "right_arm" => Vec3::new(
                (walk_pos * 0.6662 + PI).cos() * 2.0 * walk_speed * 0.5,
                0.0,
                0.0,
            ),
            "left_arm" => Vec3::new((walk_pos * 0.6662).cos() * 2.0 * walk_speed * 0.5, 0.0, 0.0),
            "right_leg" => Vec3::new((walk_pos * 0.6662).cos() * 1.4 * walk_speed, 0.0, 0.0),
            "left_leg" => Vec3::new((walk_pos * 0.6662 + PI).cos() * 1.4 * walk_speed, 0.0, 0.0),
            _ => continue,
        };
        anim.rotation.push((i, rot));
    }

    anim
}

/// Spider: head tracking plus the eight-leg gait from `SpiderModel.setupAnim`.
/// Each leg's final rotation is its base pose (`default_rotation`) plus a swing
/// (yRot) and step (zRot) term; four phase offsets stagger the legs.
pub fn compute_spider_anim(
    model: &BakedEntityModel,
    head_x_rot_deg: f32,
    local_head_y_rot_deg: f32,
    walk_pos: f32,
    walk_speed: f32,
) -> PartAnim {
    use std::f32::consts::{FRAC_PI_2, PI};
    let mut anim = PartAnim::default();

    let pos = walk_pos * 0.6662;
    // Per-leg-group swing (yaw) and step (vertical) terms; right legs add, left
    // legs subtract (vanilla `+=`/`-=`).
    let swing = |phase: f32| -((pos * 2.0 + phase).cos() * 0.4) * walk_speed;
    let step = |phase: f32| ((pos + phase).sin() * 0.4).abs() * walk_speed;
    let three_half_pi = 3.0 * FRAC_PI_2;

    // Each leg's exact render-space orientation = F·vanilla·F = Rz(-z)·Ry(+y)
    // (Y unchanged under the Y-flip; X/Z negate). Build it as a quaternion so the
    // engine reproduces vanilla's composition order exactly.
    let leg_quat =
        |full_y: f32, full_z: f32| Quat::from_rotation_z(-full_z) * Quat::from_rotation_y(full_y);

    for (i, part) in model.parts.iter().enumerate() {
        let base = part.default_rotation;
        let q = match part.name.as_str() {
            "head" => {
                anim.rotation
                    .push((i, head_rotation(head_x_rot_deg, local_head_y_rot_deg)));
                continue;
            }
            "right_hind_leg" => leg_quat(base.y + swing(0.0), base.z + step(0.0)),
            "left_hind_leg" => leg_quat(base.y - swing(0.0), base.z - step(0.0)),
            "right_middle_hind_leg" => leg_quat(base.y + swing(PI), base.z + step(PI)),
            "left_middle_hind_leg" => leg_quat(base.y - swing(PI), base.z - step(PI)),
            "right_middle_front_leg" => {
                leg_quat(base.y + swing(FRAC_PI_2), base.z + step(FRAC_PI_2))
            }
            "left_middle_front_leg" => {
                leg_quat(base.y - swing(FRAC_PI_2), base.z - step(FRAC_PI_2))
            }
            "right_front_leg" => {
                leg_quat(base.y + swing(three_half_pi), base.z + step(three_half_pi))
            }
            "left_front_leg" => {
                leg_quat(base.y - swing(three_half_pi), base.z - step(three_half_pi))
            }
            _ => continue,
        };
        anim.rotation_quat.push((i, q));
    }

    anim
}

/// The four corner positions of each cube face, in render space (Y already
/// flipped). Face order: 0 -Z, 1 +Z, 2 +Y, 3 -Y, 4 -X, 5 +X.
fn cube_face_positions(cube: &ModelCube) -> [[[f32; 3]; 4]; 6] {
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

    [
        [[x1, yb, z0], [x0, yb, z0], [x0, yt, z0], [x1, yt, z0]],
        [[x0, yb, z1], [x1, yb, z1], [x1, yt, z1], [x0, yt, z1]],
        [[x0, yt, z0], [x0, yt, z1], [x1, yt, z1], [x1, yt, z0]],
        [[x0, yb, z1], [x0, yb, z0], [x1, yb, z0], [x1, yb, z1]],
        [[x0, yb, z1], [x0, yb, z0], [x0, yt, z0], [x0, yt, z1]],
        [[x1, yb, z0], [x1, yb, z1], [x1, yt, z1], [x1, yt, z0]],
    ]
}

/// Emit two triangles for one quad face, mapping the normalized UV rect onto
/// its corners (`u_min`/`v_min` is the texture's top-left).
fn push_face(
    positions: &[[f32; 3]; 4],
    u_min: f32,
    u_max: f32,
    v_min: f32,
    v_max: f32,
    vertices: &mut Vec<ChunkVertex>,
) {
    let uvs = [
        [u_min, v_max],
        [u_max, v_max],
        [u_max, v_min],
        [u_min, v_min],
    ];
    for &i in &[0usize, 1, 2, 0, 2, 3] {
        vertices.push(ChunkVertex {
            position: positions[i],
            tex_coords: crate::renderer::chunk::mesher::pack_uv(uvs[i][0], uvs[i][1]),
            light_tint: crate::renderer::chunk::mesher::pack_light_tint(
                1.0,
                crate::renderer::chunk::mesher::PACKED_WHITE_SHIFTED,
            ),
        });
    }
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

    // Entity box-unwrap UV rects, face order matching `cube_face_positions`.
    let face_uv = [
        [u0 + d, v0 + d, u0 + d + w, v0 + d + h],
        [u0 + d + w + d, v0 + d, u0 + d + w + d + w, v0 + d + h],
        [u0 + d, v0, u0 + d + w, v0 + d],
        [u0 + d + w, v0, u0 + d + w + w, v0 + d],
        [u0, v0 + d, u0 + d, v0 + d + h],
        [u0 + d + w, v0 + d, u0 + d + w + d, v0 + d + h],
    ];

    let positions = cube_face_positions(cube);

    // Indices 4 (-X) and 5 (+X) are the side faces. When mirror is set, vanilla's
    // minX/maxX swap effectively exchanges their UV regions; every face also has
    // its U flipped.
    for (idx, pos) in positions.iter().enumerate() {
        let src = match (cube.mirror, idx) {
            (true, 4) => &face_uv[5],
            (true, 5) => &face_uv[4],
            _ => &face_uv[idx],
        };
        let v_min = src[1] / th;
        let v_max = src[3] / th;
        let (u_min, u_max) = if cube.mirror {
            (src[2] / tw, src[0] / tw)
        } else {
            (src[0] / tw, src[2] / tw)
        };
        push_face(pos, u_min, u_max, v_min, v_max, vertices);
    }
}

/// Like [`generate_cube_vertices`] but with explicit per-face UV rects (face
/// order -Z, +Z, +Y, -Y, -X, +X) instead of the entity box-unwrap, for block
/// models whose texture layout isn't a box-unwrap (e.g. signs).
pub(crate) fn generate_cube_vertices_faces(
    cube: &ModelCube,
    face_uvs: &[[f32; 4]; 6],
    tex_w: u32,
    tex_h: u32,
    vertices: &mut Vec<ChunkVertex>,
) {
    let tw = tex_w as f32;
    let th = tex_h as f32;
    let positions = cube_face_positions(cube);
    for (pos, uv) in positions.iter().zip(face_uvs) {
        push_face(
            pos,
            uv[0] / tw,
            uv[2] / tw,
            uv[1] / th,
            uv[3] / th,
            vertices,
        );
    }
}
