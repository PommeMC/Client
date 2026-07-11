use glam::{DVec3, dvec3};

use super::aabb::Aabb;
use super::block_shape;
use crate::entity::components::Velocity;
use crate::world::block::has_collision;
use crate::world::chunk::ChunkStore;

pub fn collect_block_aabbs(chunk_store: &ChunkStore, region: &Aabb) -> Vec<Aabb> {
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
                if !has_collision(state) {
                    continue;
                }
                match block_shape::partial_shape(state) {
                    Some(boxes) => {
                        for &[lx0, ly0, lz0, lx1, ly1, lz1] in boxes {
                            aabbs.push(Aabb::new(
                                dvec3(bx as f64 + lx0, by as f64 + ly0, bz as f64 + lz0),
                                dvec3(bx as f64 + lx1, by as f64 + ly1, bz as f64 + lz1),
                            ));
                        }
                    }
                    None => aabbs.push(Aabb::block(bx, by, bz)),
                }
            }
        }
    }

    aabbs
}

pub fn no_collision(chunk_store: &ChunkStore, aabb: &Aabb) -> bool {
    collect_block_aabbs(chunk_store, aabb)
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

pub fn resolve_collision(
    chunk_store: &ChunkStore,
    player_aabb: Aabb,
    velocity: Velocity,
    step_height: f64,
) -> (DVec3, bool) {
    let expanded = player_aabb.expand(*velocity);
    let block_aabbs = collect_block_aabbs(chunk_store, &expanded);

    let (resolved, on_ground) = collide_along_axes(&block_aabbs, player_aabb, velocity);

    let horizontal_blocked = resolved.x != velocity.x || resolved.z != velocity.z;
    if step_height > 0.0 && on_ground && horizontal_blocked {
        let step_up = dvec3(velocity.x, step_height, velocity.z);
        let step_expanded = player_aabb
            .expand(step_up)
            .expand(dvec3(0.0, -step_height, 0.0));
        let step_aabbs = collect_block_aabbs(chunk_store, &step_expanded);

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
