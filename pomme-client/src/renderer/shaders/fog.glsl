// Distances live in the camera UBO's spare .w lanes; cylindrical metric like vanilla.
// Vanilla's linear render-distance band: the last `clamp(blocks/10, 4, 64)` blocks fade
// into the sky color, keeping the near/mid field crisp (no all-distance haze).
float fog_factor(vec3 rel, float fog_start, float fog_end) {
    float dist = max(length(rel.xz), abs(rel.y));
    float band = fog_end > fog_start ? (dist - fog_start) / (fog_end - fog_start) : 0.0;
    return clamp(band, 0.0, 1.0);
}

vec3 apply_fog(vec3 color, float fog, vec3 fog_color) {
    return mix(color, fog_color, clamp(fog, 0.0, 1.0));
}

// Shared chunk surface shading (opaque terrain and translucent water): linear
// tint and light, fade to fog while a section is still appearing, then distance
// fog. Callers supply their own alpha (cutout vs blend).
vec3 shade_chunk_surface(
    vec3 tex_rgb,
    vec3 tint,
    float light,
    float visibility,
    vec3 fog_color,
    float fog
) {
    vec3 linear_tint = pow(tint, vec3(2.2));
    float linear_light = pow(light, 2.2);
    vec3 tinted = tex_rgb * linear_tint * linear_light;
    if (visibility < 1.0) {
        tinted = mix(fog_color, tinted, visibility);
    }
    return apply_fog(tinted, fog, fog_color);
}
