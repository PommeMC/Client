#version 450

layout(location = 0) in vec4 in_color;
layout(location = 1) in vec2 in_local_pos;
layout(location = 2) in vec2 in_rect_size;

layout(location = 0) out vec4 frag_color;

void main() {
    frag_color = in_color;
}
