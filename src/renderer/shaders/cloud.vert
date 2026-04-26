#version 450

layout(set = 0, binding = 0) uniform CloudUniform {
    mat4 view_proj;
    vec4 cloud_color;
    vec3 camera_pos;
    float cloud_offset;
    float cloud_height;
    float _pad1;
    float _pad2;
    float _pad3;
};

layout(location = 0) in vec3 position;

layout(location = 0) out vec2 v_world_xz;
layout(location = 1) out float v_dist;

void main() {
    vec3 world_pos = position + vec3(camera_pos.x, cloud_height, camera_pos.z);
    gl_Position = view_proj * vec4(world_pos - camera_pos, 1.0);
    v_world_xz = vec2(world_pos.x + cloud_offset, world_pos.z);
    v_dist = length(world_pos - camera_pos);
}
