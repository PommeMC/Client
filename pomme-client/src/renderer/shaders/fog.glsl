// Vanilla's two-band fog (core include/fog.glsl): the render-distance band
// rides in the camera UBO's spare .w lanes (cylindrical, the last
// `clamp(blocks/10, 4, 64)` blocks), the RD-independent environmental band in
// `fog_env.xy` (spherical, the ambient haze); whichever is denser wins.
float linear_fog_value(float dist, float fog_start, float fog_end) {
    if (dist <= fog_start) {
        return 0.0;
    }
    if (dist >= fog_end) {
        return 1.0;
    }
    return (dist - fog_start) / (fog_end - fog_start);
}

float total_fog_value(vec3 rel, vec4 fog_env, float rd_start, float rd_end) {
    float spherical = length(rel);
    float cylindrical = max(length(rel.xz), abs(rel.y));
    return max(
        linear_fog_value(spherical, fog_env.x, fog_env.y),
        linear_fog_value(cylindrical, rd_start, rd_end)
    );
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
