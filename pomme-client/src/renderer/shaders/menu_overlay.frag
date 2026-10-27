#version 450

layout(set = 0, binding = 0) uniform Globals {
    float screen_w;
    float screen_h;
};

// Slot n's texture is at binding n, its sampler at binding 9 + n.
layout(set = 1, binding = 0) uniform texture2D font_tex;
layout(set = 1, binding = 9) uniform sampler font_tex_sampler;
layout(set = 1, binding = 1) uniform texture2D sprite_tex;
layout(set = 1, binding = 10) uniform sampler sprite_tex_sampler;
layout(set = 1, binding = 2) uniform texture2D item_tex;
layout(set = 1, binding = 11) uniform sampler item_tex_sampler;
layout(set = 1, binding = 3) uniform texture2DArray mc_font_tex;
layout(set = 1, binding = 12) uniform sampler mc_font_tex_sampler;
layout(set = 1, binding = 4) uniform texture2D blur_tex;
layout(set = 1, binding = 13) uniform sampler blur_tex_sampler;
layout(set = 1, binding = 5) uniform texture2D favicon_tex;
layout(set = 1, binding = 14) uniform sampler favicon_tex_sampler;
layout(set = 1, binding = 6) uniform texture2D overlay_tex;
layout(set = 1, binding = 15) uniform sampler overlay_tex_sampler;
layout(set = 1, binding = 7) uniform texture2D underwater_tex;
layout(set = 1, binding = 16) uniform sampler underwater_tex_sampler;
layout(set = 1, binding = 8) uniform texture2DArray mc_font_color_tex;
layout(set = 1, binding = 17) uniform sampler mc_font_color_tex_sampler;

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

// `tex` times the vertex colour, premultiplied for the overlay's blend.
vec4 tinted(vec4 tex) {
    float a = tex.a * v_color.a;
    return vec4(tex.rgb * v_color.rgb * a, a);
}

void main() {
    if (v_mode > 9.5) {
        // Plain premultiplied fill; color pre-converted CPU-side (sleep fade).
        out_color = v_color;
        return;
    }

    if (v_mode > 8.5) {
        // Underwater tint: standard alpha blend over the scene.
        vec4 tex = texture(sampler2D(underwater_tex, underwater_tex_sampler), v_uv);
        out_color = tinted(tex);
        return;
    }

    if (v_mode > 7.5) {
        // Vignette. Vanilla blends (ZERO, ONE_MINUS_SRC_COLOR): each channel of
        // dst is scaled by 1 - src, where src = grayscale texture * brightness.
        // Emitting (0, 0, 0, src) through the premultiplied One /
        // OneMinusSrcAlpha blend scales dst by the same 1 - src. (dst alpha is
        // scaled too, unlike vanilla; the swapchain composite ignores alpha.)
        float src = texture(sampler2D(overlay_tex, overlay_tex_sampler), v_uv).r * v_color.r;
        out_color = vec4(0.0, 0.0, 0.0, src);
        return;
    }

    if (v_mode > 6.5) {
        // Camera overlay (pumpkin blur): standard alpha blend.
        vec4 tex = texture(sampler2D(overlay_tex, overlay_tex_sampler), v_uv);
        out_color = tinted(tex);
        return;
    }

    if (v_mode > 5.5) {
        vec4 tex = texture(sampler2D(favicon_tex, favicon_tex_sampler), v_uv);
        out_color = tinted(tex);
        return;
    }

    if (v_mode > 4.5) {
        vec2 local = (v_uv - 0.5) * v_rect_size;
        vec2 half_s = v_rect_size * 0.5;
        float d = sdf_rounded_rect(local, half_s, v_corner_radius);
        float alpha = 1.0 - smoothstep(-1.0, 1.0, d);

        // The blur lives in vanilla's `main`, in bottom-up rows.
        vec2 screen_uv = gl_FragCoord.xy / vec2(screen_w, screen_h);
        vec2 main_uv = vec2(screen_uv.x, 1.0 - screen_uv.y);
        vec4 blurred = texture(sampler2D(blur_tex, blur_tex_sampler), main_uv);

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
        // Colored glyphs are mode 4.25, gray 4.0.
        if (v_mode > 4.125) {
            vec4 tex = texture(sampler2DArray(mc_font_color_tex, mc_font_color_tex_sampler), vec3(v_uv, v_rect_size.x));
            out_color = tinted(tex);
        } else {
            float coverage = texture(sampler2DArray(mc_font_tex, mc_font_tex_sampler), vec3(v_uv, v_rect_size.x)).r;
            out_color = vec4(v_color.rgb * coverage * v_color.a, coverage * v_color.a);
        }
        return;
    }

    if (v_mode > 2.5) {
        vec4 tex = texture(sampler2D(item_tex, item_tex_sampler), v_uv);
        out_color = vec4(tex.rgb * v_color.rgb * v_color.a, tex.a * v_color.a);
        return;
    }

    if (v_mode > 1.5) {
        vec4 tex = texture(sampler2D(sprite_tex, sprite_tex_sampler), v_uv);
        out_color = tinted(tex);
        return;
    }

    if (v_mode > 0.5) {
        vec4 tex = texture(sampler2D(font_tex, font_tex_sampler), v_uv);
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
