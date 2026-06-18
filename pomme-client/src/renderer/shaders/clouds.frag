#version 450

layout(push_constant) uniform Push {
    vec4 tint;
    vec4 offset;
} pc;

layout(location = 0) in vec4 v_color;
layout(location = 1) in float v_dist;

layout(location = 0) out vec4 out_color;

void main() {
    // pc.offset.w = fog-fade end; fade cloud alpha linearly to zero by it, so the
    // field melts into the sky (vanilla `1 - linear_fog_value(dist, 0, FogCloudsEnd)`).
    float fog = clamp(v_dist / pc.offset.w, 0.0, 1.0);
    float a = pc.tint.a * (1.0 - fog);
    if (a < 0.01) discard;
    out_color = vec4(v_color.rgb * pc.tint.rgb, a);
}
