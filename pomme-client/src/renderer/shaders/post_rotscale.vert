#version 450

// Vanilla post/rotscale.vsh.

layout(push_constant) uniform PushConstants {
    vec2 in_scale;
    vec2 in_offset;
    float in_rotation;
};

layout(location = 0) out vec2 v_uv;
layout(location = 1) out vec2 v_scaled;

void main() {
    v_uv = vec2((gl_VertexIndex << 1) & 2, gl_VertexIndex & 2);
    gl_Position = vec4(v_uv * 2.0 - 1.0, 0.0, 1.0);

    float in_radians = in_rotation * 0.0174532925;
    float cosine = cos(in_radians);
    float sine = sin(in_radians);
    float rot_u = v_uv.x * cosine - v_uv.y * sine;
    float rot_v = v_uv.y * cosine + v_uv.x * sine;
    v_scaled = vec2(rot_u, rot_v) * in_scale + in_offset;
}
