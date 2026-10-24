use azalea_block::BlockState;
use glam::{DVec3, dvec3};

use super::aabb::Aabb;
use super::block_shape::{self, CollisionContext};
use crate::entity::components::Velocity;
use crate::position::BlockPos;
use crate::world::block::model::Direction;
use crate::world::block::{BlockStateTable, SpecialCollision};
use crate::world::block_entity_anim::BlockEntityAnimStore;
use crate::world::chunk::ChunkStore;
use crate::world::piston;

/// What the block collision scan reads.
#[derive(Clone, Copy)]
pub struct CollisionWorld<'a> {
    pub chunks: &'a ChunkStore,
    pub block_state_table: BlockStateTable,
    /// Shulker lids follow their open animation and moving pistons their
    /// blocks; `None` keeps lids closed and moving pistons empty.
    pub block_entity_anim: Option<&'a BlockEntityAnimStore>,
    /// Vanilla `PistonMovingBlockEntity.NOCLIP`: while a piston moves an
    /// entity, its own moving blocks heading that way don't collide.
    pub piston_noclip: Option<Direction>,
}

impl<'a> CollisionWorld<'a> {
    pub fn new(chunks: &'a ChunkStore, block_state_table: BlockStateTable) -> Self {
        Self {
            chunks,
            block_state_table,
            block_entity_anim: None,
            piston_noclip: None,
        }
    }

    /// With the block entities shulker lids and moving pistons collide by.
    pub fn with_block_entities(
        chunks: &'a ChunkStore,
        block_state_table: BlockStateTable,
        block_entity_anim: &'a BlockEntityAnimStore,
    ) -> Self {
        Self {
            block_entity_anim: Some(block_entity_anim),
            ..Self::new(chunks, block_state_table)
        }
    }
}

impl CollisionWorld<'_> {
    /// `ShulkerBoxBlockEntity.getBoundingBox` moved to `pos`; `None` while
    /// closed, where the static full cube applies.
    fn shulker_lid(&self, state: BlockState, pos: BlockPos) -> Option<Aabb> {
        let block_state_data = self.block_state_table.block_state_data(state);
        if block_state_data.special_collision() != SpecialCollision::ShulkerBox {
            return None;
        }
        let extension = f64::from(0.5 * self.block_entity_anim?.container(&pos)?.openness(1.0));
        if extension <= 0.0 {
            return None;
        }
        let mut local = [0.0, 0.0, 0.0, 1.0, 1.0, 1.0];
        match block_state_data.properties().get("facing").unwrap_or("up") {
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
            block_state_data.collision_shape_position(pos.x, pos.y, pos.z),
        ))
    }

    /// Vanilla `MovingPistonBlock.getCollisionShape`, through
    /// `PistonMovingBlockEntity.getCollisionShape`: nothing without a moving
    /// block, else the block where it has got to plus, for a retracting
    /// piston, its extended base.
    fn moving_piston(&self, pos: BlockPos, boxes: &mut Vec<Aabb>) {
        let Some(piston) = self
            .block_entity_anim
            .and_then(|anim| anim.pistons.get(&pos))
        else {
            return;
        };
        let blocks = self.block_state_table;
        let empty = &CollisionContext::EMPTY;
        let moved = piston.moved_state;
        if !piston.extending && piston.source && piston::is_piston_base(blocks, moved) {
            let base = blocks
                .with_property(moved, "extended", "true")
                .unwrap_or(moved);
            state_boxes(blocks, base, pos, empty, DVec3::ZERO, boxes);
        }
        if piston.progress < 1.0 && self.piston_noclip == Some(piston.movement_direction()) {
            return;
        }
        let state = if piston.source {
            let short = piston.extending != (1.0 - piston.progress < 0.25);
            piston::piston_head(blocks, piston.direction, false, short)
        } else {
            moved
        };
        state_boxes(blocks, state, pos, empty, piston.extension(), boxes);
    }

    /// Shapes that come from block entity state rather than the block table;
    /// `false` where the table applies.
    fn special_boxes(&self, state: BlockState, pos: BlockPos, boxes: &mut Vec<Aabb>) -> bool {
        if let Some(lid) = self.shulker_lid(state, pos) {
            boxes.push(lid);
            return true;
        }
        if self
            .block_state_table
            .block_state_data(state)
            .special_collision()
            == SpecialCollision::MovingPiston
        {
            self.moving_piston(pos, boxes);
            return true;
        }
        false
    }

    fn for_each_block_collision(
        &self,
        region: &Aabb,
        ctx: &CollisionContext,
        visit: impl FnMut(BlockPos, &[Aabb]),
    ) {
        for_each_block_collision(
            self.block_state_table,
            region,
            ctx,
            |x, y, z| self.chunks.get_block_state(x, y, z),
            |state, pos, boxes| self.special_boxes(state, pos, boxes),
            visit,
        );
    }
}

/// `state`'s collision boxes at `pos`, moved by `offset`.
pub(crate) fn state_boxes(
    block_state_table: BlockStateTable,
    state: BlockState,
    pos: BlockPos,
    ctx: &CollisionContext,
    offset: DVec3,
    boxes: &mut Vec<Aabb>,
) {
    let block_state_data = block_state_table.block_state_data(state);
    match block_shape::collision_shape(block_state_data, pos.y, ctx) {
        Some(local) => {
            let base = block_state_data.collision_shape_position(pos.x, pos.y, pos.z) + offset;
            boxes.extend(local.iter().map(|&b| Aabb::from_local(b, base)));
        }
        None => boxes.push(Aabb::block(pos.x, pos.y, pos.z).offset(offset)),
    }
}

/// Vanilla `BlockCollisions`: scans one cell past the region so shapes that
/// leave their cell (fences, walls) are found, keeping only those from the
/// padding shell, and visits each block whose shape intersects the region
/// with that whole shape.
fn for_each_block_collision(
    block_state_table: BlockStateTable,
    region: &Aabb,
    ctx: &CollisionContext,
    state_at: impl Fn(i32, i32, i32) -> BlockState,
    special_boxes: impl Fn(BlockState, BlockPos, &mut Vec<Aabb>) -> bool,
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
                let block_state_data = block_state_table.block_state_data(state);
                if shell_axes == 1 && !block_state_data.has_large_collision_shape()
                    || shell_axes == 2
                        && block_state_data.special_collision() != SpecialCollision::MovingPiston
                {
                    continue;
                }
                let pos = BlockPos::new(bx, by, bz);
                boxes.clear();
                if !special_boxes(state, pos, &mut boxes) {
                    state_boxes(block_state_table, state, pos, ctx, DVec3::ZERO, &mut boxes);
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

    /// Boxes near a player moving down from `feet`, with `state` at the
    /// origin and air elsewhere.
    fn boxes_near(state: BlockState, feet: DVec3, ctx: &CollisionContext) -> Vec<Aabb> {
        let region = Aabb::new(
            feet - dvec3(PLAYER_HALF_WIDTH, 0.1, PLAYER_HALF_WIDTH),
            feet + dvec3(PLAYER_HALF_WIDTH, 1.8, PLAYER_HALF_WIDTH),
        );
        let mut aabbs = Vec::new();
        for_each_block_collision(
            BlockStateTable::native(),
            &region,
            ctx,
            |x, y, z| {
                if (x, y, z) == (0, 0, 0) {
                    state
                } else {
                    BlockState::AIR
                }
            },
            |_, _, _| false,
            |_, boxes| aabbs.extend_from_slice(boxes),
        );
        aabbs
    }

    #[test]
    fn fence_top_is_found_from_the_cell_above() {
        let fence = BlockStateTable::native().find_state("oak_fence", &[]);
        let feet = dvec3(0.5, 1.55, 0.5);
        let ctx = CollisionContext::entity(feet.y, false, false);
        let boxes = boxes_near(fence, feet, &ctx);
        assert!(boxes.iter().any(|b| b.max.y == 1.5), "{boxes:?}");
    }

    #[test]
    fn scaffolding_is_solid_only_from_above() {
        let scaffolding = BlockStateTable::native()
            .find_state("scaffolding", &[("bottom", "false"), ("distance", "0")]);
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
        let scaffolding = BlockStateTable::native()
            .find_state("scaffolding", &[("bottom", "true"), ("distance", "3")]);
        let ctx = CollisionContext::entity(0.5, false, false);
        let boxes = block_shape::collision_shape(
            BlockStateTable::native().block_state_data(scaffolding),
            0,
            &ctx,
        );
        assert_eq!(boxes, Some(&[[0.0, 0.0, 0.0, 1.0, 2.0 / 16.0, 1.0]][..]));
    }

    #[test]
    fn powder_snow_holds_only_leather_boots() {
        let snow = BlockStateTable::native().find_state("powder_snow", &[]);
        let feet = dvec3(0.5, 1.0, 0.5);
        let boots = CollisionContext::entity(feet.y, false, true);
        assert!(!boxes_near(snow, feet, &boots).is_empty());
        let barefoot = CollisionContext::entity(feet.y, false, false);
        assert!(boxes_near(snow, feet, &barefoot).is_empty());
        assert!(boxes_near(snow, feet, &CollisionContext::EMPTY).is_empty());
    }

    #[test]
    fn powder_snow_catches_a_long_fall() {
        let snow = BlockStateTable::native().find_state("powder_snow", &[]);
        let falling = CollisionContext::entity(0.5, true, false).with_fall_distance(2.500_001);
        let boxes = block_shape::collision_shape(
            BlockStateTable::native().block_state_data(snow),
            0,
            &falling,
        );
        assert_eq!(
            boxes,
            Some(&[[0.0, 0.0, 0.0, 1.0, 0.9_f32 as f64, 1.0]][..])
        );
        let short = CollisionContext::entity(0.5, true, false).with_fall_distance(2.5);
        assert_eq!(
            block_shape::collision_shape(
                BlockStateTable::native().block_state_data(snow),
                0,
                &short
            ),
            Some(&[][..])
        );
    }

    #[test]
    fn explicit_collision_overrides_survive_no_collision_registration() {
        let ctx = CollisionContext::entity(0.0, false, false);

        let pitcher = BlockStateTable::native()
            .find_state("pitcher_crop", &[("age", "0"), ("half", "lower")]);
        assert!(
            !BlockStateTable::native()
                .block_state_data(pitcher)
                .has_collision()
        );
        let pitcher_shape = block_shape::collision_shape(
            BlockStateTable::native().block_state_data(pitcher),
            0,
            &ctx,
        )
        .unwrap();
        assert!(!pitcher_shape.is_empty());

        let wall_sign = BlockStateTable::native().find_state(
            "oak_wall_hanging_sign",
            &[("facing", "north"), ("waterlogged", "false")],
        );
        assert!(
            !BlockStateTable::native()
                .block_state_data(wall_sign)
                .has_collision()
        );
        assert_eq!(
            block_shape::collision_shape(
                BlockStateTable::native().block_state_data(wall_sign),
                0,
                &ctx
            ),
            Some(&[[0.0, 0.875, 0.375, 1.0, 1.0, 0.625]][..])
        );
    }

    #[test]
    fn shulker_collision_uses_current_block_entity_open_progress() {
        let state = BlockStateTable::native().find_state("shulker_box", &[("facing", "east")]);
        let pos = BlockPos::new(10, 64, -3);
        let mut anim = BlockEntityAnimStore::default();
        anim.set_open_count(pos, 1);
        anim.tick();
        let chunks = ChunkStore::new(2);
        let world = CollisionWorld::with_block_entities(&chunks, BlockStateTable::native(), &anim);

        let aabb = world.shulker_lid(state, pos).unwrap();
        assert_eq!(aabb.min, dvec3(10.0, 64.0, -3.0));
        let extension = f64::from(0.5_f32 * 0.1_f32);
        assert_eq!(aabb.max.x.to_bits(), ((1.0 + extension) + 10.0).to_bits());
        assert_eq!(aabb.max.y, 65.0);
        assert_eq!(aabb.max.z, -2.0);
        assert!(
            CollisionWorld::new(&chunks, BlockStateTable::native())
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

    #[test]
    fn moving_pistons_collide_where_their_block_has_got_to() {
        use crate::world::block::model::Direction;
        use crate::world::piston::MovingPiston;
        let pos = BlockPos::new(4, 64, 4);
        let moving = BlockStateTable::native()
            .find_state("moving_piston", &[("facing", "east"), ("type", "normal")]);
        let stone = BlockStateTable::native().find_state("stone", &[]);
        let mut anim = BlockEntityAnimStore::default();
        let mut piston = MovingPiston::new(stone, Direction::East, true, false);
        piston.progress = 0.5;
        anim.pistons.insert(pos, piston);
        let chunks = ChunkStore::new(2);
        let mut world =
            CollisionWorld::with_block_entities(&chunks, BlockStateTable::native(), &anim);
        let boxes = |world: &CollisionWorld| {
            let mut boxes = Vec::new();
            assert!(world.special_boxes(moving, pos, &mut boxes));
            boxes
        };
        // Halfway out of the cell to the west, heading east.
        let halfway = boxes(&world);
        assert_eq!(halfway.len(), 1);
        assert_eq!(halfway[0].min, dvec3(3.5, 64.0, 4.0));
        assert_eq!(halfway[0].max, dvec3(4.5, 65.0, 5.0));
        world.piston_noclip = Some(Direction::East);
        assert!(boxes(&world).is_empty());
        world.piston_noclip = None;

        // A retracting piston: its extended base, and the head on its way in.
        let base = BlockStateTable::native()
            .find_state("piston", &[("facing", "east"), ("extended", "false")]);
        let mut retracting = MovingPiston::new(base, Direction::East, false, true);
        retracting.progress = 0.5;
        let mut retract_anim = BlockEntityAnimStore::default();
        retract_anim.pistons.insert(pos, retracting);
        world.block_entity_anim = Some(&retract_anim);
        let boxes = boxes(&world);
        assert!(
            boxes.iter().any(|b| b.min.x == 4.0 && b.max.x < 5.0),
            "{boxes:?}"
        );
        assert!(boxes.iter().any(|b| b.max.x > 5.0), "{boxes:?}");

        let mut empty = Vec::new();
        assert!(world.special_boxes(moving, BlockPos::new(0, 64, 0), &mut empty));
        assert!(empty.is_empty());
    }
}
