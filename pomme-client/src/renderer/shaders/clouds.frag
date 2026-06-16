#version 450

layout(push_constant) uniform Push {
    vec4 tint;
    vec4 offset;
} pc;

layout(location = 0) in vec4 v_color;
layout(location = 1) in float v_dist;

layout(location = 0) out vec4 out_color;

// Width of the soft fade band, in blocks, before the cloud disc edge.
const float FADE_BAND = 64.0;

void main() {
    // pc.offset.w = disc edge distance; fade alpha to zero over the last band.
    float edge = pc.offset.w;
    float fade = 1.0 - smoothstep(edge - FADE_BAND, edge, v_dist);
    float a = pc.tint.a * fade;
    if (a < 0.01) discard;
    out_color = vec4(v_color.rgb * pc.tint.rgb, a);
}
