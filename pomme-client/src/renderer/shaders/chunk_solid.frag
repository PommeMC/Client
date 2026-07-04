#version 450

// Solid (opaque) terrain pass. Unlike chunk.frag this has no `discard`, so the
// driver keeps early-Z: with the front-to-back draw order, fragments occluded by
// nearer terrain are rejected before this shader runs. `early_fragment_tests`
// makes that explicit. Only sprites with no transparent texels are routed here
// (see AtlasRegion::opaque), so the alpha test chunk.frag does is unnecessary.
layout(early_fragment_tests) in;

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
    vec3 shaded =
        shade_chunk_surface(color.rgb, v_tint, v_light, v_visibility, v_fog_color, v_fog);
    out_color = vec4(shaded, 1.0);
}
