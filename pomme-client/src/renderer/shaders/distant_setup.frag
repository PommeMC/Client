#version 450

// Distant terrain's depth target starts at the near plane wherever the
// chunks drew something, so distant terrain draws only where they didn't.
// The depth alone marks those pixels, the swapchain depth having no stencil.

layout(set = 0, binding = 0) uniform texture2D frame_depth;
layout(set = 0, binding = 1) uniform sampler depth_sampler;

layout(location = 0) out vec4 out_color;

void main() {
    if (texelFetch(sampler2D(frame_depth, depth_sampler), ivec2(gl_FragCoord.xy), 0).r == 1.0) {
        discard;
    }
    gl_FragDepth = 0.0;
    out_color = vec4(0.0);
}
