// Vanilla `include/sample_lightmap.glsl`: `uv` is (block, sky) light in
// 0..240, between texel centres so the bilinear sampler blends smooth light.
// Set 2 unless the includer picks another.
#ifndef LIGHTMAP_SET
#define LIGHTMAP_SET 2
#endif
layout(set = LIGHTMAP_SET, binding = 0) uniform texture2D lightmap_texture;
layout(set = LIGHTMAP_SET, binding = 1) uniform sampler lightmap_sampler;

vec4 sample_lightmap(vec2 uv) {
    return textureLod(
        sampler2D(lightmap_texture, lightmap_sampler),
        clamp(uv / 256.0 + 0.5 / 16.0, vec2(0.5 / 16.0), vec2(15.5 / 16.0)),
        0.0
    );
}

// A light lane (`LightLevels::lane`): block + sky * 256, each 0..240. A
// negative lane is one of vanilla's EMISSIVE layers, which skip the lightmap.
vec4 light_lane_color(float lane) {
    if (lane < 0.0) return vec4(1.0);
    return sample_lightmap(vec2(mod(lane, 256.0), floor(lane / 256.0)));
}

// Vanilla `terrain.vsh`'s `Color * sample_lightmap(Sampler2, UV2)` for a
// packed terrain vertex: block light in `light_tint.r`, sky light in the top
// byte of `uv`.
vec3 terrain_vertex_color(vec4 light_tint, uint uv) {
    vec2 light = vec2(round(light_tint.r * 255.0), float(uv >> 24));
    return light_tint.gba * sample_lightmap(light).rgb;
}
