//! An order-independent hash of the mesher's output, to prove a refactor
//! leaves it unchanged (`just mesh-hash`). Vertices are decoded rather than
//! hashed as bytes, and sprites are named rather than numbered, since atlas
//! sprite ids and vertex and triangle order vary between runs.

use std::hash::{DefaultHasher, Hash, Hasher};

use super::fixtures::{Assets, Scene, build_world};
use crate::position::ChunkPos;
use crate::renderer::chunk::mesher::PackedVertex;

/// Hashes the centre column of `scene`, meshed like `mesh::bench` does: its
/// geometry alone (each quad's positions, UVs and sprites, however it splits
/// into triangles), then every triangle with every vertex byte.
pub fn scene_hashes(assets: &Assets, scene: Scene) -> [u64; 2] {
    let dispatcher = assets.mesh_dispatcher();
    let store = build_world(scene, assets.block_state_table);
    let data =
        dispatcher.mesh_sections_now(&store, ChunkPos::new(0, 0), 0..store.section_count(), 0);
    let names = assets.mesh.uv_map.sprite_names();

    let geometry = |v: &PackedVertex| (v.pos, v.sprite_uv_fixed(), names.get(&v.sprite).copied());
    let mut hashers = [DefaultHasher::new(), DefaultHasher::new()];
    for section in &data.sections {
        let (solid, cutout) = section.indices.split_at(section.solid_index_count as usize);
        for hasher in &mut hashers {
            section.section_index.hash(hasher);
        }
        for indices in [solid, cutout, &section.water_indices[..]] {
            let vertex = |i: u32| &section.vertices[i as usize];
            quads_hash(indices, |i| geometry(vertex(i))).hash(&mut hashers[0]);
            triangles_hash(indices, |i| {
                let v = vertex(i);
                (geometry(v), v.uv >> 24, v.light_tint)
            })
            .hash(&mut hashers[1]);
        }
    }
    hashers.map(|hasher| hasher.finish())
}

/// The pass's quads (every six indices) as a sorted set of their corners.
fn quads_hash<K: Ord + Hash>(indices: &[u32], decode: impl Fn(u32) -> K) -> u64 {
    sorted_hash(
        indices
            .as_chunks::<6>()
            .0
            .iter()
            .map(|quad| {
                let mut corners: Vec<_> = quad.iter().map(|&i| decode(i)).collect();
                corners.sort_unstable();
                corners.dedup();
                corners
            })
            .collect(),
    )
}

/// The pass's triangles as a sorted set, each starting from its smallest
/// vertex so winding is kept but the starting corner isn't.
fn triangles_hash<K: Ord + Hash>(indices: &[u32], decode: impl Fn(u32) -> K) -> u64 {
    let triangles = indices
        .as_chunks::<3>()
        .0
        .iter()
        .map(|tri| {
            let mut corners = tri.map(&decode);
            let first = (0..3)
                .min_by(|&a, &b| corners[a].cmp(&corners[b]))
                .unwrap_or(0);
            corners.rotate_left(first);
            corners
        })
        .collect();
    sorted_hash(triangles)
}

fn sorted_hash<T: Ord + Hash>(mut items: Vec<T>) -> u64 {
    items.sort_unstable();
    let mut hasher = DefaultHasher::new();
    items.hash(&mut hasher);
    hasher.finish()
}
