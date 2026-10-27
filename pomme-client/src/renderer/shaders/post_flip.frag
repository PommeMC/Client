#version 450

// Moves the frame between the swapchain and vanilla's `main`, which holds
// GL's bottom-up rows: a vertical flip.

layout(set = 0, binding = 0) uniform texture2D in_tex;
layout(set = 0, binding = 1) uniform sampler in_tex_sampler;

layout(location = 0) in vec2 v_uv;
layout(location = 0) out vec4 out_color;

void main() {
    vec4 color = texture(sampler2D(in_tex, in_tex_sampler), vec2(v_uv.x, 1.0 - v_uv.y));
    out_color = vec4(color.rgb, 1.0);
}
