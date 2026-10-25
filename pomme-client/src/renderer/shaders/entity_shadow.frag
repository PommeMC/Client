#version 450

#include "fog.glsl"

layout(set = 1, binding = 0) uniform texture2D shadow_tex;
layout(set = 1, binding = 1) uniform sampler shadow_tex_sampler;

layout(location = 0) in vec4 v_color;
layout(location = 1) in vec2 v_uv;
layout(location = 2) in float v_fog;
layout(location = 3) in vec3 v_fog_color;

layout(location = 0) out vec4 out_color;

void main() {
    vec4 color = texture(sampler2D(shadow_tex, shadow_tex_sampler), clamp(v_uv, 0.0, 1.0)) * v_color;
    out_color = vec4(apply_fog(color.rgb, v_fog, v_fog_color), color.a);
}
