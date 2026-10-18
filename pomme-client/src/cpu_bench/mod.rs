//! Headless CPU benchmarks (`just bench [filter]`): the chunk mesher, the
//! per-frame fade rebuild, cave culling and chunk packet decoding, with no
//! window or Vulkan device.
//! Criterion reports each run's change against the last.

mod chunk_decode;
mod fade;
mod fixtures;
mod mesh;
mod occlusion;

use criterion::Criterion;

use crate::dirs::DataDirs;

/// Runs every bench whose id contains `filter` (all when `None`); returns the
/// process exit code.
pub fn run(version: &str, dirs: &DataDirs, filter: Option<&str>) -> i32 {
    crate::world::block::init(version);

    let mut c = Criterion::default();
    if let Some(filter) = filter {
        c = c.with_filter(filter);
    }

    // The mesh and visgraph fixtures load the block registry and atlas from
    // disk; skip that when the filter leaves neither group to run.
    let needs_assets = filter.is_none_or(|f| {
        ["mesh", "visgraph"]
            .iter()
            .any(|group| group.contains(f) || f.contains(group))
    });
    if needs_assets {
        let assets = match fixtures::Assets::load(version, dirs) {
            Ok(assets) => assets,
            Err(e) => {
                eprintln!("cpu-bench: {e}");
                return 1;
            }
        };
        mesh::bench(&mut c, &assets);
        occlusion::bench_sections(&mut c, &assets);
    }
    chunk_decode::bench(&mut c);
    fade::bench(&mut c);
    occlusion::bench_walk(&mut c);

    c.final_summary();
    0
}
