#version 450

#include "fog.glsl"

// Translucent water: same shading as chunk.frag but no alpha cutout and a fixed
// alpha so the blended pass lets the scene (and underwater fog) show through.
layout(set = 1, binding = 0) uniform sampler2D atlas_texture;

layout(location = 0) in vec2 v_tex_coords;
layout(location = 1) in float v_light;
layout(location = 2) in vec3 v_tint;
layout(location = 3) flat in float v_visibility;
layout(location = 4) in vec3 v_fog_color;
layout(location = 5) in float v_fog;

layout(location = 0) out vec4 out_color;

const float WATER_ALPHA = 0.7;

void main() {
    vec4 color = texture(atlas_texture, v_tex_coords);
    vec3 linear_tint = pow(v_tint, vec3(2.2));
    float linear_light = pow(v_light, 2.2);
    vec3 tinted = color.rgb * linear_tint * linear_light;

    if (v_visibility < 1.0) {
        tinted = mix(v_fog_color, tinted, v_visibility);
    }

    tinted = apply_fog(tinted, v_fog, v_fog_color);

    out_color = vec4(tinted, WATER_ALPHA);
}
