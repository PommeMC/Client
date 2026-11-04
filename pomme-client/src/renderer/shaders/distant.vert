#version 450

// Distant terrain (renderer/distant): each quad is eight bytes in the bound
// arena, read by `gl_VertexIndex >> 2`; its corners come from its model's
// face data, lit through the lightmap. Fog comes later, in distant_blit.frag.

#include "lightmap.glsl"
#include "distant_models.glsl"
#define DISTANT_SET 0
#define SCENE_BINDING 0
#include "distant_section.glsl"

layout(set = 1, binding = 0) readonly buffer Quads {
    uvec2 quads[];
};
layout(set = 1, binding = 1) readonly buffer Models {
    BlockModel models[];
};
// Biome tints by kind (distant_models.glsl TINT_BIOMES per kind).
layout(set = 1, binding = 2) readonly buffer Tints {
    uint tints[];
};
// Per draw (`gl_InstanceIndex`): the section's packed key.
layout(set = 3, binding = 0) readonly buffer Positions {
    uvec2 positions[];
};

// Quad flags (cutout bit 0, translucent 1, tint state 2-3, face 4-6,
// width-1 8-11, height-1 12-15, model 16-31), the lightmap colour, the tint
// colour.
layout(location = 0) flat out uvec3 v_data;
layout(location = 1) out vec2 v_uv;

// Vanilla's directional shade (the overworld's cardinal lighting), by face.
const float FACE_SHADE[6] = float[](0.5, 1.0, 0.8, 0.8, 0.6, 0.6);

uint field(uvec2 quad, uint bits, uint shift) {
    uint word = shift < 32u ? quad.x : quad.y;
    return (word >> (shift & 31u)) & ((1u << bits) - 1u);
}

// A plane's two extents and its depth put on the axes the face's normal
// leaves free.
vec3 swizzle(uint axis, vec3 data) {
    return axis == 0u ? data.xzy : (axis == 1u ? data : data.zxy);
}

void main() {
    // The section's corner relative to the camera's section, and its level's
    // scale, blocks per voxel.
    uvec2 pos = positions[gl_InstanceIndex];
    uint lod = lod_level(pos);
    ivec3 base_section = (lod_position(pos) << lod) - camera_section;
    vec4 origin_scale = vec4(vec3(base_section << 5), float(1u << lod));
    uvec2 quad = quads[gl_VertexIndex >> 2];
    uint corner = gl_VertexIndex & 3u;
    uint face = field(quad, 3u, 0u);
    uint axis = face >> 1u;
    uint model_id = field(quad, 6u, 26u) | field(quad, 14u, 32u) << 6u;
    BlockModel model = models[model_id];
    uint face_data = model.face_data[face];
    vec2 size = vec2(field(quad, 4u, 3u), field(quad, 4u, 7u)) + 1.0;

    // The drawn texels' span, grown a hair so quads meet.
    vec4 bounds = vec4(face_data & 0xFu, face_data >> 4u & 0xFu, face_data >> 8u & 0xFu, face_data >> 12u & 0xFu) / 16.0
        + vec4(0.0, 1.0 / 16.0, 0.0, 1.0 / 16.0);
    bounds.xz -= 0.00005;
    bounds.yw -= bounds.xz;
    uint inset = face_data >> 16u & 63u;
    inset += uint(inset == 63u);
    float depth = float(inset) / 64.0;

    vec3 start = vec3(field(quad, 5u, 21u), field(quad, 5u, 16u), field(quad, 5u, 11u));
    start += swizzle(axis, vec3(bounds.xz, mix(depth, 1.0 - depth, float(face & 1u))));
    vec2 extent = bounds.yw + size - 1.0;
    vec2 mask = vec2(corner >> 1u & 1u, corner & 1u);
    vec3 point = origin_scale.xyz + (start + swizzle(axis, vec3(extent * mask, 0.0))) * origin_scale.w;
    gl_Position = distant_view_proj * vec4(point, 1.0);
    v_uv = bounds.xz + extent * mask;

    // A face short of its side cuts out once a quad spans more than one
    // block.
    uint flags = face_data >> 22u & 1u;
    flags |= uint(any(greaterThan(size, vec2(1.0)))) & (face_data >> 23u & 1u);
    flags |= uint((model.flags & MODEL_TRANSLUCENT) != 0u) << 1u;
    flags |= (face_data >> 24u & 3u) << 2u;
    flags |= face << 4u;
    flags |= (uint(size.x) - 1u) << 8u | (uint(size.y) - 1u) << 12u;
    flags |= model_id << 16u;

    // The quad's light (block high, sky low) through the lightmap, shaded per
    // face for models that take directional shade; the model's tint, constant or
    // by the quad's biome.
    uint light = field(quad, 8u, 55u);
    vec3 lit = sample_lightmap(vec2(float(light >> 4u), float(light & 0xFu)) * 16.0).rgb;
    if ((model.flags & MODEL_SHADED) != 0u) {
        lit *= FACE_SHADE[face];
    }
    uint tint = model.tint;
    if ((model.flags & MODEL_BIOME_TINT) != 0u) {
        tint = tints[tint * TINT_BIOMES + field(quad, 9u, 46u)];
    }
    v_data = uvec3(flags, packUnorm4x8(vec4(lit, 1.0)), tint);
}
