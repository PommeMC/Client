#version 450

// Vanilla post/entity_sobel.fsh: alpha marks where the silhouette's alpha
// changes, the colour averages the alpha-weighted neighbourhood.

layout(set = 0, binding = 0) uniform texture2D in_tex;
layout(set = 0, binding = 1) uniform sampler in_tex_sampler;

layout(location = 0) in vec2 v_uv;
layout(location = 0) out vec4 out_color;

vec4 sample_at(vec2 uv) {
    return texture(sampler2D(in_tex, in_tex_sampler), uv);
}

void main() {
    vec2 one_texel = 1.0 / vec2(textureSize(sampler2D(in_tex, in_tex_sampler), 0));
    vec4 center = sample_at(v_uv);
    vec4 left = sample_at(v_uv - vec2(one_texel.x, 0.0));
    vec4 right = sample_at(v_uv + vec2(one_texel.x, 0.0));
    vec4 up = sample_at(v_uv - vec2(0.0, one_texel.y));
    vec4 down = sample_at(v_uv + vec2(0.0, one_texel.y));
    float left_diff = abs(center.a - left.a);
    float right_diff = abs(center.a - right.a);
    float up_diff = abs(center.a - up.a);
    float down_diff = abs(center.a - down.a);
    float total = clamp(left_diff + right_diff + up_diff + down_diff, 0.0, 1.0);
    vec3 color = center.rgb * center.a + left.rgb * left.a + right.rgb * right.a
        + up.rgb * up.a + down.rgb * down.a;
    out_color = vec4(color * 0.2, total);
}
