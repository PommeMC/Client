#version 450

layout(set = 0, binding = 0) uniform SkyUBO {
    mat4 view_proj;
    vec4 fog_color;
};

layout(push_constant) uniform SkyDraw {
    mat4 pose;
    vec4 color;
    uint stage;
};

layout(location = 0) in vec3 in_position;
layout(location = 1) in vec2 in_uv;

layout(location = 0) out vec2 v_uv;
layout(location = 1) out float v_spherical;
layout(location = 2) out float v_cylindrical;

void main() {
    gl_Position = view_proj * pose * vec4(in_position, 1.0);
    v_uv = in_uv;
    // Vanilla `sky.vsh`'s fog distances, from the untranslated position.
    v_spherical = length(in_position);
    v_cylindrical = max(length(in_position.xz), abs(in_position.y));
}
