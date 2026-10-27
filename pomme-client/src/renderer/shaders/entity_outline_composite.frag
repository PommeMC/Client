#version 450

// Vanilla `blitEntityOutline` (core/blit_screen.fsh).

layout(set = 0, binding = 0) uniform texture2D in_tex;
layout(set = 0, binding = 1) uniform sampler in_tex_sampler;

layout(location = 0) in vec2 v_uv;
layout(location = 0) out vec4 out_color;

void main() {
    out_color = texture(sampler2D(in_tex, in_tex_sampler), v_uv);
}
