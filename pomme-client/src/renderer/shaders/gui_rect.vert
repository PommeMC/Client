#version 450

layout(location = 0) in vec2 in_pos;
layout(location = 1) in vec4 in_color;
layout(location = 2) in float in_mode;
layout(location = 3) in vec2 in_rect_size;

layout(location = 0) out vec4 out_color;
layout(location = 1) out vec2 out_local_pos;
layout(location = 2) out vec2 out_rect_size;

layout(set = 0, binding = 0) uniform Globals {
    vec2 screen_size;
};

void main() {
    vec2 ndc = (in_pos / screen_size) * 2.0 - 1.0;
    gl_Position = vec4(ndc, 0.0, 1.0);
    out_color = in_color;
    out_local_pos = in_pos;
    out_rect_size = in_rect_size;
}
