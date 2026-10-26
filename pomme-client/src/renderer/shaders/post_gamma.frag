#version 450

// Moves the frame between the sRGB swapchain and vanilla's `main`, which
// holds gamma-space colours in GL's bottom-up rows: a vertical flip, then
// `exponent` (1/2.2 into `main`, 2.2 back out).

layout(set = 0, binding = 0) uniform texture2D in_tex;
layout(set = 0, binding = 1) uniform sampler in_tex_sampler;

layout(push_constant) uniform PushConstants {
    float exponent;
};

layout(location = 0) in vec2 v_uv;
layout(location = 0) out vec4 out_color;

void main() {
    vec4 color = texture(sampler2D(in_tex, in_tex_sampler), vec2(v_uv.x, 1.0 - v_uv.y));
    out_color = vec4(pow(color.rgb, vec3(exponent)), 1.0);
}
