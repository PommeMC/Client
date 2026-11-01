#version 450

// Voxy's `chunkoutline/outline.vsh`: a 16³ box per drawn chunk section,
// drawn depth-only so the far side of each box bounds what distant terrain
// may draw within loaded chunks (distant.frag discards nearer fragments).

#include "camera_ubo.glsl"

layout(push_constant) uniform BoundPc {
    // The camera's view projection with the distant near and far planes.
    mat4 distant_view_proj;
};

// Each drawn section's coordinates (gpu.rs `BoundStore`).
layout(set = 1, binding = 0) readonly buffer Sections {
    ivec4 sections[];
};

void main() {
    ivec3 origin = sections[gl_InstanceIndex].xyz * 16 - camera_block.xyz;
    // The shared cube indices give the corner's bits: x, y, z.
    uint corner = gl_VertexIndex;
    vec3 point = vec3(origin + ivec3(corner & 1u, corner >> 1u & 1u, corner >> 2u) * 16);
    gl_Position = distant_view_proj * vec4(point - camera_pos.xyz, 1.0);
    // Brought a hair closer, so a face on the box's edge still counts as inside.
    gl_Position.z -= 0.0005;
}
