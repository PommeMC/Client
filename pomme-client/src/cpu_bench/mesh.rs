//! Meshing one full column (snapshot build included), per scene.

use std::hint::black_box;

use criterion::Criterion;

use super::fixtures::{Assets, Scene, build_world};
use crate::position::ChunkPos;

pub fn bench(c: &mut Criterion, assets: &Assets) {
    // `mesh_sections_now` meshes on the calling thread; the dispatcher's
    // workers stay idle.
    let dispatcher = assets.mesh_dispatcher();

    let mut group = c.benchmark_group("mesh");
    group.sample_size(30);
    for scene in Scene::ALL {
        let store = build_world(scene, assets.block_state_table);
        let sections = 0..store.section_count();
        group.bench_function(scene.name(), |b| {
            b.iter(|| {
                black_box(dispatcher.mesh_sections_now(
                    &store,
                    ChunkPos::new(0, 0),
                    sections.clone(),
                    0,
                ))
            })
        });
    }
    group.finish();
}
