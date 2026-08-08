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
    let limb = |name: &str, pivot: Vec3, uv: (u32, u32), origin_y: f32, h: f32| {
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
    let limb = |name: &str, pivot: Vec3, uv: (u32, u32), h: f32| {
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
    let leg = |name: &str, x: f32, shin_uv: (u32, u32), foot_uv: (u32, u32)| {
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
    let wing = |name: &str, x: f32, origin_x: f32, uv: (u32, u32)| {
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
fn vbox(tex_offset: (u32, u32), origin: (f32, f32, f32), size: (f32, f32, f32)) -> ModelCube {
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
