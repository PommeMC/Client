#version 450

layout(set = 1, binding = 0) uniform sampler2D entity_tex;

layout(location = 0) in vec2 v_tex_coords;

layout(location = 0) out vec4 out_color;

void main() {
    vec4 color = texture(entity_tex, v_tex_coords);
    if (color.a < 0.5) discard;
    out_color = color;
}
