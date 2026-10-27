#version 450

layout(set = 1, binding = 0) uniform texture2D precip_tex;
layout(set = 1, binding = 1) uniform sampler precip_tex_sampler;

layout(location = 0) in vec2 v_uv;
layout(location = 1) in vec4 v_color;

layout(location = 0) out vec4 out_color;

void main() {
    // TODO: vanilla's particle fog.
    vec4 color = texture(sampler2D(precip_tex, precip_tex_sampler), v_uv) * v_color;
    if (color.a < 0.1) discard;
    out_color = color;
}
