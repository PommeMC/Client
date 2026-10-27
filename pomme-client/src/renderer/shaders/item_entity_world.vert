#version 450

#include "item_instance.glsl"
#include "fog.glsl"

#include "camera_ubo.glsl"

#include "item_tint.glsl"

#include "light.glsl"

layout(location = 0) in vec3 position;
layout(location = 1) in vec2 tex_coords;
layout(location = 2) in vec4 light_tint;
layout(location = 3) in vec4 normal_packed;
layout(location = 4) in uint tint_index;

layout(location = 0) out vec2 v_tex_coords;
layout(location = 1) out float v_light;
layout(location = 2) out vec4 v_tint;
layout(location = 3) out float v_fog;
layout(location = 4) out vec3 v_fog_color;
layout(location = 5) flat out float v_world_light;

void main() {
    vec4 world_pos = model * vec4(position, 1.0);
    vec3 rel = world_pos.xyz - camera_pos.xyz;
    gl_Position = view_proj * vec4(rel, 1.0);

    // Vanilla's VertexConsumer transforms the baked quad direction by the
    // item's normal matrix before item.vsh applies Lighting.Entry.LEVEL.
    vec3 world_normal = normalize(normal_matrix * normal_packed.xyz);

    v_tex_coords = tex_coords;
    v_light = mix_light(level_light(world_normal, instance_light.y > 0.5));
    v_tint = resolve_item_tint(tint_index, item_tint_base, item_tint_count);
    v_fog = total_fog_value(rel, fog_env, camera_pos.w, fog_color.w);
    v_fog_color = fog_color.rgb;
    v_world_light = instance_light.x;
}
