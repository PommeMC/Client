#version 450

layout(set = 1, binding = 0) uniform sampler2D shadow_tex;

layout(location = 0) in vec4 v_color;
layout(location = 1) in vec2 v_uv;

layout(location = 0) out vec4 out_color;

void main() {
    vec4 tex = texture(shadow_tex, clamp(v_uv, 0.0, 1.0));
    // shadow.png is white/grayscale with alpha gradient.
    // In vanilla rendertype_entity_shadow:
    // color = texture(Sampler0, clamp(texCoord0, 0.0, 1.0)) * vertexColor
    // fragColor = color;
    vec4 c = tex * v_color;
    if (c.a < 0.001) {
        discard;
    }
    out_color = c;
}
