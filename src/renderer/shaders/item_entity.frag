#version 450

layout(set = 1, binding = 0) uniform sampler2D atlas_texture;

layout(location = 0) in vec2 v_tex_coords;
layout(location = 1) in float v_light;
layout(location = 2) in vec3 v_tint;

layout(location = 0) out vec4 out_color;

vec3 linear_to_srgb(vec3 c) {
    return pow(c, vec3(1.0 / 2.2));
}

vec3 srgb_to_linear(vec3 c) {
    return pow(c, vec3(2.2));
}

void main() {
    vec4 color = texture(atlas_texture, v_tex_coords);
    if (color.a < 0.5) discard;
    vec3 srgb = linear_to_srgb(color.rgb);
    vec3 tinted = srgb * v_tint * v_light;
    out_color = vec4(srgb_to_linear(tinted), color.a);
}
