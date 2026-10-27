#version 450

#include "fog.glsl"
#include "atlas_sprite.glsl"

layout(location = 0) in vec2 v_sprite_uv;
layout(location = 1) in vec3 v_color;
layout(location = 3) flat in float v_visibility;
layout(location = 4) in vec3 v_fog_color;
layout(location = 5) in float v_fog;
layout(location = 6) flat in uint v_sprite;

layout(location = 0) out vec4 out_color;

void main() {
    vec4 color = sample_atlas_sprite(v_sprite_uv, v_sprite);
    if (color.a < 0.5) discard;
    vec3 shaded =
        shade_chunk_surface(color.rgb, v_color, v_visibility, v_fog_color, v_fog);
    out_color = vec4(shaded, color.a);
}
