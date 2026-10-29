//! The draw-list rebuild `dispatch_cull` runs every frame while any section
//! fades in, over a full square of columns at each render distance.

use std::collections::HashMap;
use std::hint::black_box;
use std::time::{Duration, Instant};

use criterion::Criterion;

use super::fixtures::{EYE, columns_within};
use crate::renderer::chunk::buffer::{ChunkAlloc, build_meta, sort_meta};
use crate::world::chunk::OVERWORLD_MIN_Y;

/// The meshed (non-empty) sections of a typical overworld column.
const SECTIONS: std::ops::Range<i32> = 2..14;

pub fn bench(c: &mut Criterion) {
    let mut group = c.benchmark_group("fade_rebuild");
    for rd in [8, 16, 32] {
        let uploaded_at = Instant::now();
        let mut chunks = HashMap::new();
        let mut vis = HashMap::new();
        for pos in columns_within(rd) {
            let alloc = ChunkAlloc::synthetic(pos, OVERWORLD_MIN_Y, SECTIONS, uploaded_at);
            chunks.insert(pos, alloc);
            vis.insert(pos, u32::MAX);
        }
        let now = uploaded_at + Duration::from_millis(300);
        let mut meta = Vec::new();

        // `fading`: every section mid-fade, as while streaming. `static`: the
        // same rebuild with fading off, as after an edit or visibility change.
        for (case, fade_enabled) in [("fading", true), ("static", false)] {
            group.bench_function(format!("{case}/rd{rd}"), |b| {
                b.iter(|| {
                    build_meta(&chunks, Some(&vis), fade_enabled, EYE, now, &mut meta);
                    sort_meta(&mut meta, EYE);
                    black_box(meta.len())
                })
            });
        }
    }
    group.finish();
}
