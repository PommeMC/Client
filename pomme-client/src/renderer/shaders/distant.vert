#version 450

// Distant terrain (renderer/distant): section-local boxes textured with
// their block's baked model (Voxy's `quads3.vert`, `quad_util.glsl`), lit
// through the lightmap and fogged like terrain.

#include "fog.glsl"
#include "camera_ubo.glsl"
#include "lightmap.glsl"
#include "distant_models.glsl"

layout(push_constant) uniform DistantPc {
    // The camera's view projection with the distant near and far planes.
    mat4 distant_view_proj;
    // The section's corner (at the dimension's min_y), relative to the
    // camera anchor, and its column width in blocks.
    vec4 origin;
};

layout(location = 0) in uint in_xz;
layout(location = 1) in uint in_y;
layout(location = 2) in uvec2 in_light_face;
layout(location = 3) in vec4 in_color;
layout(location = 4) in uint in_model;
layout(location = 5) in uvec2 in_flags;

// The tint over the texture, or with no model the colour, and its alpha.
layout(location = 0) out vec4 v_color;
layout(location = 1) out vec3 v_fog_color;
layout(location = 2) out float v_fog;
// Anchor-relative x/z, nudged half a block into the face's own column, and
// the anchor: what distant.frag finds the column's chunk from.
layout(location = 3) out vec2 v_column_pos;
layout(location = 4) flat out ivec2 v_anchor;
// A side face's step from v_column_pos into the column it faces; zero for
// tops and bottoms.
layout(location = 5) flat out vec2 v_outward;
// 1 for a seam (mesh.rs `mesh_section`).
layout(location = 6) flat out uint v_seam;
// The lightmap's colour under the face's directional shade.
layout(location = 7) out vec3 v_light;
// Where on its face texture the fragment is, a texture per column: the
// integer part steps a column, the fraction is the texel.
layout(location = 8) out vec2 v_uv;
// The face texture's corner in the model atlas, in texels.
layout(location = 9) flat out vec2 v_tile;
// The face's flags (distant_models.glsl).
layout(location = 10) flat out uint v_flags;

// Distant Horizons' micro offset: how far a corner moves out along its
// quad's plane.
const float MICRO_OFFSET = 0.01;

// mesh.rs `Face`: up, down, north, south, west, east.
const vec2 FACE_NUDGE[6] = vec2[](
    vec2(0.0), vec2(0.0),
    vec2(0.0, 0.5), vec2(0.0, -0.5),
    vec2(0.5, 0.0), vec2(-0.5, 0.0)
);
// Vanilla's directional shade (the overworld's cardinal lighting).
const float FACE_SHADE[6] = float[](1.0, 0.5, 0.8, 0.8, 0.6, 0.6);
// The face's index in its model's (mesh.rs `Face::voxy_index`).
const uint MODEL_FACE[6] = uint[](1u, 0u, 2u, 3u, 4u, 5u);
// Per model face, the block axes its texture's x and y run along
// (bakery.rs `TILE_AXES`).
const ivec2 TILE_AXES[6] = ivec2[](
    ivec2(0, 2), ivec2(0, 2), ivec2(0, 1), ivec2(0, 1), ivec2(1, 2), ivec2(1, 2)
);
// A corner's micro offset along one axis, from its 2 bits (mesh.rs
// `DistantVertex::face`).
float micro_offset(uint bits) {
    return (bits & 1u) == 0u ? 0.0 : ((bits & 2u) == 0u ? MICRO_OFFSET : -MICRO_OFFSET);
}

void main() {
    vec3 local = vec3(float(in_xz & 0xFFFFu), float(in_y), float(in_xz >> 16));
    uint face = min(in_light_face.y & 7u, 5u);
    uint model_face = MODEL_FACE[face];
    vec3 columns = local / origin.w;
    v_uv = vec2(columns[TILE_AXES[model_face].x], columns[TILE_AXES[model_face].y]);
    v_tile = vec2(in_model % ATLAS_COLUMNS, in_model / ATLAS_COLUMNS) * TILE_SIZE
        + vec2(model_face >> 1u, model_face & 1u) * FACE_SIZE;
    v_flags = in_flags.x | (in_model == 0u ? 0u : TEXTURED);

    local.xz += vec2(micro_offset(in_light_face.y >> 3), micro_offset(in_light_face.y >> 5));
    vec3 world = origin.xyz + local;
    vec3 rel = world - camera_pos.xyz;
    gl_Position = distant_view_proj * vec4(rel, 1.0);

    uint light = in_light_face.x;
    vec2 lightmap_uv = vec2(float(light & 0xFu), float(light >> 4)) * 16.0;
    v_light = sample_lightmap(lightmap_uv).rgb * FACE_SHADE[face];
    v_color = in_color;
    v_fog_color = fog_color.rgb;
    v_fog = total_fog_value(rel, fog_env, camera_pos.w, fog_color.w);
    vec2 nudge = FACE_NUDGE[face];
    v_column_pos = world.xz + nudge;
    v_anchor = camera_block.xz;
    v_outward = -2.0 * nudge;
    v_seam = in_light_face.y >> 7;
}
