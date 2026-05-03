#version 450

layout(set = 0, binding = 0) uniform sampler2D src_tex;

layout(push_constant) uniform PushConstants {
    vec2 direction;
};

layout(location = 0) in vec2 v_uv;
layout(location = 0) out vec4 out_color;

void main() {
    float weights[5] = float[](0.227027, 0.1945946, 0.1216216, 0.054054, 0.016216);

    vec4 result = texture(src_tex, v_uv) * weights[0];
    for (int i = 1; i < 5; i++) {
        vec2 offset = direction * float(i);
        result += texture(src_tex, v_uv + offset) * weights[i];
        result += texture(src_tex, v_uv - offset) * weights[i];
    }
    out_color = result;
}
