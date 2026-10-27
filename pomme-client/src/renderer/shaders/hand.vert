#version 450

#include "lightmap.glsl"

#include "light.glsl"

layout(set = 0, binding = 0) uniform HandUniform {
    mat4 mvp;
    mat4 normal_matrix;
    // x = the player's light lane, y = 1 under the nether's directional light.
    vec4 light;
};

layout(location = 0) in vec3 position;
layout(location = 1) in vec2 tex_coords;
layout(location = 2) in vec4 normal;

layout(location = 0) out vec2 v_uv;
layout(location = 1) out vec4 v_light;

void main() {
    gl_Position = mvp * vec4(position, 1.0);
    v_uv = tex_coords;
    // Vanilla `entity.vsh`: the directional light, then the lightmap.
    vec3 world_normal = normalize(mat3(normal_matrix) * normal.xyz);
    vec4 lightmap = light_lane_color(light.x);
    v_light = vec4(lightmap.rgb * mix_light(level_light(world_normal, light.y != 0.0)), lightmap.a);
}
