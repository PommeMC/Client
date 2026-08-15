#version 450

// Terrain fragment shader for both opaque and cutout sprites. Alpha-test
// discard keeps cutout holes empty; fully opaque texels always pass.

#include "fog.glsl"

layout(set = 1, binding = 0) uniform sampler2D atlas_texture;

layout(location = 0) in vec2 v_tex_coords;
layout(location = 1) in float v_light;
layout(location = 2) in vec3 v_tint;
layout(location = 3) flat in float v_visibility;
layout(location = 4) in vec3 v_fog_color;
layout(location = 5) in float v_fog;
layout(location = 6) in vec4 v_region;

layout(location = 0) out vec4 out_color;

vec2 chunk_atlas_uv(vec2 tex_coords, vec4 region) {
    return region.z > 0.0 ? region.xy + fract(tex_coords) * region.zw : tex_coords;
}

void main() {
    vec4 color = texture(atlas_texture, chunk_atlas_uv(v_tex_coords, v_region));
    if (color.a < 0.5) discard;
    vec3 shaded =
        shade_chunk_surface(color.rgb, v_tint, v_light, v_visibility, v_fog_color, v_fog);
    out_color = vec4(shaded, color.a);
}
