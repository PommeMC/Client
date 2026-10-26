#version 450

// The item mesh for vanilla `rendertype_outline`: positions and the outline
// colour as the tint, for entity_outline.frag (which reads only its rgb).

#include "camera_ubo.glsl"
#include "item_instance.glsl"
#include "item_tint.glsl"

layout(location = 0) in vec3 position;
layout(location = 1) in vec2 tex_coords;

layout(location = 0) out vec2 v_tex_coords;
layout(location = 1) out vec4 v_tint;

void main() {
    vec4 world_pos = model * vec4(position, 1.0);
    gl_Position = view_proj * vec4(world_pos.xyz - camera_pos.xyz, 1.0);
    v_tex_coords = tex_coords;
    v_tint = unpack_argb(outline_color);
}
