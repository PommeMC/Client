#version 450

// Vanilla post/box_blur.fsh. Every chain using it here sets a radius, so the
// `MenuBlurRadius` fallback for one under 0.5 never applies.

layout(set = 0, binding = 0) uniform texture2D in_tex;
layout(set = 0, binding = 1) uniform sampler in_tex_sampler;

layout(push_constant) uniform PushConstants {
    vec2 blur_dir;
    float radius;
};

layout(location = 0) in vec2 v_uv;
layout(location = 0) out vec4 out_color;

void main() {
    vec2 one_texel = 1.0 / vec2(textureSize(sampler2D(in_tex, in_tex_sampler), 0));
    vec2 sample_step = one_texel * blur_dir;

    vec4 blurred = vec4(0.0);
    float actual_radius = round(radius);
    for (float a = -actual_radius + 0.5; a <= actual_radius; a += 2.0) {
        blurred += texture(sampler2D(in_tex, in_tex_sampler), v_uv + sample_step * a);
    }
    blurred += texture(sampler2D(in_tex, in_tex_sampler), v_uv + sample_step * actual_radius) / 2.0;
    out_color = blurred / (actual_radius + 0.5);
}
