//! Distant terrain's worker: a forgotten chunk turned into columns and
//! merged down every level, and a section meshed, per scene.

use std::hint::black_box;

use criterion::Criterion;

use super::fixtures::{Assets, Scene, build_world, columns_within};
use crate::position::ChunkPos;
use crate::renderer::distant::column::build_chunk;
use crate::renderer::distant::mesh::mesh_section;
use crate::renderer::distant::store::{LEVELS, SectionKey, Store};

pub fn bench(c: &mut Criterion, assets: &Assets) {
    let colors = assets.mesh_dispatcher().distant_colors();

    let mut group = c.benchmark_group("distant");
    for scene in Scene::ALL {
        let world = build_world(scene, assets.block_state_table);
        let capture = |store: &mut Store, pos: ChunkPos| {
            let chunk = world
                .get_chunk(&pos)
                .expect("the scene's columns are loaded");
            let light = world.light_data.get(&(pos.x, pos.z)).map(|light| &**light);
            let columns = build_chunk(&chunk.read(), light, world.min_y(), &colors);
            store.capture(pos, columns)
        };
        group.bench_function(format!("capture/{}", scene.name()), |b| {
            b.iter(|| black_box(capture(&mut Store::default(), ChunkPos::new(0, 0))))
        });

        let mut store = Store::default();
        for pos in columns_within(1) {
            capture(&mut store, pos);
        }
        for level in [0, LEVELS - 1] {
            let key = SectionKey { level, x: 0, z: 0 };
            let section = store.get(key).expect("chunk (0, 0)'s sections");
            group.bench_function(format!("mesh/{}/level{level}", scene.name()), |b| {
                b.iter(|| black_box(mesh_section(key, section, |run| colors.colors(run))))
            });
        }
    }
    group.finish();
}
