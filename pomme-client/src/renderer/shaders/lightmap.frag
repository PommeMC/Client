#version 450

// Vanilla `core/lightmap.fsh`: one texel per block (x) and sky (y) light
// level. Texel rows run top-down here and bottom-up in vanilla, as do the
// rows terrain samples, so `gl_FragCoord` stands in for `texCoord * 16`.

layout(push_constant) uniform LightmapInfo {
    vec4 block_light_tint;   // w: BlockFactor
    vec4 sky_light_color;    // w: SkyFactor
    vec4 ambient_color;      // w: BrightnessFactor
    vec4 night_vision_color; // w: NightVisionFactor
    float darkness_scale;
    float boss_overlay_world_darkening_factor;
};

layout(location = 0) out vec4 frag_color;

float get_brightness(float level) {
    return level / (4.0 - 3.0 * level);
}

vec3 not_gamma(vec3 color) {
    float max_component = max(max(color.x, color.y), color.z);
    float max_inverted = 1.0 - max_component;
    float max_scaled = 1.0 - max_inverted * max_inverted * max_inverted * max_inverted;
    return color * (max_scaled / max_component);
}

float parabolic_mix_factor(float level) {
    return (2.0 * level - 1.0) * (2.0 * level - 1.0);
}

void main() {
    float block_level = floor(gl_FragCoord.x) / 15.0;
    float sky_level = floor(gl_FragCoord.y) / 15.0;

    float block_brightness = get_brightness(block_level) * block_light_tint.w;
    float sky_brightness = get_brightness(sky_level) * sky_light_color.w;

    vec3 night_vision = night_vision_color.rgb * night_vision_color.w;
    vec3 color = max(ambient_color.rgb, night_vision);

    color += sky_light_color.rgb * sky_brightness;

    vec3 block_light_color = mix(block_light_tint.rgb, vec3(1.0), 0.9 * parabolic_mix_factor(block_level));
    color += block_light_color * block_brightness;

    color = mix(color, color * vec3(0.7, 0.6, 0.6), boss_overlay_world_darkening_factor);

    color = color - vec3(darkness_scale);

    color = clamp(color, 0.0, 1.0);
    color = mix(color, not_gamma(color), ambient_color.w);

    frag_color = vec4(color, 1.0);
}
