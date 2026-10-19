#version 450
#extension GL_EXT_multiview : require

layout(set = 0, binding = 0) uniform HandUniform {
    // One per eye in a multiview (VR) pass.
    mat4 mvps[2];
};

layout(location = 0) in vec3 position;
layout(location = 1) in vec2 tex_coords;

layout(location = 0) out vec2 v_uv;

void main() {
    gl_Position = mvps[gl_ViewIndex] * vec4(position, 1.0);
    v_uv = tex_coords;
}
