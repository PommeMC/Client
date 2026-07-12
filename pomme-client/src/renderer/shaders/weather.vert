#version 450

layout(set = 0, binding = 0) uniform CameraUniform {
    mat4 view_proj;
    vec4 camera_pos;
    vec4 fog_color;
};

layout(location = 0) in vec3 position;
layout(location = 1) in vec2 uv;
layout(location = 2) in float brightness;

layout(location = 0) out vec2 v_uv;
layout(location = 1) out float v_brightness;

void main() {
    // Positions arrive anchor-relative (rebased in f64 CPU-side); camera_pos
    // is the eye's offset from the anchor (matches item_entity.vert).
    gl_Position = view_proj * vec4(position - camera_pos.xyz, 1.0);
    v_uv = uv;
    v_brightness = brightness;
}
