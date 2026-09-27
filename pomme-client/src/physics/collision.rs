use azalea_block::BlockState;
use azalea_core::position::BlockPos;
use glam::{DVec3, dvec3};

use super::aabb::Aabb;
use super::block_shape::{self, CollisionContext};
use crate::entity::components::Velocity;
use crate::world::block::{
    SpecialCollision, block_properties, collision_shape_position, has_large_collision_shape,
    special_collision,
};
use crate::world::block_entity_anim::BlockEntityAnimStore;
use crate::world::chunk::ChunkStore;

/// What the block collision scan reads.
#[derive(Clone, Copy)]
pub struct CollisionWorld<'a> {
    pub chunks: &'a ChunkStore,
    /// Shulker lids follow their open animation; `None` keeps them closed.
    pub block_entity_anim: Option<&'a BlockEntityAnimStore>,
}

impl<'a> From<&'a ChunkStore> for CollisionWorld<'a> {
    fn from(chunks: &'a ChunkStore) -> Self {
        Self {
            chunks,
            block_entity_anim: None,
        }
    }
}

impl CollisionWorld<'_> {
    /// `ShulkerBoxBlockEntity.getBoundingBox` moved to `pos`; `None` while
    /// closed, where the static full cube applies.
    fn shulker_lid(&self, state: BlockState, pos: BlockPos) -> Option<Aabb> {
        if special_collision(state) != SpecialCollision::ShulkerBox {
            return None;
        }
        let extension = f64::from(0.5 * self.block_entity_anim?.container(&pos)?.openness(1.0));
        if extension <= 0.0 {
            return None;
        }
        let mut local = [0.0, 0.0, 0.0, 1.0, 1.0, 1.0];
        match block_properties(state).get("facing").unwrap_or("up") {
            "down" => local[1] -= extension,
            "up" => local[4] += extension,
            "north" => local[2] -= extension,
            "south" => local[5] += extension,
            "west" => local[0] -= extension,
            "east" => local[3] += extension,
            _ => {}
        }
        Some(Aabb::from_local(
            local,
            collision_shape_position(state, pos.x, pos.y, pos.z),
        ))
    }

    fn for_each_block_collision(
        &self,
        region: &Aabb,
        ctx: &CollisionContext,
        visit: impl FnMut(BlockPos, &[Aabb]),
    ) {
        for_each_block_collision(
            region,
            ctx,
            |x, y, z| self.chunks.get_block_state(x, y, z),
            |state, pos| self.shulker_lid(state, pos),
            visit,
        );
    }
}

/// Vanilla `BlockCollisions`: scans one cell past the region so shapes that
/// leave their cell (fences, walls) are found, keeping only those from the
/// padding shell, and visits each block whose shape intersects the region
/// with that whole shape.
fn for_each_block_collision(
    region: &Aabb,
    ctx: &CollisionContext,
    state_at: impl Fn(i32, i32, i32) -> BlockState,
    shulker_lid: impl Fn(BlockState, BlockPos) -> Option<Aabb>,
    mut visit: impl FnMut(BlockPos, &[Aabb]),
) {
    let lo = |v: f64| (v - 1.0e-7).floor() as i32 - 1;
    let hi = |v: f64| (v + 1.0e-7).floor() as i32 + 1;
    let (min_x, min_y, min_z) = (lo(region.min.x), lo(region.min.y), lo(region.min.z));
    let (max_x, max_y, max_z) = (hi(region.max.x), hi(region.max.y), hi(region.max.z));
    let mut boxes = Vec::new();

    for by in min_y..=max_y {
        for bz in min_z..=max_z {
            for bx in min_x..=max_x {
                // `Cursor3D.getNextType`: how many axes sit on the shell.
                let shell_axes = u8::from(bx == min_x || bx == max_x)
                    + u8::from(by == min_y || by == max_y)
                    + u8::from(bz == min_z || bz == max_z);
                if shell_axes == 3 {
                    continue;
                }
                let state = state_at(bx, by, bz);
                if shell_axes == 1 && !has_large_collision_shape(state)
                    || shell_axes == 2 && special_collision(state) != SpecialCollision::MovingPiston
                {
                    continue;
                }
                let pos = BlockPos::new(bx, by, bz);
                boxes.clear();
                if let Some(lid) = shulker_lid(state, pos) {
                    boxes.push(lid);
                } else {
                    match block_shape::collision_shape(state, by, ctx) {
                        Some(local) => {
                            let offset = collision_shape_position(state, bx, by, bz);
                            boxes.extend(local.iter().map(|&b| Aabb::from_local(b, offset)));
                        }
                        None => boxes.push(Aabb::block(bx, by, bz)),
                    }
                }
                if boxes.iter().any(|b| b.intersects(region)) {
                    visit(pos, &boxes);
                }
            }
        }
    }
}

pub fn collect_block_aabbs(
    world: CollisionWorld<'_>,
    region: &Aabb,
    ctx: &CollisionContext,
) -> Vec<Aabb> {
    let mut aabbs = Vec::new();
    world.for_each_block_collision(region, ctx, |_, boxes| aabbs.extend_from_slice(boxes));
    aabbs
}

pub fn no_collision(world: CollisionWorld<'_>, aabb: &Aabb, ctx: &CollisionContext) -> bool {
    collect_block_aabbs(world, aabb, ctx).is_empty()
}

/// `Vec3i.compareTo` order: Y, then Z, then X.
fn support_pos_greater(candidate: BlockPos, current: BlockPos) -> bool {
    (candidate.y, candidate.z, candidate.x) > (current.y, current.z, current.x)
}

/// Vanilla `CollisionGetter.findSupportingBlock`: the colliding block nearest
/// `entity_position` by `distToCenterSqr`; ties go to the greater position.
pub fn find_supporting_block(
    world: CollisionWorld<'_>,
    test_area: &Aabb,
    entity_position: DVec3,
    ctx: &CollisionContext,
) -> Option<BlockPos> {
    let mut best: Option<(BlockPos, f64)> = None;
    world.for_each_block_collision(test_area, ctx, |pos, _| {
        let center = dvec3(
            f64::from(pos.x) + 0.5,
            f64::from(pos.y) + 0.5,
            f64::from(pos.z) + 0.5,
        );
        let distance = (center - entity_position).length_squared();
        let wins = best.is_none_or(|(current, best_distance)| {
            distance < best_distance
                || distance == best_distance && support_pos_greater(pos, current)
        });
        if wins {
            best = Some((pos, distance));
        }
    });
    best.map(|(pos, _)| pos)
}

fn collide_along_axes(
    block_aabbs: &[Aabb],
    player_aabb: Aabb,
    mut velocity: Velocity,
) -> (DVec3, bool) {
    let original_y = velocity.y;

    for block in block_aabbs {
        velocity.y = block.clip_y_collide(&player_aabb, velocity.y);
    }
    let mut resolved = player_aabb.offset(dvec3(0.0, velocity.y, 0.0));

    let x_first = velocity.x.abs() >= velocity.z.abs();

    if x_first {
        for block in block_aabbs {
            velocity.x = block.clip_x_collide(&resolved, velocity.x);
        }
        resolved = resolved.offset(dvec3(velocity.x, 0.0, 0.0));

        for block in block_aabbs {
            velocity.z = block.clip_z_collide(&resolved, velocity.z);
        }
    } else {
        for block in block_aabbs {
            velocity.z = block.clip_z_collide(&resolved, velocity.z);
        }
        resolved = resolved.offset(dvec3(0.0, 0.0, velocity.z));

        for block in block_aabbs {
            velocity.x = block.clip_x_collide(&resolved, velocity.x);
        }
    }

    let on_ground = original_y < 0.0 && velocity.y != original_y;

    (*velocity, on_ground)
}

/// Vanilla `Entity.collectCandidateStepUpHeights`: the colliders' Y faces
/// within reach, narrowed to float, deduplicated and sorted.
fn collect_candidate_step_up_heights(
    grounded_aabb: Aabb,
    colliders: &[Aabb],
    max_step_height: f64,
    step_height_to_skip: f64,
) -> Vec<f64> {
    let max_step_height = max_step_height as f32;
    let step_height_to_skip = step_height_to_skip as f32;
    let mut candidates = Vec::<f32>::new();

    for collider in colliders {
        for coord in [collider.min.y, collider.max.y] {
            let relative = (coord - grounded_aabb.min.y) as f32;
            if relative < 0.0 || relative == step_height_to_skip || relative > max_step_height {
                continue;
            }
            if !candidates.contains(&relative) {
                candidates.push(relative);
            }
        }
    }

    candidates.sort_unstable_by(f32::total_cmp);
    candidates.into_iter().map(f64::from).collect()
}

/// Vanilla `Entity.collide`, stepping up when this move landed or the entity
/// was already on the ground.
pub fn resolve_collision(
    world: CollisionWorld<'_>,
    player_aabb: Aabb,
    velocity: Velocity,
    max_step_height: f64,
    was_on_ground: bool,
    ctx: &CollisionContext,
) -> (DVec3, bool) {
    let expanded = player_aabb.expand(*velocity);
    let block_aabbs = collect_block_aabbs(world, &expanded, ctx);

    let (movement_step, on_ground_after_collision) =
        collide_along_axes(&block_aabbs, player_aabb, velocity);

    let horizontal_blocked = movement_step.x != velocity.x || movement_step.z != velocity.z;
    let mut resolved = movement_step;

    if max_step_height > 0.0 && (on_ground_after_collision || was_on_ground) && horizontal_blocked {
        let grounded_aabb = if on_ground_after_collision {
            player_aabb.offset(dvec3(0.0, movement_step.y, 0.0))
        } else {
            player_aabb
        };
        let mut step_region = grounded_aabb.expand(dvec3(velocity.x, max_step_height, velocity.z));
        if !on_ground_after_collision {
            step_region = step_region.expand(dvec3(0.0, -f64::from(1.0e-5_f32), 0.0));
        }
        let step_aabbs = collect_block_aabbs(world, &step_region, ctx);
        let candidates = collect_candidate_step_up_heights(
            grounded_aabb,
            &step_aabbs,
            max_step_height,
            movement_step.y,
        );
        let base_horizontal_distance =
            movement_step.x * movement_step.x + movement_step.z * movement_step.z;

        for candidate in candidates {
            let (step_from_ground, _) = collide_along_axes(
                &step_aabbs,
                grounded_aabb,
                Velocity::new(velocity.x, candidate, velocity.z),
            );
            let step_horizontal_distance =
                step_from_ground.x * step_from_ground.x + step_from_ground.z * step_from_ground.z;
            if step_horizontal_distance > base_horizontal_distance {
                let distance_to_ground = player_aabb.min.y - grounded_aabb.min.y;
                resolved = step_from_ground - dvec3(0.0, distance_to_ground, 0.0);
                break;
            }
        }
    }

    // Vanilla derives on-ground from the final movement, after any step.
    let on_ground = velocity.y < 0.0 && resolved.y != velocity.y;
    (resolved, on_ground)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::player::PLAYER_HALF_WIDTH;
    use crate::world::block::{find_state, has_collision};

    /// Boxes near a player moving down from `feet`, with `state` at the
    /// origin and air elsewhere.
    fn boxes_near(state: BlockState, feet: DVec3, ctx: &CollisionContext) -> Vec<Aabb> {
        let region = Aabb::new(
            feet - dvec3(PLAYER_HALF_WIDTH, 0.1, PLAYER_HALF_WIDTH),
            feet + dvec3(PLAYER_HALF_WIDTH, 1.8, PLAYER_HALF_WIDTH),
        );
        let mut aabbs = Vec::new();
        for_each_block_collision(
            &region,
            ctx,
            |x, y, z| {
                if (x, y, z) == (0, 0, 0) {
                    state
                } else {
                    BlockState::AIR
                }
            },
            |_, _| None,
            |_, boxes| aabbs.extend_from_slice(boxes),
        );
        aabbs
    }

    #[test]
    fn fence_top_is_found_from_the_cell_above() {
        crate::world::block::init("26.2");
        let fence = find_state("oak_fence", &[]);
        let feet = dvec3(0.5, 1.55, 0.5);
        let ctx = CollisionContext::entity(feet.y, false, false);
        let boxes = boxes_near(fence, feet, &ctx);
        assert!(boxes.iter().any(|b| b.max.y == 1.5), "{boxes:?}");
    }

    #[test]
    fn scaffolding_is_solid_only_from_above() {
        crate::world::block::init("26.2");
        let scaffolding = find_state("scaffolding", &[("bottom", "false"), ("distance", "0")]);
        let top = dvec3(0.5, 1.0, 0.5);
        let inside = dvec3(0.5, 0.5, 0.5);
        let standing = CollisionContext::entity(top.y, false, false);
        assert!(!boxes_near(scaffolding, top, &standing).is_empty());
        let sneaking = CollisionContext::entity(top.y, true, false);
        assert!(boxes_near(scaffolding, top, &sneaking).is_empty());
        let climbing = CollisionContext::entity(inside.y, false, false);
        assert!(boxes_near(scaffolding, inside, &climbing).is_empty());
        // A particle inside falls through; the empty context always stands.
        let particle = CollisionContext::position(inside.y);
        assert!(boxes_near(scaffolding, inside, &particle).is_empty());
        assert!(!boxes_near(scaffolding, inside, &CollisionContext::EMPTY).is_empty());
    }

    #[test]
    fn unstable_scaffolding_keeps_its_bottom_plate() {
        crate::world::block::init("26.2");
        let scaffolding = find_state("scaffolding", &[("bottom", "true"), ("distance", "3")]);
        let ctx = CollisionContext::entity(0.5, false, false);
        let boxes = block_shape::collision_shape(scaffolding, 0, &ctx);
        assert_eq!(boxes, Some(&[[0.0, 0.0, 0.0, 1.0, 2.0 / 16.0, 1.0]][..]));
    }

    #[test]
    fn powder_snow_holds_only_leather_boots() {
        crate::world::block::init("26.2");
        let snow = find_state("powder_snow", &[]);
        let feet = dvec3(0.5, 1.0, 0.5);
        let boots = CollisionContext::entity(feet.y, false, true);
        assert!(!boxes_near(snow, feet, &boots).is_empty());
        let barefoot = CollisionContext::entity(feet.y, false, false);
        assert!(boxes_near(snow, feet, &barefoot).is_empty());
        assert!(boxes_near(snow, feet, &CollisionContext::EMPTY).is_empty());
    }

    #[test]
    fn powder_snow_catches_a_long_fall() {
        crate::world::block::init("26.2");
        let snow = find_state("powder_snow", &[]);
        let falling = CollisionContext::entity(0.5, true, false).with_fall_distance(2.500_001);
        let boxes = block_shape::collision_shape(snow, 0, &falling);
        assert_eq!(
            boxes,
            Some(&[[0.0, 0.0, 0.0, 1.0, 0.9_f32 as f64, 1.0]][..])
        );
        let short = CollisionContext::entity(0.5, true, false).with_fall_distance(2.5);
        assert_eq!(block_shape::collision_shape(snow, 0, &short), Some(&[][..]));
    }

    #[test]
    fn explicit_collision_overrides_survive_no_collision_registration() {
        crate::world::block::init("26.2");
        let ctx = CollisionContext::entity(0.0, false, false);

        let pitcher = find_state("pitcher_crop", &[("age", "0"), ("half", "lower")]);
        assert!(!has_collision(pitcher));
        let pitcher_shape = block_shape::collision_shape(pitcher, 0, &ctx).unwrap();
        assert!(!pitcher_shape.is_empty());

        let wall_sign = find_state(
            "oak_wall_hanging_sign",
            &[("facing", "north"), ("waterlogged", "false")],
        );
        assert!(!has_collision(wall_sign));
        assert_eq!(
            block_shape::collision_shape(wall_sign, 0, &ctx),
            Some(&[[0.0, 0.875, 0.375, 1.0, 1.0, 0.625]][..])
        );
    }

    #[test]
    fn shulker_collision_uses_current_block_entity_open_progress() {
        crate::world::block::init("26.2");
        let state = find_state("shulker_box", &[("facing", "east")]);
        let pos = BlockPos::new(10, 64, -3);
        let mut anim = BlockEntityAnimStore::default();
        anim.set_open_count(pos, 1);
        anim.tick();
        let chunks = ChunkStore::new(2);
        let world = CollisionWorld {
            chunks: &chunks,
            block_entity_anim: Some(&anim),
        };

        let aabb = world.shulker_lid(state, pos).unwrap();
        assert_eq!(aabb.min, dvec3(10.0, 64.0, -3.0));
        let extension = f64::from(0.5_f32 * 0.1_f32);
        assert_eq!(aabb.max.x.to_bits(), ((1.0 + extension) + 10.0).to_bits());
        assert_eq!(aabb.max.y, 65.0);
        assert_eq!(aabb.max.z, -2.0);
        assert!(
            CollisionWorld::from(&chunks)
                .shulker_lid(state, pos)
                .is_none()
        );
    }

    #[test]
    fn supporting_block_tie_break_matches_vec3i_order() {
        let current = BlockPos::new(0, 64, 0);
        assert!(support_pos_greater(BlockPos::new(1, 64, 0), current));
        assert!(support_pos_greater(BlockPos::new(-5, 64, 1), current));
        assert!(support_pos_greater(BlockPos::new(-5, 65, -5), current));
        assert!(!support_pos_greater(BlockPos::new(5, 63, 5), current));
    }

    #[test]
    fn step_candidates_match_vanilla_sorted_shape_faces() {
        let grounded = Aabb::new(dvec3(0.0, 1.0, 0.0), dvec3(0.6, 2.8, 0.6));
        let colliders = [
            Aabb::new(dvec3(0.6, 1.5, 0.0), dvec3(1.6, 2.0, 1.0)),
            Aabb::new(dvec3(0.6, 1.25, 0.0), dvec3(1.6, 1.5, 1.0)),
        ];

        assert_eq!(
            collect_candidate_step_up_heights(grounded, &colliders, 0.6, 0.0),
            vec![0.25, 0.5]
        );
        assert_eq!(
            collect_candidate_step_up_heights(grounded, &colliders, 0.6, 0.5),
            vec![0.25]
        );
    }
}
