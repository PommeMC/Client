#version 450

layout(set = 0, binding = 0) uniform Globals {
    float screen_w;
    float screen_h;
};

layout(set = 1, binding = 0) uniform sampler2D font_tex;
layout(set = 1, binding = 1) uniform sampler2D sprite_tex;
layout(set = 1, binding = 2) uniform sampler2D item_tex;
layout(set = 1, binding = 3) uniform sampler2D mc_font_tex;
layout(set = 1, binding = 4) uniform sampler2D blur_tex;
layout(set = 1, binding = 5) uniform sampler2D favicon_tex;

layout(location = 0) in vec2 v_uv;
layout(location = 1) in vec4 v_color;
layout(location = 2) in float v_mode;
layout(location = 3) in vec2 v_rect_size;
layout(location = 4) in float v_corner_radius;

layout(location = 0) out vec4 out_color;

float sdf_rounded_rect(vec2 p, vec2 half_size, float radius) {
    vec2 q = abs(p) - half_size + vec2(radius);
    return min(max(q.x, q.y), 0.0) + length(max(q, vec2(0.0))) - radius;
}

void main() {
    if (v_mode > 5.5) {
        vec4 tex = texture(favicon_tex, v_uv);
        out_color = vec4(tex.rgb * tex.a * v_color.a, tex.a * v_color.a);
        return;
    }

    if (v_mode > 4.5) {
        vec2 local = (v_uv - 0.5) * v_rect_size;
        vec2 half_s = v_rect_size * 0.5;
        float d = sdf_rounded_rect(local, half_s, v_corner_radius);
        float alpha = 1.0 - smoothstep(-1.0, 1.0, d);

        vec2 screen_uv = gl_FragCoord.xy / vec2(screen_w, screen_h);
        vec4 blurred = texture(blur_tex, screen_uv);

        vec3 tinted = blurred.rgb * v_color.rgb;
        float a = v_color.a * alpha;

        float border_band = smoothstep(-2.0, 0.0, d) * alpha;
        vec4 border_color = vec4(1.0, 1.0, 1.0, 0.06);

        vec4 col = vec4(tinted * a, a);
        col = col + border_color * border_band * (1.0 - col.a);

        out_color = col;
        return;
    }

    if (v_mode > 3.5) {
        vec4 tex = texture(mc_font_tex, v_uv);
        vec3 linear_color = pow(v_color.rgb, vec3(2.2));
        out_color = vec4(linear_color * tex.a * v_color.a, tex.a * v_color.a);
        return;
    }

    if (v_mode > 2.5) {
        vec4 tex = texture(item_tex, v_uv);
        out_color = vec4(tex.rgb * v_color.rgb * tex.a * v_color.a, tex.a * v_color.a);
        return;
    }

    if (v_mode > 1.5) {
        vec4 tex = texture(sprite_tex, v_uv);
        out_color = vec4(tex.rgb * v_color.rgb * tex.a * v_color.a, tex.a * v_color.a);
        return;
    }

    if (v_mode > 0.5) {
        vec4 tex = texture(font_tex, v_uv);
        out_color = vec4(v_color.rgb * tex.a, v_color.a * tex.a);
        return;
    }

    vec2 local = (v_uv - 0.5) * v_rect_size;
    vec2 half_s = v_rect_size * 0.5;
    float d = sdf_rounded_rect(local, half_s, v_corner_radius);

    float alpha = 1.0 - smoothstep(-1.0, 1.0, d);

    float border_band = smoothstep(-2.0, 0.0, d) * alpha;
    vec4 border_color = vec4(0.12, 0.12, 0.12, 0.12);

    vec4 premul = vec4(v_color.rgb * v_color.a, v_color.a);
    vec4 col = premul * alpha;
    col = col + border_color * border_band * (1.0 - col.a);

    out_color = col;
}
