// The item pipelines' push constants, as item_entity.rs writes them, shared
// by every item stage.
layout(push_constant) uniform PushConstants {
    mat4 model;
    float world_light;
    float nether_lighting;
    uint item_tint_base;
    uint item_tint_count;
    mat3 normal_matrix;
};
