#version 450

// Vanilla post/invert.fsh.

layout(set = 0, binding = 0) uniform texture2D in_tex;
layout(set = 0, binding = 1) uniform sampler in_tex_sampler;

layout(push_constant) uniform PushConstants {
    float inverse_amount;
};

layout(location = 0) in vec2 v_uv;
layout(location = 0) out vec4 out_color;

void main() {
    vec4 diffuse_color = texture(sampler2D(in_tex, in_tex_sampler), v_uv);
    vec4 invert_color = 1.0 - diffuse_color;
    vec4 color = mix(diffuse_color, invert_color, inverse_amount);
    out_color = vec4(color.rgb, 1.0);
}
