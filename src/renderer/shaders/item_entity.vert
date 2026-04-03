#version 450

layout(set = 0, binding = 0) uniform CameraUniform {
    mat4 view_proj;
    vec4 camera_pos;
};

layout(push_constant) uniform PushConstants {
    mat4 model;
};

layout(location = 0) in vec3 position;
layout(location = 1) in vec2 tex_coords;
layout(location = 2) in float light;
layout(location = 3) in vec3 tint;

layout(location = 0) out vec2 v_tex_coords;
layout(location = 1) out float v_light;
layout(location = 2) out vec3 v_tint;

void main() {
    vec4 world_pos = model * vec4(position, 1.0);
    gl_Position = view_proj * vec4(world_pos.xyz - camera_pos.xyz, 1.0);
    v_tex_coords = tex_coords;
    v_light = light;
    v_tint = tint;
}
