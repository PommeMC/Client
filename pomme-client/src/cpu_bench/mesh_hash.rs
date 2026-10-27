//! An order-independent hash of the mesher's output, to prove a refactor
//! leaves it unchanged (`just mesh-hash`). Vertices are decoded rather than
//! hashed as bytes, and sprites are named rather than numbered, since atlas
//! sprite ids and vertex and triangle order vary between runs.

use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};

use super::fixtures::{Assets, Scene, build_world};
use crate::position::ChunkPos;
use crate::renderer::chunk::mesher::PackedVertex;

/// Hashes the centre column of `scene`, meshed like `mesh::bench` does.
pub fn scene_hash(assets: &Assets, scene: Scene) -> u64 {
    let dispatcher = assets.mesh_dispatcher();
    let store = build_world(scene, assets.block_state_table);
    let data =
        dispatcher.mesh_sections_now(&store, ChunkPos::new(0, 0), 0..store.section_count(), 0);
    let names = assets.mesh.uv_map.sprite_names();

    let mut hasher = DefaultHasher::new();
    for section in &data.sections {
        let (solid, cutout) = section.indices.split_at(section.solid_index_count as usize);
        section.section_index.hash(&mut hasher);
        for indices in [solid, cutout, &section.water_indices[..]] {
            triangles_hash(&section.vertices, indices, &names).hash(&mut hasher);
        }
    }
    hasher.finish()
}

/// The pass's triangles as a sorted set, each starting from its smallest
/// vertex so winding is kept but the starting corner isn't.
fn triangles_hash(vertices: &[PackedVertex], indices: &[u32], names: &HashMap<u16, &str>) -> u64 {
    let decode = |i: u32| {
        let v = &vertices[i as usize];
        (
            v.pos,
            v.sprite_uv_fixed(),
            names.get(&v.sprite).copied(),
            v.light_tint,
        )
    };
    let mut triangles: Vec<_> = indices
        .as_chunks::<3>()
        .0
        .iter()
        .map(|tri| {
            let mut corners = tri.map(decode);
            let first = (0..3).min_by_key(|&i| corners[i]).unwrap_or(0);
            corners.rotate_left(first);
            corners
        })
        .collect();
    triangles.sort_unstable();
    let mut hasher = DefaultHasher::new();
    triangles.hash(&mut hasher);
    hasher.finish()
}
