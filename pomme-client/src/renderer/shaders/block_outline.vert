#version 450

// Vanilla `rendertype_lines.vsh`: each line a quad `LineWidth` pixels wide
// in screen space.

#include "fog.glsl"

#include "camera_ubo.glsl"

layout(push_constant) uniform LinePush {
    vec4 line_color;
    vec2 screen_size;
    float line_width;
};

layout(location = 0) in vec3 position;
layout(location = 1) in vec3 normal;
layout(location = 2) in float side;

layout(location = 0) out vec4 v_color;
layout(location = 1) out float v_fog;
layout(location = 2) out vec3 v_fog_color;

// `VIEW_SHRINK`, times `LayeringTransform.VIEW_OFFSET_Z_LAYERING`'s
// perspective scale on the model-view.
const float VIEW_SCALE = (1.0 - 1.0 / 256.0) * (1.0 - 1.0 / 4096.0);

void main() {
    vec3 rel = position - camera_pos.xyz;
    vec4 line_start = view_proj * vec4(rel * VIEW_SCALE, 1.0);
    vec4 line_end = view_proj * vec4((rel + normal) * VIEW_SCALE, 1.0);

    vec3 ndc1 = line_start.xyz / line_start.w;
    vec3 ndc2 = line_end.xyz / line_end.w;

    vec2 line_screen_direction = normalize((ndc2.xy - ndc1.xy) * screen_size);
    vec2 line_offset = vec2(-line_screen_direction.y, line_screen_direction.x) * line_width / screen_size;
    if (line_offset.x < 0.0) {
        line_offset *= -1.0;
    }
    gl_Position = vec4((ndc1 + vec3(line_offset * side, 0.0)) * line_start.w, line_start.w);

    v_color = line_color;
    v_fog = total_fog_value(rel, fog_env, camera_pos.w, fog_color.w);
    v_fog_color = fog_color.rgb;
}
