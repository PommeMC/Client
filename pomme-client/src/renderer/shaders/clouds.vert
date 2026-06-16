#version 450

layout(set = 0, binding = 0) uniform CameraUniform {
    mat4 view_proj;
    vec4 camera_pos;
    vec4 fog_color;
};

// tint: per-frame cloud colour (rgb) and alpha (a).
// offset.xyz: camera-relative translation of the cloud grid (the sub-cell
// scroll plus the cloud-layer height), applied so the cached mesh only needs
// rebuilding when the camera crosses a whole cloud cell.
layout(push_constant) uniform Push {
    vec4 tint;
    vec4 offset;
} pc;

layout(location = 0) in vec3 position;
layout(location = 1) in vec4 color;

layout(location = 0) out vec4 v_color;
layout(location = 1) out float v_dist;

void main() {
    // `position` is baked in the integer cell grid; adding the offset yields the
    // camera-relative position (== world - camera_pos), matching weather.vert.
    vec3 rel = position + pc.offset.xyz;
    gl_Position = view_proj * vec4(rel, 1.0);
    v_color = color;
    // Horizontal distance from the camera, for the disc-edge fade.
    v_dist = length(rel.xz);
}
