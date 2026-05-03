#version 450

layout(set = 1, binding = 0) uniform sampler2D atlas_texture;

layout(location = 0) in vec2 v_tex_coords;
layout(location = 1) in float v_light;
layout(location = 2) in vec3 v_tint;
layout(location = 3) flat in float v_visibility;
layout(location = 4) in vec3 v_fog_color;

layout(location = 0) out vec4 out_color;

void main() {
    vec4 color = texture(atlas_texture, v_tex_coords);
    if (color.a < 0.5) discard;
    vec3 linear_tint = pow(v_tint, vec3(2.2));
    float linear_light = pow(v_light, 2.2);
    vec3 tinted = color.rgb * linear_tint * linear_light;

    if (v_visibility < 1.0) {
        tinted = mix(v_fog_color, tinted, v_visibility);
    }

    out_color = vec4(tinted, color.a);
}
