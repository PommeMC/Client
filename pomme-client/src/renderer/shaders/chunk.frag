#version 450

#include "fog.glsl"

layout(set = 1, binding = 0) uniform sampler2D atlas_texture;

layout(location = 0) in vec2 v_tex_coords;
layout(location = 1) in float v_light;
layout(location = 2) in vec3 v_tint;
layout(location = 3) flat in float v_visibility;
layout(location = 4) in vec3 v_fog_color;
layout(location = 5) in float v_fog;

layout(location = 0) out vec4 out_color;

void main() {
    vec4 color = texture(atlas_texture, v_tex_coords);
    if (color.a < 0.5) discard;
    vec3 shaded =
        shade_chunk_surface(color.rgb, v_tint, v_light, v_visibility, v_fog_color, v_fog);
    out_color = vec4(shaded, color.a);
}
