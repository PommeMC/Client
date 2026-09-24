#version 450

layout(set = 1, binding = 0) uniform sampler2D preview_tex;
layout(push_constant) uniform PreviewPush {
    vec4 tint;
    float alpha_cutoff;
} pc;

layout(location = 0) in vec2 v_uv;
layout(location = 0) out vec4 out_color;

void main() {
    vec4 color = texture(preview_tex, v_uv);
    if (color.a < pc.alpha_cutoff) discard;
    out_color = color * pc.tint;
}
