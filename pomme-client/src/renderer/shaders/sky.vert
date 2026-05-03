#version 450

layout(set = 0, binding = 0) uniform SkyUBO {
    mat4 view_proj;
    vec4 sky_color;
    vec4 sunrise_color;
    float sun_angle;
    float moon_angle;
    float star_angle;
    float star_brightness;
    float celestial_alpha;
    float moon_brightness;
    float moon_phase;
    float _pad;
};

layout(push_constant) uniform PushConstants {
    uint mode;
};

layout(location = 0) in vec3 in_position;
layout(location = 1) in vec2 in_uv;

layout(location = 0) out vec2 v_uv;

vec3 celestial_rotate(vec3 p, float angle) {
    float ca = cos(angle);
    float sa = sin(angle);
    return vec3(-(p.y * sa + p.z * ca), p.y * ca - p.z * sa, p.x);
}

void main() {
    vec3 pos = in_position;

    if (mode == 1) {
        pos = celestial_rotate(pos, star_angle);
    } else if (mode == 2) {
        pos = celestial_rotate(pos, sun_angle);
    } else if (mode == 3) {
        pos = celestial_rotate(pos, moon_angle);
    } else if (mode == 5) {
        float s = sin(sun_angle) < 0.0 ? -1.0 : 1.0;
        pos = vec3(
            -in_position.y * s,
            -in_position.z,
            in_position.x * s * sunrise_color.a
        );
    }

    gl_Position = view_proj * vec4(pos, 1.0);
    v_uv = in_uv;
}
