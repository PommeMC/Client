#version 450

layout(set = 1, binding = 0) uniform texture2D preview_tex;
layout(set = 1, binding = 1) uniform sampler preview_tex_sampler;
layout(push_constant) uniform PreviewPush {
    vec4 tint;
    float alpha_cutoff;
} pc;

layout(location = 0) in vec2 v_uv;
layout(location = 0) out vec4 out_color;

void main() {
    vec4 color = texture(sampler2D(preview_tex, preview_tex_sampler), v_uv);
    if (color.a < pc.alpha_cutoff) discard;
    out_color = color * pc.tint;
}
