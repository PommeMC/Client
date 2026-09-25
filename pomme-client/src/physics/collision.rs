use azalea_block::BlockState;
use glam::{DVec3, dvec3};

use super::aabb::Aabb;
use super::block_shape::{self, CollisionContext};
use crate::entity::components::Velocity;
use crate::world::block::{block_id, collision_shape_position, has_large_collision_shape};
use crate::world::chunk::ChunkStore;

/// Vanilla `BlockCollisions`: scans one cell past the region so shapes that
/// leave their cell (fences, walls) are found, keeping only those from the
/// padding shell, and returns the boxes that intersect the region.
pub fn collect_block_aabbs(
    chunk_store: &ChunkStore,
    region: &Aabb,
    ctx: &CollisionContext,
) -> Vec<Aabb> {
    block_aabbs(region, ctx, |x, y, z| chunk_store.get_block_state(x, y, z))
}

fn block_aabbs(
    region: &Aabb,
    ctx: &CollisionContext,
    state_at: impl Fn(i32, i32, i32) -> BlockState,
) -> Vec<Aabb> {
    let mut aabbs = Vec::new();

    let lo = |v: f64| (v - 1.0e-7).floor() as i32 - 1;
    let hi = |v: f64| (v + 1.0e-7).floor() as i32 + 1;
    let (min_x, min_y, min_z) = (lo(region.min.x), lo(region.min.y), lo(region.min.z));
    let (max_x, max_y, max_z) = (hi(region.max.x), hi(region.max.y), hi(region.max.z));

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
                    || shell_axes == 2 && block_id(state) != "moving_piston"
                {
                    continue;
                }
                match block_shape::collision_shape(state, by, ctx) {
                    Some(boxes) => {
                        let offset = collision_shape_position(state, bx, by, bz);
                        aabbs.extend(
                            boxes
                                .iter()
                                .map(|&b| Aabb::from_local(b, offset))
                                .filter(|b| b.intersects(region)),
                        );
                    }
                    None => {
                        let cell = Aabb::block(bx, by, bz);
                        if cell.intersects(region) {
                            aabbs.push(cell);
                        }
                    }
                }
            }
        }
    }

    aabbs
}

pub fn no_collision(chunk_store: &ChunkStore, aabb: &Aabb, ctx: &CollisionContext) -> bool {
    collect_block_aabbs(chunk_store, aabb, ctx).is_empty()
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

pub fn resolve_collision(
    chunk_store: &ChunkStore,
    player_aabb: Aabb,
    velocity: Velocity,
    step_height: f64,
    ctx: &CollisionContext,
) -> (DVec3, bool) {
    let expanded = player_aabb.expand(*velocity);
    let block_aabbs = collect_block_aabbs(chunk_store, &expanded, ctx);

    let (resolved, on_ground) = collide_along_axes(&block_aabbs, player_aabb, velocity);

    let horizontal_blocked = resolved.x != velocity.x || resolved.z != velocity.z;
    if step_height > 0.0 && on_ground && horizontal_blocked {
        let step_up = dvec3(velocity.x, step_height, velocity.z);
        let step_expanded = player_aabb
            .expand(step_up)
            .expand(dvec3(0.0, -step_height, 0.0));
        let step_aabbs = collect_block_aabbs(chunk_store, &step_expanded, ctx);

        let mut up_vel = step_height;
        for block in &step_aabbs {
            up_vel = block.clip_y_collide(&player_aabb, up_vel);
        }
        let raised = player_aabb.offset(dvec3(0.0, up_vel, 0.0));

        let (step_resolved, _) = collide_along_axes(
            &step_aabbs,
            raised,
            Velocity::new(velocity.x, 0.0, velocity.z),
        );

        let after_move = raised.offset(dvec3(step_resolved.x, 0.0, step_resolved.z));
        let mut down_vel = -(up_vel - velocity.y);
        for block in &step_aabbs {
            down_vel = block.clip_y_collide(&after_move, down_vel);
        }

        let step_total = dvec3(step_resolved.x, up_vel + down_vel, step_resolved.z);

        let step_h_dist = step_total.x * step_total.x + step_total.z * step_total.z;
        let orig_h_dist = resolved.x * resolved.x + resolved.z * resolved.z;

        if step_h_dist > orig_h_dist {
            let step_on_ground = down_vel != -(up_vel - velocity.y);
            return (step_total, step_on_ground || on_ground);
        }
    }

    (resolved, on_ground)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::player::PLAYER_HALF_WIDTH;
    use crate::world::block::find_state;

    /// Boxes near a player moving down from `feet`, with `state` at the
    /// origin and air elsewhere.
    fn boxes_near(state: BlockState, feet: DVec3, ctx: &CollisionContext) -> Vec<Aabb> {
        let region = Aabb::new(
            feet - dvec3(PLAYER_HALF_WIDTH, 0.1, PLAYER_HALF_WIDTH),
            feet + dvec3(PLAYER_HALF_WIDTH, 1.8, PLAYER_HALF_WIDTH),
        );
        block_aabbs(&region, ctx, |x, y, z| {
            if (x, y, z) == (0, 0, 0) {
                state
            } else {
                BlockState::AIR
            }
        })
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
}
