#version 450

layout(set = 0, binding = 0) uniform SkyUBO {
    mat4 view_proj;
    vec4 sky_color;
    vec4 sunrise_color;
    vec4 fog_color;
    float sun_angle;
    float moon_angle;
    float star_angle;
    float star_brightness;
    float celestial_alpha;
};

layout(set = 1, binding = 0) uniform texture2D celestial_tex;
layout(set = 1, binding = 1) uniform sampler celestial_tex_sampler;

layout(push_constant) uniform PushConstants {
    uint mode;
};

layout(location = 0) in vec2 v_uv;
layout(location = 1) in float v_spherical;
layout(location = 2) in float v_cylindrical;

layout(location = 0) out vec4 out_color;

#include "fog.glsl"

// Vanilla `sky.fsh`: faded to the fog colour out to `FogSkyEnd`.
vec3 fogged(vec3 color) {
    float end = fog_color.w;
    float fog = max(
        linear_fog_value(v_spherical, 0.0, end),
        linear_fog_value(v_cylindrical, end, end)
    );
    return mix(color, fog_color.rgb, fog);
}

void main() {
    if (mode == 0) {
        out_color = vec4(fogged(sky_color.rgb), 1.0);
    } else if (mode == 1) {
        // Additive, alpha-weighted (b, b, b, b): b squared.
        float b = star_brightness;
        out_color = vec4(vec3(b * b), 1.0);
    } else if (mode == 2) {
        vec4 color = texture(sampler2D(celestial_tex, celestial_tex_sampler), v_uv);
        if (color.a < 0.1) discard;
        out_color = vec4(color.rgb, color.a * celestial_alpha);
    } else if (mode == 3) {
        // Each phase is its own texture, its quad's uvs turned half a turn.
        vec4 color = texture(sampler2D(celestial_tex, celestial_tex_sampler), 1.0 - v_uv);
        if (color.a < 0.1) discard;
        out_color = vec4(color.rgb, color.a * celestial_alpha);
    } else if (mode == 4) {
        out_color = vec4(fogged(vec3(0.0)), 1.0);
    } else if (mode == 5) {
        float alpha = v_uv.x * sunrise_color.a;
        if (alpha < 0.001) discard;
        out_color = vec4(sunrise_color.rgb, alpha);
    } else if (mode == 6) {
        // Vanilla's end sky: the texture times grey 0x28.
        vec4 color = texture(sampler2D(celestial_tex, celestial_tex_sampler), v_uv);
        out_color = vec4(color.rgb * (40.0 / 255.0), color.a);
    }
}
