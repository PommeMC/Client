#version 450

#include "fog.glsl"

layout(set = 1, binding = 0) uniform sampler2D atlas_texture;

layout(location = 0) in vec2 v_uv;
layout(location = 1) in vec4 v_color;
layout(location = 2) in float v_fog;
layout(location = 3) in vec3 v_fog_color;

layout(location = 0) out vec4 out_color;

void main() {
    vec4 tex = texture(atlas_texture, v_uv);
    // Vanilla particle.fsh discards below 0.1 (items use 0.5).
    vec4 color = vec4(tex.rgb * v_color.rgb, tex.a * v_color.a);
    if (color.a < 0.1) discard;
    color.rgb = apply_fog(color.rgb, v_fog, v_fog_color);
    out_color = color;
}
