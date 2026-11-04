//! Distant terrain's worker: a forgotten chunk voxelized and mipped into
//! every level, and a section meshed, per scene; and a backlog of columns
//! larger than the section cache taken in again from disk, in queue order
//! and in the order the thread takes its last backlog in.

use std::hint::black_box;

use criterion::Criterion;

use super::fixtures::{Assets, Scene, build_world, columns_within};
use crate::position::ChunkPos;
use crate::renderer::distant::build::Factory;
use crate::renderer::distant::models::ModelFactory;
use crate::renderer::distant::section::{LEVELS, SectionKey};
use crate::renderer::distant::voxel::States;
use crate::renderer::distant::world::{World, ingest_order};

pub fn bench(c: &mut Criterion, assets: &Assets) {
    let colors = assets.mesh_dispatcher().distant_colors();
    let states = States::new(assets.block_state_table);

    let mut group = c.benchmark_group("distant");
    for scene in Scene::ALL {
        let world = build_world(scene, assets.block_state_table);
        let ingest = |into: &mut World, pos: ChunkPos| {
            let chunk = world
                .get_chunk(&pos)
                .expect("the scene's columns are loaded");
            let light = world.light_data.get(&(pos.x, pos.z)).cloned();
            into.ingest_chunk(
                &states,
                pos,
                &chunk.read().sections,
                light.as_deref(),
                world.min_y(),
            )
        };
        group.bench_function(format!("ingest/{}", scene.name()), |b| {
            b.iter(|| black_box(ingest(&mut World::default(), ChunkPos::new(0, 0))))
        });

        let mut distant = World::default();
        for pos in columns_within(1) {
            ingest(&mut distant, pos);
        }
        // The section holding the surface at full detail, and the one
        // holding everything at the coarsest.
        for key in [
            SectionKey::of_vanilla(0, 0, 4, 0),
            SectionKey::of_vanilla(LEVELS - 1, 0, 0, 0),
        ] {
            let section = distant.get(key).expect("chunk (0, 0)'s sections");
            let mut models = ModelFactory::new();
            let mut factory = Factory::new();
            let build = |factory: &mut Factory, models: &mut ModelFactory| {
                factory
                    .build(key, section, |k| distant.get(k), &mut colors.baking(models))
                    .quads
                    .len()
            };
            // Models bake once, before the meshing measured.
            build(&mut factory, &mut models);
            group.bench_function(format!("build/{}/level{}", scene.name(), key.level), |b| {
                b.iter(|| black_box(build(&mut factory, &mut models)))
            });
        }
    }
    group.finish();
    backlog(c, assets, &states);
}

/// Columns per side of the backlog: 48² columns hold about 3600 level-0
/// sections, past the cache.
const BACKLOG_SIDE: i32 = 48;

fn backlog(c: &mut Criterion, assets: &Assets, states: &States) {
    let world = build_world(Scene::Plains, assets.block_state_table);
    let chunk = world
        .get_chunk(&ChunkPos::new(0, 0))
        .expect("the scene's columns are loaded");
    let sections = chunk.read().sections.clone();
    let light = world.light_data.get(&(0, 0)).cloned();
    let dir = std::env::temp_dir().join(format!("pomme-bench-distant-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let ingest = |columns: &[ChunkPos]| {
        let mut into = World::new(Some(dir.clone()));
        for &pos in columns {
            into.ingest_chunk(states, pos, &sections, light.as_deref(), world.min_y());
        }
        into.flush();
    };
    let mut columns: Vec<ChunkPos> = (0..BACKLOG_SIDE)
        .flat_map(|x| (0..BACKLOG_SIDE).map(move |z| ChunkPos::new(x, z)))
        .collect();
    fastrand::Rng::with_seed(7).shuffle(&mut columns);
    // Everything on disk already, as on the way out.
    ingest(&columns);

    let mut group = c.benchmark_group("distant/backlog");
    group.sample_size(10);
    group.bench_function("queued", |b| b.iter(|| ingest(black_box(&columns))));
    let mut ordered = columns.clone();
    ordered.sort_by_key(|&pos| ingest_order(pos));
    group.bench_function("z-order", |b| b.iter(|| ingest(black_box(&ordered))));
    group.finish();
    let _ = std::fs::remove_dir_all(dir);
}
