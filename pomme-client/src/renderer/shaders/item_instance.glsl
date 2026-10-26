// The item pipelines' per-instance data (binding 1), as item_entity.rs's
// `ItemInstance` lays it out.
layout(location = 5) in mat4 model;
layout(location = 9) in mat3 normal_matrix;
layout(location = 12) in vec2 instance_light;
layout(location = 13) in uint item_tint_base;
layout(location = 14) in uint item_tint_count;
layout(location = 15) in uint outline_color;
