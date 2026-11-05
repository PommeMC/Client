#version 450

#include "fog.glsl"

layout(set = 1, binding = 0) uniform texture2DArray font_tex;
layout(set = 1, binding = 1) uniform texture2DArray font_color_tex;
layout(set = 1, binding = 2) uniform sampler font_tex_sampler;

layout(location = 0) in vec2 v_uv;
layout(location = 1) in float v_layer;
layout(location = 2) in float v_tex;
layout(location = 3) in vec4 v_color;
layout(location = 4) in float v_fog;
layout(location = 5) in vec3 v_fog_color;

layout(location = 0) out vec4 out_color;

void main() {
    float gray = texture(sampler2DArray(font_tex, font_tex_sampler), vec3(v_uv, v_layer)).r;
    vec4 colored = texture(sampler2DArray(font_color_tex, font_tex_sampler), vec3(v_uv, v_layer));
    vec4 glyph = mix(vec4(1.0, 1.0, 1.0, gray), colored, v_tex);
    if (glyph.a <= 0.05) discard;
    vec3 color = apply_fog(v_color.rgb * glyph.rgb, v_fog, v_fog_color);
    out_color = vec4(color, v_color.a * glyph.a);
}
