// Vanilla `include/light.glsl` under `Lighting.Entry.LEVEL`: two world-space
// lights, the second flipped below the horizon in the nether.
vec2 level_light(vec3 normal, bool nether) {
    vec3 light0 = normalize(vec3(0.2, 1.0, -0.7));
    vec3 light1 = normalize(vec3(-0.2, nether ? -1.0 : 1.0, 0.7));
    return vec2(dot(light0, normal), dot(light1, normal));
}

// `minecraft_mix_light_separate`'s factor.
float mix_light(vec2 light) {
    vec2 value = max(vec2(0.0), light);
    return min(1.0, (value.x + value.y) * 0.6 + 0.4);
}

// `entity.vsh`'s light for a model-space `normal` at `rel` (camera to
// vertex). As with vanilla's PER_FACE_LIGHTING, a face seen from behind takes
// the flipped normal; vanilla's EMISSIVE layers (a negative light lane, see
// `light_lane_color`) take none.
float entity_diffuse(float lane, mat4 model, vec3 normal, vec3 rel, bool nether) {
    if (lane < 0.0) return 1.0;
    vec3 world_normal = normalize(transpose(inverse(mat3(model))) * normal);
    vec2 light = level_light(world_normal, nether);
    return mix_light(dot(world_normal, rel) <= 0.0 ? light : -light);
}
