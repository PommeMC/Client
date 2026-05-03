#version 450

layout(set = 0, binding = 0) uniform Globals {
    float screen_w;
    float screen_h;
};

layout(location = 0) in vec2 pos;
layout(location = 1) in vec2 uv;
layout(location = 2) in vec4 color;
layout(location = 3) in float mode;
layout(location = 4) in vec2 rect_size;
layout(location = 5) in float corner_radius;

layout(location = 0) out vec2 v_uv;
layout(location = 1) out vec4 v_color;
layout(location = 2) out float v_mode;
layout(location = 3) out vec2 v_rect_size;
layout(location = 4) out float v_corner_radius;

void main() {
    gl_Position = vec4(
        pos.x / screen_w * 2.0 - 1.0,
        pos.y / screen_h * 2.0 - 1.0,
        0.0,
        1.0
    );
    v_uv = uv;
    v_color = color;
    v_mode = mode;
    v_rect_size = rect_size;
    v_corner_radius = corner_radius;
}
