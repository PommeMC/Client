use azalea_core::position::BlockPos;
use glam::{DVec3, dvec3};

use super::aabb::Aabb;
use super::block_shape;
use crate::entity::components::Velocity;
use crate::world::block::{
    block_id, block_properties, collision_shape_position_delta, has_collision,
};
use crate::world::block_entity_anim::BlockEntityAnimStore;
use crate::world::chunk::ChunkStore;

const EMPTY_SHAPE: &[block_shape::LocalBox] = &[];
const FULL_CUBE_SHAPE: &[block_shape::LocalBox] = &[[0.0, 0.0, 0.0, 1.0, 1.0, 1.0]];
const SCAFFOLDING_STABLE: &[block_shape::LocalBox] = &[
    [0.0, 0.875, 0.0, 1.0, 1.0, 1.0],
    [0.0, 0.0, 0.0, 0.125, 1.0, 0.125],
    [0.875, 0.0, 0.0, 1.0, 1.0, 0.125],
    [0.0, 0.0, 0.875, 0.125, 1.0, 1.0],
    [0.875, 0.0, 0.875, 1.0, 1.0, 1.0],
];
const SCAFFOLDING_UNSTABLE_BOTTOM: &[block_shape::LocalBox] = &[[0.0, 0.0, 0.0, 1.0, 0.125, 1.0]];
const POWDER_SNOW_FALLING: &[block_shape::LocalBox] = &[[0.0, 0.0, 0.0, 1.0, 0.9_f32 as f64, 1.0]];
const COLLISION_CONTEXT_EPSILON: f64 = 1.0e-5_f32 as f64;

#[derive(Clone, Copy)]
pub struct PlayerCollisionContext<'a> {
    pub feet_y: f64,
    pub descending: bool,
    pub fall_distance: f64,
    pub leather_boots: bool,
    pub block_entity_anim: Option<&'a BlockEntityAnimStore>,
}

fn is_above(context: &PlayerCollisionContext<'_>, shape_max_y: f64, block_y: i32) -> bool {
    context.feet_y > block_y as f64 + shape_max_y - COLLISION_CONTEXT_EPSILON
}

fn player_partial_shape(
    state: azalea_block::BlockState,
    block_y: i32,
    context: &PlayerCollisionContext<'_>,
) -> Option<&'static [block_shape::LocalBox]> {
    match block_id(state) {
        "scaffolding" => {
            if is_above(context, 1.0, block_y) && !context.descending {
                return Some(SCAFFOLDING_STABLE);
            }
            let props = block_properties(state);
            let distance_nonzero = props.get("distance").is_some_and(|v| v != "0");
            let bottom = props.get("bottom") == Some("true");
            if distance_nonzero && bottom && is_above(context, 0.0, block_y) {
                return Some(SCAFFOLDING_UNSTABLE_BOTTOM);
            }
            Some(EMPTY_SHAPE)
        }
        "powder_snow" => {
            if context.fall_distance > 2.5 {
                return Some(POWDER_SNOW_FALLING);
            }
            if context.leather_boots && is_above(context, 1.0, block_y) && !context.descending {
                return Some(FULL_CUBE_SHAPE);
            }
            Some(EMPTY_SHAPE)
        }
        _ => block_shape::partial_shape(state),
    }
}

fn collision_shape_for(
    state: azalea_block::BlockState,
    block_y: i32,
    context: Option<&PlayerCollisionContext<'_>>,
) -> Option<&'static [block_shape::LocalBox]> {
    context.map_or_else(
        || block_shape::partial_shape(state),
        |context| player_partial_shape(state, block_y, context),
    )
}

fn shulker_collision_aabb(
    state: azalea_block::BlockState,
    bx: i32,
    by: i32,
    bz: i32,
    context: &PlayerCollisionContext<'_>,
) -> Option<Aabb> {
    if !block_id(state).ends_with("shulker_box") {
        return None;
    }
    let anim = context
        .block_entity_anim?
        .container(&BlockPos::new(bx, by, bz))?;
    let extension = 0.5 * f64::from(anim.openness(1.0));
    if extension <= 0.0 {
        return None;
    }

    let mut min = dvec3(bx as f64, by as f64, bz as f64);
    let mut max = min + DVec3::ONE;
    match block_properties(state).get("facing").unwrap_or("up") {
        "down" => min.y -= extension,
        "up" => max.y += extension,
        "north" => min.z -= extension,
        "south" => max.z += extension,
        "west" => min.x -= extension,
        "east" => max.x += extension,
        _ => {}
    }
    Some(Aabb::new(min, max))
}

pub fn collect_block_aabbs(chunk_store: &ChunkStore, region: &Aabb) -> Vec<Aabb> {
    collect_block_aabbs_inner(chunk_store, region, None)
}

pub fn collect_player_block_aabbs(
    chunk_store: &ChunkStore,
    region: &Aabb,
    context: &PlayerCollisionContext<'_>,
) -> Vec<Aabb> {
    collect_block_aabbs_inner(chunk_store, region, Some(context))
}

fn collect_block_aabbs_inner(
    chunk_store: &ChunkStore,
    region: &Aabb,
    context: Option<&PlayerCollisionContext<'_>>,
) -> Vec<Aabb> {
    let mut aabbs = Vec::new();

    let min_x = region.min.x.floor() as i32;
    let min_y = region.min.y.floor() as i32;
    let min_z = region.min.z.floor() as i32;
    let max_x = region.max.x.ceil() as i32;
    let max_y = region.max.y.ceil() as i32;
    let max_z = region.max.z.ceil() as i32;

    for by in min_y..max_y {
        for bz in min_z..max_z {
            for bx in min_x..max_x {
                let state = chunk_store.get_block_state(bx, by, bz);
                if let Some(context) = context
                    && let Some(shulker) = shulker_collision_aabb(state, bx, by, bz, context)
                {
                    aabbs.push(shulker);
                    continue;
                }
                let shape = collision_shape_for(state, by, context);
                match shape {
                    Some(boxes) => {
                        // Explicit getCollisionShape overrides are authoritative
                        // even for blocks registered with `noCollision` (notably
                        // PitcherCrop and WallHangingSign in 26.2). An empty
                        // generated shape means truly non-colliding.
                        if boxes.is_empty() {
                            continue;
                        }
                        let offset = dvec3(bx as f64, by as f64, bz as f64)
                            + collision_shape_position_delta(state, bx, bz);
                        aabbs.extend(boxes.iter().map(|&b| Aabb::from_local(b, offset)));
                    }
                    None => {
                        // `None` is the compact full-cube representation. Older
                        // tables also use None for unsupported/no-collision
                        // states, so retain the registration guard there.
                        if has_collision(state) {
                            aabbs.push(Aabb::block(bx, by, bz));
                        }
                    }
                }
            }
        }
    }

    aabbs
}

/// Vanilla `CollisionGetter.findSupportingBlock` for Pomme's current block-only
/// collision world. The winner is the intersecting block whose center is
/// nearest the entity position; exact ties choose the greater Vec3i ordering
/// (Y, then Z, then X), matching vanilla.
fn support_pos_greater(candidate: BlockPos, current: BlockPos) -> bool {
    (candidate.y, candidate.z, candidate.x) > (current.y, current.z, current.x)
}

pub fn find_supporting_block(
    chunk_store: &ChunkStore,
    test_area: &Aabb,
    entity_position: DVec3,
    context: Option<&PlayerCollisionContext<'_>>,
) -> Option<BlockPos> {
    const SHAPE_EPSILON: f64 = 1.0e-7;

    let min_x = (test_area.min.x - SHAPE_EPSILON).floor() as i32 - 1;
    let min_y = (test_area.min.y - SHAPE_EPSILON).floor() as i32 - 1;
    let min_z = (test_area.min.z - SHAPE_EPSILON).floor() as i32 - 1;
    let max_x = (test_area.max.x + SHAPE_EPSILON).floor() as i32 + 1;
    let max_y = (test_area.max.y + SHAPE_EPSILON).floor() as i32 + 1;
    let max_z = (test_area.max.z + SHAPE_EPSILON).floor() as i32 + 1;

    let mut best: Option<BlockPos> = None;
    let mut best_distance = f64::MAX;

    for by in min_y..=max_y {
        for bz in min_z..=max_z {
            for bx in min_x..=max_x {
                let state = chunk_store.get_block_state(bx, by, bz);
                if let Some(context) = context
                    && let Some(shulker) = shulker_collision_aabb(state, bx, by, bz, context)
                {
                    if !shulker.intersects(test_area) {
                        continue;
                    }
                    let candidate = BlockPos::new(bx, by, bz);
                    let center = dvec3(bx as f64 + 0.5, by as f64 + 0.5, bz as f64 + 0.5);
                    let distance = (center - entity_position).length_squared();
                    let wins_tie =
                        best.is_none_or(|current| support_pos_greater(candidate, current));
                    if distance < best_distance || (distance == best_distance && wins_tie) {
                        best = Some(candidate);
                        best_distance = distance;
                    }
                    continue;
                }
                let shape = collision_shape_for(state, by, context);
                let offset = dvec3(bx as f64, by as f64, bz as f64)
                    + collision_shape_position_delta(state, bx, bz);
                let intersects = match shape {
                    Some(boxes) => boxes
                        .iter()
                        .map(|&shape| Aabb::from_local(shape, offset))
                        .any(|shape| shape.intersects(test_area)),
                    None => has_collision(state) && Aabb::block(bx, by, bz).intersects(test_area),
                };
                if !intersects {
                    continue;
                }

                let candidate = BlockPos::new(bx, by, bz);
                let center = offset + dvec3(0.5, 0.5, 0.5);
                let distance = (center - entity_position).length_squared();
                let wins_tie = best.is_none_or(|current| support_pos_greater(candidate, current));
                if distance < best_distance || (distance == best_distance && wins_tie) {
                    best = Some(candidate);
                    best_distance = distance;
                }
            }
        }
    }

    best
}

pub fn no_player_collision(
    chunk_store: &ChunkStore,
    aabb: &Aabb,
    context: &PlayerCollisionContext<'_>,
) -> bool {
    collect_player_block_aabbs(chunk_store, aabb, context)
        .iter()
        .all(|block| !block.intersects(aabb))
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

fn collect_candidate_step_up_heights(
    grounded_aabb: Aabb,
    colliders: &[Aabb],
    max_step_height: f64,
    step_height_to_skip: f64,
) -> Vec<f64> {
    // Vanilla stores candidate heights as floats even though collision geometry
    // is double precision. Preserve that narrowing/dedup/sort behavior.
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

    candidates.sort_unstable_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    candidates.into_iter().map(f64::from).collect()
}

pub fn resolve_collision(
    chunk_store: &ChunkStore,
    player_aabb: Aabb,
    velocity: Velocity,
    max_step_height: f64,
    was_on_ground: bool,
) -> (DVec3, bool) {
    resolve_collision_inner(
        chunk_store,
        player_aabb,
        velocity,
        max_step_height,
        was_on_ground,
        None,
    )
}

pub fn resolve_player_collision(
    chunk_store: &ChunkStore,
    player_aabb: Aabb,
    velocity: Velocity,
    max_step_height: f64,
    was_on_ground: bool,
    context: &PlayerCollisionContext<'_>,
) -> (DVec3, bool) {
    resolve_collision_inner(
        chunk_store,
        player_aabb,
        velocity,
        max_step_height,
        was_on_ground,
        Some(context),
    )
}

fn resolve_collision_inner(
    chunk_store: &ChunkStore,
    player_aabb: Aabb,
    velocity: Velocity,
    max_step_height: f64,
    was_on_ground: bool,
    context: Option<&PlayerCollisionContext<'_>>,
) -> (DVec3, bool) {
    let expanded = player_aabb.expand(*velocity);
    let block_aabbs = match context {
        Some(context) => collect_player_block_aabbs(chunk_store, &expanded, context),
        None => collect_block_aabbs(chunk_store, &expanded),
    };

    let (movement_step, on_ground_after_collision) =
        collide_along_axes(&block_aabbs, player_aabb, velocity);

    let horizontal_blocked = movement_step.x != velocity.x || movement_step.z != velocity.z;
    let mut resolved = movement_step;

    // Vanilla `Entity.collide`: a step is allowed when this move landed on the
    // ground OR the entity was grounded before the move. Candidate heights are
    // actual Y faces from nearby collision shapes, tested lowest-first.
    if max_step_height > 0.0 && (on_ground_after_collision || was_on_ground) && horizontal_blocked {
        let grounded_aabb = if on_ground_after_collision {
            player_aabb.offset(dvec3(0.0, movement_step.y, 0.0))
        } else {
            player_aabb
        };
        let mut step_region = grounded_aabb.expand(dvec3(velocity.x, max_step_height, velocity.z));
        if !on_ground_after_collision {
            // Vanilla expands 1e-5 below a previously-grounded box so collision
            // shapes sharing the floor face participate in candidate discovery.
            step_region = step_region.expand(dvec3(0.0, -1.0e-5_f32 as f64, 0.0));
        }
        let step_aabbs = match context {
            Some(context) => collect_player_block_aabbs(chunk_store, &step_region, context),
            None => collect_block_aabbs(chunk_store, &step_region),
        };
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

    // Vanilla derives on-ground from the final movement result, not from the
    // pre-step collision result. A zero/upward step can therefore clear the
    // flag for this tick even when stepping began from the ground.
    let on_ground = velocity.y < 0.0 && resolved.y != velocity.y;
    (resolved, on_ground)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_collision_overrides_survive_no_collision_registration() {
        crate::world::block::init("26.2");

        let pitcher =
            crate::world::block::find_state("pitcher_crop", &[("age", "0"), ("half", "lower")]);
        assert!(!has_collision(pitcher));
        let pitcher_shape = collision_shape_for(pitcher, 64, None).unwrap();
        assert!(!pitcher_shape.is_empty());

        let wall_sign = crate::world::block::find_state(
            "oak_wall_hanging_sign",
            &[("facing", "north"), ("waterlogged", "false")],
        );
        assert!(!has_collision(wall_sign));
        let sign_shape = collision_shape_for(wall_sign, 64, None).unwrap();
        assert_eq!(sign_shape, &[[0.0, 0.875, 0.375, 1.0, 1.0, 0.625]]);
    }

    #[test]
    fn scaffolding_collision_matches_player_context() {
        crate::world::block::init("26.2");
        let stable = crate::world::block::find_state(
            "scaffolding",
            &[
                ("bottom", "false"),
                ("distance", "0"),
                ("waterlogged", "false"),
            ],
        );
        let above = PlayerCollisionContext {
            feet_y: 65.01,
            descending: false,
            fall_distance: 0.0,
            leather_boots: false,
            block_entity_anim: None,
        };
        assert_eq!(
            player_partial_shape(stable, 64, &above),
            Some(SCAFFOLDING_STABLE)
        );

        let descending = PlayerCollisionContext {
            descending: true,
            ..above
        };
        assert_eq!(
            player_partial_shape(stable, 64, &descending),
            Some(EMPTY_SHAPE)
        );

        let unstable_bottom = crate::world::block::find_state(
            "scaffolding",
            &[
                ("bottom", "true"),
                ("distance", "1"),
                ("waterlogged", "false"),
            ],
        );
        let inside = PlayerCollisionContext {
            feet_y: 64.01,
            descending: true,
            ..above
        };
        assert_eq!(
            player_partial_shape(unstable_bottom, 64, &inside),
            Some(SCAFFOLDING_UNSTABLE_BOTTOM)
        );
    }

    #[test]
    fn powder_snow_collision_matches_player_context() {
        crate::world::block::init("26.2");
        let powder = crate::world::block::find_state("powder_snow", &[]);
        let ordinary = PlayerCollisionContext {
            feet_y: 65.01,
            descending: false,
            fall_distance: 0.0,
            leather_boots: false,
            block_entity_anim: None,
        };
        assert_eq!(
            player_partial_shape(powder, 64, &ordinary),
            Some(EMPTY_SHAPE)
        );

        let boots = PlayerCollisionContext {
            leather_boots: true,
            ..ordinary
        };
        assert_eq!(
            player_partial_shape(powder, 64, &boots),
            Some(FULL_CUBE_SHAPE)
        );

        let descending = PlayerCollisionContext {
            descending: true,
            ..boots
        };
        assert_eq!(
            player_partial_shape(powder, 64, &descending),
            Some(EMPTY_SHAPE)
        );

        let falling = PlayerCollisionContext {
            fall_distance: 2.500_001,
            leather_boots: false,
            ..ordinary
        };
        assert_eq!(
            player_partial_shape(powder, 64, &falling),
            Some(POWDER_SNOW_FALLING)
        );
    }

    #[test]
    fn shulker_collision_uses_current_block_entity_open_progress() {
        crate::world::block::init("26.2");
        let state = crate::world::block::find_state("shulker_box", &[("facing", "east")]);
        let pos = BlockPos::new(10, 64, -3);
        let mut anim = BlockEntityAnimStore::default();
        anim.set_open_count(pos, 1);
        anim.tick();
        let context = PlayerCollisionContext {
            feet_y: 65.0,
            descending: false,
            fall_distance: 0.0,
            leather_boots: false,
            block_entity_anim: Some(&anim),
        };

        let aabb = shulker_collision_aabb(state, pos.x, pos.y, pos.z, &context).unwrap();
        assert_eq!(aabb.min, dvec3(10.0, 64.0, -3.0));
        let vanilla_extension = f64::from(0.5_f32 * 0.1_f32);
        assert_eq!(aabb.max.x.to_bits(), (11.0 + vanilla_extension).to_bits());
        assert_eq!(aabb.max.y, 65.0);
        assert_eq!(aabb.max.z, -2.0);
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
