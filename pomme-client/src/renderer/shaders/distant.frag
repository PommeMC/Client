#version 450

// The quad's face texture repeated per block from the model atlas, cut out,
// tinted and lit; nothing nearer than the far side of the drawn chunks'
// boxes (distant_bound.vert).

#include "distant_models.glsl"

// Every model's face textures (models.rs).
layout(set = 1, binding = 3) uniform texture2D model_atlas;
layout(set = 1, binding = 4) uniform sampler model_sampler;
layout(set = 3, binding = 1) uniform texture2D depth_bound;

layout(location = 0) flat in uvec3 v_data;
layout(location = 1) in vec2 v_uv;

layout(location = 0) out vec4 out_color;

void main() {
    uint flags = v_data.x;
    uint face = flags >> 4u & 7u;
    uint model_id = flags >> 16u;
    vec2 size = vec2(flags >> 8u & 0xFu, flags >> 12u & 0xFu);
    // The block within the quad, and the texel within its face.
    vec2 tile;
    vec2 texel = modf(v_uv, tile);
    vec2 face_origin = vec2(model_id % ATLAS_COLUMNS, model_id / ATLAS_COLUMNS) * TILE_SIZE
        + vec2(face >> 1u, face & 1u) * FACE_SIZE;
    vec2 pos = (face_origin + texel * FACE_SIZE) / ATLAS_SIZE;
    // Mipped by how fast the unwrapped coordinate moves, so the wrap from
    // block to block doesn't pick the smallest mip; taken before any
    // fragment leaves.
    vec2 unwrapped = v_uv * (FACE_SIZE / ATLAS_SIZE);
    vec2 grad_x = dFdx(unwrapped);
    vec2 grad_y = dFdy(unwrapped);
    if (any(notEqual(clamp(tile, vec2(0.0), size), tile))) {
        discard;
    }
    if (gl_FragCoord.z < texelFetch(sampler2D(depth_bound, model_sampler), ivec2(gl_FragCoord.xy), 0).r) {
        discard;
    }
    vec4 color = textureGrad(sampler2D(model_atlas, model_sampler), pos, grad_x, grad_y);
    // Cut out by the unmipped alpha, which mips would smear.
    vec4 exact = textureLod(sampler2D(model_atlas, model_sampler), pos, 0.0);
    if ((flags & 2u) != 0u) {
        if (exact.a == 0.0) {
            discard;
        }
    } else {
        color.a = 1.0;
        if ((flags & 1u) != 0u && exact.a <= 0.1) {
            discard;
        }
    }
    // Tint state 2 everywhere, 1 only on grey texels, as a grass block's
    // side.
    uint tint = flags >> 2u & 3u;
    bool grey = abs(exact.r - exact.g) < 0.02 && abs(exact.g - exact.b) < 0.02;
    if (tint == 2u || (tint == 1u && grey)) {
        color.rgb *= unpackUnorm4x8(v_data.z).rgb;
    }
    color.rgb *= unpackUnorm4x8(v_data.y).rgb;
    out_color = color;
}
