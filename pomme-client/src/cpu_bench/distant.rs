//! Distant terrain's worker: a forgotten chunk turned into columns and
//! merged down every level, and a section meshed, per scene.

use std::hint::black_box;

use criterion::Criterion;

use super::fixtures::{Assets, Scene, build_world, columns_within};
use crate::position::ChunkPos;
use crate::renderer::distant::column::build_chunk;
use crate::renderer::distant::mesh::mesh_section;
use crate::renderer::distant::models::ModelFactory;
use crate::renderer::distant::store::{LEVELS, SectionKey, Store};
use crate::renderer::distant::voxel::States;
use crate::renderer::distant::world::World;

pub fn bench(c: &mut Criterion, assets: &Assets) {
    let colors = assets.mesh_dispatcher().distant_colors();

    let mut group = c.benchmark_group("distant");
    for scene in Scene::ALL {
        let world = build_world(scene, assets.block_state_table);
        let column = |pos: ChunkPos| {
            let chunk = world
                .get_chunk(&pos)
                .expect("the scene's columns are loaded");
            let light = world.light_data.get(&(pos.x, pos.z)).cloned();
            (chunk, light)
        };
        let capture = |store: &mut Store, pos: ChunkPos| {
            let (chunk, light) = column(pos);
            let columns = build_chunk(&chunk.read(), light.as_deref(), world.min_y(), &colors);
            store.capture(pos, columns)
        };
        group.bench_function(format!("capture/{}", scene.name()), |b| {
            b.iter(|| black_box(capture(&mut Store::default(), ChunkPos::new(0, 0))))
        });
        let states = States::new(assets.block_state_table);
        group.bench_function(format!("ingest/{}", scene.name()), |b| {
            b.iter(|| {
                let pos = ChunkPos::new(0, 0);
                let (chunk, light) = column(pos);
                black_box(World::default().ingest_chunk(
                    &states,
                    pos,
                    &chunk.read(),
                    light.as_deref(),
                    world.min_y(),
                ))
            })
        });

        let mut store = Store::default();
        for pos in columns_within(1) {
            capture(&mut store, pos);
        }
        for level in [0, LEVELS - 1] {
            let key = SectionKey { level, x: 0, z: 0 };
            let section = store.get(key).expect("chunk (0, 0)'s sections");
            // Models bake once, before the meshing measured.
            let mut models = ModelFactory::new();
            mesh_section(key, section, |run| colors.run_look(&mut models, run));
            group.bench_function(format!("mesh/{}/level{level}", scene.name()), |b| {
                b.iter(|| {
                    black_box(mesh_section(key, section, |run| {
                        colors.run_look(&mut models, run)
                    }))
                })
            });
        }
    }
    group.finish();
}
