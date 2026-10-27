#version 450

#include "lightmap.glsl"

layout(set = 0, binding = 0) uniform HandUniform {
    mat4 mvp;
    // x = the player's light lane; yzw unused.
    vec4 light;
};

layout(location = 0) in vec3 position;
layout(location = 1) in vec2 tex_coords;

layout(location = 0) out vec2 v_uv;
layout(location = 1) out vec4 v_light;

void main() {
    gl_Position = mvp * vec4(position, 1.0);
    v_uv = tex_coords;
    v_light = light_lane_color(light.x);
}
