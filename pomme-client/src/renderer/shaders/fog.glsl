// Distances live in the camera UBO's spare .w lanes; cylindrical metric like vanilla.
// The hard render-distance band plus a gentle long-range haze (FOG_FALLOFF keeps the
// foreground clear) so far terrain melts into the sky instead of ending in a wall.
const float FOG_FALLOFF = 3.0;

float fog_factor(vec3 rel, float fog_start, float fog_end) {
    float dist = max(length(rel.xz), abs(rel.y));
    float band = fog_end > fog_start ? (dist - fog_start) / (fog_end - fog_start) : 0.0;
    float gradual = fog_end > 0.0 ? pow(clamp(dist / fog_end, 0.0, 1.0), FOG_FALLOFF) : 0.0;
    return clamp(max(band, gradual), 0.0, 1.0);
}

vec3 apply_fog(vec3 color, float fog, vec3 fog_color) {
    return mix(color, fog_color, clamp(fog, 0.0, 1.0));
}
