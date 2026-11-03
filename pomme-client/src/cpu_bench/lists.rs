//! The chunk draw lists' per-frame work: one list's front-to-back sort step,
//! and a column arriving and leaving, over a full square of columns at each
//! render distance.

use std::hint::black_box;

use criterion::Criterion;

use super::fixtures::{EYE, columns_within};
use crate::position::ChunkPos;
use crate::renderer::chunk::buffer::Lists;
use crate::world::chunk::OVERWORLD_MIN_Y;

/// The meshed (non-empty) sections of a typical overworld column.
const SECTIONS: std::ops::Range<i32> = 2..14;
/// Arenas the columns spread over, as a full render distance fills.
const ARENAS: u16 = 48;

pub fn bench(c: &mut Criterion) {
    let mut group = c.benchmark_group("draw_lists");
    for rd in [8, 16, 32] {
        let mut lists = Lists::default();
        for (i, pos) in columns_within(rd).enumerate() {
            lists.list(pos, OVERWORLD_MIN_Y, SECTIONS, (i % ARENAS as usize) as u16);
        }

        group.bench_function(format!("sort_step/rd{rd}"), |b| {
            b.iter(|| {
                lists.sort_from(0, EYE);
                lists.sort_step(EYE);
                black_box(&lists);
            })
        });
        let churn = ChunkPos::new(rd + 1, 0);
        group.bench_function(format!("churn/rd{rd}"), |b| {
            b.iter(|| {
                lists.list(churn, OVERWORLD_MIN_Y, SECTIONS, 0);
                lists.drop_column(churn);
            })
        });
    }
    group.finish();
}
