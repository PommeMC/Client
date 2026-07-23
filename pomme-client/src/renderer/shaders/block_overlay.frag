#version 450

layout(set = 1, binding = 0) uniform sampler2D destroy_tex;

layout(push_constant) uniform OverlayPush {
    float v_base;
    float v_scale;
};

layout(location = 0) in vec2 v_uv;

layout(location = 0) out vec4 out_color;

void main() {
    // Raw projected crack UVs; wrap like vanilla's REPEAT sampler, then remap
    // V into this stage's row of the vertical atlas.
    vec2 t = fract(v_uv);
    vec4 color = texture(destroy_tex, vec2(t.x, v_base + t.y * v_scale));
    if (color.a < 0.1) discard;
    out_color = color;
}
