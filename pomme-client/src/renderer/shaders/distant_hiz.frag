#version 450

// Voxy's `hiz/blit.fsh`: each level of the Hi-Z holds the farthest of the
// four texels under it in the level before (level 0 reads the distant
// image's depth), so a box whose nearest point lies beyond every texel it
// covers is hidden. Voxy also turns the other texels to the near plane
// where one is at the far plane; under the max that changes nothing, so it
// isn't done here.

layout(set = 0, binding = 0) uniform texture2D source;
layout(set = 0, binding = 1) uniform sampler source_sampler;

layout(location = 0) in vec2 v_uv;
layout(location = 0) out vec4 out_depth;

void main() {
    vec4 depths = textureGather(sampler2D(source, source_sampler), v_uv, 0);
    out_depth = vec4(max(max(depths.x, depths.y), max(depths.z, depths.w)));
}
