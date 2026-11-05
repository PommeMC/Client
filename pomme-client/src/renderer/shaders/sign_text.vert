#version 450

#include "fog.glsl"
#include "camera_ubo.glsl"
#include "lightmap.glsl"

layout(location = 0) in vec3 position;
layout(location = 1) in vec2 uv;
layout(location = 2) in float layer;
layout(location = 3) in float tex_select;
layout(location = 4) in vec4 color;
layout(location = 5) in float light;

layout(location = 0) out vec2 v_uv;
layout(location = 1) out float v_layer;
layout(location = 2) out float v_tex;
layout(location = 3) out vec4 v_color;
layout(location = 4) out float v_fog;
layout(location = 5) out vec3 v_fog_color;

void main() {
    vec3 rel = position - camera_pos.xyz;
    gl_Position = view_proj * vec4(rel, 1.0);
    v_uv = uv;
    v_layer = layer;
    v_tex = tex_select;
    v_color = color * light_lane_color(light);
    v_fog = total_fog_value(rel, fog_env, camera_pos.w, fog_color.w);
    v_fog_color = fog_color.rgb;
}
