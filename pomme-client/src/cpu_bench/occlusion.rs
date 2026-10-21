//! Cave culling: the per-section VisGraph flood fill, and the section walk
//! over a whole render distance.

use std::collections::HashMap;
use std::hint::black_box;

use criterion::Criterion;

use super::fixtures::{Assets, EYE, Scene, build_world, columns_within};
use crate::position::ChunkPos;
use crate::renderer::chunk::occlusion_graph::{
    VisibilitySet, compute_visibility, compute_visible_mask,
};
use crate::world::chunk::{OVERWORLD_HEIGHT, OVERWORLD_MIN_Y};

fn cell(x: usize, y: usize, z: usize) -> usize {
    (y * 16 + z) * 16 + x
}

/// The flood fill over every section of the centre column, per scene, on
/// opacity precomputed the way the mesher does (opaque full cubes).
pub fn bench_sections(c: &mut Criterion, assets: &Assets) {
    let mut group = c.benchmark_group("visgraph");
    for scene in Scene::ALL {
        let store = build_world(scene);
        let min_y = store.min_y();
        let sections: Vec<Vec<bool>> = (0..store.section_count())
            .map(|si| {
                let mut opaque = vec![false; 4096];
                for y in 0..16 {
                    for z in 0..16 {
                        for x in 0..16 {
                            let state = store.get_block_state(x, min_y + si * 16 + y, z);
                            opaque[cell(x as usize, y as usize, z as usize)] =
                                assets.registry.is_opaque_full_cube(state);
                        }
                    }
                }
                opaque
            })
            .collect();
        group.bench_function(scene.name(), |b| {
            b.iter(|| {
                for opaque in &sections {
                    black_box(compute_visibility(|x, y, z| opaque[cell(x, y, z)]));
                }
            })
        });
    }
    group.finish();
}

/// The section walk from a surface camera, over terrain that is open above
/// ground and mostly sealed below it.
pub fn bench_walk(c: &mut Criterion) {
    const SECTION_COUNT: i32 = OVERWORLD_HEIGHT as i32 / 16;
    let cam_si = (EYE.y as i32 - OVERWORLD_MIN_Y) / 16;

    let mut group = c.benchmark_group("section_walk");
    for rd in [8, 16, 32] {
        let mut rng = fastrand::Rng::with_seed(0x5345_4354);
        let mut section_vis = HashMap::new();
        for pos in columns_within(rd) {
            for si in 0..SECTION_COUNT {
                let vis = if si >= cam_si - 1 || rng.u32(0..10) == 0 {
                    VisibilitySet::all()
                } else {
                    VisibilitySet::none()
                };
                section_vis.insert((pos, si), vis);
            }
        }
        group.bench_function(format!("rd{rd}"), |b| {
            b.iter(|| {
                black_box(compute_visible_mask(
                    &section_vis,
                    ChunkPos::new(0, 0),
                    cam_si,
                    EYE,
                    OVERWORLD_MIN_Y,
                    SECTION_COUNT,
                    rd,
                ))
            })
        });
    }
    group.finish();
}
