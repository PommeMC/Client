#version 450

#include "fog.glsl"

layout(set = 1, binding = 0) uniform texture2D custom_tex;
layout(set = 1, binding = 1) uniform sampler custom_tex_sampler;

layout(location = 0) in vec2 v_uv;
layout(location = 1) in vec4 v_color;
layout(location = 2) in float v_fog;
layout(location = 3) in vec3 v_fog_color;
layout(location = 4) in vec4 v_light;
layout(location = 5) in float v_diffuse;

layout(location = 0) out vec4 out_color;

void main() {
    vec4 color = texture(sampler2D(custom_tex, custom_tex_sampler), v_uv) * v_color;
    // `entity.fsh`'s ALPHA_CUTOUT 0.1.
    if (color.a < 0.1) discard;
    color.rgb *= v_diffuse;
    color *= v_light;
    out_color = vec4(apply_fog(color.rgb, v_fog, v_fog_color), color.a);
}
