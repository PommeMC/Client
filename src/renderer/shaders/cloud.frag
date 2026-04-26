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

layout(set = 1, binding = 0) uniform sampler2D cloud_tex;

layout(location = 0) in vec2 v_world_xz;
layout(location = 1) in float v_dist;

layout(location = 0) out vec4 out_color;

void main() {
    float cell_size = 12.0;
    vec2 uv = v_world_xz / (256.0 * cell_size);
    uv = fract(uv);

    float cloud = texture(cloud_tex, uv).r;
    if (cloud < 0.5) discard;

    float fog = clamp(1.0 - v_dist / 2048.0, 0.0, 1.0);
    float alpha = cloud_color.a * fog;
    out_color = vec4(cloud_color.rgb * alpha, alpha);
}
