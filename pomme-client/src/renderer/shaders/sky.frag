#version 450

layout(set = 0, binding = 0) uniform SkyUBO {
    mat4 view_proj;
    vec4 fog_color;
};

layout(set = 1, binding = 0) uniform texture2D celestial_tex;
layout(set = 1, binding = 1) uniform sampler celestial_tex_sampler;

layout(push_constant) uniform SkyDraw {
    mat4 pose;
    vec4 color;
    uint stage;
};

layout(location = 0) in vec2 v_uv;
layout(location = 1) in float v_spherical;
layout(location = 2) in float v_cylindrical;

layout(location = 0) out vec4 out_color;

#include "fog.glsl"

// `SkyStage`: Iris's render stages.
const uint SUNSET = 2u;
const uint CUSTOM_SKY = 3u;
const uint SUN = 4u;
const uint MOON = 5u;
const uint STARS = 6u;

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
    if (stage == SUN || stage == MOON) {
        vec4 tex = texture(sampler2D(celestial_tex, celestial_tex_sampler), v_uv);
        if (tex.a == 0.0) discard;
        out_color = tex * color;
    } else if (stage == CUSTOM_SKY) {
        out_color = texture(sampler2D(celestial_tex, celestial_tex_sampler), v_uv) * color;
    } else if (stage == SUNSET) {
        // The fan's centre vertex is opaque, its ring clear.
        float alpha = v_uv.x * color.a;
        if (alpha < 0.001) discard;
        out_color = vec4(color.rgb, alpha);
    } else if (stage == STARS) {
        // Additive, alpha-weighted (b, b, b, b): b squared.
        out_color = vec4(color.rgb * color.a, 1.0);
    } else {
        out_color = vec4(fogged(color.rgb), 1.0);
    }
}
