// Distances live in the camera UBO's spare .w lanes; cylindrical metric like vanilla.
float fog_factor(vec3 rel, float fog_start, float fog_end) {
    float span = fog_end - fog_start;
    float dist = max(length(rel.xz), abs(rel.y));
    return span > 0.0 ? (dist - fog_start) / span : 0.0;
}

vec3 apply_fog(vec3 color, float fog, vec3 fog_color) {
    return mix(color, fog_color, clamp(fog, 0.0, 1.0));
}
