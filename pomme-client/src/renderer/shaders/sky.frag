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

layout(set = 1, binding = 0) uniform sampler2D celestial_tex;

layout(push_constant) uniform PushConstants {
    uint mode;
};

layout(location = 0) in vec2 v_uv;

layout(location = 0) out vec4 out_color;

void main() {
    if (mode == 0) {
        out_color = vec4(sky_color.rgb, 1.0);
    } else if (mode == 1) {
        float b = star_brightness;
        if (b < 0.01) discard;
        out_color = vec4(b, b, b, b);
    } else if (mode == 2) {
        vec4 color = texture(celestial_tex, v_uv);
        if (color.a < 0.1) discard;
        out_color = vec4(color.rgb, color.a * celestial_alpha);
    } else if (mode == 3) {
        int phase = int(moon_phase);
        float u_off = float(phase % 4) * 0.25;
        float v_off = float(phase / 4) * 0.5;
        vec2 moon_uv = vec2(u_off + v_uv.x * 0.25, v_off + v_uv.y * 0.5);
        vec4 color = texture(celestial_tex, moon_uv);
        if (color.a < 0.1) discard;
        out_color = vec4(color.rgb, color.a * celestial_alpha * moon_brightness);
    } else if (mode == 4) {
        out_color = vec4(0.0, 0.0, 0.0, 1.0);
    } else if (mode == 5) {
        float alpha = v_uv.x * sunrise_color.a;
        if (alpha < 0.001) discard;
        out_color = vec4(sunrise_color.rgb, alpha);
    }
}
