#version 450

layout(set = 1, binding = 0) uniform sampler2D atlas_texture;

layout(push_constant) uniform PushConstants {
    layout(offset = 64) float world_light;
};

layout(location = 0) in vec2 v_tex_coords;
layout(location = 1) in float v_light;
layout(location = 2) in vec3 v_tint;

layout(location = 0) out vec4 out_color;

void main() {
    vec4 color = texture(atlas_texture, v_tex_coords);
    if (color.a < 0.5) discard;
    vec3 linear_tint = pow(v_tint, vec3(2.2));
    float linear_light = pow(world_light * v_light, 2.2);
    vec3 tinted = color.rgb * linear_tint * linear_light;
    out_color = vec4(tinted, color.a);
}
