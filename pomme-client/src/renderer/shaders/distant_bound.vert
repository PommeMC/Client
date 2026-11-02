#version 450

// A box per run of drawn chunk sections, drawn depth-only so the far side of
// each box bounds what distant terrain may draw within loaded chunks
// (distant.frag discards nearer fragments).

#include "camera_ubo.glsl"

layout(push_constant) uniform BoundPc {
    // The camera's view projection with the distant near and far planes.
    mat4 distant_view_proj;
    // The render distance in blocks; boxes past it leave distant terrain be.
    float render_distance;
};

// Each box's section coordinates and its height in sections (gpu.rs
// `BoundStore`).
layout(set = 1, binding = 0) readonly buffer Sections {
    ivec4 sections[];
};

// Whether the box from `lo` to `hi` (camera-relative, grown a block) has a
// point within the render distance of the camera.
bool should_render(ivec3 lo, ivec3 hi) {
    vec3 nearest = mix(mix(vec3(0.0), vec3(lo), greaterThan(lo, ivec3(0))), vec3(hi), lessThan(hi, ivec3(0)))
        - camera_pos.xyz;
    return dot(nearest.xz, nearest.xz) < render_distance * render_distance
        && abs(nearest.y) < render_distance;
}

void main() {
    ivec4 box = sections[gl_InstanceIndex];
    ivec3 origin = box.xyz * 16 - camera_block.xyz;
    ivec3 size = ivec3(16, box.w * 16, 16);
    if (!should_render(origin - 1, origin + size + 1)) {
        gl_Position = vec4(-100.0, -100.0, -100.0, 0.0);
        return;
    }
    // The shared cube indices give the corner's bits: x, y, z.
    uint corner = gl_VertexIndex;
    vec3 point = vec3(origin + ivec3(corner & 1u, corner >> 1u & 1u, corner >> 2u) * size);
    gl_Position = distant_view_proj * vec4(point - camera_pos.xyz, 1.0);
    // Brought a hair closer, so a face on the box's edge still counts as inside.
    gl_Position.z -= 0.0005;
}
