//! Headless CPU benchmarks (`just bench [filter]`): the chunk mesher and its
//! worker queue, the per-frame fade rebuild, cave culling and chunk packet
//! decoding, with no window or Vulkan device.
//! Criterion reports each run's change against the last.

mod chunk_decode;
mod dispatch;
mod fade;
mod fixtures;
mod mesh;
mod mesh_hash;
mod occlusion;

use criterion::Criterion;

use crate::dirs::DataDirs;
use crate::resources::ResourceManager;
use crate::resources::packs::ResourceConfiguration;

/// Runs every bench whose id contains `filter` (all when `None`); returns the
/// process exit code.
pub fn run(version: &str, dirs: &DataDirs, filter: Option<&str>) -> i32 {
    let block_state_table = block_state_table(version);

    let mut c = Criterion::default();
    if let Some(filter) = filter {
        c = c.with_filter(filter);
    }

    // The mesh and visgraph fixtures load block render assets and the atlas from
    // disk; skip that when the filter leaves neither group to run.
    let needs_assets = filter.is_none_or(|f| {
        ["mesh", "dispatch", "visgraph"]
            .iter()
            .any(|group| group.contains(f) || f.contains(group))
    });
    if needs_assets {
        let Some(assets) = load_assets(version, dirs, block_state_table) else {
            return 1;
        };
        mesh::bench(&mut c, &assets);
        dispatch::bench(&mut c, &assets);
        occlusion::bench_sections(&mut c, &assets);
    }
    chunk_decode::bench(&mut c, block_state_table);
    fade::bench(&mut c);
    occlusion::bench_walk(&mut c);

    c.final_summary();
    0
}

/// Prints [`mesh_hash::scene_hashes`] for every bench scene; returns the
/// process exit code.
pub fn mesh_hash(version: &str, dirs: &DataDirs) -> i32 {
    let Some(assets) = load_assets(version, dirs, block_state_table(version)) else {
        return 1;
    };
    println!("scene    geometry         all bytes");
    for scene in fixtures::Scene::ALL {
        let [geometry, all] = mesh_hash::scene_hashes(&assets, scene);
        println!("{:<8} {geometry:016x} {all:016x}", scene.name());
    }
    0
}

fn block_state_table(version: &str) -> crate::world::block::BlockStateTable {
    let protocol = pomme_protocol::ProtocolVersion::from_name(version)
        .map(|v| v.protocol)
        .unwrap_or_else(|| {
            tracing::warn!(
                "no block-state data for version {version}, using {} — expect state drift",
                pomme_protocol::version::NATIVE.name
            );
            pomme_protocol::version::NATIVE.protocol
        });
    crate::world::block::BlockStateTable::for_protocol(protocol)
}

fn load_assets(
    version: &str,
    dirs: &DataDirs,
    block_state_table: crate::world::block::BlockStateTable,
) -> Option<fixtures::Assets> {
    let mut configuration =
        ResourceConfiguration::new(dirs.game_dir.join("resourcepacks/.server_cache"));
    configuration.set_installation(Some(dirs.installation_location(version)));
    // Resource ownership stays on the calling thread; workers receive only
    // owned CPU mesh assets. Packet-only benchmarks never open these sources.
    let resources = match ResourceManager::from_configuration(&configuration) {
        Ok(resources) => resources,
        Err(error) => {
            eprintln!(
                "cpu-bench: failed to open {}: {error}",
                dirs.client_jar.display()
            );
            return None;
        }
    };
    match fixtures::Assets::load(version, dirs, block_state_table, &resources) {
        Ok(assets) => Some(assets),
        Err(error) => {
            eprintln!("cpu-bench: {error}");
            None
        }
    }
}
