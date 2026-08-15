// Bit layout for the greedy terrain face record (one u64 / two u32 words).
const uint GREEDY_BLOCK_SHIFT = 52u;
const uint GREEDY_DIR_SHIFT = 49u;
const uint GREEDY_WIDTH_SHIFT = 45u;
const uint GREEDY_HEIGHT_SHIFT = 41u;
const uint GREEDY_GLOBAL_SHIFT = 25u;
const uint GREEDY_SHADE_SHIFTS[4] = uint[4](21u, 17u, 13u, 9u);

uint greedyBits64(uvec2 words, uint shift, uint count) {
    uint mask = count == 32u ? 0xffffffffu : ((1u << count) - 1u);
    uint end = shift + count;
    if (end <= 32u) {
        return (words.x >> shift) & mask;
    }
    if (shift >= 32u) {
        return (words.y >> (shift - 32u)) & mask;
    }
    uint lo_count = 32u - shift;
    uint lo = (words.x >> shift) & ((1u << lo_count) - 1u);
    uint hi = words.y & ((1u << (count - lo_count)) - 1u);
    return lo | (hi << lo_count);
}

uvec2 loadGreedyFace(uint word_offset) {
    return uvec2(meshData[word_offset], meshData[word_offset + 1u]);
}

uint greedyBlockIndex(uvec2 words) { return greedyBits64(words, GREEDY_BLOCK_SHIFT, 12u); }
uint greedyDirection(uvec2 words) { return greedyBits64(words, GREEDY_DIR_SHIFT, 3u); }
uint greedyWidth(uvec2 words) { return greedyBits64(words, GREEDY_WIDTH_SHIFT, 4u) + 1u; }
uint greedyHeight(uvec2 words) { return greedyBits64(words, GREEDY_HEIGHT_SHIFT, 4u) + 1u; }
uint greedyGlobalId(uvec2 words) { return greedyBits64(words, GREEDY_GLOBAL_SHIFT, 16u); }
uint greedyTintIndex(uvec2 words) { return greedyBits64(words, 0u, 9u); }

uint greedyShadeCorner(uvec2 words, uint corner) {
    return greedyBits64(words, GREEDY_SHADE_SHIFTS[corner], 4u);
}

vec3 greedyBlockOrigin(uint block_index) {
    return vec3(float(block_index & 0xfu), float(block_index >> 8u),
        float((block_index >> 4u) & 0xfu));
}

vec3 greedyTint(uint table_word, uint tint_index) {
    if (tint_index == 0u) {
        return vec3(1.0);
    }
    uint packed = meshData[table_word + tint_index];
    return vec3(float(packed & 0xffu) / 255.0, float((packed >> 8u) & 0xffu) / 255.0,
        float((packed >> 16u) & 0x7fu) / 127.0);
}

// Vertex order matches `face_positions` in model.rs.
vec3 greedyCornerPos(GlobalFace face, uint direction, vec3 origin, uint width, uint height, uint corner) {
    if (width == 1u && height == 1u) {
        return origin + face.positions[corner].xyz;
    }

    float W = float(width);
    float H = float(height);
    if (direction == 0u) { // Down
        float x = (corner == 2u || corner == 3u) ? W : 0.0;
        float z = (corner == 0u || corner == 3u) ? H : 0.0;
        return origin + vec3(x, face.positions[corner].y, z);
    } else if (direction == 1u) { // Up
        float x = (corner == 2u || corner == 3u) ? W : 0.0;
        float z = (corner == 1u || corner == 2u) ? H : 0.0;
        return origin + vec3(x, face.positions[corner].y, z);
    } else if (direction == 2u) { // North
        float x = (corner == 0u || corner == 1u) ? W : 0.0;
        float y = (corner == 0u || corner == 3u) ? H : 0.0;
        return origin + vec3(x, y, face.positions[corner].z);
    } else if (direction == 3u) { // South
        float x = (corner == 2u || corner == 3u) ? W : 0.0;
        float y = (corner == 0u || corner == 3u) ? H : 0.0;
        return origin + vec3(x, y, face.positions[corner].z);
    } else if (direction == 4u) { // West
        float y = (corner == 0u || corner == 3u) ? H : 0.0;
        float z = (corner == 2u || corner == 3u) ? W : 0.0;
        return origin + vec3(face.positions[corner].x, y, z);
    }
    // East
    float y = (corner == 0u || corner == 3u) ? H : 0.0;
    float z = (corner == 0u || corner == 1u) ? W : 0.0;
    return origin + vec3(face.positions[corner].x, y, z);
}

void greedyFaceTileOffset(uint direction, uint width, uint height, uint corner, out float cu, out float cv) {
    float W = float(width);
    float H = float(height);
    if (direction == 0u) { // Down
        cu = (corner == 2u || corner == 3u) ? W : 0.0;
        cv = (corner == 0u || corner == 3u) ? H : 0.0;
    } else if (direction == 1u) { // Up
        cu = (corner == 2u || corner == 3u) ? W : 0.0;
        cv = (corner == 1u || corner == 2u) ? H : 0.0;
    } else if (direction == 2u) { // North
        cu = (corner == 0u || corner == 1u) ? W : 0.0;
        cv = (corner == 0u || corner == 3u) ? H : 0.0;
    } else if (direction == 3u) { // South
        cu = (corner == 2u || corner == 3u) ? W : 0.0;
        cv = (corner == 0u || corner == 3u) ? H : 0.0;
    } else if (direction == 4u) { // West
        cu = (corner == 2u || corner == 3u) ? W : 0.0;
        cv = (corner == 0u || corner == 3u) ? H : 0.0;
    } else { // East
        cu = (corner == 0u || corner == 1u) ? W : 0.0;
        cv = (corner == 0u || corner == 3u) ? H : 0.0;
    }
}

vec4 greedyFaceRegion(GlobalFace face) {
    vec2 region_min = face.uvs[0].zw;
    vec2 region_max = face.uvs[1].zw;
    return vec4(region_min, region_max - region_min);
}

vec2 greedyCornerUv(GlobalFace face, uint direction, uint width, uint height, uint corner) {
    vec4 region = greedyFaceRegion(face);
    vec2 region_min = region.xy;
    vec2 region_span = region.zw;
    vec2 norm0 = (face.uvs[0].xy - region_min) / region_span;
    if (width == 1u && height == 1u) {
        return (face.uvs[corner].xy - region_min) / region_span;
    }
    vec2 norm_du = (face.uvs[3].xy - face.uvs[0].xy) / region_span;
    vec2 norm_dv = (face.uvs[1].xy - face.uvs[0].xy) / region_span;
    float cu;
    float cv;
    greedyFaceTileOffset(direction, width, height, corner, cu, cv);
    return norm0 + norm_du * cu + norm_dv * cv;
}
