// One section's draw metadata (set 0, binding 0 of the cull chain), a single
// declaration shared by the passes so the layout can't drift from the Rust
// struct (renderer/chunk/buffer.rs `ChunkMeta`).
struct ChunkMeta {
    vec4 aabb_min;
    vec4 aabb_max;
    // Quads per pass in the section's vertex slice, solid first then cutout
    // then water; draws run against the shared static quad index buffer (6
    // indices and 4 vertices per quad, first_index always 0).
    uint solid_quads;
    uint cutout_quads;
    int vertex_offset;
    // Upload stamp for the vertex shader's fade.
    uint uploaded_ms;
    // ivec3 + uint pack into one 16-byte slot, matching the Rust layout.
    ivec3 origin;
    uint water_quads;
};

// Water ordering bucket count; matches renderer/chunk/buffer.rs
// `WATER_BUCKETS` (the candidate counter sits one slot past the buckets).
const uint WATER_BUCKETS = 512;
