#version 450

layout(set = 1, binding = 0) uniform texture2D skin_tex;
layout(set = 1, binding = 1) uniform sampler skin_tex_sampler;

layout(location = 0) in vec2 v_uv;
layout(location = 1) in vec4 v_light;

layout(location = 0) out vec4 out_color;

void main() {
    vec4 color = texture(sampler2D(skin_tex, skin_tex_sampler), v_uv);
    if (color.a < 0.5) discard;
    out_color = color * v_light;
}
