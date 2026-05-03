#version 450

layout(set = 0, binding = 0) uniform HandUniform {
    mat4 mvp;
};

layout(location = 0) in vec3 position;
layout(location = 1) in vec2 tex_coords;

layout(location = 0) out vec2 v_uv;

void main() {
    gl_Position = mvp * vec4(position, 1.0);
    v_uv = tex_coords;
}
