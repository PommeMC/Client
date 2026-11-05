#version 450

// Distant terrain's image laid into the frame, its depth carried over from
// the distant projection to the camera's so what's drawn after still sorts
// against it, fogged like the chunks and faded out over the last tenth of
// its distance.

#include "camera_ubo.glsl"
#include "fog.glsl"

layout(push_constant) uniform BlitPc {
    // The inverse of the view projection distant terrain drew with.
    mat4 inverse_distant_view_proj;
    // The fade: `alpha *= 1 - saturate(distance * fade.y + fade.x)`.
    vec4 fade;
};

layout(set = 1, binding = 0) uniform texture2D distant_color;
layout(set = 1, binding = 1) uniform texture2D distant_depth;
layout(set = 1, binding = 2) uniform sampler distant_sampler;

layout(location = 0) in vec2 v_uv;

layout(location = 0) out vec4 out_color;

void main() {
    ivec2 pixel = ivec2(gl_FragCoord.xy);
    float depth = texelFetch(sampler2D(distant_depth, distant_sampler), pixel, 0).r;
    // 0 where the chunks drew (distant_setup.frag), 1 where nothing did.
    if (depth == 0.0 || depth == 1.0) {
        discard;
    }
    vec4 point = inverse_distant_view_proj * vec4(v_uv * 2.0 - 1.0, depth, 1.0);
    vec3 rel = point.xyz / point.w;
    vec4 clip = view_proj * vec4(rel, 1.0);
    // Kept just inside the far plane, where the chunks' depth clears to.
    gl_FragDepth = min(clip.z / clip.w, 1.0 - 2.0 / 16777215.0);

    vec4 color = texelFetch(sampler2D(distant_color, distant_sampler), pixel, 0);
    if (color.a == 0.0) {
        discard;
    }
    float fog = total_fog_value(rel, fog_env, camera_pos.w, fog_color.w);
    color.rgb = apply_fog(color.rgb, fog, fog_color.rgb);
    color.a *= 1.0 - clamp(length(rel.xz) * fade.y + fade.x, 0.0, 1.0);
    out_color = color;
}
