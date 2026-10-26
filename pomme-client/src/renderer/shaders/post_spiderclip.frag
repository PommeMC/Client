#version 450

// Vanilla post/spiderclip.fsh. Its `clamp(scaledCoord, 0.0, 1.0)` discards
// the result, so it's dropped.

layout(set = 0, binding = 0) uniform texture2D in_tex;
layout(set = 0, binding = 1) uniform sampler in_tex_sampler;
layout(set = 0, binding = 2) uniform texture2D blur_tex;
layout(set = 0, binding = 3) uniform sampler blur_tex_sampler;

// The leading members are post_rotscale.vert's.
layout(push_constant) uniform PushConstants {
    vec2 in_scale;
    vec2 in_offset;
    float in_rotation;
    vec4 scissor;
    vec4 vignette;
};

layout(location = 0) in vec2 v_uv;
layout(location = 1) in vec2 v_scaled;
layout(location = 0) out vec4 out_color;

void main() {
    vec4 scaled_texel = texture(sampler2D(in_tex, in_tex_sampler), v_scaled);
    vec4 blur_texel = texture(sampler2D(blur_tex, blur_tex_sampler), v_uv);
    vec4 texel = scaled_texel;

    if (v_scaled.x < scissor.x) texel = blur_texel;
    if (v_scaled.y < scissor.y) texel = blur_texel;
    if (v_scaled.x > scissor.z) texel = blur_texel;
    if (v_scaled.y > scissor.w) texel = blur_texel;

    if (v_scaled.x < vignette.x) texel = mix(blur_texel, texel, (scissor.x - v_scaled.x) / (scissor.x - vignette.x));
    if (v_scaled.y < vignette.y) texel = mix(blur_texel, texel, (scissor.y - v_scaled.y) / (scissor.y - vignette.y));
    if (v_scaled.x > vignette.z) texel = mix(blur_texel, texel, (scissor.z - v_scaled.x) / (scissor.z - vignette.z));
    if (v_scaled.y > vignette.w) texel = mix(blur_texel, texel, (scissor.w - v_scaled.y) / (scissor.w - vignette.w));
    out_color = vec4(texel.rgb, 1.0);
}
