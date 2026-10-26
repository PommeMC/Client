#version 450

// Vanilla post/bits.fsh.

layout(set = 0, binding = 0) uniform texture2D in_tex;
layout(set = 0, binding = 1) uniform sampler in_tex_sampler;

layout(push_constant) uniform PushConstants {
    float resolution;
    float mosaic_size;
};

const float SATURATION = 1.5;

layout(location = 0) in vec2 v_uv;
layout(location = 0) out vec4 out_color;

void main() {
    vec2 in_size = vec2(textureSize(sampler2D(in_tex, in_tex_sampler), 0));
    vec2 mosaic_in_size = in_size / mosaic_size;
    vec2 fract_pix = fract(v_uv * mosaic_in_size) / mosaic_in_size;

    vec4 base_texel = texture(sampler2D(in_tex, in_tex_sampler), v_uv - fract_pix);

    vec3 fract_texel = base_texel.rgb - fract(base_texel.rgb * resolution) / resolution;
    float luma = dot(fract_texel, vec3(0.3, 0.59, 0.11));
    vec3 chroma = (fract_texel - luma) * SATURATION;
    base_texel.rgb = luma + chroma;
    base_texel.a = 1.0;

    out_color = base_texel;
}
