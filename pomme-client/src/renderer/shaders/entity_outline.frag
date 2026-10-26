#version 450

// Vanilla rendertype_outline.fsh: the texture only cuts the silhouette, and
// the outline colour (the instance tint) fills it flat.

layout(set = 1, binding = 0) uniform texture2D entity_tex;
layout(set = 1, binding = 1) uniform sampler entity_tex_sampler;

layout(location = 0) in vec2 v_tex_coords;
layout(location = 1) in vec4 v_tint;

layout(location = 0) out vec4 out_color;

void main() {
    if (texture(sampler2D(entity_tex, entity_tex_sampler), v_tex_coords).a == 0.0) discard;
    out_color = vec4(v_tint.rgb, 1.0);
}
