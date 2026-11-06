//! Block collision: the scan for a player-sized box's block boxes, and the
//! whole move resolve built on it, along a line crossing a column border.

use std::hint::black_box;

use criterion::Criterion;
use glam::dvec3;

use super::fixtures::{SURFACE_Y, Scene, build_world};
use crate::entity::components::Velocity;
use crate::physics::aabb::Aabb;
use crate::physics::block_shape::CollisionContext;
use crate::physics::collision::{CollisionWorld, collect_block_aabbs, resolve_collision};
use crate::player::{PLAYER_HALF_WIDTH, STANDING_HEIGHT};
use crate::world::block::BlockStateTable;

/// Player boxes standing on the surface, stepping across the x = 0 border.
fn boxes() -> Vec<Aabb> {
    (0..64)
        .map(|i| {
            let x = -1.0 + f64::from(i) / 32.0;
            Aabb::from_center(
                dvec3(x, f64::from(SURFACE_Y), 8.5),
                PLAYER_HALF_WIDTH,
                STANDING_HEIGHT / 2.0,
            )
        })
        .collect()
}

pub fn bench(c: &mut Criterion, block_state_table: BlockStateTable) {
    let walk = Velocity::new(0.2, -0.0784, 0.1);
    let ctx = CollisionContext::EMPTY;
    let mut group = c.benchmark_group("collision");
    for scene in Scene::ALL {
        let store = build_world(scene, block_state_table);
        let world = CollisionWorld::new(&store, block_state_table);
        let boxes = boxes();
        group.bench_function(format!("scan/{}", scene.name()), |b| {
            b.iter(|| {
                for aabb in &boxes {
                    black_box(collect_block_aabbs(world, &aabb.expand(*walk), &ctx));
                }
            })
        });
        group.bench_function(format!("resolve/{}", scene.name()), |b| {
            b.iter(|| {
                for aabb in &boxes {
                    black_box(resolve_collision(world, *aabb, walk, 0.6, true, &ctx));
                }
            })
        });
    }
    group.finish();
}
