#version 450

#include "fog.glsl"

layout(set = 1, binding = 0) uniform sampler2D entity_tex;

layout(location = 0) in vec2 v_tex_coords;
layout(location = 1) in vec4 v_tint;
layout(location = 2) in float v_fog;
layout(location = 3) in vec3 v_fog_color;

layout(location = 0) out vec4 out_color;

void main() {
    vec4 color = texture(entity_tex, v_tex_coords);
    if (color.a < 0.5) discard;
    vec4 lit = color * v_tint;
    out_color = vec4(apply_fog(lit.rgb, v_fog, v_fog_color), lit.a);
}
