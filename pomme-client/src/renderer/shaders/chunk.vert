#version 450

layout(set = 0, binding = 0) uniform CameraUniform {
    mat4 view_proj;
    vec4 camera_pos;
    vec4 fog_color;
};

layout(location = 0) in vec3 position;
layout(location = 1) in vec2 tex_coords;
layout(location = 2) in vec4 light_tint;

layout(location = 0) out vec2 v_tex_coords;
layout(location = 1) out float v_light;
layout(location = 2) out vec3 v_tint;
layout(location = 3) flat out float v_visibility;
layout(location = 4) out vec3 v_fog_color;

void main() {
    gl_Position = view_proj * vec4(position - camera_pos.xyz, 1.0);
    v_tex_coords = tex_coords;
    v_light = light_tint.r;
    v_tint = light_tint.gba;
    v_visibility = uintBitsToFloat(gl_InstanceIndex);
    v_fog_color = fog_color.rgb;
}
