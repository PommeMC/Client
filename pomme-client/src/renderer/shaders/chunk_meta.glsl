// One section's draw metadata (set 0, binding 0 of the cull chain), a single
// declaration shared by the passes so the layout can't drift from the Rust
// struct (renderer/chunk/abi.rs `ChunkMeta`).
struct ChunkMeta {
    vec3 aabb_min;
    uint region_slot;
    vec3 aabb_max;
    uint visibility_generation;
    ivec3 origin;
    uint _pad;
    // Descriptor ranges in fixed order: opaque (regular solid + lava),
    // cutout, then translucent water. Each descriptor draws a non-indexed
    // stream of six vertices per packed face.
    uint batch_word_offset;
    uint solid_batch_count;
    uint cutout_batch_count;
    uint fluid_batch_count;
};

// Water ordering bucket count; matches renderer/chunk/state.rs
// `WATER_BUCKETS` (the candidate counter sits one slot past the buckets).
const uint WATER_BUCKETS = 512;
