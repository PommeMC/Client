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
