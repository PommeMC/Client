//! Distant terrain's worker: a forgotten chunk voxelized and mipped into
//! every level, and a section meshed, per scene.

use std::hint::black_box;

use criterion::Criterion;

use super::fixtures::{Assets, Scene, build_world, columns_within};
use crate::position::ChunkPos;
use crate::renderer::distant::build::Factory;
use crate::renderer::distant::models::ModelFactory;
use crate::renderer::distant::section::{LEVELS, SectionKey};
use crate::renderer::distant::voxel::States;
use crate::renderer::distant::world::World;

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
            into.ingest_chunk(&states, pos, &chunk.read(), light.as_deref(), world.min_y())
        };
        group.bench_function(format!("ingest/{}", scene.name()), |b| {
            b.iter(|| black_box(ingest(&mut World::default(), ChunkPos::new(0, 0))))
        });

        let mut voxy = World::default();
        for pos in columns_within(1) {
            ingest(&mut voxy, pos);
        }
        // The section holding the surface at full detail, and the one
        // holding everything at the coarsest.
        for key in [
            SectionKey::of_vanilla(0, 0, 4, 0),
            SectionKey::of_vanilla(LEVELS - 1, 0, 0, 0),
        ] {
            let section = voxy.get(key).expect("chunk (0, 0)'s sections");
            let mut models = ModelFactory::new();
            let mut factory = Factory::new();
            let build = |factory: &mut Factory, models: &mut ModelFactory| {
                factory
                    .build(key, section, |k| voxy.get(k), &mut colors.baking(models))
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
}
