#version 450

// Voxy's `gl46/cull/raster.frag`: a fragment that passes the depth test
// marks its section seen this frame.

#define DISTANT_SET 0
#define VISIBILITY_BINDING 2
#include "distant_section.glsl"

layout(early_fragment_tests) in;

layout(location = 0) flat in uint v_id;
layout(location = 1) flat in uint v_value;

void main() {
    visibility_data[v_id] = v_value;
}
