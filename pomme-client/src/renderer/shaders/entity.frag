#version 450

#include "fog.glsl"

layout(set = 1, binding = 0) uniform texture2D entity_tex;
layout(set = 1, binding = 1) uniform sampler entity_tex_sampler;

layout(location = 0) in vec2 v_tex_coords;
layout(location = 1) in vec4 v_tint;
layout(location = 2) in float v_fog;
layout(location = 3) in vec3 v_fog_color;
layout(location = 4) in vec4 v_overlay;
layout(location = 5) in float v_alpha_cutoff;
layout(location = 6) in vec4 v_light;

layout(location = 0) out vec4 out_color;

void main() {
    vec4 color = texture(sampler2D(entity_tex, entity_tex_sampler), v_tex_coords);
    if (color.a < v_alpha_cutoff) discard;
    vec4 lit = color * v_tint;
    lit.rgb = mix(v_overlay.rgb, lit.rgb, v_overlay.a);
    lit *= v_light;
    out_color = vec4(apply_fog(lit.rgb, v_fog, v_fog_color), lit.a);
}
