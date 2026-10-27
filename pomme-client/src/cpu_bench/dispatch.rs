//! Meshing a load's worth of columns through the worker queue, from publish
//! to the last result, per scene.

use criterion::Criterion;

use super::fixtures::{Assets, EYE, Scene, build_world_within, columns_within};
use crate::position::ChunkPos;
use crate::renderer::chunk::mesher::MeshRequest;

/// Columns meshed per iteration: every one within this radius, each with its
/// full neighbourhood loaded.
const RADIUS: i32 = 8;

pub fn bench(c: &mut Criterion, assets: &Assets) {
    let mut group = c.benchmark_group("dispatch");
    group.sample_size(10);
    for scene in Scene::ALL {
        let store = build_world_within(scene, assets.block_state_table, RADIUS + 1);
        let mut dispatcher = assets.mesh_dispatcher();
        let columns: Vec<ChunkPos> = columns_within(RADIUS).collect();
        let sections = 0..store.section_count();
        // Generations only rise, so every iteration's jobs are fresh.
        let mut generation = 0;
        group.bench_function(format!("{}/rd{RADIUS}", scene.name()), |b| {
            b.iter(|| {
                let requests = columns
                    .iter()
                    .map(|&pos| {
                        generation += 1;
                        MeshRequest {
                            pos,
                            lod: 0,
                            incarnation: 1,
                            content_gen: generation,
                            sections: sections.clone(),
                            recompile: false,
                        }
                    })
                    .collect();
                dispatcher.publish(&store, EYE, requests);
                let mut received = 0;
                while received < columns.len() {
                    let before = received;
                    received += dispatcher.drain_results().count();
                    if received == before {
                        std::thread::yield_now();
                    }
                }
            })
        });
    }
    group.finish();
}
