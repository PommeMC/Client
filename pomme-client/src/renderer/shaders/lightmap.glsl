// Vanilla `include/sample_lightmap.glsl`: `uv` is (block, sky) light in
// 0..240, between texel centres so the bilinear sampler blends smooth light.
layout(set = 2, binding = 0) uniform texture2D lightmap_texture;
layout(set = 2, binding = 1) uniform sampler lightmap_sampler;

vec4 sample_lightmap(vec2 uv) {
    return textureLod(
        sampler2D(lightmap_texture, lightmap_sampler),
        clamp(uv / 256.0 + 0.5 / 16.0, vec2(0.5 / 16.0), vec2(15.5 / 16.0)),
        0.0
    );
}

// Vanilla `terrain.vsh`'s `Color * sample_lightmap(Sampler2, UV2)` for a
// packed terrain vertex: block light in `light_tint.r`, sky light in the top
// byte of `uv`.
vec3 terrain_vertex_color(vec4 light_tint, uint uv) {
    vec2 light = vec2(round(light_tint.r * 255.0), float(uv >> 24));
    return light_tint.gba * sample_lightmap(light).rgb;
}
