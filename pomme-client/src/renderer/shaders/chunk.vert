#version 450

#include "fog.glsl"
#include "camera_ubo.glsl"
#include "packing.glsl"

struct GlobalFace {
    vec4 positions[4];
    vec4 uvs[4];
    uvec4 material;
};
struct GlobalCuboid {
    GlobalFace faces[6];
};

layout(set = 2, binding = 0, std430) readonly buffer MeshData {
    uint meshData[];
};
layout(set = 2, binding = 1, std430) readonly buffer CuboidBuffer {
    GlobalCuboid globalCuboids[];
};

#include "greedy_face.glsl"

layout(location = 0) out vec2 v_tex_coords;
layout(location = 1) out float v_light;
layout(location = 2) out vec3 v_tint;
layout(location = 3) flat out float v_visibility;
layout(location = 4) out vec3 v_fog_color;
layout(location = 5) out float v_fog;
layout(location = 6) out vec4 v_region;

uint faceBits(uint f, uint shift, uint mask) {
    return (f >> shift) & mask;
}

uint shadeCorner(uint f, uint corner) {
    return faceBits(f, 12u + corner * 5u, 0x1fu);
}

vec3 sectionTint(uvec2 c) {
    return vec3(
        float((c.x >> 28u) | ((c.y & 0xfu) << 4u)) / 255.0,
        float((c.y >> 4u) & 0xffu) / 255.0,
        float((c.y >> 12u) & 0x7fu) / 127.0
    );
}

void main() {
    uint local_vertex = uint(gl_VertexIndex) % 6u;
    uint batch_word = uint(gl_InstanceIndex);
    bool is_fluid = meshData[batch_word + 3u] != 0xffffffffu;
    uint face_index = uint(gl_VertexIndex) / 6u;
    uint s0;
    uint s1;
    uint s2;
    uint s3;
    bool useDiagonalA;
    uint corner_id;
    vec3 rel;
    vec2 uv;
    float light;
    vec3 tint;
    vec4 region = vec4(0.0);
    ivec3 origin = ivec3(int(meshData[batch_word + 4u]), int(meshData[batch_word + 5u]),
        int(meshData[batch_word + 6u]));

    if (is_fluid) {
        uint face_id = meshData[batch_word] + face_index;
        uint f = meshData[face_id];
        s0 = shadeCorner(f, 0u);
        s1 = shadeCorner(f, 1u);
        s2 = shadeCorner(f, 2u);
        s3 = shadeCorner(f, 3u);
        useDiagonalA = abs(int(s0) - int(s2)) <= abs(int(s1) - int(s3));
        const uint diagonal_a[6] = uint[6](0u, 1u, 2u, 0u, 2u, 3u);
        const uint diagonal_b[6] = uint[6](0u, 1u, 3u, 1u, 2u, 3u);
        corner_id = useDiagonalA ? diagonal_a[local_vertex] : diagonal_b[local_vertex];

        uint quad_id = faceBits(f, 0u, 0xfffu);
        uint direction = quad_id % 6u;
        uint section_word = meshData[batch_word + 2u];
        uvec2 section_cuboid = uvec2(
            meshData[section_word + (quad_id / 6u) * 2u],
            meshData[section_word + (quad_id / 6u) * 2u + 1u]
        );
        uint global_id = (section_cuboid.x >> 12u) & 0xffffu;
        GlobalFace face = globalCuboids[global_id].faces[direction];
        vec4 local = face.positions[corner_id];
        uv = face.uvs[corner_id].xy;

        uint packed_heights = meshData[meshData[batch_word + 3u] + quad_id / 6u];
        bool high_x = local.x >= 0.5;
        bool high_z = local.z >= 0.5;
        uint height_corner = high_x ? (high_z ? 2u : 3u) : (high_z ? 1u : 0u);
        float height = float((packed_heights >> (height_corner * 4u)) & 0x0fu) / 15.0;
        local.y *= height;

        rel = vec3(origin - camera_block.xyz) + local.xyz +
            vec3(section_cuboid.x & 0xfu, (section_cuboid.x >> 4u) & 0xfu,
                 (section_cuboid.x >> 8u) & 0xfu) -
            camera_pos.xyz;
        light = float(shadeCorner(f, corner_id)) / 31.0;
        tint = face.material.y != 0u ? sectionTint(section_cuboid) : vec3(1.0);
    } else {
        uvec2 words = loadGreedyFace(meshData[batch_word] + face_index * 2u);
        s0 = greedyShadeCorner(words, 0u);
        s1 = greedyShadeCorner(words, 1u);
        s2 = greedyShadeCorner(words, 2u);
        s3 = greedyShadeCorner(words, 3u);
        uint width = greedyWidth(words);
        uint height = greedyHeight(words);
        useDiagonalA = (width == 1u && height == 1u)
            ? abs(int(s0) - int(s2)) <= abs(int(s1) - int(s3))
            : true;
        const uint diagonal_a[6] = uint[6](0u, 1u, 2u, 0u, 2u, 3u);
        const uint diagonal_b[6] = uint[6](0u, 1u, 3u, 1u, 2u, 3u);
        corner_id = useDiagonalA ? diagonal_a[local_vertex] : diagonal_b[local_vertex];

        uint direction = greedyDirection(words);
        uint global_id = greedyGlobalId(words);
        GlobalFace face = globalCuboids[global_id].faces[direction];
        vec3 block_origin = greedyBlockOrigin(greedyBlockIndex(words));
        vec3 pos = greedyCornerPos(face, direction, block_origin, width, height, corner_id);
        rel = vec3(origin - camera_block.xyz) + pos - camera_pos.xyz;
        uv = greedyCornerUv(face, direction, width, height, corner_id);
        region = greedyFaceRegion(face);
        light = float(greedyShadeCorner(words, corner_id)) / 15.0;
        uint tint_index = greedyTintIndex(words);
        tint = tint_index != 0u
            ? greedyTint(meshData[batch_word + 2u], tint_index)
            : vec3(1.0);
    }

    gl_Position = view_proj * vec4(rel, 1.0);
    v_tex_coords = uv;
    v_region = region;
    v_light = light;
    v_tint = tint;
    v_visibility = clamp(float(uint(camera_block.w) - meshData[batch_word + 7u]) / FADE_MS, 0.0, 1.0);
    v_fog_color = fog_color.rgb;
    v_fog = total_fog_value(rel, fog_env, camera_pos.w, fog_color.w);
}
