#version 450

// Vanilla post/entity_outline_box_blur.fsh. The radius is fixed at 2.0
// (vanilla declares a `Radius` uniform and ignores it), and the alpha is left
// unnormalised, which thickens the band.

layout(set = 0, binding = 0) uniform texture2D in_tex;
layout(set = 0, binding = 1) uniform sampler in_tex_sampler;

layout(push_constant) uniform PushConstants {
    vec2 blur_dir;
};

layout(location = 0) in vec2 v_uv;
layout(location = 0) out vec4 out_color;

void main() {
    vec2 one_texel = 1.0 / vec2(textureSize(sampler2D(in_tex, in_tex_sampler), 0));
    vec2 sample_step = one_texel * blur_dir;
    vec4 blurred = vec4(0.0);
    float radius = 2.0;
    for (float a = -radius + 0.5; a <= radius; a += 2.0) {
        blurred += texture(sampler2D(in_tex, in_tex_sampler), v_uv + sample_step * a);
    }
    blurred += texture(sampler2D(in_tex, in_tex_sampler), v_uv + sample_step * radius) / 2.0;
    out_color = vec4((blurred / (radius + 0.5)).rgb, blurred.a);
}
