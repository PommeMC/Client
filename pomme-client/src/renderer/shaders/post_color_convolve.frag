#version 450

// Vanilla post/color_convolve.fsh.

layout(set = 0, binding = 0) uniform texture2D in_tex;
layout(set = 0, binding = 1) uniform sampler in_tex_sampler;

layout(push_constant) uniform PushConstants {
    vec3 red_matrix;
    vec3 green_matrix;
    vec3 blue_matrix;
};

const vec3 GRAY = vec3(0.3, 0.59, 0.11);
const float SATURATION = 1.8;

layout(location = 0) in vec2 v_uv;
layout(location = 0) out vec4 out_color;

void main() {
    vec4 in_texel = texture(sampler2D(in_tex, in_tex_sampler), v_uv);

    vec3 color = vec3(
        dot(in_texel.rgb, red_matrix),
        dot(in_texel.rgb, green_matrix),
        dot(in_texel.rgb, blue_matrix)
    );

    float luma = dot(color, GRAY);
    vec3 chroma = color - luma;
    color = chroma * SATURATION + luma;

    out_color = vec4(color, 1.0);
}
