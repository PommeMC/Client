#version 450

layout(set = 1, binding = 0) uniform texture2D precip_tex;
layout(set = 1, binding = 1) uniform sampler precip_tex_sampler;

layout(location = 0) in vec2 v_uv;
layout(location = 1) in float v_brightness;

layout(location = 0) out vec4 out_color;

void main() {
    vec4 c = texture(sampler2D(precip_tex, precip_tex_sampler), v_uv);
    float a = c.a * v_brightness;
    if (a < 0.01) discard;
    out_color = vec4(c.rgb, a);
}
