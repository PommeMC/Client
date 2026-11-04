#version 450

#include "fog.glsl"

layout(location = 0) in vec4 v_color;
layout(location = 1) in float v_fog;
layout(location = 2) in vec3 v_fog_color;

layout(location = 0) out vec4 out_color;

void main() {
    out_color = vec4(apply_fog(v_color.rgb, v_fog, v_fog_color), v_color.a);
}
