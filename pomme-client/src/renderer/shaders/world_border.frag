#version 450

layout(set = 1, binding = 0) uniform texture2D border_tex;
layout(set = 1, binding = 1) uniform sampler border_tex_sampler;

layout(location = 0) in vec2 v_uv;
layout(location = 1) in vec4 v_color;

layout(location = 0) out vec4 out_color;

void main() {
    vec4 tex = texture(sampler2D(border_tex, border_tex_sampler), v_uv);
    if (tex.a == 0.0) {
        discard;
    }
    out_color = tex * v_color;
}
