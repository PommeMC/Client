use glam::{Mat4, Quat, Vec3};

use super::chunk::mesher::ChunkVertex;

#[derive(Clone, Copy)]
pub struct ModelCube {
    pub origin: Vec3,
    pub size: Vec3,
    /// Signed: several vanilla fish parts use negative texOffs.
    pub tex_offset: (i32, i32),
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

/// Mirror a cube's geometry across x=0 WITHOUT flipping UVs (e.g. chicken
/// wings and legs share one un-mirrored texture — vanilla quirk). Pair with
/// `mirror: true` where vanilla's `.mirror()` UV flip is also wanted.
fn mirror_x_geom(c: ModelCube) -> ModelCube {
    ModelCube {
        origin: Vec3::new(-(c.origin.x + c.size.x), c.origin.y, c.origin.z),
        ..c
    }
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
    /// Vanilla entity convention: cube Y negated at bake, root pivots at
    /// `(24 - y)/16` (child pivots just negate y), euler signs (-x, -y, +z).
    /// All mob models use this.
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

            let pivot = part.offset + extra_translation;
            let offset = match self.convention {
                ModelConvention::EntityYDown => {
                    // The +24 re-bases vanilla's y-down ground plane onto the
                    // engine's y-up origin; child pivots are relative to their
                    // parent (already re-based), so they only mirror.
                    let rebase = if part.parent.is_some() { 0.0 } else { 24.0 };
                    Vec3::new(pivot.x, rebase - pivot.y, pivot.z)
                }
                ModelConvention::BlockYUp => pivot,
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
        mirror: true,
        ..mirror_x_geom(c)
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

/// Vanilla `CubeDeformation` inflate applied mesh-wide (vanilla rebuilds
/// layer meshes at `createMesh(g)`): every cube grows by `g` on top of its
/// own deformation.
fn inflate(parts: &mut [EntityPart], g: f32) {
    for part in parts {
        for cube in &mut part.cubes {
            cube.deformation += g;
        }
    }
}

/// Vanilla `BabyZombieModel.createBodyLayer(g)`: a dedicated 64x64 baby mesh
/// with its own UVs, not a scaled adult. `g` inflates everything but the
/// head, whose second cube is a fixed 0.25 overlay.
fn baby_zombie_parts(g: f32) -> Vec<EntityPart> {
    let limb = |name: &str, pivot: Vec3, uv: (i32, i32), origin_y: f32, h: f32| {
        vpart(
            name,
            None,
            pivot,
            vec![ModelCube {
                deformation: g,
                ..vbox(uv, (-1.0, origin_y, -1.0), (2.0, h, 2.0))
            }],
        )
    };
    vec![
        vpart(
            "body",
            None,
            Vec3::new(0.0, 17.5, 0.0),
            vec![ModelCube {
                deformation: g,
                ..vbox((16, 16), (-2.0, -2.5, -1.0), (4.0, 5.0, 2.0))
            }],
        ),
        vpart(
            "head",
            None,
            Vec3::new(0.0, 15.25, 0.0),
            vec![
                vbox((3, 3), (-3.0, -6.25, -3.0), (6.0, 6.0, 6.0)),
                ModelCube {
                    deformation: 0.25,
                    ..vbox((35, 3), (-3.0, -6.15, -3.0), (6.0, 6.0, 6.0))
                },
            ],
        ),
        limb("right_arm", Vec3::new(-3.0, 15.5, 0.0), (36, 16), -0.5, 5.0),
        limb("left_arm", Vec3::new(3.0, 15.5, 0.0), (28, 16), -0.5, 5.0),
        limb("right_leg", Vec3::new(-1.0, 20.0, 0.0), (8, 16), 0.0, 4.0),
        limb("left_leg", Vec3::new(1.0, 20.0, 0.0), (0, 16), 0.0, 4.0),
    ]
}

pub fn bake_baby_zombie_model() -> BakedEntityModel {
    bake_model(baby_zombie_parts(0.0), 64, 64)
}

/// Husk: the zombie mesh under vanilla's `MeshTransformer.scaling(1.0625)`
/// (`ModelLayers.HUSK`). The baby husk is NOT scaled.
pub fn bake_husk_model() -> BakedEntityModel {
    bake_scaled(zombie_parts(), 1.0625, 64)
}

/// Vanilla `DrownedModel.createBodyLayer(g)`: the zombie mesh with the left
/// arm and leg given their own UV regions instead of mirrored ones. `g` 0.25
/// is the clothing layer.
// TODO: swim pose and body pitch (`DrownedModel.setupAnim` swimAmount path)
// once a swim_amount ramp from the pose metadata exists.
pub fn bake_drowned_model(g: f32) -> BakedEntityModel {
    let mut parts = zombie_parts();
    // humanoid_parts order: 3 = left_arm, 5 = left_leg.
    parts[3].cubes = vec![vbox((32, 48), (-1.0, -2.0, -2.0), (4.0, 12.0, 4.0))];
    parts[5].cubes = vec![vbox((16, 48), (-2.0, 0.0, -2.0), (4.0, 12.0, 4.0))];
    inflate(&mut parts, g);
    bake_model(parts, 64, 64)
}

/// Vanilla `BabyDrownedModel` delegates to `BabyZombieModel` — zombie UVs,
/// not the drowned left-limb remap.
pub fn bake_baby_drowned_outer_model() -> BakedEntityModel {
    bake_model(baby_zombie_parts(0.25), 64, 64)
}

/// Vanilla `ZombieVillagerModel`: villager-shaped 10-tall head (the nose is a
/// cube inside the head part) and jacketed body on zombie-animated limbs,
/// 64x64 sheet. NOT villager-scaled (`LayerDefinitions` applies no
/// `villagerLikeScale` here).
fn zombie_villager_parts() -> Vec<EntityPart> {
    vec![
        vpart(
            "head",
            None,
            Vec3::ZERO,
            vec![
                vbox((0, 0), (-4.0, -10.0, -4.0), (8.0, 10.0, 8.0)),
                // Nose.
                vbox((24, 0), (-1.0, -3.0, -6.0), (2.0, 4.0, 2.0)),
            ],
        ),
        vpart(
            "hat",
            Some(0),
            Vec3::ZERO,
            vec![ModelCube {
                deformation: 0.5,
                ..vbox((32, 0), (-4.0, -10.0, -4.0), (8.0, 10.0, 8.0))
            }],
        ),
        EntityPart {
            default_rotation: Vec3::new(-std::f32::consts::FRAC_PI_2, 0.0, 0.0),
            ..vpart(
                "hat_rim",
                Some(1),
                Vec3::ZERO,
                vec![vbox((30, 47), (-8.0, -8.0, -6.0), (16.0, 16.0, 1.0))],
            )
        },
        vpart(
            "body",
            None,
            Vec3::ZERO,
            vec![
                vbox((16, 20), (-4.0, 0.0, -3.0), (8.0, 12.0, 6.0)),
                // Jacket.
                ModelCube {
                    deformation: 0.05,
                    ..vbox((0, 38), (-4.0, 0.0, -3.0), (8.0, 20.0, 6.0))
                },
            ],
        ),
        vpart(
            "right_arm",
            None,
            Vec3::new(-5.0, 2.0, 0.0),
            vec![vbox((44, 22), (-3.0, -2.0, -2.0), (4.0, 12.0, 4.0))],
        ),
        vpart(
            "left_arm",
            None,
            Vec3::new(5.0, 2.0, 0.0),
            vec![ModelCube {
                mirror: true,
                ..vbox((44, 22), (-1.0, -2.0, -2.0), (4.0, 12.0, 4.0))
            }],
        ),
        vpart(
            "right_leg",
            None,
            Vec3::new(-2.0, 12.0, 0.0),
            vec![vbox((0, 22), (-2.0, 0.0, -2.0), (4.0, 12.0, 4.0))],
        ),
        vpart(
            "left_leg",
            None,
            Vec3::new(2.0, 12.0, 0.0),
            vec![ModelCube {
                mirror: true,
                ..vbox((0, 22), (-2.0, 0.0, -2.0), (4.0, 12.0, 4.0))
            }],
        ),
    ]
}

pub fn bake_zombie_villager_model(no_hat: bool) -> BakedEntityModel {
    let mut parts = zombie_villager_parts();
    if no_hat {
        clear_head_subtree(&mut parts);
    }
    bake_model(parts, 64, 64)
}

/// Vanilla `BabyZombieVillagerModel`: hand-authored 64x64 baby mesh with real
/// arm/leg parts (humanoid-animated, unlike the crossed-arm baby villager).
/// The hat_rim hangs off the head, not the hat.
fn baby_zombie_villager_parts() -> Vec<EntityPart> {
    let limb = |name: &str, pivot: Vec3, uv: (i32, i32), h: f32| {
        vpart(
            name,
            None,
            pivot,
            vec![vbox(uv, (-1.0, -0.5, -1.0), (2.0, h, 2.0))],
        )
    };
    vec![
        vpart(
            "body",
            None,
            Vec3::new(0.0, 18.75, 0.0),
            vec![
                vbox((0, 15), (-2.0, -2.75, -1.5), (4.0, 5.0, 3.0)),
                ModelCube {
                    deformation: 0.1,
                    ..vbox((16, 22), (-2.0, -2.75, -1.5), (4.0, 6.0, 3.0))
                },
            ],
        ),
        vpart(
            "head",
            None,
            Vec3::new(0.0, 16.0, 0.0),
            vec![vbox((0, 0), (-4.0, -8.0, -3.5), (8.0, 8.0, 7.0))],
        ),
        vpart(
            "hat",
            Some(1),
            Vec3::new(0.0, -4.0, 0.0),
            vec![ModelCube {
                deformation: 0.3,
                ..vbox((0, 31), (-4.0, -4.0, -3.5), (8.0, 8.0, 7.0))
            }],
        ),
        vpart(
            "hat_rim",
            Some(1),
            Vec3::new(0.0, -4.5, 0.0),
            vec![vbox((0, 46), (-7.0, -0.5, -6.0), (14.0, 1.0, 12.0))],
        ),
        vpart(
            "nose",
            Some(1),
            Vec3::new(0.0, -1.0, -4.0),
            vec![vbox((23, 0), (-1.0, -1.0, -0.5), (2.0, 2.0, 1.0))],
        ),
        limb("right_arm", Vec3::new(-3.0, 15.5, 0.0), (24, 15), 5.0),
        limb("left_arm", Vec3::new(3.0, 15.5, 0.0), (16, 15), 5.0),
        limb("right_leg", Vec3::new(-1.0, 21.5, 0.0), (8, 23), 3.0),
        limb("left_leg", Vec3::new(1.0, 21.5, 0.0), (0, 23), 3.0),
    ]
}

pub fn bake_baby_zombie_villager_model(no_hat: bool) -> BakedEntityModel {
    let mut parts = baby_zombie_villager_parts();
    if no_hat {
        clear_head_subtree(&mut parts);
    }
    bake_model(parts, 64, 64)
}

/// Bakes a flat part list under a synthetic cubeless root scaled by `factor`
/// (vanilla `MeshTransformer.scaling` applied at render time, so UVs stay on
/// the unscaled boxes). `factor` 1.0 still adds the root so scaled and
/// unscaled bakes share one part order.
fn bake_root_scaled(
    parts: Vec<EntityPart>,
    factor: f32,
    tex_w: u32,
    tex_h: u32,
) -> BakedEntityModel {
    let mut all = vec![vpart(
        "root",
        None,
        Vec3::new(0.0, 24.016 * (1.0 - factor), 0.0),
        vec![],
    )];
    all.extend(parts.into_iter().map(|mut part| {
        part.parent = Some(part.parent.map_or(0, |p| p + 1));
        part
    }));
    let mut model = bake_model(all, tex_w, tex_h);
    model.part_scales[0] = factor;
    model
}

/// Vanilla `AdultWolfModel.createBodyLayer(g)`, 64x32. The `head` and `tail`
/// parts are cubeless pivot containers; look goes on the container while the
/// wet-shake roll and beg tilt go on the `real_*` child.
fn wolf_parts(g: f32) -> Vec<EntityPart> {
    let leg = |name: &str, x: f32, z: f32, mirror: bool| {
        vpart(
            name,
            None,
            Vec3::new(x, 16.0, z),
            vec![ModelCube {
                deformation: g,
                mirror,
                ..vbox((0, 18), (0.0, 0.0, -1.0), (2.0, 8.0, 2.0))
            }],
        )
    };
    let cube = |uv: (i32, i32), origin: (f32, f32, f32), size: (f32, f32, f32)| ModelCube {
        deformation: g,
        ..vbox(uv, origin, size)
    };
    vec![
        vpart("head", None, Vec3::new(-1.0, 13.5, -7.0), vec![]),
        vpart(
            "real_head",
            Some(0),
            Vec3::ZERO,
            vec![
                cube((0, 0), (-2.0, -3.0, -2.0), (6.0, 6.0, 4.0)),
                // Both ears share one UV patch (vanilla quirk).
                cube((16, 14), (-2.0, -5.0, 0.0), (2.0, 2.0, 1.0)),
                cube((16, 14), (2.0, -5.0, 0.0), (2.0, 2.0, 1.0)),
                cube((0, 10), (-0.5, -0.001, -5.0), (3.0, 3.0, 4.0)),
            ],
        ),
        EntityPart {
            default_rotation: Vec3::new(std::f32::consts::FRAC_PI_2, 0.0, 0.0),
            ..vpart(
                "body",
                None,
                Vec3::new(0.0, 14.0, 2.0),
                vec![cube((18, 14), (-3.0, -2.0, -3.0), (6.0, 9.0, 6.0))],
            )
        },
        // The mane.
        EntityPart {
            default_rotation: Vec3::new(std::f32::consts::FRAC_PI_2, 0.0, 0.0),
            ..vpart(
                "upper_body",
                None,
                Vec3::new(-1.0, 14.0, -3.0),
                vec![cube((21, 0), (-3.0, -3.0, -3.0), (8.0, 6.0, 7.0))],
            )
        },
        leg("right_hind_leg", -2.5, 7.0, true),
        leg("left_hind_leg", 0.5, 7.0, false),
        leg("right_front_leg", -2.5, -4.0, true),
        leg("left_front_leg", 0.5, -4.0, false),
        EntityPart {
            default_rotation: Vec3::new(0.62831855, 0.0, 0.0),
            ..vpart("tail", None, Vec3::new(-1.0, 12.0, 8.0), vec![])
        },
        vpart(
            "real_tail",
            Some(8),
            Vec3::ZERO,
            vec![cube((9, 18), (0.0, 0.0, -1.0), (2.0, 8.0, 2.0))],
        ),
    ]
}

pub fn bake_wolf_model() -> BakedEntityModel {
    bake_model(wolf_parts(0.0), 64, 32)
}

// TODO: wolf armor layer (needs the equipment-asset pipeline).
pub fn bake_wolf_collar_model() -> BakedEntityModel {
    bake_model(wolf_parts(0.0), 64, 32)
}

/// Vanilla `BabyWolfModel`, 32x32 dedicated mesh: head cubes live directly on
/// `head` with separate ear parts, no mane, and the shake targets `head`/`tail`
/// themselves.
fn baby_wolf_parts() -> Vec<EntityPart> {
    let leg = |name: &str, x: f32, z: f32, uv: (i32, i32)| {
        vpart(
            name,
            None,
            Vec3::new(x, 21.0, z),
            vec![vbox(uv, (-1.0, 0.0, -1.0), (2.0, 3.0, 2.0))],
        )
    };
    vec![
        vpart(
            "head",
            None,
            Vec3::new(0.0, 18.25, -4.0),
            vec![
                ModelCube {
                    deformation: 0.025,
                    ..vbox((0, 12), (-2.99, -3.25, -3.0), (6.0, 5.0, 5.0))
                },
                vbox((17, 12), (-1.5, -0.24, -5.0), (3.0, 2.0, 2.0)),
            ],
        ),
        vpart(
            "right_ear",
            Some(0),
            Vec3::new(-2.0, -4.25, -0.5),
            vec![vbox((0, 5), (-1.0, -1.0, -0.5), (2.0, 2.0, 1.0))],
        ),
        vpart(
            "left_ear",
            Some(0),
            Vec3::new(2.0, -4.25, -0.5),
            vec![vbox((20, 5), (-1.0, -1.0, -0.5), (2.0, 2.0, 1.0))],
        ),
        vpart(
            "body",
            None,
            Vec3::new(0.0, 19.0, 0.0),
            vec![vbox((0, 0), (-3.0, -2.0, -4.0), (6.0, 4.0, 8.0))],
        ),
        leg("right_hind_leg", -1.5, 3.0, (0, 22)),
        leg("left_hind_leg", 1.5, 3.0, (8, 22)),
        // The right front leg reuses the body's UV origin (vanilla quirk).
        leg("right_front_leg", -1.5, -3.0, (0, 0)),
        leg("left_front_leg", 1.5, -3.0, (20, 0)),
        EntityPart {
            default_rotation: Vec3::new(-std::f32::consts::FRAC_PI_6, 0.0, 0.0),
            ..vpart("tail", None, Vec3::new(0.0, 19.0, 3.0), vec![])
        },
        EntityPart {
            default_rotation: Vec3::new(-3.1, 0.0, 0.0),
            ..vpart(
                "tail_r1",
                Some(8),
                Vec3::new(0.0, -0.6, 0.2),
                vec![vbox((22, 16), (-1.0, -5.7, -1.0), (2.0, 6.0, 2.0))],
            )
        },
    ]
}

pub fn bake_baby_wolf_model() -> BakedEntityModel {
    bake_model(baby_wolf_parts(), 32, 32)
}

/// Vanilla `AdultFelineModel.createBodyMesh(g)`, 64x32, shared by cat and
/// ocelot. `tail2` keeps its hardcoded -0.02 deformation even in the collar
/// bake. Left/right leg pairs share one UV patch (vanilla quirk).
fn feline_parts(g: f32) -> Vec<EntityPart> {
    let cube = |uv: (i32, i32), origin: (f32, f32, f32), size: (f32, f32, f32)| ModelCube {
        deformation: g,
        ..vbox(uv, origin, size)
    };
    let leg = |name: &str, x: f32, y: f32, z: f32, uv: (i32, i32), origin_z: f32, h: f32| {
        vpart(
            name,
            None,
            Vec3::new(x, y, z),
            vec![cube(uv, (-1.0, 0.0, origin_z), (2.0, h, 2.0))],
        )
    };
    vec![
        vpart(
            "head",
            None,
            Vec3::new(0.0, 15.0, -9.0),
            vec![
                cube((0, 0), (-2.5, -2.0, -3.0), (5.0, 4.0, 5.0)),
                cube((0, 24), (-1.5, -0.001, -4.0), (3.0, 2.0, 2.0)),
                cube((0, 10), (-2.0, -3.0, 0.0), (1.0, 1.0, 2.0)),
                cube((6, 10), (1.0, -3.0, 0.0), (1.0, 1.0, 2.0)),
            ],
        ),
        EntityPart {
            default_rotation: Vec3::new(std::f32::consts::FRAC_PI_2, 0.0, 0.0),
            ..vpart(
                "body",
                None,
                Vec3::new(0.0, 12.0, -10.0),
                vec![cube((20, 0), (-2.0, 3.0, -8.0), (4.0, 16.0, 6.0))],
            )
        },
        EntityPart {
            default_rotation: Vec3::new(0.9, 0.0, 0.0),
            ..vpart(
                "tail1",
                None,
                Vec3::new(0.0, 15.0, 8.0),
                vec![cube((0, 15), (-0.5, 0.0, 0.0), (1.0, 8.0, 1.0))],
            )
        },
        vpart(
            "tail2",
            None,
            Vec3::new(0.0, 20.0, 14.0),
            vec![ModelCube {
                deformation: -0.02,
                ..vbox((4, 15), (-0.5, 0.0, 0.0), (1.0, 8.0, 1.0))
            }],
        ),
        leg("left_hind_leg", 1.1, 18.0, 5.0, (8, 13), 1.0, 6.0),
        leg("right_hind_leg", -1.1, 18.0, 5.0, (8, 13), 1.0, 6.0),
        leg("left_front_leg", 1.2, 14.1, -5.0, (40, 0), 0.0, 10.0),
        leg("right_front_leg", -1.2, 14.1, -5.0, (40, 0), 0.0, 10.0),
    ]
}

/// Vanilla `BabyFelineModel`, 32x32 dedicated mesh. `tail2` is kept as an
/// empty part for name parity with the adult.
fn baby_feline_parts() -> Vec<EntityPart> {
    let leg = |name: &str, x: f32, z: f32, uv: (i32, i32)| {
        vpart(
            name,
            None,
            Vec3::new(x, 22.0, z),
            vec![vbox(uv, (-0.5, 0.0, -1.0), (1.0, 2.0, 2.0))],
        )
    };
    vec![
        vpart(
            "head",
            None,
            Vec3::new(0.0, 20.0, -3.125),
            vec![
                vbox((0, 0), (-2.5, -3.0, -2.875), (5.0, 4.0, 4.0)),
                vbox((18, 0), (-2.0, -4.0, -0.875), (1.0, 1.0, 2.0)),
                vbox((24, 0), (1.0, -4.0, -0.875), (1.0, 1.0, 2.0)),
                vbox((18, 3), (-1.5, -1.0, -3.875), (3.0, 2.0, 1.0)),
            ],
        ),
        leg("left_front_leg", 1.0, -1.5, (18, 18)),
        leg("right_front_leg", -1.0, -1.5, (12, 18)),
        leg("left_hind_leg", 1.0, 2.5, (18, 22)),
        vpart(
            "body",
            None,
            Vec3::new(0.0, 20.5, 0.5),
            vec![vbox((0, 8), (-2.0, -1.5, -3.5), (4.0, 3.0, 7.0))],
        ),
        leg("right_hind_leg", -1.0, 2.5, (12, 22)),
        EntityPart {
            default_rotation: Vec3::new(-0.567232, 0.0, 0.0),
            ..vpart(
                "tail1",
                None,
                Vec3::new(0.0, 19.107, 3.9151),
                vec![vbox((0, 18), (-0.5, -0.107, 0.0849), (1.0, 1.0, 5.0))],
            )
        },
        vpart("tail2", None, Vec3::ZERO, vec![]),
    ]
}

/// The cat renders the feline mesh under a 0.8 render-time root scale
/// (`AdultCatModel.CAT_TRANSFORMER`); the collar bakes inflate by 0.01 (adult)
/// or scale 1.01 (baby). The unscaled baby base still gets a root part so
/// base and collar share one part order.
pub fn bake_cat_model() -> BakedEntityModel {
    bake_root_scaled(feline_parts(0.0), 0.8, 64, 32)
}

pub fn bake_cat_collar_model() -> BakedEntityModel {
    bake_root_scaled(feline_parts(0.01), 0.8, 64, 32)
}

pub fn bake_baby_cat_model() -> BakedEntityModel {
    bake_root_scaled(baby_feline_parts(), 1.0, 32, 32)
}

pub fn bake_baby_cat_collar_model() -> BakedEntityModel {
    bake_root_scaled(baby_feline_parts(), 1.01, 32, 32)
}

pub fn bake_ocelot_model() -> BakedEntityModel {
    bake_model(feline_parts(0.0), 64, 32)
}

pub fn bake_baby_ocelot_model() -> BakedEntityModel {
    bake_model(baby_feline_parts(), 32, 32)
}

/// Vanilla `AdultRabbitModel`, 64x64. `frontlegs`/`backlegs` and the hind-leg
/// parts are cubeless pivot containers; the haunches hang off the hind legs.
fn rabbit_parts() -> Vec<EntityPart> {
    vec![
        EntityPart {
            default_rotation: Vec3::new(-std::f32::consts::FRAC_PI_8, 0.0, 0.0),
            ..vpart(
                "body",
                None,
                Vec3::new(0.0, 23.0, 4.0),
                vec![vbox((0, 0), (-4.0, -6.0, -9.0), (8.0, 6.0, 10.0))],
            )
        },
        vpart(
            "tail",
            Some(0),
            Vec3::new(0.0, -4.9916, 0.0125),
            vec![vbox((20, 16), (-2.0, -3.0084, -1.0125), (4.0, 4.0, 4.0))],
        ),
        EntityPart {
            default_rotation: Vec3::new(std::f32::consts::FRAC_PI_8, 0.0, 0.0),
            ..vpart(
                "head",
                Some(0),
                Vec3::new(0.0, -5.2929, -8.1213),
                vec![vbox((0, 16), (-2.5, -3.0, -4.0), (5.0, 5.0, 5.0))],
            )
        },
        vpart(
            "left_ear",
            Some(2),
            Vec3::new(1.5, -3.7071, -0.8787),
            vec![vbox((32, 0), (-1.0, -4.2929, -0.1213), (2.0, 5.0, 1.0))],
        ),
        vpart(
            "right_ear",
            Some(2),
            Vec3::new(-1.5, -3.7071, -0.8787),
            vec![vbox((26, 0), (-1.0, -4.2929, -0.1213), (2.0, 5.0, 1.0))],
        ),
        vpart(
            "frontlegs",
            Some(0),
            Vec3::new(0.0, -1.5349, -6.3108),
            vec![],
        ),
        EntityPart {
            default_rotation: Vec3::new(std::f32::consts::FRAC_PI_8, 0.0, 0.0),
            ..vpart(
                "right_front_leg",
                Some(5),
                Vec3::new(-2.0, 1.9239, 0.3827),
                vec![vbox((36, 18), (-0.9, -1.0, -0.9), (2.0, 4.0, 2.0))],
            )
        },
        EntityPart {
            default_rotation: Vec3::new(std::f32::consts::FRAC_PI_8, 0.0, 0.0),
            ..vpart(
                "left_front_leg",
                Some(5),
                Vec3::new(2.0, 1.9239, 0.4827),
                vec![vbox((44, 18), (-1.0, -1.0, -1.0), (2.0, 4.0, 2.0))],
            )
        },
        vpart("backlegs", None, Vec3::new(0.0, 23.0, 4.0), vec![]),
        vpart("right_hind_leg", Some(8), Vec3::new(-3.0, 0.5, 0.0), vec![]),
        EntityPart {
            default_rotation: Vec3::new(0.0, std::f32::consts::FRAC_PI_8, 0.0),
            ..vpart(
                "right_haunch",
                Some(9),
                Vec3::new(0.0, -0.5, 0.0),
                vec![vbox((20, 24), (-1.0, 0.0, -5.0), (2.0, 1.0, 6.0))],
            )
        },
        vpart("left_hind_leg", Some(8), Vec3::new(3.0, 0.5, 0.0), vec![]),
        EntityPart {
            default_rotation: Vec3::new(0.0, -std::f32::consts::FRAC_PI_8, 0.0),
            ..vpart(
                "left_haunch",
                Some(11),
                Vec3::new(0.0, -0.5, 0.0),
                vec![vbox((36, 24), (-1.0, 0.0, -5.0), (2.0, 1.0, 6.0))],
            )
        },
    ]
}

pub fn bake_rabbit_model() -> BakedEntityModel {
    bake_model(rabbit_parts(), 64, 64)
}

/// Vanilla `BabyRabbitModel`, 32x32: deeper nesting with `_r1`
/// rotation-carrier parts.
fn baby_rabbit_parts() -> Vec<EntityPart> {
    vec![
        vpart("body", None, Vec3::new(0.0, 23.0, 1.6), vec![]),
        EntityPart {
            default_rotation: Vec3::new(-std::f32::consts::FRAC_PI_6, 0.0, 0.0),
            ..vpart(
                "body_r1",
                Some(0),
                Vec3::new(0.0, -2.0, -1.6),
                vec![vbox((0, 8), (-2.0, -2.0, -3.0), (4.0, 3.0, 6.0))],
            )
        },
        vpart("tail", Some(0), Vec3::new(0.0, -2.2, 2.0), vec![]),
        EntityPart {
            default_rotation: Vec3::new(-std::f32::consts::FRAC_PI_6, 0.0, 0.0),
            ..vpart(
                "tail_r1",
                Some(2),
                Vec3::new(-0.1, 0.0, 0.0),
                vec![vbox((0, 21), (-1.4, -2.0268, -1.0177), (3.0, 3.0, 3.0))],
            )
        },
        vpart(
            "head",
            Some(0),
            Vec3::new(0.0, -5.0, -2.6),
            vec![vbox((0, 0), (-2.5, -3.0, -3.0), (5.0, 4.0, 4.0))],
        ),
        vpart(
            "right_ear",
            Some(4),
            Vec3::new(-1.5, -3.5, -0.5),
            vec![vbox((18, 0), (-1.0, -3.5, -0.5), (2.0, 4.0, 1.0))],
        ),
        vpart(
            "left_ear",
            Some(4),
            Vec3::new(1.5, -3.5, -0.5),
            vec![vbox((24, 0), (-1.0, -3.5, -0.5), (2.0, 4.0, 1.0))],
        ),
        vpart("frontlegs", Some(0), Vec3::new(0.0, -2.5, -2.6), vec![]),
        EntityPart {
            default_rotation: Vec3::new(std::f32::consts::FRAC_PI_8, 0.0, 0.0),
            ..vpart("left_front_leg", Some(7), Vec3::new(1.0, 1.0, -0.5), vec![])
        },
        EntityPart {
            default_rotation: Vec3::new(-std::f32::consts::FRAC_PI_8, 0.0, 0.0),
            ..vpart(
                "left_front_leg_r1",
                Some(8),
                Vec3::new(0.0, 1.0, 0.0),
                vec![vbox((18, 8), (-0.5, -1.5, -0.5), (1.0, 3.0, 1.0))],
            )
        },
        EntityPart {
            default_rotation: Vec3::new(std::f32::consts::FRAC_PI_8, 0.0, 0.0),
            ..vpart(
                "right_front_leg",
                Some(7),
                Vec3::new(-1.0, 1.0, -0.5),
                vec![],
            )
        },
        EntityPart {
            default_rotation: Vec3::new(-std::f32::consts::FRAC_PI_8, 0.0, 0.0),
            ..vpart(
                "right_front_leg_r1",
                Some(10),
                Vec3::new(0.0, 1.0, 0.0),
                vec![vbox((14, 8), (-0.5, -1.5, -0.5), (1.0, 3.0, 1.0))],
            )
        },
        vpart("backlegs", None, Vec3::new(0.0, 23.0, 2.0), vec![]),
        EntityPart {
            default_rotation: Vec3::new(0.0, std::f32::consts::PI, 0.0),
            ..vpart("left_hind_leg", Some(12), Vec3::new(1.5, 0.5, 0.5), vec![])
        },
        EntityPart {
            default_rotation: Vec3::new(0.0, -std::f32::consts::FRAC_PI_4, 0.0),
            ..vpart(
                "left_haunch",
                Some(13),
                Vec3::new(1.0, 0.0, 0.5),
                vec![vbox((10, 17), (-2.0, -0.5, 0.0), (2.0, 1.0, 3.0))],
            )
        },
        EntityPart {
            default_rotation: Vec3::new(0.0, std::f32::consts::PI, 0.0),
            ..vpart(
                "right_hind_leg",
                Some(12),
                Vec3::new(-1.5, 0.5, 0.5),
                vec![],
            )
        },
        EntityPart {
            default_rotation: Vec3::new(0.0, std::f32::consts::FRAC_PI_4, 0.0),
            ..vpart(
                "right_haunch",
                Some(15),
                Vec3::new(0.5, 0.0, -0.9),
                vec![vbox((0, 17), (-2.0, -0.5, 0.0), (2.0, 1.0, 3.0))],
            )
        },
    ]
}

pub fn bake_baby_rabbit_model() -> BakedEntityModel {
    bake_model(baby_rabbit_parts(), 32, 32)
}

/// Vanilla `AbstractEquineModel.createBodyMesh(NONE)`, 64x64 (shared by
/// horse, donkey/mule, skeleton/zombie horse). The body's 0.05 and the ears'
/// -0.001 deformations are hardcoded in vanilla.
fn equine_parts() -> Vec<EntityPart> {
    let leg = |name: &str, x: f32, z: f32, origin: (f32, f32, f32), mirror: bool| {
        vpart(
            name,
            None,
            Vec3::new(x, 14.0, z),
            vec![ModelCube {
                mirror,
                ..vbox((48, 21), origin, (4.0, 11.0, 4.0))
            }],
        )
    };
    vec![
        vpart(
            "body",
            None,
            Vec3::new(0.0, 11.0, 5.0),
            vec![ModelCube {
                deformation: 0.05,
                ..vbox((0, 32), (-5.0, -8.0, -17.0), (10.0, 10.0, 22.0))
            }],
        ),
        // The neck; look and the eat/rear poses drive this part.
        EntityPart {
            default_rotation: Vec3::new(std::f32::consts::FRAC_PI_6, 0.0, 0.0),
            ..vpart(
                "head_parts",
                None,
                Vec3::new(0.0, 4.0, -12.0),
                vec![vbox((0, 35), (-2.05, -6.0, -2.0), (4.0, 12.0, 7.0))],
            )
        },
        vpart(
            "head",
            Some(1),
            Vec3::ZERO,
            vec![vbox((0, 13), (-3.0, -11.0, -2.0), (6.0, 5.0, 7.0))],
        ),
        vpart(
            "mane",
            Some(1),
            Vec3::ZERO,
            vec![vbox((56, 36), (-1.0, -11.0, 5.01), (2.0, 16.0, 2.0))],
        ),
        vpart(
            "upper_mouth",
            Some(1),
            Vec3::ZERO,
            vec![vbox((0, 25), (-2.0, -11.0, -7.0), (4.0, 5.0, 5.0))],
        ),
        leg("left_hind_leg", 4.0, 7.0, (-3.0, -1.01, -1.0), true),
        leg("right_hind_leg", -4.0, 7.0, (-1.0, -1.01, -1.0), false),
        leg("left_front_leg", 4.0, -10.0, (-3.0, -1.01, -1.9), true),
        leg("right_front_leg", -4.0, -10.0, (-1.0, -1.01, -1.9), false),
        EntityPart {
            default_rotation: Vec3::new(std::f32::consts::FRAC_PI_6, 0.0, 0.0),
            ..vpart(
                "tail",
                Some(0),
                Vec3::new(0.0, -5.0, 2.0),
                vec![vbox((42, 36), (-1.5, 0.0, 0.0), (3.0, 14.0, 4.0))],
            )
        },
        vpart(
            "left_ear",
            Some(2),
            Vec3::ZERO,
            vec![ModelCube {
                deformation: -0.001,
                ..vbox((19, 16), (0.55, -13.0, 4.0), (2.0, 3.0, 1.0))
            }],
        ),
        vpart(
            "right_ear",
            Some(2),
            Vec3::ZERO,
            vec![ModelCube {
                deformation: -0.001,
                ..vbox((19, 16), (-2.55, -13.0, 4.0), (2.0, 3.0, 1.0))
            }],
        ),
    ]
}

pub fn bake_horse_model() -> BakedEntityModel {
    // `ModelLayers.HORSE` runs through `MeshTransformer.scaling(1.1)`; the
    // skeleton/zombie horses use the same mesh UNSCALED. The root-scaled
    // form keeps `setupAnim`'s pivot offsets in scaled space like vanilla
    // (`bake_scaled` pre-scales pivots, leaving runtime deltas unscaled).
    bake_root_scaled(equine_parts(), 1.1, 64, 64)
}

pub fn bake_undead_horse_model() -> BakedEntityModel {
    bake_model(equine_parts(), 64, 64)
}

/// Vanilla `DonkeyModel.modifyMesh`: donkey ears replace the horse ears and
/// two chest boxes hang off the body. `chest` false keeps the chest parts
/// with no cubes so both variants share one part order.
fn donkey_parts(chest: bool) -> Vec<EntityPart> {
    let mut parts = equine_parts();
    let ear = |name: &str, x: f32, z_rot: f32| EntityPart {
        default_rotation: vanilla_rot(0.2617994, 0.0, z_rot),
        ..vpart(
            name,
            Some(2),
            Vec3::new(x, -10.0, 4.0),
            vec![vbox((0, 12), (-1.0, -7.0, 0.0), (2.0, 7.0, 1.0))],
        )
    };
    for replacement in [
        ear("left_ear", 1.25, 0.2617994),
        ear("right_ear", -1.25, -0.2617994),
    ] {
        let i = parts
            .iter()
            .position(|p| p.name == replacement.name)
            .expect("equine mesh has both ears");
        parts[i] = replacement;
    }
    let chest_part = |name: &str, x: f32, y_rot: f32| EntityPart {
        default_rotation: Vec3::new(0.0, y_rot, 0.0),
        ..vpart(
            name,
            Some(0),
            Vec3::new(x, -8.0, 0.0),
            if chest {
                vec![vbox((26, 21), (-4.0, 0.0, -2.0), (8.0, 8.0, 3.0))]
            } else {
                vec![]
            },
        )
    };
    parts.push(chest_part("left_chest", 6.0, -std::f32::consts::FRAC_PI_2));
    parts.push(chest_part("right_chest", -6.0, std::f32::consts::FRAC_PI_2));
    parts
}

pub fn bake_donkey_model(scale: f32, chest: bool) -> BakedEntityModel {
    bake_root_scaled(donkey_parts(chest), scale, 64, 64)
}

/// Vanilla `BabyHorseModel.createBabyMesh(NONE)`, 64x64 dedicated mesh — no
/// mane or upper mouth.
fn baby_horse_parts() -> Vec<EntityPart> {
    let leg = |name: &str, x: f32, z: f32, uv: (i32, i32)| {
        vpart(
            name,
            None,
            Vec3::new(x, 16.0, z),
            vec![vbox(uv, (-1.5, -1.0, -1.5), (3.0, 9.0, 3.0))],
        )
    };
    vec![
        vpart(
            "body",
            None,
            Vec3::new(0.0, 12.5, 0.0),
            vec![vbox((0, 13), (-4.0, -3.5, -7.0), (8.0, 7.0, 14.0))],
        ),
        EntityPart {
            default_rotation: Vec3::new(-0.7418, 0.0, 0.0),
            ..vpart(
                "tail",
                Some(0),
                Vec3::new(0.0, -1.0, 7.0),
                vec![vbox((24, 34), (-1.5, -1.5, -1.0), (3.0, 3.0, 8.0))],
            )
        },
        leg("left_hind_leg", 2.4, 5.4, (12, 46)),
        leg("right_hind_leg", -2.4, 5.4, (0, 46)),
        leg("left_front_leg", 2.4, -5.4, (12, 34)),
        leg("right_front_leg", -2.4, -5.4, (0, 34)),
        EntityPart {
            default_rotation: Vec3::new(0.6109, 0.0, 0.0),
            ..vpart(
                "head_parts",
                None,
                Vec3::new(0.0, 10.0, -6.0),
                vec![vbox((30, 0), (-2.0, -6.0, -2.0), (4.0, 8.0, 4.0))],
            )
        },
        vpart(
            "head",
            Some(6),
            Vec3::new(0.0, -6.0516, -0.2951),
            vec![vbox((0, 0), (-3.0, -3.9484, -6.705), (6.0, 4.0, 9.0))],
        ),
        EntityPart {
            default_rotation: Vec3::new(0.0, 0.0, 0.2618),
            ..vpart(
                "left_ear",
                Some(7),
                Vec3::new(2.0, -4.2484, 1.9451),
                vec![vbox((0, 4), (-1.0, -2.5, -0.8), (2.0, 3.0, 1.0))],
            )
        },
        EntityPart {
            default_rotation: Vec3::new(0.0, 0.0, -0.2618),
            ..vpart(
                "right_ear",
                Some(7),
                Vec3::new(-2.0, -4.2484, 1.645),
                vec![vbox((0, 0), (-1.0, -2.5, -0.5), (2.0, 3.0, 1.0))],
            )
        },
    ]
}

pub fn bake_baby_horse_model() -> BakedEntityModel {
    bake_model(baby_horse_parts(), 64, 64)
}

/// Vanilla `BabyDonkeyModel.createBabyLayer()`, 64x64: everything hangs off
/// `body`, with `_r1` rotation-carrier parts and cubeless chest stubs (baby
/// donkeys/mules never show a chest).
fn baby_donkey_parts() -> Vec<EntityPart> {
    let leg = |name: &str, pivot: Vec3, uv: (i32, i32), origin: (f32, f32, f32)| {
        vpart(
            name,
            Some(0),
            pivot,
            vec![vbox(uv, origin, (3.0, 8.0, 3.0))],
        )
    };
    vec![
        vpart(
            "body",
            None,
            Vec3::new(1.0, 14.0, 0.0),
            vec![vbox((0, 13), (-5.0, -3.0, -7.0), (8.0, 6.0, 14.0))],
        ),
        vpart("tail", Some(0), Vec3::new(0.0, -1.5, 6.5), vec![]),
        EntityPart {
            default_rotation: Vec3::new(-0.7418, 0.0, 0.0),
            ..vpart(
                "tail_r1",
                Some(1),
                Vec3::ZERO,
                vec![vbox((24, 33), (-2.5, -1.0, -0.5), (3.0, 3.0, 8.0))],
            )
        },
        leg(
            "left_hind_leg",
            Vec3::new(2.25, 3.5, 5.25),
            (12, 44),
            (-2.5, -1.5, -1.5),
        ),
        leg(
            "right_hind_leg",
            Vec3::new(-2.4, 3.5, 5.4),
            (0, 44),
            (-2.5, -1.5, -1.5),
        ),
        leg(
            "left_front_leg",
            Vec3::new(2.4, 3.5, -5.3),
            (12, 33),
            (-2.5, -1.5, -1.5),
        ),
        leg(
            "right_front_leg",
            Vec3::new(-2.4, 3.5, -5.4),
            (0, 33),
            (-2.5, -1.5, -1.5),
        ),
        vpart("head_parts", Some(0), Vec3::new(0.0, -3.0, -5.0), vec![]),
        EntityPart {
            default_rotation: Vec3::new(std::f32::consts::FRAC_PI_8, 0.0, 0.0),
            ..vpart(
                "neck_r1",
                Some(7),
                Vec3::ZERO,
                vec![vbox((30, 9), (-3.0, -6.0, -3.0), (4.0, 8.0, 4.0))],
            )
        },
        vpart("head", Some(7), Vec3::new(0.0, -5.0, -3.0), vec![]),
        EntityPart {
            default_rotation: Vec3::new(std::f32::consts::FRAC_PI_8, 0.0, 0.0),
            ..vpart(
                "head_r1",
                Some(9),
                Vec3::new(0.0, -1.0, 1.0),
                vec![vbox((0, 0), (-4.0, -3.6, -8.4), (6.0, 4.0, 9.0))],
            )
        },
        EntityPart {
            default_rotation: vanilla_rot(0.48, 0.0, 0.48),
            ..vpart(
                "left_ear",
                Some(9),
                Vec3::new(2.0, -3.5, -1.0),
                vec![vbox((0, 0), (-2.0, -6.5, -0.3), (2.0, 7.0, 1.0))],
            )
        },
        EntityPart {
            default_rotation: vanilla_rot(0.48, 0.0, -0.48),
            ..vpart(
                "right_ear",
                Some(9),
                Vec3::new(-2.0, -3.5, -1.0),
                vec![ModelCube {
                    mirror: true,
                    ..vbox((22, 0), (-2.0, -6.5, -0.3), (2.0, 7.0, 1.0))
                }],
            )
        },
        vpart("right_chest", Some(0), Vec3::new(-1.0, 10.0, 0.0), vec![]),
        vpart("left_chest", Some(0), Vec3::new(-1.0, 10.0, 0.0), vec![]),
    ]
}

pub fn bake_baby_donkey_model() -> BakedEntityModel {
    bake_model(baby_donkey_parts(), 64, 64)
}

/// The eight tentacles shared by both squid meshes (vanilla `SquidModel`'s
/// placement loop; the yRot values are deliberately unwrapped).
fn squid_tentacles(radius: f32, y: f32, cube: ModelCube) -> Vec<EntityPart> {
    use std::f32::consts::{FRAC_PI_2, PI};
    (0..8)
        .map(|i| {
            let angle = i as f32 * PI * 2.0 / 8.0;
            EntityPart {
                default_rotation: Vec3::new(0.0, i as f32 * PI * -2.0 / 8.0 + FRAC_PI_2, 0.0),
                ..vpart(
                    &format!("tentacle{i}"),
                    None,
                    Vec3::new(angle.cos() * radius, y, angle.sin() * radius),
                    vec![cube],
                )
            }
        })
        .collect()
}

/// Vanilla `SquidModel`, 64x32, shared by squid and glow squid.
pub fn bake_squid_model() -> BakedEntityModel {
    let mut parts = vec![vpart(
        "body",
        None,
        Vec3::new(0.0, 8.0, 0.0),
        vec![ModelCube {
            deformation: 0.02,
            ..vbox((0, 0), (-6.0, -8.0, -6.0), (12.0, 16.0, 12.0))
        }],
    )];
    parts.extend(squid_tentacles(
        5.0,
        15.0,
        vbox((48, 0), (-1.0, 0.0, -1.0), (2.0, 18.0, 2.0)),
    ));
    bake_model(parts, 64, 32)
}

/// Vanilla `BabySquidModel`, 32x32 dedicated mesh (the 0.5 baby transformer
/// exists in vanilla but is unused).
pub fn bake_baby_squid_model() -> BakedEntityModel {
    let mut parts = vec![vpart(
        "body",
        None,
        Vec3::new(0.0, 13.0, 0.0),
        vec![vbox((0, 0), (-4.0, -5.0, -4.0), (8.0, 10.0, 8.0))],
    )];
    parts.extend(squid_tentacles(
        3.0,
        18.5,
        vbox((0, 18), (-1.0, -0.5, -1.0), (2.0, 6.0, 2.0)),
    ));
    bake_model(parts, 32, 32)
}

/// Vanilla `BatModel`, 32x32: six of the nine parts are zero-depth quads
/// with distinct front/back UVs — the bat renders through the backface-culled
/// pipeline (vanilla `entityCutoutCull`) so the coplanar pairs don't fight.
fn bat_parts() -> Vec<EntityPart> {
    vec![
        vpart(
            "body",
            None,
            Vec3::new(0.0, 17.0, 0.0),
            vec![vbox((0, 0), (-1.5, 0.0, -1.0), (3.0, 5.0, 2.0))],
        ),
        vpart(
            "head",
            None,
            Vec3::new(0.0, 17.0, 0.0),
            vec![vbox((0, 7), (-2.0, -3.0, -1.0), (4.0, 3.0, 2.0))],
        ),
        vpart(
            "right_ear",
            Some(1),
            Vec3::new(-1.5, -2.0, 0.0),
            vec![vbox((1, 15), (-2.5, -4.0, 0.0), (3.0, 5.0, 0.0))],
        ),
        vpart(
            "left_ear",
            Some(1),
            Vec3::new(1.1, -3.0, 0.0),
            vec![vbox((8, 15), (-0.1, -3.0, 0.0), (3.0, 5.0, 0.0))],
        ),
        vpart(
            "right_wing",
            Some(0),
            Vec3::new(-1.5, 0.0, 0.0),
            vec![vbox((12, 0), (-2.0, -2.0, 0.0), (2.0, 7.0, 0.0))],
        ),
        vpart(
            "right_wing_tip",
            Some(4),
            Vec3::new(-2.0, 0.0, 0.0),
            vec![vbox((16, 0), (-6.0, -2.0, 0.0), (6.0, 8.0, 0.0))],
        ),
        vpart(
            "left_wing",
            Some(0),
            Vec3::new(1.5, 0.0, 0.0),
            vec![vbox((12, 7), (0.0, -2.0, 0.0), (2.0, 7.0, 0.0))],
        ),
        vpart(
            "left_wing_tip",
            Some(6),
            Vec3::new(2.0, 0.0, 0.0),
            vec![vbox((16, 8), (0.0, -2.0, 0.0), (6.0, 8.0, 0.0))],
        ),
        vpart(
            "feet",
            Some(0),
            Vec3::new(0.0, 5.0, 0.0),
            vec![vbox((16, 16), (-1.5, 0.0, 0.0), (3.0, 2.0, 0.0))],
        ),
    ]
}

pub fn bake_bat_model() -> BakedEntityModel {
    bake_model(bat_parts(), 32, 32)
}

/// Vanilla `CodModel`, 32x32.
pub fn bake_cod_model() -> BakedEntityModel {
    use std::f32::consts::FRAC_PI_4;
    let parts = vec![
        vpart(
            "body",
            None,
            Vec3::new(0.0, 22.0, 0.0),
            vec![vbox((0, 0), (-1.0, -2.0, 0.0), (2.0, 4.0, 7.0))],
        ),
        vpart(
            "head",
            None,
            Vec3::new(0.0, 22.0, 0.0),
            vec![vbox((11, 0), (-1.0, -2.0, -3.0), (2.0, 4.0, 3.0))],
        ),
        vpart(
            "nose",
            None,
            Vec3::new(0.0, 22.0, -3.0),
            vec![vbox((0, 0), (-1.0, -2.0, -1.0), (2.0, 3.0, 1.0))],
        ),
        EntityPart {
            default_rotation: Vec3::new(0.0, 0.0, -FRAC_PI_4),
            ..vpart(
                "right_fin",
                None,
                Vec3::new(-1.0, 23.0, 0.0),
                vec![vbox((22, 1), (-2.0, 0.0, -1.0), (2.0, 0.0, 2.0))],
            )
        },
        EntityPart {
            default_rotation: Vec3::new(0.0, 0.0, FRAC_PI_4),
            ..vpart(
                "left_fin",
                None,
                Vec3::new(1.0, 23.0, 0.0),
                vec![vbox((22, 4), (0.0, 0.0, -1.0), (2.0, 0.0, 2.0))],
            )
        },
        vpart(
            "tail_fin",
            None,
            Vec3::new(0.0, 22.0, 7.0),
            vec![vbox((22, 3), (0.0, -2.0, 0.0), (0.0, 4.0, 4.0))],
        ),
        vpart(
            "top_fin",
            None,
            Vec3::new(0.0, 20.0, 0.0),
            vec![vbox((20, -6), (0.0, -1.0, -1.0), (0.0, 1.0, 6.0))],
        ),
    ];
    bake_model(parts, 32, 32)
}

/// Vanilla `SalmonModel`, 32x32; the back fins ride `body_back` so the tail
/// wobble carries them. Baked at 0.5 / 1.0 / 1.5 root scale for the size
/// variants (`SalmonModel.SMALL_TRANSFORMER` / `LARGE_TRANSFORMER`).
fn salmon_parts() -> Vec<EntityPart> {
    use std::f32::consts::FRAC_PI_4;
    vec![
        vpart(
            "body_front",
            None,
            Vec3::new(0.0, 20.0, -7.2),
            vec![vbox((0, 0), (-1.5, -2.5, 0.0), (3.0, 5.0, 8.0))],
        ),
        vpart(
            "body_back",
            None,
            Vec3::new(0.0, 20.0, 0.8000002),
            vec![vbox((0, 13), (-1.5, -2.5, 0.0), (3.0, 5.0, 8.0))],
        ),
        vpart(
            "head",
            None,
            Vec3::new(0.0, 20.0, -7.2),
            vec![vbox((22, 0), (-1.0, -2.0, -3.0), (2.0, 4.0, 3.0))],
        ),
        vpart(
            "back_fin",
            Some(1),
            Vec3::new(0.0, 0.0, 8.0),
            vec![vbox((20, 10), (0.0, -2.5, 0.0), (0.0, 5.0, 6.0))],
        ),
        vpart(
            "top_front_fin",
            Some(0),
            Vec3::new(0.0, -4.5, 5.0),
            vec![vbox((2, 1), (0.0, 0.0, 0.0), (0.0, 2.0, 3.0))],
        ),
        vpart(
            "top_back_fin",
            Some(1),
            Vec3::new(0.0, -4.5, -1.0),
            vec![vbox((0, 2), (0.0, 0.0, 0.0), (0.0, 2.0, 4.0))],
        ),
        EntityPart {
            default_rotation: Vec3::new(0.0, 0.0, -FRAC_PI_4),
            ..vpart(
                "right_fin",
                None,
                Vec3::new(-1.5, 21.5, -7.2),
                vec![vbox((-4, 0), (-2.0, 0.0, 0.0), (2.0, 0.0, 2.0))],
            )
        },
        EntityPart {
            default_rotation: Vec3::new(0.0, 0.0, FRAC_PI_4),
            ..vpart(
                "left_fin",
                None,
                Vec3::new(1.5, 21.5, -7.2),
                vec![vbox((0, 0), (0.0, 0.0, 0.0), (2.0, 0.0, 2.0))],
            )
        },
    ]
}

pub fn bake_salmon_model(scale: f32) -> BakedEntityModel {
    bake_root_scaled(salmon_parts(), scale, 32, 32)
}

/// Vanilla `TropicalFishSmallModel` / `TropicalFishLargeModel` (shape A / B),
/// 32x32. `g` is the 0.008 pattern-layer inflate. The small mesh carries a
/// cubeless `bottom_fin` so both shapes (and their pattern overlays) share
/// one part order (`assert_part_order_matches`).
fn tropical_fish_parts(large: bool, g: f32) -> Vec<EntityPart> {
    use std::f32::consts::FRAC_PI_4;
    let cube = |uv: (i32, i32), origin: (f32, f32, f32), size: (f32, f32, f32)| ModelCube {
        deformation: g,
        ..vbox(uv, origin, size)
    };
    if large {
        vec![
            vpart(
                "body",
                None,
                Vec3::new(0.0, 19.0, 0.0),
                vec![cube((0, 20), (-1.0, -3.0, -3.0), (2.0, 6.0, 6.0))],
            ),
            vpart(
                "tail",
                None,
                Vec3::new(0.0, 19.0, 3.0),
                vec![cube((21, 16), (0.0, -3.0, 0.0), (0.0, 6.0, 5.0))],
            ),
            EntityPart {
                default_rotation: Vec3::new(0.0, FRAC_PI_4, 0.0),
                ..vpart(
                    "right_fin",
                    None,
                    Vec3::new(-1.0, 20.0, 0.0),
                    vec![cube((2, 16), (-2.0, 0.0, 0.0), (2.0, 2.0, 0.0))],
                )
            },
            EntityPart {
                default_rotation: Vec3::new(0.0, -FRAC_PI_4, 0.0),
                ..vpart(
                    "left_fin",
                    None,
                    Vec3::new(1.0, 20.0, 0.0),
                    vec![cube((2, 12), (0.0, 0.0, 0.0), (2.0, 2.0, 0.0))],
                )
            },
            vpart(
                "top_fin",
                None,
                Vec3::new(0.0, 16.0, -3.0),
                vec![cube((20, 11), (0.0, -4.0, 0.0), (0.0, 4.0, 6.0))],
            ),
            vpart(
                "bottom_fin",
                None,
                Vec3::new(0.0, 22.0, -3.0),
                vec![cube((20, 21), (0.0, 0.0, 0.0), (0.0, 4.0, 6.0))],
            ),
        ]
    } else {
        vec![
            vpart(
                "body",
                None,
                Vec3::new(0.0, 22.0, 0.0),
                vec![cube((0, 0), (-1.0, -1.5, -3.0), (2.0, 3.0, 6.0))],
            ),
            vpart(
                "tail",
                None,
                Vec3::new(0.0, 22.0, 3.0),
                vec![cube((22, -6), (0.0, -1.5, 0.0), (0.0, 3.0, 6.0))],
            ),
            EntityPart {
                default_rotation: Vec3::new(0.0, FRAC_PI_4, 0.0),
                ..vpart(
                    "right_fin",
                    None,
                    Vec3::new(-1.0, 22.5, 0.0),
                    vec![cube((2, 16), (-2.0, -1.0, 0.0), (2.0, 2.0, 0.0))],
                )
            },
            EntityPart {
                default_rotation: Vec3::new(0.0, -FRAC_PI_4, 0.0),
                ..vpart(
                    "left_fin",
                    None,
                    Vec3::new(1.0, 22.5, 0.0),
                    vec![cube((2, 12), (0.0, -1.0, 0.0), (2.0, 2.0, 0.0))],
                )
            },
            vpart(
                "top_fin",
                None,
                Vec3::new(0.0, 20.5, -3.0),
                vec![cube((10, -5), (0.0, -3.0, 0.0), (0.0, 3.0, 6.0))],
            ),
            vpart("bottom_fin", None, Vec3::ZERO, vec![]),
        ]
    }
}

pub fn bake_tropical_fish_model(large: bool, g: f32) -> BakedEntityModel {
    bake_model(tropical_fish_parts(large, g), 32, 32)
}

/// Vanilla `PufferfishSmallModel` / `PufferfishMidModel` / `PufferfishBigModel`
/// (puff states 0/1/2), all 32x32.
pub fn bake_pufferfish_model(puff_state: u32) -> BakedEntityModel {
    use std::f32::consts::FRAC_PI_4;
    let rot = |x: f32, y: f32, name: &str, pivot: (f32, f32, f32), cube: ModelCube| EntityPart {
        default_rotation: Vec3::new(x, y, 0.0),
        ..vpart(name, None, Vec3::new(pivot.0, pivot.1, pivot.2), vec![cube])
    };
    let parts = match puff_state {
        0 => vec![
            vpart(
                "body",
                None,
                Vec3::new(0.0, 23.0, 0.0),
                vec![vbox((0, 27), (-1.5, -2.0, -1.5), (3.0, 2.0, 3.0))],
            ),
            vpart(
                "right_eye",
                None,
                Vec3::new(0.0, 20.0, 0.0),
                vec![vbox((24, 6), (-1.5, 0.0, -1.5), (1.0, 1.0, 1.0))],
            ),
            vpart(
                "left_eye",
                None,
                Vec3::new(0.0, 20.0, 0.0),
                vec![vbox((28, 6), (0.5, 0.0, -1.5), (1.0, 1.0, 1.0))],
            ),
            vpart(
                "back_fin",
                None,
                Vec3::new(0.0, 22.0, 1.5),
                vec![vbox((-3, 0), (-1.5, 0.0, 0.0), (3.0, 0.0, 3.0))],
            ),
            vpart(
                "right_fin",
                None,
                Vec3::new(-1.5, 22.0, -1.5),
                vec![vbox((25, 0), (-1.0, 0.0, 0.0), (1.0, 0.0, 2.0))],
            ),
            vpart(
                "left_fin",
                None,
                Vec3::new(1.5, 22.0, -1.5),
                vec![vbox((25, 0), (0.0, 0.0, 0.0), (1.0, 0.0, 2.0))],
            ),
        ],
        1 => vec![
            vpart(
                "body",
                None,
                Vec3::new(0.0, 22.0, 0.0),
                vec![vbox((12, 22), (-2.5, -5.0, -2.5), (5.0, 5.0, 5.0))],
            ),
            vpart(
                "right_blue_fin",
                None,
                Vec3::new(-2.5, 18.0, -1.5),
                vec![vbox((24, 0), (-2.0, 0.0, 0.0), (2.0, 0.0, 2.0))],
            ),
            vpart(
                "left_blue_fin",
                None,
                Vec3::new(2.5, 18.0, -1.5),
                vec![vbox((24, 3), (0.0, 0.0, 0.0), (2.0, 0.0, 2.0))],
            ),
            rot(
                FRAC_PI_4,
                0.0,
                "top_front_fin",
                (0.0, 17.0, -2.5),
                vbox((19, 17), (-2.5, -1.0, 0.0), (5.0, 1.0, 0.0)),
            ),
            rot(
                -FRAC_PI_4,
                0.0,
                "top_back_fin",
                (0.0, 17.0, 2.5),
                vbox((11, 17), (-2.5, -1.0, 0.0), (5.0, 1.0, 0.0)),
            ),
            rot(
                0.0,
                -FRAC_PI_4,
                "right_front_fin",
                (-2.5, 22.0, -2.5),
                vbox((5, 17), (-1.0, -5.0, 0.0), (1.0, 5.0, 0.0)),
            ),
            rot(
                0.0,
                FRAC_PI_4,
                "right_back_fin",
                (-2.5, 22.0, 2.5),
                vbox((9, 17), (-1.0, -5.0, 0.0), (1.0, 5.0, 0.0)),
            ),
            rot(
                0.0,
                -FRAC_PI_4,
                "left_back_fin",
                (2.5, 22.0, 2.5),
                vbox((1, 17), (0.0, -5.0, 0.0), (1.0, 5.0, 0.0)),
            ),
            rot(
                0.0,
                FRAC_PI_4,
                "left_front_fin",
                (2.5, 22.0, -2.5),
                vbox((1, 17), (0.0, -5.0, 0.0), (1.0, 5.0, 0.0)),
            ),
            rot(
                FRAC_PI_4,
                0.0,
                "bottom_back_fin",
                (-2.5, 22.0, 2.5),
                vbox((18, 20), (0.0, 0.0, 0.0), (5.0, 1.0, 0.0)),
            ),
            rot(
                -FRAC_PI_4,
                0.0,
                "bottom_front_fin",
                (0.0, 22.0, -2.5),
                vbox((17, 19), (-2.5, 0.0, 0.0), (5.0, 1.0, 1.0)),
            ),
        ],
        _ => vec![
            vpart(
                "body",
                None,
                Vec3::new(0.0, 22.0, 0.0),
                vec![vbox((0, 0), (-4.0, -8.0, -4.0), (8.0, 8.0, 8.0))],
            ),
            vpart(
                "right_blue_fin",
                None,
                Vec3::new(-4.0, 15.0, -2.0),
                vec![vbox((24, 0), (-2.0, 0.0, -1.0), (2.0, 1.0, 2.0))],
            ),
            vpart(
                "left_blue_fin",
                None,
                Vec3::new(4.0, 15.0, -2.0),
                vec![vbox((24, 3), (0.0, 0.0, -1.0), (2.0, 1.0, 2.0))],
            ),
            rot(
                FRAC_PI_4,
                0.0,
                "top_front_fin",
                (0.0, 14.0, -4.0),
                vbox((15, 17), (-4.0, -1.0, 0.0), (8.0, 1.0, 0.0)),
            ),
            vpart(
                "top_middle_fin",
                None,
                Vec3::new(0.0, 14.0, 0.0),
                vec![vbox((14, 16), (-4.0, -1.0, 0.0), (8.0, 1.0, 1.0))],
            ),
            rot(
                -FRAC_PI_4,
                0.0,
                "top_back_fin",
                (0.0, 14.0, 4.0),
                vbox((23, 18), (-4.0, -1.0, 0.0), (8.0, 1.0, 0.0)),
            ),
            rot(
                0.0,
                -FRAC_PI_4,
                "right_front_fin",
                (-4.0, 22.0, -4.0),
                vbox((5, 17), (-1.0, -8.0, 0.0), (1.0, 8.0, 0.0)),
            ),
            rot(
                0.0,
                FRAC_PI_4,
                "left_front_fin",
                (4.0, 22.0, -4.0),
                vbox((1, 17), (0.0, -8.0, 0.0), (1.0, 8.0, 0.0)),
            ),
            rot(
                -FRAC_PI_4,
                0.0,
                "bottom_front_fin",
                (0.0, 22.0, -4.0),
                vbox((15, 20), (-4.0, 0.0, 0.0), (8.0, 1.0, 0.0)),
            ),
            vpart(
                "bottom_middle_fin",
                None,
                Vec3::new(0.0, 22.0, 0.0),
                vec![vbox((15, 20), (-4.0, 0.0, 0.0), (8.0, 1.0, 0.0))],
            ),
            rot(
                FRAC_PI_4,
                0.0,
                "bottom_back_fin",
                (0.0, 22.0, 4.0),
                vbox((15, 20), (-4.0, 0.0, 0.0), (8.0, 1.0, 0.0)),
            ),
            rot(
                0.0,
                FRAC_PI_4,
                "right_back_fin",
                (-4.0, 22.0, 4.0),
                vbox((9, 17), (-1.0, -8.0, 0.0), (1.0, 8.0, 0.0)),
            ),
            rot(
                0.0,
                -FRAC_PI_4,
                "left_back_fin",
                (4.0, 22.0, 4.0),
                vbox((9, 17), (0.0, -8.0, 0.0), (1.0, 8.0, 0.0)),
            ),
        ],
    };
    bake_model(parts, 32, 32)
}

/// Skeleton: humanoid layout with thin 2×12×2 limbs, 64×32 sheet
/// (`SkeletonModel.createDefaultSkeletonMesh`).
fn skeleton_parts() -> Vec<EntityPart> {
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
    humanoid_parts(arm, leg, 2.0)
}

pub fn bake_skeleton_model() -> BakedEntityModel {
    bake_model(skeleton_parts(), 64, 32)
}

/// Stray/bogged clothing (`SkeletonClothingLayer`): the thick humanoid mesh
/// inflated by `g`, worn over the thin skeleton bones. 64×32 sheet.
fn skeleton_clothing_parts(g: f32) -> Vec<EntityPart> {
    let mut parts = zombie_parts();
    inflate(&mut parts, g);
    parts
}

pub fn bake_skeleton_clothing_model(g: f32) -> BakedEntityModel {
    bake_model(skeleton_clothing_parts(g), 64, 32)
}

/// The six mushroom quads on a bogged's head, three crossed pairs at 45/135
/// degrees (vanilla `BoggedModel`; the empty `mushrooms` container part is
/// flattened away, angle literals are pi/4, 3pi/4 and -pi/2 rounded to f32).
/// Sheared keeps the parts with no cubes so every bogged model shares one
/// part order.
fn mushroom_parts(sheared: bool) -> Vec<EntityPart> {
    use std::f32::consts::{FRAC_PI_2, FRAC_PI_4};
    // (name, first index, texOffs, origin y, pivot, laid flat on the back)
    let pairs = [
        (
            "red_mushroom",
            1,
            (50, 16),
            -3.0,
            Vec3::new(3.0, -8.0, 3.0),
            false,
        ),
        (
            "brown_mushroom",
            1,
            (50, 22),
            -3.0,
            Vec3::new(-3.0, -8.0, -3.0),
            false,
        ),
        (
            "brown_mushroom",
            3,
            (50, 28),
            -4.0,
            Vec3::new(-2.0, -1.0, 4.0),
            true,
        ),
    ];
    let mut parts = Vec::with_capacity(6);
    for (name, first, uv, origin_y, pivot, flat) in pairs {
        for (i, angle) in [FRAC_PI_4, 3.0 * FRAC_PI_4].into_iter().enumerate() {
            let rot = if flat {
                Vec3::new(-FRAC_PI_2, 0.0, angle)
            } else {
                Vec3::new(0.0, angle, 0.0)
            };
            let cubes = if sheared {
                vec![]
            } else {
                vec![vbox(uv, (-3.0, origin_y, 0.0), (6.0, 4.0, 0.0))]
            };
            parts.push(EntityPart {
                default_rotation: rot,
                ..vpart(&format!("{name}_{}", first + i), Some(0), pivot, cubes)
            });
        }
    }
    parts
}

pub fn bake_bogged_model(sheared: bool) -> BakedEntityModel {
    let mut parts = skeleton_parts();
    parts.extend(mushroom_parts(sheared));
    bake_model(parts, 64, 32)
}

/// Bogged clothing padded with the empty mushroom parts (overlay part order
/// must match the base's).
pub fn bake_bogged_clothing_model() -> BakedEntityModel {
    let mut parts = skeleton_clothing_parts(0.2);
    parts.extend(mushroom_parts(true));
    bake_model(parts, 64, 32)
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

/// Vanilla `AdultChickenModel.createBaseChickenModel()`. The beak and wattle
/// are children with a zero pose in vanilla, so they fold into the head part.
/// Legs share identical UVs and are not mirrored (vanilla quirk).
fn chicken_parts() -> Vec<EntityPart> {
    let leg = |name: &str, x: f32| {
        vpart(
            name,
            None,
            Vec3::new(x, 19.0, 1.0),
            vec![vbox((26, 0), (-1.0, 0.0, -3.0), (3.0, 5.0, 3.0))],
        )
    };
    let wing_cube = vbox((24, 13), (0.0, 0.0, -3.0), (1.0, 4.0, 6.0));
    let wing = |name: &str, x: f32, cube: ModelCube| {
        vpart(name, None, Vec3::new(x, 13.0, 0.0), vec![cube])
    };
    vec![
        vpart(
            "head",
            None,
            Vec3::new(0.0, 15.0, -4.0),
            vec![
                vbox((0, 0), (-2.0, -6.0, -2.0), (4.0, 6.0, 3.0)),
                // Beak.
                vbox((14, 0), (-2.0, -4.0, -4.0), (4.0, 2.0, 2.0)),
                // Wattle ("red_thing").
                vbox((14, 4), (-1.0, -2.0, -3.0), (2.0, 2.0, 2.0)),
            ],
        ),
        EntityPart {
            default_rotation: Vec3::new(std::f32::consts::FRAC_PI_2, 0.0, 0.0),
            ..vpart(
                "body",
                None,
                Vec3::new(0.0, 16.0, 0.0),
                vec![vbox((0, 9), (-3.0, -4.0, -3.0), (6.0, 8.0, 6.0))],
            )
        },
        leg("right_leg", -2.0),
        leg("left_leg", 1.0),
        wing("right_wing", -4.0, wing_cube),
        wing("left_wing", 4.0, mirror_x_geom(wing_cube)),
    ]
}

pub fn bake_chicken_model() -> BakedEntityModel {
    bake_model(chicken_parts(), 64, 32)
}

/// Vanilla `ColdChickenModel`: the base chicken plus a head crest and a
/// zero-width tail-feather plane.
pub fn bake_cold_chicken_model() -> BakedEntityModel {
    let mut parts = chicken_parts();
    // Head crest (its -2.015 z avoids z-fighting), then body tail feathers.
    parts
        .iter_mut()
        .find(|p| p.name == "head")
        .expect("chicken mesh has a head")
        .cubes
        .push(vbox((44, 0), (-3.0, -7.0, -2.015), (6.0, 3.0, 4.0)));
    parts
        .iter_mut()
        .find(|p| p.name == "body")
        .expect("chicken mesh has a body")
        .cubes
        .push(vbox((38, 9), (0.0, 3.0, -1.0), (0.0, 3.0, 5.0)));
    bake_model(parts, 64, 32)
}

/// Vanilla `BabyChickenModel`: a wholly separate 16x16 mesh with the head
/// fused into the body (so no head look). The wing pivots are X-flipped
/// relative to the adult (vanilla quirk; the legs are not).
pub fn bake_baby_chicken_model() -> BakedEntityModel {
    let leg = |name: &str, x: f32, shin_uv: (i32, i32), foot_uv: (i32, i32)| {
        vpart(
            name,
            None,
            Vec3::new(x, 22.0, 0.5),
            vec![
                vbox(shin_uv, (-0.5, 0.0, 0.0), (1.0, 2.0, 0.0)),
                vbox(foot_uv, (-0.5, 2.0, -1.0), (1.0, 0.0, 1.0)),
            ],
        )
    };
    let wing = |name: &str, x: f32, origin_x: f32, uv: (i32, i32)| {
        vpart(
            name,
            None,
            Vec3::new(x, 20.0, 0.0),
            vec![vbox(uv, (origin_x, 0.0, -1.0), (1.0, 0.0, 2.0))],
        )
    };
    let parts = vec![
        vpart(
            "body",
            None,
            Vec3::new(0.0, 20.25, -1.25),
            vec![
                vbox((0, 0), (-2.0, -2.25, -0.75), (4.0, 4.0, 4.0)),
                // Beak.
                vbox((10, 8), (-1.0, -0.25, -1.75), (2.0, 1.0, 1.0)),
            ],
        ),
        leg("left_leg", 1.0, (2, 2), (0, 1)),
        leg("right_leg", -1.0, (0, 2), (0, 0)),
        wing("right_wing", 2.0, 0.0, (6, 8)),
        wing("left_wing", -2.0, -1.0, (4, 8)),
    ];
    bake_model(parts, 16, 16)
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

/// One vanilla `texOffs(u, v).addBox(origin, size)`.
fn vbox(tex_offset: (i32, i32), origin: (f32, f32, f32), size: (f32, f32, f32)) -> ModelCube {
    ModelCube {
        origin: origin.into(),
        size: size.into(),
        tex_offset,
        deformation: 0.0,
        mirror: false,
    }
}

fn vpart(name: &str, parent: Option<usize>, offset: Vec3, cubes: Vec<ModelCube>) -> EntityPart {
    EntityPart {
        name: name.into(),
        offset,
        default_rotation: Vec3::ZERO,
        cubes,
        parent,
    }
}

fn villager_parts() -> Vec<EntityPart> {
    vec![
        vpart(
            "head",
            None,
            Vec3::ZERO,
            vec![vbox((0, 0), (-4.0, -10.0, -4.0), (8.0, 10.0, 8.0))],
        ),
        vpart(
            "hat",
            Some(0),
            Vec3::ZERO,
            vec![ModelCube {
                deformation: 0.51,
                ..vbox((32, 0), (-4.0, -10.0, -4.0), (8.0, 10.0, 8.0))
            }],
        ),
        EntityPart {
            default_rotation: Vec3::new(-std::f32::consts::FRAC_PI_2, 0.0, 0.0),
            ..vpart(
                "hat_rim",
                Some(1),
                Vec3::ZERO,
                vec![vbox((30, 47), (-8.0, -8.0, -6.0), (16.0, 16.0, 1.0))],
            )
        },
        vpart(
            "nose",
            Some(0),
            Vec3::new(0.0, -2.0, 0.0),
            vec![vbox((24, 0), (-1.0, -1.0, -6.0), (2.0, 4.0, 2.0))],
        ),
        vpart(
            "body",
            None,
            Vec3::ZERO,
            vec![vbox((16, 20), (-4.0, 0.0, -3.0), (8.0, 12.0, 6.0))],
        ),
        vpart(
            "jacket",
            Some(4),
            Vec3::ZERO,
            vec![ModelCube {
                deformation: 0.5,
                ..vbox((0, 38), (-4.0, 0.0, -3.0), (8.0, 20.0, 6.0))
            }],
        ),
        EntityPart {
            default_rotation: Vec3::new(-0.75, 0.0, 0.0),
            ..vpart(
                "arms",
                None,
                Vec3::new(0.0, 3.0, -1.0),
                vec![
                    vbox((44, 22), (-8.0, -2.0, -2.0), (4.0, 8.0, 4.0)),
                    ModelCube {
                        mirror: true,
                        ..vbox((44, 22), (4.0, -2.0, -2.0), (4.0, 8.0, 4.0))
                    },
                    vbox((40, 38), (-4.0, 2.0, -2.0), (8.0, 4.0, 4.0)),
                ],
            )
        },
        vpart(
            "right_leg",
            None,
            Vec3::new(-2.0, 12.0, 0.0),
            vec![vbox((0, 22), (-2.0, 0.0, -2.0), (4.0, 12.0, 4.0))],
        ),
        vpart(
            "left_leg",
            None,
            Vec3::new(2.0, 12.0, 0.0),
            vec![ModelCube {
                mirror: true,
                ..vbox((0, 22), (-2.0, 0.0, -2.0), (4.0, 12.0, 4.0))
            }],
        ),
    ]
}

fn baby_villager_parts() -> Vec<EntityPart> {
    // Vanilla's literal is -1.0472 (~ -PI/3 from the Blockbench export).
    let arm_pitch = Vec3::new(-std::f32::consts::FRAC_PI_3, 0.0, 0.0);
    vec![
        vpart("arms", None, Vec3::new(0.0, 17.5, 0.0), vec![]),
        EntityPart {
            default_rotation: arm_pitch,
            ..vpart(
                "right_hand",
                Some(0),
                Vec3::new(-3.0, 1.4025, -0.9599),
                vec![
                    vbox((36, 15), (-1.0, -2.4925, -1.8401), (2.0, 4.0, 2.0)),
                    vbox((16, 15), (5.0, -2.4925, -1.8401), (2.0, 4.0, 2.0)),
                ],
            )
        },
        EntityPart {
            default_rotation: arm_pitch,
            ..vpart(
                "middlearm_r1",
                Some(0),
                Vec3::new(0.0, 0.9024, -1.8175),
                vec![vbox((24, 17), (-2.0, -0.9924, -0.9825), (4.0, 2.0, 2.0))],
            )
        },
        vpart(
            "right_leg",
            None,
            Vec3::new(-1.0, 21.5, 0.0),
            vec![vbox((8, 23), (-1.0, -0.5, -1.0), (2.0, 3.0, 2.0))],
        ),
        vpart(
            "left_leg",
            None,
            Vec3::new(1.0, 21.5, 0.0),
            vec![vbox((0, 23), (-1.0, -0.5, -1.0), (2.0, 3.0, 2.0))],
        ),
        vpart(
            "head",
            None,
            Vec3::new(0.0, 16.0, 0.0),
            vec![vbox((0, 0), (-4.0, -8.0, -3.5), (8.0, 8.0, 7.0))],
        ),
        vpart(
            "hat",
            Some(5),
            Vec3::new(0.0, -4.0, 0.0),
            vec![ModelCube {
                deformation: 0.3,
                ..vbox((0, 30), (-4.0, -4.0, -3.5), (8.0, 8.0, 7.0))
            }],
        ),
        // Unlike the adult model, the baby hat_rim hangs off the head, not the
        // hat.
        vpart(
            "hat_rim",
            Some(5),
            Vec3::new(0.0, -4.5, 0.0),
            vec![vbox((0, 45), (-7.0, -0.5, -6.0), (14.0, 1.0, 12.0))],
        ),
        vpart(
            "nose",
            Some(5),
            Vec3::new(0.0, -2.0, -4.0),
            vec![vbox((23, 0), (-1.0, 0.0, -0.5), (2.0, 2.0, 1.0))],
        ),
        vpart(
            "body",
            None,
            Vec3::new(0.0, 18.75, 0.0),
            vec![vbox((0, 15), (-2.0, -2.75, -1.5), (4.0, 5.0, 3.0))],
        ),
        vpart(
            "bb_main",
            None,
            Vec3::new(0.5, 24.0, 0.0),
            vec![ModelCube {
                deformation: 0.2,
                ..vbox((16, 21), (-2.5, -8.0, -1.5), (4.0, 6.0, 3.0))
            }],
        ),
    ]
}

/// Vanilla `VillagerModel.createNoHatModel`:
/// `clearChild("head").clearRecursively()` empties the cubes of the whole head
/// subtree (head, hat, hat_rim, nose) while keeping the parts, so part order
/// still matches the full model.
fn clear_head_subtree(parts: &mut [EntityPart]) {
    for part in parts {
        if matches!(part.name.as_str(), "head" | "hat" | "hat_rim" | "nose") {
            part.cubes.clear();
        }
    }
}

/// Vanilla `villagerLikeScale` (`LayerDefinitions`): the adult layers bake
/// with the roots scaled 0.9375 and their poses adjusted to keep feet
/// grounded (`PartPose.scaled(f).translated(0, 24.016 * (1 - f), 0)`).
const VILLAGER_SCALE: f32 = 0.9375;

/// Bakes with vanilla's `MeshTransformer.scaling(factor)` applied to the
/// roots only: the transform chain propagates a root's scale to child pivots
/// and geometry like vanilla's pose stack (children would double-scale).
// TODO: pre-scaling pivots leaves runtime `PartAnim` translations unscaled
// (vanilla scales the whole tree from the root); converge the remaining
// callers onto `bake_root_scaled` and delete this.
fn bake_scaled(mut parts: Vec<EntityPart>, factor: f32, tex_h: u32) -> BakedEntityModel {
    let mut scales = Vec::with_capacity(parts.len());
    for part in parts.iter_mut() {
        let is_root = part.parent.is_none();
        if is_root {
            part.offset = part.offset * factor + Vec3::new(0.0, 24.016 * (1.0 - factor), 0.0);
        }
        scales.push(if is_root { factor } else { 1.0 });
    }
    let mut model = bake_model(parts, 64, tex_h);
    model.part_scales = scales;
    model
}

pub fn bake_villager_model(no_hat: bool) -> BakedEntityModel {
    let mut parts = villager_parts();
    if no_hat {
        clear_head_subtree(&mut parts);
    }
    bake_scaled(parts, VILLAGER_SCALE, 64)
}

pub fn bake_baby_villager_model(no_hat: bool) -> BakedEntityModel {
    let mut parts = baby_villager_parts();
    if no_hat {
        clear_head_subtree(&mut parts);
    }
    bake_model(parts, 64, 64)
}

/// Vanilla `EndermanModel`: `HumanoidModel.createMesh` output is fully
/// replaced, so only the literal parts below matter. The hat stays a separate
/// child part (unlike the flattened player/zombie hats): the creepy pose
/// shifts the head up and the hat back down so the inset overlay keeps its
/// place. The feet sink ~1px into the ground -- vanilla.
pub fn bake_enderman_model() -> BakedEntityModel {
    let limb = |name: &str, pivot: Vec3, origin_y: f32, mirror: bool| {
        vpart(
            name,
            None,
            pivot,
            vec![ModelCube {
                mirror,
                ..vbox((56, 0), (-1.0, origin_y, -1.0), (2.0, 30.0, 2.0))
            }],
        )
    };
    let parts = vec![
        vpart(
            "head",
            None,
            Vec3::new(0.0, -13.0, 0.0),
            vec![vbox((0, 0), (-4.0, -8.0, -4.0), (8.0, 8.0, 8.0))],
        ),
        vpart(
            "hat",
            Some(0),
            Vec3::ZERO,
            vec![ModelCube {
                deformation: -0.5,
                ..vbox((0, 16), (-4.0, -8.0, -4.0), (8.0, 8.0, 8.0))
            }],
        ),
        vpart(
            "body",
            None,
            Vec3::new(0.0, -14.0, 0.0),
            vec![vbox((32, 16), (-4.0, 0.0, -2.0), (8.0, 12.0, 4.0))],
        ),
        limb("right_arm", Vec3::new(-5.0, -12.0, 0.0), -2.0, false),
        limb("left_arm", Vec3::new(5.0, -12.0, 0.0), -2.0, true),
        limb("right_leg", Vec3::new(-2.0, -5.0, 0.0), 0.0, false),
        limb("left_leg", Vec3::new(2.0, -5.0, 0.0), 0.0, true),
    ];
    bake_model(parts, 64, 32)
}

/// Vanilla `SlimeModel.createInnerBodyLayer`: the gel core with eyes and
/// mouth. All parts are static; size and squish scale the whole entity via
/// the render-info body transform.
pub fn bake_slime_inner_model() -> BakedEntityModel {
    let parts = vec![
        vpart(
            "cube",
            None,
            Vec3::ZERO,
            vec![vbox((0, 16), (-3.0, 17.0, -3.0), (6.0, 6.0, 6.0))],
        ),
        // Eyes and mouth poke past the cube faces (z -3.5, x beyond +-3) to
        // avoid z-fighting.
        vpart(
            "right_eye",
            None,
            Vec3::ZERO,
            vec![vbox((32, 0), (-3.25, 18.0, -3.5), (2.0, 2.0, 2.0))],
        ),
        vpart(
            "left_eye",
            None,
            Vec3::ZERO,
            vec![vbox((32, 4), (1.25, 18.0, -3.5), (2.0, 2.0, 2.0))],
        ),
        vpart(
            "mouth",
            None,
            Vec3::ZERO,
            vec![vbox((32, 8), (0.0, 21.0, -3.5), (1.0, 1.0, 1.0))],
        ),
    ];
    bake_model(parts, 64, 32)
}

/// Vanilla `SlimeModel.createOuterBodyLayer`: the translucent shell (its
/// alpha lives in the texture). Padded with empty parts so the part list
/// matches the inner model (`assert_part_order_matches` shares anim indices).
pub fn bake_slime_outer_model() -> BakedEntityModel {
    let parts = vec![
        vpart(
            "cube",
            None,
            Vec3::ZERO,
            vec![vbox((0, 0), (-4.0, 16.0, -4.0), (8.0, 8.0, 8.0))],
        ),
        vpart("right_eye", None, Vec3::ZERO, vec![]),
        vpart("left_eye", None, Vec3::ZERO, vec![]),
        vpart("mouth", None, Vec3::ZERO, vec![]),
    ];
    bake_model(parts, 64, 32)
}

/// Vanilla `WitchModel`: `VillagerModel.createBodyModel()` with the hat
/// replaced by the witch's stacked cone, a mole added to the nose, and a
/// 64x128 sheet. The villager `hat_rim` survives vanilla's child merge but
/// its witch.png region is fully transparent, so its cubes are dropped.
fn witch_parts() -> Vec<EntityPart> {
    let mut parts = villager_parts();
    parts[1] = vpart(
        "hat",
        Some(0),
        Vec3::new(-5.0, -10.03125, -5.0),
        vec![vbox((0, 64), (0.0, 0.0, 0.0), (10.0, 2.0, 10.0))],
    );
    parts[2].cubes.clear(); // hat_rim
    parts.extend([
        EntityPart {
            default_rotation: Vec3::new(-0.05235988, 0.0, 0.02617994),
            ..vpart(
                "hat2",
                Some(1),
                Vec3::new(1.75, -4.0, 2.0),
                vec![vbox((0, 76), (0.0, 0.0, 0.0), (7.0, 4.0, 7.0))],
            )
        },
        EntityPart {
            default_rotation: Vec3::new(-0.10471976, 0.0, 0.05235988),
            ..vpart(
                "hat3",
                Some(9),
                Vec3::new(1.75, -4.0, 2.0),
                vec![vbox((0, 87), (0.0, 0.0, 0.0), (4.0, 4.0, 4.0))],
            )
        },
        EntityPart {
            default_rotation: Vec3::new(-0.20943952, 0.0, 0.10471976),
            ..vpart(
                "hat4",
                Some(10),
                Vec3::new(1.75, -2.0, 2.0),
                vec![ModelCube {
                    deformation: 0.25,
                    ..vbox((0, 95), (0.0, 0.0, 0.0), (1.0, 2.0, 1.0))
                }],
            )
        },
        // The mole samples the unused top-left corner of the head texture.
        vpart(
            "mole",
            Some(3),
            Vec3::new(0.0, -2.0, 0.0),
            vec![ModelCube {
                deformation: -0.25,
                ..vbox((0, 0), (0.0, 3.0, -6.75), (1.0, 1.0, 1.0))
            }],
        ),
    ]);
    parts
}

pub fn bake_witch_model() -> BakedEntityModel {
    bake_scaled(witch_parts(), VILLAGER_SCALE, 128)
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

/// Chicken (`ChickenModel.setupAnim` + `AdultChickenModel.setupAnim`). The
/// baby model has no head part, so its match arm never fires there (vanilla
/// babies don't turn their heads either).
pub fn compute_chicken_anim(
    model: &BakedEntityModel,
    head_x_rot_deg: f32,
    local_head_y_rot_deg: f32,
    walk_pos: f32,
    walk_speed: f32,
    flap: f32,
    flap_speed: f32,
) -> PartAnim {
    let mut anim = PartAnim::default();
    let flap_angle = (flap.sin() + 1.0) * flap_speed;
    // The negation is vanilla's `+ PI` leg phase: cos(x + PI) = -cos(x).
    let leg_swing = (walk_pos * 0.6662).cos() * 1.4 * walk_speed;

    for (i, part) in model.parts.iter().enumerate() {
        let rot = match part.name.as_str() {
            "head" => head_rotation(head_x_rot_deg, local_head_y_rot_deg),
            "right_leg" => Vec3::new(leg_swing, 0.0, 0.0),
            "left_leg" => Vec3::new(-leg_swing, 0.0, 0.0),
            "right_wing" => Vec3::new(0.0, 0.0, flap_angle),
            "left_wing" => Vec3::new(0.0, 0.0, -flap_angle),
            _ => continue,
        };
        anim.rotation.push((i, rot));
    }

    anim
}

fn head_rotation(head_x_rot_deg: f32, local_head_y_rot_deg: f32) -> Vec3 {
    vanilla_rot(
        head_x_rot_deg.to_radians(),
        local_head_y_rot_deg.to_radians(),
        0.0,
    )
}

/// Composes a vanilla `(xRot, yRot, zRot)` in vanilla's ZYX order and
/// re-expresses it in the engine's XYZ euler (whose signs
/// `compute_part_transforms` applies). Required whenever two or more axes are
/// set at once — single-axis rotations can be pushed directly.
fn vanilla_rot(x: f32, y: f32, z: f32) -> Vec3 {
    if y == 0.0 && z == 0.0 {
        return Vec3::new(x, 0.0, 0.0);
    }
    let rot = Quat::from_rotation_z(z) * Quat::from_rotation_y(y) * Quat::from_rotation_x(x);
    let (ex, ey, ez) = rot.to_euler(glam::EulerRot::XYZ);
    Vec3::new(ex, ey, ez)
}

/// Vanilla `Mth.lerp(delta, from, to)`.
fn lerp(delta: f32, from: f32, to: f32) -> f32 {
    from + delta * (to - from)
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

/// Villager: head look, legs at half the humanoid swing, arms static (crossed
/// pose in the default rotation). When unhappy the head shakes "no" and looks
/// down (vanilla `VillagerModel.setupAnim`). Part names are shared by the
/// adult and baby models.
pub fn compute_villager_anim(
    model: &BakedEntityModel,
    head_x_rot_deg: f32,
    local_head_y_rot_deg: f32,
    walk_pos: f32,
    walk_speed: f32,
    is_unhappy: bool,
    age_in_ticks: f32,
) -> PartAnim {
    let mut anim = PartAnim::default();

    for (i, part) in model.parts.iter().enumerate() {
        let rot = match part.name.as_str() {
            "head" => {
                if is_unhappy {
                    // Vanilla composes ZYX: zRot = shake, yRot = yaw, xRot = 0.4.
                    let rot = Quat::from_rotation_z(0.3 * (0.45 * age_in_ticks).sin())
                        * Quat::from_rotation_y(local_head_y_rot_deg.to_radians())
                        * Quat::from_rotation_x(0.4);
                    let (x, y, z) = rot.to_euler(glam::EulerRot::XYZ);
                    Vec3::new(x, y, z)
                } else {
                    head_rotation(head_x_rot_deg, local_head_y_rot_deg)
                }
            }
            "right_leg" => Vec3::new((walk_pos * 0.6662).cos() * 1.4 * walk_speed * 0.5, 0.0, 0.0),
            "left_leg" => Vec3::new(
                (walk_pos * 0.6662 + std::f32::consts::PI).cos() * 1.4 * walk_speed * 0.5,
                0.0,
                0.0,
            ),
            _ => continue,
        };
        anim.rotation.push((i, rot));
    }

    anim
}

/// Vanilla `EndermanModel.setupAnim` on top of the humanoid walk: limb x-swing
/// halved and clamped to +-0.4 AFTER the arm bob, tiny fixed leg y/z splay,
/// and the creepy head raise (the hat counter-shifts so the inset overlay
/// stays put).
// TODO: carried-block arm pose and the humanoid attack swing once carried
// blocks / attack animation land.
pub fn compute_enderman_anim(
    model: &BakedEntityModel,
    head_x_rot_deg: f32,
    local_head_y_rot_deg: f32,
    walk_pos: f32,
    walk_speed: f32,
    age_in_ticks: f32,
    is_creepy: bool,
) -> PartAnim {
    let mut anim = PartAnim::default();
    let half_clamp = |x: f32| (x * 0.5).clamp(-0.4, 0.4);
    // Left limbs are the exact negation of the right (vanilla's `+ PI` swing
    // phase and `side = -1` bob), and `half_clamp` is odd, so one value per
    // pair suffices. The vanilla 2.0 * 0.5 arm-swing factors cancel.
    let swing = (walk_pos * 0.6662).cos();
    let (bob_x, bob_z) = bob_arm(age_in_ticks, 1.0);
    let arm_x = half_clamp(bob_x - swing * walk_speed);
    let leg_x = half_clamp(swing * 1.4 * walk_speed);

    for (i, part) in model.parts.iter().enumerate() {
        let rot = match part.name.as_str() {
            "head" => {
                if is_creepy {
                    anim.translation.push((i, Vec3::new(0.0, -5.0, 0.0)));
                }
                head_rotation(head_x_rot_deg, local_head_y_rot_deg)
            }
            "hat" => {
                if is_creepy {
                    anim.translation.push((i, Vec3::new(0.0, 5.0, 0.0)));
                }
                continue;
            }
            "right_arm" => Vec3::new(arm_x, 0.0, bob_z),
            "left_arm" => Vec3::new(-arm_x, 0.0, -bob_z),
            "right_leg" => Vec3::new(leg_x, 0.005, 0.005),
            "left_leg" => Vec3::new(-leg_x, -0.005, -0.005),
            _ => continue,
        };
        anim.rotation.push((i, rot));
    }

    anim
}

/// Vanilla `WitchModel.setupAnim`: the villager pose (`super.setupAnim`) plus
/// the nose. `nose_wobble_speed` is `0.01 * (entity_id % 10)` -- an id
/// divisible by 10 means a still nose (vanilla). Drinking overrides only the
/// x rotation and pivot; the z wobble keeps going.
#[allow(clippy::too_many_arguments)]
pub fn compute_witch_anim(
    model: &BakedEntityModel,
    head_x_rot_deg: f32,
    local_head_y_rot_deg: f32,
    walk_pos: f32,
    walk_speed: f32,
    age_in_ticks: f32,
    nose_wobble_speed: f32,
    is_holding_item: bool,
) -> PartAnim {
    let mut anim = compute_villager_anim(
        model,
        head_x_rot_deg,
        local_head_y_rot_deg,
        walk_pos,
        walk_speed,
        false,
        age_in_ticks,
    );

    for (i, part) in model.parts.iter().enumerate() {
        if part.name == "nose" {
            let (sin, cos) = (age_in_ticks * nose_wobble_speed).sin_cos();
            let mut rot = Vec3::new(sin * 4.5_f32.to_radians(), 0.0, cos * 2.5_f32.to_radians());
            if is_holding_item {
                // Vanilla moves the nose pivot from (0, -2, 0) to
                // (0, 1, -1.5); the translation is additive.
                anim.translation.push((i, Vec3::new(0.0, 3.0, -1.5)));
                rot.x = -0.9;
            }
            anim.rotation.push((i, rot));
        }
    }

    anim
}

/// Inputs for the wolf pose (`WolfModel.setupAnim` + the adult/baby
/// overrides).
pub struct WolfAnimInputs {
    pub is_sitting: bool,
    pub is_angry: bool,
    pub is_baby: bool,
    /// Vanilla `getTailAngle()` in radians, resolved by the caller.
    pub tail_angle: f32,
    /// Beg head tilt (`getHeadRollAngle`), radians.
    pub head_roll_angle: f32,
    /// Wet-shake progress 0..2 (`getShakeAnim`), lerped.
    pub shake_anim: f32,
}

/// The head-to-tail shake ripple (`WolfRenderState.getBodyRollAngle`).
fn wolf_body_roll(shake_anim: f32, offset: f32) -> f32 {
    use std::f32::consts::PI;
    let progress = ((shake_anim + offset) / 1.8).clamp(0.0, 1.0);
    (progress * PI).sin() * (progress * PI * 11.0).sin() * 0.15 * PI
}

#[allow(clippy::too_many_arguments)]
pub fn compute_wolf_anim(
    model: &BakedEntityModel,
    head_x_rot_deg: f32,
    local_head_y_rot_deg: f32,
    walk_pos: f32,
    walk_speed: f32,
    inputs: &WolfAnimInputs,
) -> PartAnim {
    use std::f32::consts::{FRAC_PI_2, PI};
    let mut anim = PartAnim::default();
    let age_scale = if inputs.is_baby { 0.5 } else { 1.0 };
    let roll = |offset: f32| wolf_body_roll(inputs.shake_anim, offset);
    // Beg tilt + shake stack on the head segment.
    let head_z = inputs.head_roll_angle + roll(0.0);
    let tail_wag = if inputs.is_angry {
        0.0
    } else {
        (walk_pos * 0.6662).cos() * 1.4 * walk_speed
    };
    let walk = |phase: f32| (walk_pos * 0.6662 + phase).cos() * 1.4 * walk_speed;

    for (i, part) in model.parts.iter().enumerate() {
        let name = part.name.as_str();
        let rot = match name {
            // Adult: look on the container, shake/beg on real_head. Baby: all
            // on head (it has no real_head).
            "head" if inputs.is_baby => vanilla_rot(
                head_x_rot_deg.to_radians(),
                local_head_y_rot_deg.to_radians(),
                head_z,
            ),
            "head" => head_rotation(head_x_rot_deg, local_head_y_rot_deg),
            "real_head" => Vec3::new(0.0, 0.0, head_z),
            "body" => {
                let x = if inputs.is_sitting {
                    // Baby `setSittingPose` subtracts a further PI/2.
                    if inputs.is_baby {
                        std::f32::consts::FRAC_PI_4 - FRAC_PI_2
                    } else {
                        std::f32::consts::FRAC_PI_4
                    }
                } else if inputs.is_baby {
                    0.0
                } else {
                    FRAC_PI_2
                };
                if inputs.is_sitting {
                    anim.translation
                        .push((i, Vec3::new(0.0, 4.0 * age_scale, -2.0 * age_scale)));
                }
                vanilla_rot(x, 0.0, roll(-0.16))
            }
            "upper_body" => {
                if inputs.is_sitting {
                    // Vanilla shifts the mane down 2 unscaled by age.
                    anim.translation.push((i, Vec3::new(0.0, 2.0, 0.0)));
                    vanilla_rot(1.2566371, 0.0, roll(-0.08))
                } else {
                    vanilla_rot(FRAC_PI_2, 0.0, roll(-0.08))
                }
            }
            "tail" => {
                if inputs.is_sitting {
                    anim.translation
                        .push((i, Vec3::new(0.0, 9.0 * age_scale, -2.0 * age_scale)));
                }
                let z = if inputs.is_baby { roll(-0.2) } else { 0.0 };
                vanilla_rot(inputs.tail_angle, tail_wag, z)
            }
            "real_tail" => Vec3::new(0.0, 0.0, roll(-0.2)),
            "right_hind_leg" | "left_hind_leg" => {
                if inputs.is_sitting {
                    anim.translation
                        .push((i, Vec3::new(0.0, 6.7 * age_scale, -5.0 * age_scale)));
                    Vec3::new(4.712389, 0.0, 0.0)
                } else {
                    let phase = if name == "right_hind_leg" { 0.0 } else { PI };
                    Vec3::new(walk(phase), 0.0, 0.0)
                }
            }
            "right_front_leg" | "left_front_leg" => {
                if inputs.is_sitting {
                    let dx = if name == "right_front_leg" {
                        0.01
                    } else {
                        -0.01
                    };
                    anim.translation
                        .push((i, Vec3::new(dx * age_scale, 1.0 * age_scale, 0.0)));
                    Vec3::new(5.811947, 0.0, 0.0)
                } else {
                    let phase = if name == "right_front_leg" { PI } else { 0.0 };
                    Vec3::new(walk(phase), 0.0, 0.0)
                }
            }
            _ => continue,
        };
        anim.rotation.push((i, rot));
    }

    anim
}

/// Inputs for the feline pose (`AdultFelineModel`/`BabyFelineModel`
/// `setupAnim`). Ocelots pass everything but crouch/sprint as false/zero.
pub struct FelineAnimInputs {
    pub is_crouching: bool,
    pub is_sprinting: bool,
    pub is_sitting: bool,
    pub lie_down_amount: f32,
    pub lie_down_amount_tail: f32,
    pub relax_state_one_amount: f32,
    pub is_baby: bool,
}

/// Ported line-by-line from the two vanilla `setupAnim` bodies; the adult and
/// baby sitting/lying blocks differ materially. Vanilla's `rotLerp` here is a
/// plain lerp (every span is far below the wrap threshold).
pub fn compute_feline_anim(
    model: &BakedEntityModel,
    head_x_rot_deg: f32,
    local_head_y_rot_deg: f32,
    walk_pos: f32,
    walk_speed: f32,
    inputs: &FelineAnimInputs,
) -> PartAnim {
    use std::f32::consts::{FRAC_PI_2, FRAC_PI_4, FRAC_PI_6, PI};
    let b = inputs.is_baby;
    let age = if b { 0.5 } else { 1.0 };

    // Vanilla-space euler + pivot-delta accumulators, seeded from the mesh
    // defaults (vanilla resetPose) with the head look applied.
    let mut head = Vec3::new(
        head_x_rot_deg.to_radians(),
        local_head_y_rot_deg.to_radians(),
        0.0,
    );
    let mut head_t = Vec3::ZERO;
    let mut body_x = if b { 0.0 } else { FRAC_PI_2 };
    let mut body_t = Vec3::ZERO;
    let mut tail1 = Vec3::new(if b { -0.567232 } else { 0.9 }, 0.0, 0.0);
    let mut tail1_t = Vec3::ZERO;
    let mut tail2_x = 0.0f32;
    let mut tail2_t = Vec3::ZERO;
    let mut lhl = Vec3::ZERO;
    let mut rhl = Vec3::ZERO;
    let mut lfl = Vec3::ZERO;
    let mut rfl = Vec3::ZERO;
    let mut lhl_t = Vec3::ZERO;
    let mut rhl_t = Vec3::ZERO;
    let mut lfl_t = Vec3::ZERO;
    let mut rfl_t = Vec3::ZERO;
    let tail1_default_y = if b { 19.107 } else { 15.0 };
    let tail2_default_y = if b { 0.0 } else { 20.0 };

    if inputs.is_crouching {
        body_t.y += 1.0 * age;
        head_t.y += 2.0 * age;
        tail1_t.y += 1.0 * age;
        tail2_t.y += -4.0 * age;
        tail2_t.z += 2.0 * age;
        tail1.x = FRAC_PI_2;
        tail2_x = FRAC_PI_2;
    } else if inputs.is_sprinting {
        // Vanilla copies the pivot absolutely: tail2.y = tail1.y.
        tail2_t.y = (tail1_default_y + tail1_t.y) - tail2_default_y;
        tail2_t.z += 2.0 * age;
        tail1.x = FRAC_PI_2;
        tail2_x = FRAC_PI_2;
    }

    if !inputs.is_sitting {
        if !b {
            body_x = FRAC_PI_2;
        }
        let pos = walk_pos * 0.6662;
        if inputs.is_sprinting {
            lhl.x = pos.cos() * walk_speed;
            rhl.x = (pos + 0.3).cos() * walk_speed;
            lfl.x = (pos + PI + 0.3).cos() * walk_speed;
            rfl.x = (pos + PI).cos() * walk_speed;
            tail2_x = 1.7278761 + 0.31415927 * walk_pos.cos() * walk_speed;
        } else {
            lhl.x = pos.cos() * walk_speed;
            rhl.x = (pos + PI).cos() * walk_speed;
            lfl.x = (pos + PI).cos() * walk_speed;
            rfl.x = pos.cos() * walk_speed;
            let sway = if inputs.is_crouching {
                0.47123894
            } else {
                FRAC_PI_4
            };
            tail2_x = 1.7278761 + sway * walk_pos.cos() * walk_speed;
        }
    } else if b {
        body_x += -0.43633232;
        body_t.y += 1.25;
        head_t.z += 0.75;
        tail1.x += 0.5454154;
        tail1_t.y += 4.0;
        tail1_t.z -= 0.9;
        lhl_t.z -= 0.9;
        rhl_t.z -= 0.9;
    } else {
        body_x = FRAC_PI_4;
        body_t.y += -4.0 * age;
        body_t.z += 5.0 * age;
        head_t.y += -3.3 * age;
        head_t.z += 1.0 * age;
        tail1_t.y += 8.0 * age;
        tail1_t.z += -2.0 * age;
        tail2_t.y += 2.0 * age;
        tail2_t.z += -0.8 * age;
        tail1.x = 1.7278761;
        tail2_x = 2.670354;
        lfl.x = -0.15707964;
        lfl_t.y += 2.0 * age;
        lfl_t.z -= 2.0 * age;
        rfl.x = -0.15707964;
        rfl_t.y += 2.0 * age;
        rfl_t.z -= 2.0 * age;
        lhl.x = -FRAC_PI_2;
        lhl_t.y += 3.0 * age;
        lhl_t.z -= 4.0 * age;
        rhl.x = -FRAC_PI_2;
        rhl_t.y += 3.0 * age;
        rhl_t.z -= 4.0 * age;
    }

    let a = inputs.lie_down_amount;
    let at = inputs.lie_down_amount_tail;
    if a > 0.0 {
        if b {
            body_t.x += 1.0;
            head.x = lerp(a, head.x, 0.17453292);
            head.z = lerp(a, head.z, -1.3089969);
            head_t += Vec3::new(1.5, 0.75, -0.5);
            rfl.x = -FRAC_PI_4;
            rfl_t += Vec3::new(3.5, -0.5, 0.0);
            lfl.x = -FRAC_PI_2;
            lfl_t += Vec3::new(1.5, -1.0, -2.0);
            rhl = Vec3::new(0.6981317, 0.34906584, -0.34906584);
            rhl_t += Vec3::new(2.5, -0.25, 0.5);
            lhl_t += Vec3::new(1.5, 0.0, -1.0);
            // Vanilla `+=` on a rotLerp result (double-applies); ported
            // verbatim.
            tail1.x += lerp(at, tail1.x, -FRAC_PI_6);
            tail1.y += lerp(at, tail1.y, 0.0);
            tail1.z += lerp(at, tail1.z, -0.17453292);
            tail1_t += Vec3::new(1.0, 0.5, -0.25);
        } else {
            head.z = lerp(a, head.z, -1.2707963);
            head.y = lerp(a, head.y, 1.2707963);
            lfl.x = -1.2707963;
            rfl.x = -0.47079635;
            rfl.z = -0.2;
            rfl_t.x += age;
            lhl.x = -0.4;
            rhl.x = 0.5;
            rhl.z = -0.5;
            rhl_t.x += 0.8 * age;
            rhl_t.y += 2.0 * age;
            tail1.x = lerp(at, tail1.x, 0.8);
            tail2_x = lerp(at, tail2_x, -0.4);
        }
    }
    let r = inputs.relax_state_one_amount;
    if r > 0.0 {
        head.x = lerp(r, head.x, -0.58177644);
    }

    let mut anim = PartAnim::default();
    for (i, part) in model.parts.iter().enumerate() {
        let (rot, tr) = match part.name.as_str() {
            "head" => (vanilla_rot(head.x, head.y, head.z), head_t),
            "body" => (Vec3::new(body_x, 0.0, 0.0), body_t),
            "tail1" => (vanilla_rot(tail1.x, tail1.y, tail1.z), tail1_t),
            "tail2" => (Vec3::new(tail2_x, 0.0, 0.0), tail2_t),
            "left_hind_leg" => (vanilla_rot(lhl.x, lhl.y, lhl.z), lhl_t),
            "right_hind_leg" => (vanilla_rot(rhl.x, rhl.y, rhl.z), rhl_t),
            "left_front_leg" => (vanilla_rot(lfl.x, lfl.y, lfl.z), lfl_t),
            "right_front_leg" => (vanilla_rot(rfl.x, rfl.y, rfl.z), rfl_t),
            _ => continue,
        };
        anim.rotation.push((i, rot));
        if tr != Vec3::ZERO {
            anim.translation.push((i, tr));
        }
    }
    anim
}

/// Keyframe interpolation mode; vanilla stores it per keyframe and uses the
/// NEXT keyframe's mode for each span.
#[derive(Clone, Copy)]
enum Kfi {
    Lin,
    Cat,
}

/// One keyframe: timestamp (seconds) + raw vanilla values (rotation channels
/// in degrees, position channels in y-down model units).
struct Kf {
    t: f32,
    x: f32,
    y: f32,
    z: f32,
    i: Kfi,
}

struct KfChannel {
    part: &'static str,
    rotation: bool,
    frames: &'static [Kf],
}

struct KfAnim {
    length: f32,
    looping: bool,
    channels: &'static [KfChannel],
}

const fn kf(t: f32, x: f32, y: f32, z: f32, i: Kfi) -> Kf {
    Kf { t, x, y, z, i }
}

/// Vanilla `Mth.catmullrom`.
fn catmullrom(a: f32, p0: Vec3, p1: Vec3, p2: Vec3, p3: Vec3) -> Vec3 {
    0.5 * (2.0 * p1
        + (p2 - p0) * a
        + (2.0 * p0 - 5.0 * p1 + 4.0 * p2 - p3) * a * a
        + (3.0 * p1 - p0 - 3.0 * p2 + p3) * a * a * a)
}

/// Vanilla `KeyframeAnimation.apply` sampling: previous keyframe by binary
/// search, alpha clamped, interpolation taken from the next keyframe.
fn kf_sample(frames: &[Kf], t: f32) -> Vec3 {
    let v = |k: &Kf| Vec3::new(k.x, k.y, k.z);
    let prev = frames.partition_point(|k| k.t < t).saturating_sub(1);
    let next = (prev + 1).min(frames.len() - 1);
    if next == prev {
        return v(&frames[prev]);
    }
    let alpha = ((t - frames[prev].t) / (frames[next].t - frames[prev].t)).clamp(0.0, 1.0);
    match frames[next].i {
        Kfi::Lin => v(&frames[prev]).lerp(v(&frames[next]), alpha),
        Kfi::Cat => catmullrom(
            alpha,
            v(&frames[prev.saturating_sub(1)]),
            v(&frames[prev]),
            v(&frames[next]),
            v(&frames[(next + 1).min(frames.len() - 1)]),
        ),
    }
}

/// Applies a keyframe animation additively into vanilla-space euler/pivot
/// deltas (vanilla `offsetRotation`/`offsetPos`; the position Y-negation of
/// `KeyframeAnimations.posVec` is applied here).
fn apply_kf_anim(
    anim: &KfAnim,
    model: &BakedEntityModel,
    elapsed_secs: f32,
    rot_delta: &mut [Vec3],
    pos_delta: &mut [Vec3],
    touched_rot: &mut [bool],
) {
    use std::f32::consts::PI;
    let t = if anim.looping {
        elapsed_secs % anim.length
    } else {
        elapsed_secs
    };
    for ch in anim.channels {
        let Some(idx) = model.parts.iter().position(|p| p.name == ch.part) else {
            continue;
        };
        let v = kf_sample(ch.frames, t);
        if ch.rotation {
            rot_delta[idx] += v * (PI / 180.0);
            touched_rot[idx] = true;
        } else {
            pos_delta[idx] += Vec3::new(v.x, -v.y, v.z);
        }
    }
}

/// `RabbitAnimation.HOP` (0.75s, looping) — transcribed from the decompiled
/// definition; all-zero channels omitted. `BABY_RABBIT_HOP` is the
/// `BabyRabbitAnimation` counterpart.
static RABBIT_HOP: KfAnim = {
    use Kfi::{Cat, Lin};
    KfAnim {
        length: 0.75,
        looping: true,
        channels: &[
            KfChannel {
                part: "body",
                rotation: true,
                frames: &[
                    kf(0.0, 0.0, 0.0, 0.0, Lin),
                    kf(0.125, 0.0, 0.0, 0.0, Lin),
                    kf(0.2083, 4.0, 0.0, 0.0, Cat),
                    kf(0.2917, 32.5, 0.0, 0.0, Lin),
                    kf(0.4167, 33.0, 0.0, 0.0, Cat),
                    kf(0.5833, 18.0, 0.0, 0.0, Cat),
                    kf(0.75, 0.0, 0.0, 0.0, Lin),
                ],
            },
            KfChannel {
                part: "head",
                rotation: true,
                frames: &[
                    kf(0.0, 0.0, 0.0, 0.0, Lin),
                    kf(0.125, 0.0, 0.0, 0.0, Lin),
                    kf(0.2083, -4.0, 0.0, 0.0, Lin),
                    kf(0.2917, -32.17, 0.0, 0.0, Lin),
                    kf(0.375, -34.58, 0.0, 0.0, Lin),
                    kf(0.5833, -20.0, 0.0, 0.0, Lin),
                    kf(0.75, 0.0, 0.0, 0.0, Lin),
                ],
            },
            KfChannel {
                part: "backlegs",
                rotation: true,
                frames: &[
                    kf(0.0, 0.0, 0.0, 0.0, Lin),
                    kf(0.125, 0.0, 0.0, 0.0, Lin),
                    kf(0.25, 125.0, 0.0, 0.0, Lin),
                    kf(0.2917, 125.0, 0.0, 0.0, Lin),
                    kf(0.375, 120.0, 0.0, 0.0, Lin),
                    kf(0.4583, 95.0, 0.0, 0.0, Lin),
                    kf(0.5417, 42.0, 0.0, 0.0, Lin),
                    kf(0.6667, 0.0, 0.0, 0.0, Lin),
                    kf(0.75, 0.0, 0.0, 0.0, Lin),
                ],
            },
            KfChannel {
                part: "frontlegs",
                rotation: true,
                frames: &[
                    kf(0.0, -0.17, 0.0, 0.0, Lin),
                    kf(0.125, -0.17, 0.0, 0.0, Lin),
                    kf(0.2083, 25.25, 0.0, 0.0, Lin),
                    kf(0.2917, -65.0, 0.0, 0.0, Cat),
                    kf(0.4583, -67.5, 0.0, 0.0, Cat),
                    kf(0.625, -1.25, 0.0, 0.0, Lin),
                    kf(0.749, -1.25, 0.0, 0.0, Lin),
                    kf(0.75, 0.0, 0.0, 0.0, Lin),
                ],
            },
            KfChannel {
                part: "frontlegs",
                rotation: false,
                frames: &[
                    kf(0.0, 0.0, 0.0, 0.0, Lin),
                    kf(0.125, 0.0, 0.0, 0.0, Lin),
                    kf(0.3333, 0.0, 0.5, 0.6, Lin),
                    kf(0.4167, 0.0, 0.9, 0.4, Lin),
                    kf(0.75, 0.0, 0.0, 0.0, Lin),
                ],
            },
            KfChannel {
                part: "right_front_leg",
                rotation: true,
                frames: &[
                    kf(0.0, 0.0, 0.0, 0.0, Cat),
                    kf(0.125, 0.0, 0.0, 0.0, Cat),
                    kf(0.2083, 0.0, 0.0, 0.0, Cat),
                    kf(0.3333, 0.0, 0.0, -17.5, Cat),
                    kf(0.5, 0.0, 0.0, -17.5, Cat),
                    kf(0.5833, 0.0, 0.0, -2.0, Cat),
                    kf(0.75, 0.0, 0.0, 0.0, Cat),
                ],
            },
            KfChannel {
                part: "left_front_leg",
                rotation: true,
                frames: &[
                    kf(0.0, 0.0, 0.0, 0.0, Cat),
                    kf(0.125, 0.0, 0.0, 0.0, Cat),
                    kf(0.2083, 0.0, 0.0, 0.0, Cat),
                    kf(0.3333, 0.0, 0.0, 20.0, Cat),
                    kf(0.5, 0.0, 0.0, 20.0, Cat),
                    kf(0.5833, 0.0, 0.0, 2.0, Cat),
                    kf(0.75, 0.0, 0.0, 0.0, Cat),
                ],
            },
            KfChannel {
                part: "left_ear",
                rotation: true,
                frames: &[
                    kf(0.0, 0.0, 0.0, 0.0, Cat),
                    kf(0.125, 2.5, 0.0, 0.0, Cat),
                    kf(0.375, -48.5, 0.0, 0.0, Cat),
                    kf(0.5417, -41.24, 0.0, 0.0, Cat),
                    kf(0.75, 0.0, 0.0, 0.0, Lin),
                ],
            },
            KfChannel {
                part: "left_ear",
                rotation: false,
                frames: &[
                    kf(0.0, -0.025, 0.0, 0.0, Cat),
                    kf(0.2083, -0.025, -0.2, 0.0, Cat),
                    kf(0.375, -0.02, -0.3, 0.0, Cat),
                    kf(0.75, -0.025, 0.0, 0.0, Cat),
                ],
            },
            KfChannel {
                part: "right_ear",
                rotation: true,
                frames: &[
                    kf(0.0, 0.0, 0.0, 0.0, Cat),
                    kf(0.125, 7.5, 0.0, 0.0, Cat),
                    kf(0.375, -31.5, 0.0, 0.0, Cat),
                    kf(0.5, -35.33, 0.0, 0.0, Cat),
                    kf(0.75, 0.0, 0.0, 0.0, Cat),
                ],
            },
            KfChannel {
                part: "right_ear",
                rotation: false,
                frames: &[
                    kf(0.0, 0.025, 0.0, 0.0, Cat),
                    kf(0.2083, 0.025, -0.3, 0.0, Cat),
                    kf(0.375, 0.02, -0.23, 0.0, Cat),
                    kf(0.75, 0.025, 0.0, 0.0, Cat),
                ],
            },
            KfChannel {
                part: "right_hind_leg",
                rotation: true,
                frames: &[
                    kf(0.0, 0.0, -2.5, 0.0, Lin),
                    kf(0.1667, 0.0, -2.5, 0.0, Lin),
                    kf(0.2083, 47.5, 0.0, 0.0, Cat),
                    kf(0.4167, 47.5, 0.0, 0.0, Cat),
                    kf(0.4583, 0.0, 0.0, 0.0, Lin),
                    kf(0.75, 0.0, -2.5, 0.0, Lin),
                ],
            },
            KfChannel {
                part: "left_hind_leg",
                rotation: true,
                frames: &[
                    kf(0.0, 0.0, 0.0, 0.0, Lin),
                    kf(0.1667, 0.0, 0.0, 0.0, Lin),
                    kf(0.2083, 47.5, 0.0, 0.0, Lin),
                    kf(0.4167, 47.5, 0.0, 0.0, Cat),
                    kf(0.4583, 0.0, 0.0, 0.0, Lin),
                    kf(0.75, 0.0, 0.0, 0.0, Lin),
                ],
            },
            KfChannel {
                part: "tail",
                rotation: true,
                frames: &[
                    kf(0.0, 0.0, 0.0, 0.0, Lin),
                    kf(0.125, -25.0, 0.0, 0.0, Lin),
                    kf(0.3333, 15.0, 0.0, 0.0, Lin),
                    kf(0.375, 27.5, 0.0, 0.0, Lin),
                    kf(0.75, 0.0, 0.0, 0.0, Lin),
                ],
            },
        ],
    }
};

static BABY_RABBIT_HOP: KfAnim = {
    use Kfi::{Cat, Lin};
    KfAnim {
        length: 0.75,
        looping: true,
        channels: &[
            KfChannel {
                part: "body",
                rotation: true,
                frames: &[
                    kf(0.0, 0.0, 0.0, 0.01, Lin),
                    kf(0.125, 0.0, 0.0, 0.01, Lin),
                    kf(0.2083, 3.75, 0.0, 0.0, Cat),
                    kf(0.2917, 32.5, 0.0, 0.0, Lin),
                    kf(0.4167, 33.0, 0.0, 0.0, Cat),
                    kf(0.5833, 18.0, 0.0, 0.0, Cat),
                    kf(0.75, 0.0, 0.0, 0.01, Lin),
                ],
            },
            KfChannel {
                part: "head",
                rotation: true,
                frames: &[
                    kf(0.0, 0.0, 0.0, 0.0, Lin),
                    kf(0.125, 0.0, 0.0, 0.0, Lin),
                    kf(0.2083, -5.25, 0.0, 0.0, Cat),
                    kf(0.2917, -32.17, 0.0, 0.0, Cat),
                    kf(0.375, -34.58, 0.0, 0.0, Cat),
                    kf(0.5833, -20.0, 0.0, 0.0, Cat),
                    kf(0.75, 0.0, 0.0, 0.0, Lin),
                ],
            },
            KfChannel {
                part: "backlegs",
                rotation: true,
                frames: &[
                    kf(0.0, 0.0, 0.0, 0.0, Lin),
                    kf(0.125, 0.0, 0.0, 0.0, Lin),
                    kf(0.25, 125.0, 0.0, 0.0, Cat),
                    kf(0.375, 125.5, 0.0, 0.0, Cat),
                    kf(0.4583, 95.0, 0.0, 0.0, Lin),
                    kf(0.5417, 42.0, 0.0, 0.0, Lin),
                    kf(0.6667, 0.0, 0.0, 0.0, Lin),
                    kf(0.75, 0.0, 0.0, 0.0, Lin),
                ],
            },
            KfChannel {
                part: "frontlegs",
                rotation: true,
                frames: &[
                    kf(0.0, -0.17, 0.0, 0.0, Lin),
                    kf(0.125, -0.17, 0.0, 0.0, Lin),
                    kf(0.2083, 14.61, 0.0, 0.0, Lin),
                    kf(0.3333, -74.37, 0.0, 0.0, Cat),
                    kf(0.5, -78.19, 0.0, 0.0, Cat),
                    kf(0.5417, -62.47, 0.0, 0.0, Cat),
                    kf(0.625, -1.25, 0.0, 0.0, Lin),
                    kf(0.749, -1.25, 0.0, 0.0, Lin),
                    kf(0.75, 0.0, 0.0, 0.0, Lin),
                ],
            },
            KfChannel {
                part: "frontlegs",
                rotation: false,
                frames: &[
                    kf(0.0, 0.0, 0.0, 0.0, Lin),
                    kf(0.125, 0.0, 0.0, 0.0, Lin),
                    kf(0.2083, 0.0, -0.16, 0.16, Lin),
                    kf(0.3333, 0.0, 0.1, -0.1, Cat),
                    kf(0.75, 0.0, 0.0, 0.0, Lin),
                ],
            },
            KfChannel {
                part: "right_front_leg",
                rotation: true,
                frames: &[
                    kf(0.0, 0.0, 0.0, 0.0, Lin),
                    kf(0.125, 0.0, 0.0, 0.0, Lin),
                    kf(0.375, 0.0, 0.0, -8.45, Cat),
                    kf(0.4583, 0.0, 0.0, -8.48, Cat),
                    kf(0.5833, 0.0, 0.0, -2.0, Cat),
                    kf(0.75, 0.0, 0.0, 0.0, Cat),
                ],
            },
            KfChannel {
                part: "right_front_leg",
                rotation: false,
                frames: &[
                    kf(0.0, 0.0, 0.0, 0.0, Lin),
                    kf(0.125, 0.0, 0.0, 0.0, Lin),
                    kf(0.2083, 0.0, 0.5, -0.5, Cat),
                    kf(0.75, 0.0, 0.0, 0.0, Lin),
                ],
            },
            KfChannel {
                part: "left_front_leg",
                rotation: true,
                frames: &[
                    kf(0.0, 0.0, 0.0, 0.0, Lin),
                    kf(0.125, 0.0, 0.0, 0.0, Lin),
                    kf(0.375, 0.0, 0.0, 10.44, Cat),
                    kf(0.4583, 0.0, 0.0, 10.61, Cat),
                    kf(0.5833, 0.0, 0.0, 2.0, Cat),
                    kf(0.75, 0.0, 0.0, 0.0, Cat),
                ],
            },
            KfChannel {
                part: "left_front_leg",
                rotation: false,
                frames: &[
                    kf(0.0, 0.0, 0.0, 0.0, Lin),
                    kf(0.125, 0.0, 0.0, 0.0, Lin),
                    kf(0.2083, 0.0, 0.5, -0.5, Cat),
                    kf(0.75, 0.0, 0.0, 0.0, Lin),
                ],
            },
            KfChannel {
                part: "left_ear",
                rotation: true,
                frames: &[
                    kf(0.0, 0.0, 0.0, 0.0, Lin),
                    kf(0.1667, 0.0, 0.0, 0.0, Lin),
                    kf(0.375, -48.5, 0.0, 0.0, Cat),
                    kf(0.5417, -41.24, 0.0, 0.0, Cat),
                    kf(0.75, 0.0, 0.0, 0.0, Lin),
                ],
            },
            KfChannel {
                part: "left_ear",
                rotation: false,
                frames: &[
                    kf(0.0, -0.02, 0.0, 0.0, Lin),
                    kf(0.1667, -0.02, 0.0, 0.0, Lin),
                    kf(0.375, -0.025, -0.5, 0.0, Cat),
                    kf(0.5417, -0.02, -0.38, 0.0, Cat),
                    kf(0.75, -0.02, 0.0, 0.0, Cat),
                ],
            },
            KfChannel {
                part: "right_ear",
                rotation: true,
                frames: &[
                    kf(0.0, 0.0, 0.0, 0.0, Lin),
                    kf(0.1667, 0.0, 0.0, 0.0, Lin),
                    kf(0.375, -44.95, 0.0, 0.0, Cat),
                    kf(0.75, 0.0, 0.0, 0.0, Cat),
                ],
            },
            KfChannel {
                part: "right_ear",
                rotation: false,
                frames: &[
                    kf(0.0, 0.05, 0.0, 0.0, Lin),
                    kf(0.1667, 0.05, 0.0, 0.0, Lin),
                    kf(0.3333, 0.05, -0.475, 0.0, Lin),
                    kf(0.5417, 0.04, -0.385, 0.0, Cat),
                    kf(0.75, 0.05, 0.0, 0.0, Cat),
                ],
            },
            KfChannel {
                part: "right_hind_leg",
                rotation: true,
                frames: &[
                    kf(0.0, 0.0, -2.5, 0.0, Lin),
                    kf(0.0833, 0.0, -2.5, 0.0, Lin),
                    kf(0.25, -25.0, -22.5, -17.5, Cat),
                    kf(0.375, -25.0, -22.5, -17.5, Cat),
                    kf(0.5417, 0.0, -2.5, 0.0, Lin),
                    kf(0.75, 0.0, -2.5, 0.0, Lin),
                ],
            },
            KfChannel {
                part: "left_hind_leg",
                rotation: true,
                frames: &[
                    kf(0.0, 0.0, 0.0, 0.0, Lin),
                    kf(0.0833, 0.0, 0.0, 0.0, Lin),
                    kf(0.25, -25.0, 25.0, 22.5, Cat),
                    kf(0.375, -25.0, 25.0, 22.5, Cat),
                    kf(0.5417, 0.0, 0.0, 0.0, Lin),
                    kf(0.75, 0.0, 0.0, 0.0, Lin),
                ],
            },
            KfChannel {
                part: "tail",
                rotation: true,
                frames: &[
                    kf(0.0, 0.0, 0.0, 0.0, Lin),
                    kf(0.125, 0.0, 0.0, 0.0, Lin),
                    kf(0.3333, 15.0, 0.0, 0.0, Cat),
                    kf(0.375, 47.5, 0.0, 0.0, Cat),
                    kf(0.5, 43.33, 0.0, 0.0, Cat),
                    kf(0.75, 0.0, 0.0, 0.0, Cat),
                ],
            },
        ],
    }
};

/// Rabbit: head look plus the HOP keyframe animation (vanilla 26.2 rabbits
/// are keyframe-animated; there is no walk cycle).
// TODO: IDLE_HEAD_TILT keyframe animation (idle flavor only; head look covers
// the idle pose meanwhile).
pub fn compute_rabbit_anim(
    model: &BakedEntityModel,
    head_x_rot_deg: f32,
    local_head_y_rot_deg: f32,
    hop_elapsed_secs: Option<f32>,
    is_baby: bool,
) -> PartAnim {
    let mut anim = PartAnim::default();
    let mut rot_delta = vec![Vec3::ZERO; model.parts.len()];
    let mut pos_delta = vec![Vec3::ZERO; model.parts.len()];
    let mut touched_rot = vec![false; model.parts.len()];

    if let Some(elapsed) = hop_elapsed_secs {
        let table = if is_baby {
            &BABY_RABBIT_HOP
        } else {
            &RABBIT_HOP
        };
        apply_kf_anim(
            table,
            model,
            elapsed,
            &mut rot_delta,
            &mut pos_delta,
            &mut touched_rot,
        );
    }

    for (i, part) in model.parts.iter().enumerate() {
        if part.name == "head" {
            // The look replaces the default pose (vanilla assigns absolutely);
            // keyframe offsets add on top.
            let e = Vec3::new(
                head_x_rot_deg.to_radians(),
                local_head_y_rot_deg.to_radians(),
                0.0,
            ) + rot_delta[i];
            anim.rotation.push((i, vanilla_rot(e.x, e.y, e.z)));
        } else if touched_rot[i] {
            let e = part.default_rotation + rot_delta[i];
            anim.rotation.push((i, vanilla_rot(e.x, e.y, e.z)));
        }
        if pos_delta[i] != Vec3::ZERO {
            anim.translation.push((i, pos_delta[i]));
        }
    }
    anim
}

/// Which equine `setupAnim` hook set applies (`AbstractEquineModel` defaults
/// vs the `BabyHorseModel` / `BabyDonkeyModel` overrides).
#[derive(Clone, Copy, PartialEq)]
pub enum EquineKind {
    Adult,
    BabyHorse,
    BabyDonkey,
}

pub struct EquineAnimInputs {
    pub kind: EquineKind,
    /// Client-simulated springs (grass eating, rearing, feeding mouth sway),
    /// lerped.
    pub eat_anim: f32,
    pub stand_anim: f32,
    pub feeding_anim: f32,
    /// Tail swish (client-local `tailCounter` RNG).
    pub animate_tail: bool,
}

/// Vanilla `AbstractEquineModel.setupAnim` with the per-model hooks.
// TODO: the in-water 0.2x leg-cycle damping (`state.isInWater`).
pub fn compute_equine_anim(
    model: &BakedEntityModel,
    head_x_rot_deg: f32,
    local_head_y_rot_deg: f32,
    walk_pos: f32,
    walk_speed: f32,
    age_in_ticks: f32,
    inputs: &EquineAnimInputs,
) -> PartAnim {
    use std::f32::consts::PI;
    let kind = inputs.kind;
    let clamped_y_rot_rad = local_head_y_rot_deg.clamp(-20.0, 20.0).to_radians();
    let mut head_rot_x_rad = head_x_rot_deg.to_radians();
    if walk_speed > 0.2 {
        head_rot_x_rad += (walk_pos * 0.8).cos() * 0.15 * walk_speed;
    }
    let eating = inputs.eat_anim;
    let standing = inputs.stand_anim;
    let i_standing = 1.0 - standing;
    let feeding = inputs.feeding_anim;
    let age_scale = if kind == EquineKind::Adult { 1.0 } else { 0.5 };

    // Per-model hooks (vanilla `getLegStandAngle` etc.).
    let (stand_angle, leg_y_off, leg_z_off, leg_x_rot_off, tail_x_off) = match kind {
        EquineKind::Adult => (0.2617994, 12.0, 4.0, -std::f32::consts::FRAC_PI_3, 0.0),
        EquineKind::BabyHorse => (
            0.2617994,
            4.0,
            0.0,
            -std::f32::consts::FRAC_PI_3,
            -std::f32::consts::FRAC_PI_2,
        ),
        EquineKind::BabyDonkey => (
            std::f32::consts::FRAC_PI_3,
            1.0,
            0.5,
            0.0,
            -std::f32::consts::FRAC_PI_4,
        ),
    };
    // The baby donkey forces a -30 degree pitch and a 90 degree eating angle.
    let (effective_pitch, eat_pose_angle) = if kind == EquineKind::BabyDonkey {
        ((-30.0f32).to_radians(), std::f32::consts::FRAC_PI_2)
    } else {
        (head_rot_x_rad, 2.1816616)
    };

    let sin_age = age_in_ticks.sin();
    let stand_or_eat = standing.max(eating);
    let base_head_angle = (1.0 - stand_or_eat)
        * (std::f32::consts::FRAC_PI_6 + effective_pitch + feeding * sin_age * 0.05);
    let head_parts_x = standing * (0.2617994 + effective_pitch)
        + eating * (eat_pose_angle + sin_age * 0.05)
        + base_head_angle;
    let head_parts_y = clamped_y_rot_rad * (standing + (1.0 - stand_or_eat));

    // `animateHeadPartsPlacement` deltas from the per-mesh default pivot:
    // `y += lerp(eating, lerp(standing, 0, y_stand), y_eat)` and an absolute
    // z lerp toward `z_to` from the mesh's default z.
    let head_parts_t = match kind {
        EquineKind::Adult => Vec3::new(
            0.0,
            lerp(eating, lerp(standing, 0.0, -8.0), 7.0),
            standing * (-4.0 - (-12.0)),
        ),
        EquineKind::BabyHorse => Vec3::new(
            0.0,
            lerp(eating, lerp(standing, 0.0, -2.0), 2.0),
            standing * (-4.0 - (-6.0)),
        ),
        // Baby donkey lerps y absolutely from its default -3.
        EquineKind::BabyDonkey => {
            Vec3::new(0.0, eating * (-1.2 - (-3.0)), standing * (-3.6 - (-5.0)))
        }
    };

    let leg_anim1 = (walk_pos * 0.6662 + PI).cos();
    let leg_x_rot = leg_anim1 * 0.8 * walk_speed;
    let stand_leg_angle = stand_angle * standing;
    let bob = (age_in_ticks * 0.6 + PI).cos();
    // Vanilla assigns the `r`-named value to the LEFT front leg (naming
    // quirk) — ported verbatim.
    let front_left_x = (leg_x_rot_off + bob) * standing + leg_x_rot * i_standing;
    let front_right_x = (leg_x_rot_off - bob) * standing - leg_x_rot * i_standing;
    let hind_left_x = stand_leg_angle - leg_anim1 * 0.5 * walk_speed * i_standing;
    let hind_right_x = stand_leg_angle + leg_anim1 * 0.5 * walk_speed * i_standing;
    let front_leg_t = Vec3::new(0.0, -leg_y_off * standing, leg_z_off * standing);
    // Vanilla copies the left front leg's pivot onto the right absolutely
    // (`rightFrontLeg.z = leftFrontLeg.z`); only the baby donkey's defaults
    // differ (left -5.3, right -5.4), so re-base the right leg by the gap.
    let front_right_t = if kind == EquineKind::BabyDonkey {
        front_leg_t + Vec3::new(0.0, 0.0, -5.3 - (-5.4))
    } else {
        front_leg_t
    };

    // `BabyDonkeyModel.offsetLegPositionWhenStanding`, incl. vanilla's
    // left-leg read for the right leg.
    let (hind_left_t, hind_right_t) = if kind == EquineKind::BabyDonkey {
        let left_y = 3.5 + standing * (-0.3 - 3.5);
        let right_y = left_y + standing * (-0.3 - left_y);
        (
            Vec3::new(0.0, left_y - 3.5, 0.0),
            Vec3::new(0.0, right_y - 3.5, 0.0),
        )
    } else {
        (Vec3::ZERO, Vec3::ZERO)
    };

    let tail_x = tail_x_off + std::f32::consts::FRAC_PI_6 + walk_speed * 0.75;
    let tail_y_rot = if inputs.animate_tail {
        (age_in_ticks * 0.7).cos()
    } else {
        0.0
    };
    let tail_t = Vec3::new(0.0, walk_speed * age_scale, walk_speed * 2.0 * age_scale);

    let mut anim = PartAnim::default();
    for (i, part) in model.parts.iter().enumerate() {
        let (rot, tr) = match part.name.as_str() {
            "head_parts" => (vanilla_rot(head_parts_x, head_parts_y, 0.0), head_parts_t),
            "body" => (
                Vec3::new(standing * -std::f32::consts::FRAC_PI_4, 0.0, 0.0),
                Vec3::ZERO,
            ),
            "left_front_leg" => (Vec3::new(front_left_x, 0.0, 0.0), front_leg_t),
            "right_front_leg" => (Vec3::new(front_right_x, 0.0, 0.0), front_right_t),
            "left_hind_leg" => (Vec3::new(hind_left_x, 0.0, 0.0), hind_left_t),
            "right_hind_leg" => (Vec3::new(hind_right_x, 0.0, 0.0), hind_right_t),
            "tail" => (vanilla_rot(tail_x, tail_y_rot, 0.0), tail_t),
            _ => continue,
        };
        anim.rotation.push((i, rot));
        if tr != Vec3::ZERO {
            anim.translation.push((i, tr));
        }
    }
    anim
}

/// Squid (`SquidModel.setupAnim`): every tentacle pitches by the stroke
/// angle on top of its baked yaw.
pub fn compute_squid_anim(model: &BakedEntityModel, tentacle_angle: f32) -> PartAnim {
    let mut anim = PartAnim::default();
    for (i, part) in model.parts.iter().enumerate() {
        if part.name.starts_with("tentacle") {
            anim.rotation
                .push((i, vanilla_rot(tentacle_angle, part.default_rotation.y, 0.0)));
        }
    }
    anim
}

/// `BatAnimation.BAT_RESTING` — a single-keyframe static pose (the 180
/// degree head/body flip is the upside-down hang). 0.5s looping.
static BAT_RESTING: KfAnim = {
    use Kfi::Lin;
    KfAnim {
        length: 0.5,
        looping: true,
        channels: &[
            KfChannel {
                part: "head",
                rotation: true,
                frames: &[kf(0.0, 180.0, 0.0, 0.0, Lin)],
            },
            KfChannel {
                part: "head",
                rotation: false,
                frames: &[kf(0.0, 0.0, 0.5, 0.0, Lin)],
            },
            KfChannel {
                part: "body",
                rotation: true,
                frames: &[kf(0.0, 180.0, 0.0, 0.0, Lin)],
            },
            KfChannel {
                part: "body",
                rotation: false,
                frames: &[kf(0.0, 0.0, 0.5, 0.0, Lin)],
            },
            KfChannel {
                part: "right_wing",
                rotation: true,
                frames: &[kf(0.0, 0.0, -10.0, 0.0, Lin)],
            },
            KfChannel {
                part: "right_wing",
                rotation: false,
                frames: &[kf(0.0, 0.0, 0.0, 1.0, Lin)],
            },
            KfChannel {
                part: "right_wing_tip",
                rotation: true,
                frames: &[kf(0.0, 0.0, -120.0, 0.0, Lin)],
            },
            KfChannel {
                part: "left_wing",
                rotation: true,
                frames: &[kf(0.0, 0.0, 10.0, 0.0, Lin)],
            },
            KfChannel {
                part: "left_wing",
                rotation: false,
                frames: &[kf(0.0, 0.0, 0.0, 1.0, Lin)],
            },
            KfChannel {
                part: "left_wing_tip",
                rotation: true,
                frames: &[kf(0.0, 0.0, 120.0, 0.0, Lin)],
            },
        ],
    }
};

/// `BatAnimation.BAT_FLYING`, 0.5s looping.
static BAT_FLYING: KfAnim = {
    use Kfi::Lin;
    KfAnim {
        length: 0.5,
        looping: true,
        channels: &[
            KfChannel {
                part: "head",
                rotation: true,
                frames: &[
                    kf(0.0, 0.0, 0.0, 0.0, Lin),
                    kf(0.125, 20.0, 0.0, 0.0, Lin),
                    kf(0.5, 0.0, 0.0, 0.0, Lin),
                ],
            },
            KfChannel {
                part: "head",
                rotation: false,
                frames: &[
                    kf(0.0, 0.0, 0.0, 0.0, Lin),
                    kf(0.125, 0.0, 2.0, 0.0, Lin),
                    kf(0.25, 0.0, 1.0, 0.0, Lin),
                    kf(0.375, 0.0, 0.0, 0.0, Lin),
                    kf(0.4583, 0.0, -1.0, 0.0, Lin),
                    kf(0.5, 0.0, 0.0, 0.0, Lin),
                ],
            },
            KfChannel {
                part: "body",
                rotation: true,
                frames: &[
                    kf(0.0, 40.0, 0.0, 0.0, Lin),
                    kf(0.25, 52.5, 0.0, 0.0, Lin),
                    kf(0.5, 40.0, 0.0, 0.0, Lin),
                ],
            },
            KfChannel {
                part: "body",
                rotation: false,
                frames: &[
                    kf(0.0, 0.0, 0.0, 0.0, Lin),
                    kf(0.125, 0.0, 2.0, 0.0, Lin),
                    kf(0.25, 0.0, 1.0, 0.0, Lin),
                    kf(0.375, 0.0, 0.0, 0.0, Lin),
                    kf(0.4583, 0.0, -1.0, 0.0, Lin),
                    kf(0.5, 0.0, 0.0, 0.0, Lin),
                ],
            },
            KfChannel {
                part: "feet",
                rotation: true,
                frames: &[
                    kf(0.0, 10.0, 0.0, 0.0, Lin),
                    kf(0.125, -21.25, 0.0, 0.0, Lin),
                    kf(0.25, -12.5, 0.0, 0.0, Lin),
                    kf(0.5, 10.0, 0.0, 0.0, Lin),
                ],
            },
            KfChannel {
                part: "right_wing",
                rotation: true,
                frames: &[
                    kf(0.0, 0.0, 85.0, 0.0, Lin),
                    kf(0.125, 0.0, -55.0, 0.0, Lin),
                    kf(0.25, 0.0, 50.0, 0.0, Lin),
                    kf(0.375, 0.0, 70.0, 0.0, Lin),
                    kf(0.5, 0.0, 85.0, 0.0, Lin),
                ],
            },
            KfChannel {
                part: "left_wing",
                rotation: true,
                frames: &[
                    kf(0.0, 0.0, -85.0, 0.0, Lin),
                    kf(0.125, 0.0, 55.0, 0.0, Lin),
                    kf(0.25, 0.0, -50.0, 0.0, Lin),
                    kf(0.375, 0.0, -70.0, 0.0, Lin),
                    kf(0.5, 0.0, -85.0, 0.0, Lin),
                ],
            },
            KfChannel {
                part: "right_wing_tip",
                rotation: true,
                frames: &[
                    kf(0.0, 0.0, 10.5, 0.0, Lin),
                    kf(0.0417, 0.0, 65.5, 0.0, Lin),
                    kf(0.2083, 0.0, -135.0, 0.0, Lin),
                    kf(0.5, 0.0, 10.5, 0.0, Lin),
                ],
            },
            KfChannel {
                part: "left_wing_tip",
                rotation: true,
                frames: &[
                    kf(0.0, 0.0, -10.5, 0.0, Lin),
                    kf(0.0417, 0.0, -65.5, 0.0, Lin),
                    kf(0.2083, 0.0, 135.0, 0.0, Lin),
                    kf(0.5, 0.0, -10.5, 0.0, Lin),
                ],
            },
        ],
    }
};

/// Bat (`BatModel.setupAnim`): purely keyframe-driven; resting adds the
/// absolute head yaw before the keyframe offsets.
pub fn compute_bat_anim(
    model: &BakedEntityModel,
    local_head_y_rot_deg: f32,
    elapsed_secs: Option<f32>,
    resting: bool,
) -> PartAnim {
    let mut anim = PartAnim::default();
    let mut rot_delta = vec![Vec3::ZERO; model.parts.len()];
    let mut pos_delta = vec![Vec3::ZERO; model.parts.len()];
    let mut touched_rot = vec![false; model.parts.len()];

    if let Some(elapsed) = elapsed_secs {
        let table = if resting { &BAT_RESTING } else { &BAT_FLYING };
        apply_kf_anim(
            table,
            model,
            elapsed,
            &mut rot_delta,
            &mut pos_delta,
            &mut touched_rot,
        );
    }

    for (i, part) in model.parts.iter().enumerate() {
        let head_look = resting && part.name == "head";
        if touched_rot[i] || head_look {
            let mut e = part.default_rotation + rot_delta[i];
            if head_look {
                e.y += local_head_y_rot_deg.to_radians();
            }
            anim.rotation.push((i, vanilla_rot(e.x, e.y, e.z)));
        }
        if pos_delta[i] != Vec3::ZERO {
            anim.translation.push((i, pos_delta[i]));
        }
    }
    anim
}

/// The four fish (`CodModel`/`SalmonModel`/`TropicalFish*Model`/
/// `Pufferfish*Model` `setupAnim`). The pufferfish gate keeps the other
/// fishes' identically-named static pectoral fins on their default pose.
pub fn compute_fish_anim(
    model: &BakedEntityModel,
    age_in_ticks: f32,
    is_in_water: bool,
    is_pufferfish: bool,
) -> PartAnim {
    let amp = if is_in_water { 1.0 } else { 1.5 };
    let mut anim = PartAnim::default();
    for (i, part) in model.parts.iter().enumerate() {
        let rot = match part.name.as_str() {
            "tail_fin" | "tail" => Vec3::new(0.0, -amp * 0.45 * (0.6 * age_in_ticks).sin(), 0.0),
            "body_back" => {
                let (a, ang) = if is_in_water { (1.0, 1.0) } else { (1.3, 1.7) };
                Vec3::new(0.0, -a * 0.25 * (ang * 0.6 * age_in_ticks).sin(), 0.0)
            }
            "right_fin" | "right_blue_fin" if is_pufferfish => {
                Vec3::new(0.0, 0.0, -0.2 + 0.4 * (age_in_ticks * 0.2).sin())
            }
            "left_fin" | "left_blue_fin" if is_pufferfish => {
                Vec3::new(0.0, 0.0, 0.2 - 0.4 * (age_in_ticks * 0.2).sin())
            }
            _ => continue,
        };
        anim.rotation.push((i, rot));
    }
    anim
}

/// The four corner positions of each cube face, in render space (Y already
/// flipped). Face order: 0 -Z, 1 +Z, 2 +Y, 3 -Y, 4 -X, 5 +X. Every face is
/// wound with its outward normal (the convention the backface-culled
/// pipeline fronts on); the ±X corner order is reversed relative to the
/// others, which their callers compensate for by swapping V.
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
        [[x0, yt, z1], [x0, yt, z0], [x0, yb, z0], [x0, yb, z1]],
        [[x1, yt, z0], [x1, yt, z1], [x1, yb, z1], [x1, yb, z0]],
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
    // Vanilla samplers REPEAT while pomme's vertex format clamps UVs to the
    // sheet; shift any face rect that lies wholly off-sheet (negative
    // texOffs fins) back into range. A rect still straddling the right seam
    // afterwards is split below; other straddles keep the clamp.
    let wrap = |lo: f32, hi: f32, extent: f32| {
        let shift = -(lo / extent).floor() * extent;
        if shift != 0.0 && hi + shift <= extent {
            (lo + shift, hi + shift)
        } else {
            (lo, hi)
        }
    };

    for (idx, pos) in positions.iter().enumerate() {
        let src = match (cube.mirror, idx) {
            (true, 4) => &face_uv[5],
            (true, 5) => &face_uv[4],
            _ => &face_uv[idx],
        };
        let (su, eu) = wrap(src[0], src[2], tw);
        let (sv, ev) = wrap(src[1], src[3], th);
        // The ±X faces are wound through a reversed corner order (see
        // `cube_face_positions`); swapping V keeps the texture upright.
        let (v_min, v_max) = if idx >= 4 {
            (ev / th, sv / th)
        } else {
            (sv / th, ev / th)
        };
        if !cube.mirror && su >= 0.0 && su < tw && eu > tw {
            // Still straddles the right sheet seam after the wrap (the small
            // tropical fish tail's +X face): split at the seam so both
            // halves land in range. U runs along corners 0->1 and 3->2.
            let t = (tw - su) / (eu - su);
            let m01 = mix3(pos[0], pos[1], t);
            let m32 = mix3(pos[3], pos[2], t);
            let quad_a = [pos[0], m01, m32, pos[3]];
            let quad_b = [m01, pos[1], pos[2], m32];
            push_face(&quad_a, su / tw, 1.0, v_min, v_max, vertices);
            push_face(&quad_b, 0.0, (eu - tw) / tw, v_min, v_max, vertices);
        } else {
            let (u_min, u_max) = if cube.mirror {
                (eu / tw, su / tw)
            } else {
                (su / tw, eu / tw)
            };
            push_face(pos, u_min, u_max, v_min, v_max, vertices);
        }
    }
}

fn mix3(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
    ]
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
    for (idx, (pos, uv)) in positions.iter().zip(face_uvs).enumerate() {
        // The ±X faces are wound through a reversed corner order (see
        // `cube_face_positions`); swapping V keeps the texture upright.
        let (v_min, v_max) = if idx >= 4 {
            (uv[3] / th, uv[1] / th)
        } else {
            (uv[1] / th, uv[3] / th)
        };
        push_face(pos, uv[0] / tw, uv[2] / tw, v_min, v_max, vertices);
    }
}
