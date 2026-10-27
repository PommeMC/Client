#version 450

#include "fog.glsl"

layout(set = 1, binding = 0) uniform texture2D atlas_texture;
layout(set = 1, binding = 1) uniform sampler atlas_texture_sampler;

layout(location = 0) in vec2 v_tex_coords;
layout(location = 1) in float v_light;
layout(location = 2) in vec4 v_tint;
layout(location = 3) in float v_fog;
layout(location = 4) in vec3 v_fog_color;
layout(location = 5) flat in vec4 v_world_light;

layout(location = 0) out vec4 out_color;

void main() {
    vec4 color = texture(sampler2D(atlas_texture, atlas_texture_sampler), v_tex_coords);
    // Vanilla ITEM_CUTOUT and ITEM_TRANSLUCENT both use ALPHA_CUTOUT=0.1.
    if (color.a < 0.1) discard;
    // Vanilla's item shader multiplies the sampled texture by the normalized
    // vertex color directly. Decoding the tint a second time makes grayscale
    // tint-mask textures such as bush substantially too dark.
    vec4 tinted = color * v_tint;
    tinted.rgb *= v_light;
    tinted *= v_world_light;
    tinted.rgb = apply_fog(tinted.rgb, v_fog, v_fog_color);
    out_color = tinted;
}
