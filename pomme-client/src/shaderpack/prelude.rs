//! The GLSL Pomme wraps around a pack stage: the `pomme_*` stand-ins for
//! OpenGL's built-ins (see `glsl::translate::builtin`), filled from Pomme's
//! own vertex formats before the pack's `main` runs.
//!
//! Packs render in OpenGL's conventions: a GL projection (clip z in -1..1,
//! which the vertex `main` remaps) and images stored bottom row first, so
//! `gl_FragCoord` and texture coordinates both count up from the bottom as
//! the pack expects. Only `final` flips onto the upright swapchain.

use super::glsl::translate::Prelude;
use super::preprocess::render_stage;
use crate::renderer::chunk::mesher::{
    POS_BIAS, POS_RANGE, SPRITE_BITS, TERRAIN_UV_BITS, TERRAIN_UV_FIXED_SCALE,
};

/// The vertex formats a pack program can be fed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Geometry {
    /// `PackedVertex` plus the section's `ChunkMeta`.
    Terrain,
    /// `PackedVertex`, the section origin in a push constant (`draw_water`).
    Water,
    /// `EntityVertex` plus the entity renderer's per-instance data.
    Entity,
    /// `EntityVertex` with the block-entity push constants.
    BlockEntity,
    /// `SkyVertex` with the sky push block (`SkyPipeline::draws`).
    Sky,
    /// A full-screen triangle (`deferred`, `composite`).
    FullScreen,
    /// `final`, flipped onto the upright swapchain.
    Final,
}

/// Frame data the preludes read, set 0 binding 1; `PackFrame` on the Rust side.
const FRAME: &str = "
layout(std140, set = 0, binding = 1) uniform PommeFrame {
    mat4 pomme_view;
    mat4 pomme_projection;
    vec4 pomme_eye;
    ivec4 pomme_anchor;
    vec4 pomme_atlas_size;
    // x: fog start, y: fog end, in blocks; zw unused.
    vec4 pomme_fog;
    vec4 pomme_fog_color;
};
";

/// The built-in matrices and `renderStage`, both stages.
const MATRICES: &str = "
int pomme_render_stage;
mat4 pomme_ModelViewMatrix;
mat4 pomme_ModelViewMatrixInverse;
mat4 pomme_ProjectionMatrix;
mat4 pomme_ProjectionMatrixInverse;
mat4 pomme_ModelViewProjectionMatrix;
mat3 pomme_NormalMatrix;
mat4 pomme_TextureMatrix[2];

// The fixed-function fog `gl_Fog` reported.
struct pomme_FogParameters {
    vec4 color;
    float density;
    float start;
    float end;
    float scale;
};
pomme_FogParameters pomme_Fog;

void pomme_matrices(mat4 model_view, mat4 projection) {
    pomme_ModelViewMatrix = model_view;
    pomme_ModelViewMatrixInverse = inverse(model_view);
    pomme_ProjectionMatrix = projection;
    pomme_ProjectionMatrixInverse = inverse(projection);
    pomme_ModelViewProjectionMatrix = projection * model_view;
    pomme_NormalMatrix = transpose(inverse(mat3(model_view)));
    pomme_TextureMatrix[0] = mat4(1.0);
    // OptiFine's lightmap matrix: 0..240 light coordinates to texel centres.
    pomme_TextureMatrix[1] = mat4(
        vec4(1.0 / 256.0, 0.0, 0.0, 0.0),
        vec4(0.0, 1.0 / 256.0, 0.0, 0.0),
        vec4(0.0, 0.0, 1.0 / 256.0, 0.0),
        vec4(1.0 / 32.0, 1.0 / 32.0, 1.0 / 32.0, 1.0));
    pomme_Fog = pomme_FogParameters(
        pomme_fog_color, 1.0, pomme_fog.x, pomme_fog.y, 1.0 / max(pomme_fog.y - pomme_fog.x, 1e-4));
}

// Iris's composite projection: the 0..1 square onto clip space.
const mat4 pomme_screen_projection = mat4(
    vec4(2.0, 0.0, 0.0, 0.0),
    vec4(0.0, 2.0, 0.0, 0.0),
    vec4(0.0, 0.0, 1.0, 0.0),
    vec4(-1.0, -1.0, 0.0, 1.0));
";

/// Built-in functions naga's GLSL frontend lacks, both stages.
const POLYFILLS: &str = "
float pomme_modf(float x, out float i) { i = trunc(x); return x - i; }
vec2 pomme_modf(vec2 x, out vec2 i) { i = trunc(x); return x - i; }
vec3 pomme_modf(vec3 x, out vec3 i) { i = trunc(x); return x - i; }
vec4 pomme_modf(vec4 x, out vec4 i) { i = trunc(x); return x - i; }
";

/// The vertex inputs' stand-ins.
const VERTEX_INPUTS: &str = "
vec4 pomme_Vertex;
vec3 pomme_Normal;
vec4 pomme_Color;
vec4 pomme_MultiTexCoord0;
vec4 pomme_MultiTexCoord1;
vec4 pomme_MultiTexCoord2;

vec4 pomme_ftransform() {
    return pomme_ModelViewProjectionMatrix * pomme_Vertex;
}

// Light lanes (`LightLevels::lane`): block + sky * 256, each 0..240.
vec4 pomme_light_coords(float lane) {
    return vec4(mod(lane, 256.0), floor(lane / 256.0), 0.0, 1.0);
}
";

/// The pack's `gl_Position` from GL clip space to Vulkan's 0..1 depth.
const GL_DEPTH: &str = "    gl_Position.z = (gl_Position.z + gl_Position.w) * 0.5;\n";

pub struct Preludes {
    vertex_decls: String,
    vertex_main: String,
    fragment_decls: String,
    fragment_main: String,
}

impl Geometry {
    /// Whether a `gbuffers_*` program draws it, rather than a full-screen pass.
    pub fn is_geometry(self) -> bool {
        !matches!(self, Self::FullScreen | Self::Final)
    }
}

impl Preludes {
    pub fn new(geometry: Geometry) -> Self {
        let (inputs, setup, model_view) = match geometry {
            Geometry::Terrain => (terrain_inputs(TERRAIN_ORIGIN), TERRAIN_SETUP, "pomme_view"),
            Geometry::Water => (terrain_inputs(WATER_ORIGIN), TERRAIN_SETUP, "pomme_view"),
            Geometry::Entity => (ENTITY_INPUTS.to_owned(), ENTITY_SETUP, ENTITY_MODEL_VIEW),
            Geometry::BlockEntity => (
                BLOCK_ENTITY_INPUTS.to_owned(),
                ENTITY_SETUP,
                ENTITY_MODEL_VIEW,
            ),
            Geometry::Sky => (
                format!("{SKY_DRAW}{SKY_INPUTS}"),
                SKY_SETUP,
                "pomme_view * pomme_sky_pose",
            ),
            Geometry::FullScreen | Geometry::Final => {
                (String::new(), FULL_SCREEN_SETUP, "mat4(1.0)")
            }
        };
        let stage = match geometry {
            Geometry::Sky => "int(pomme_sky_stage)".to_owned(),
            other => render_stage(match other {
                // TODO: cutout terrain reads TERRAIN_CUTOUT.
                Geometry::Terrain => "TERRAIN_SOLID",
                Geometry::Water => "TERRAIN_TRANSLUCENT",
                Geometry::Entity => "ENTITIES",
                Geometry::BlockEntity => "BLOCK_ENTITIES",
                _ => "NONE",
            })
            .to_string(),
        };
        let stage = format!("    pomme_render_stage = {stage};\n");
        let fragment_inputs = if geometry == Geometry::Sky {
            SKY_DRAW
        } else {
            ""
        };
        let (fragment_view, projection) = if geometry.is_geometry() {
            ("pomme_view", "pomme_projection")
        } else {
            ("mat4(1.0)", "pomme_screen_projection")
        };
        let flip = if geometry == Geometry::Final {
            "    gl_Position.y = -gl_Position.y;\n"
        } else {
            ""
        };
        Self {
            vertex_decls: format!(
                "{FRAME}{MATRICES}{POLYFILLS}{VERTEX_INPUTS}{EYE_OFFSET}{inputs}"
            ),
            // Iris reads the lightmap coordinate as TexCoord2 too.
            vertex_main: format!(
                "\nvoid main() {{\n{stage}{setup}    pomme_MultiTexCoord2 = pomme_MultiTexCoord1;\n    pomme_matrices({model_view}, {projection});\n    pack_main();\n{GL_DEPTH}{flip}}}\n"
            ),
            fragment_decls: format!("{FRAME}{MATRICES}{POLYFILLS}{fragment_inputs}"),
            fragment_main: format!(
                "\nvoid main() {{\n{stage}    pomme_matrices({fragment_view}, {projection});\n    pack_main();\n}}\n"
            ),
        }
    }

    pub fn vertex(&self) -> Prelude<'_> {
        Prelude {
            decls: &self.vertex_decls,
            main: &self.vertex_main,
        }
    }

    pub fn fragment(&self) -> Prelude<'_> {
        Prelude {
            decls: &self.fragment_decls,
            main: &self.fragment_main,
        }
    }
}

/// The translation from the render anchor to the eye: Pomme's positions are
/// anchor-relative, the pack's model-view matrix eye-relative.
const EYE_OFFSET: &str = "
mat4 pomme_eye_offset() {
    mat4 m = mat4(1.0);
    m[3] = vec4(-pomme_eye.xyz, 1.0);
    return m;
}
";

/// Where a terrain vertex's section origin comes from: the section meta
/// for the indirect passes, a push constant for `draw_water`.
const TERRAIN_ORIGIN: &str = "
layout(location = 5) in ivec3 pomme_in_origin;
layout(location = 6) in float pomme_in_visibility;

vec3 pomme_section_origin() {
    return vec3(pomme_in_origin - pomme_anchor.xyz);
}
";

const WATER_ORIGIN: &str = "
layout(push_constant) uniform PommeSection {
    vec4 pomme_origin_fade;
};

vec3 pomme_section_origin() {
    return pomme_origin_fade.xyz;
}
";

fn terrain_inputs(origin: &str) -> String {
    format!(
        "
const float POMME_POS_RANGE = {POS_RANGE:?};
const float POMME_POS_BIAS = {POS_BIAS:?};
const float POMME_UV_SCALE = {TERRAIN_UV_FIXED_SCALE:?};
const uint POMME_UV_BITS = {TERRAIN_UV_BITS}u;
const uint POMME_SPRITE_BITS = {SPRITE_BITS}u;

layout(location = 0) in vec2 pomme_in_pos_xy;
layout(location = 1) in float pomme_in_pos_z;
layout(location = 2) in uint pomme_in_uv;
layout(location = 3) in uint pomme_in_sprite;
layout(location = 4) in vec4 pomme_in_light_tint;
{origin}
// Level-0 sprite rectangles in atlas texels, as the atlas uploads them.
layout(std430, set = 2, binding = 0) readonly buffer PommeSprites {{
    uvec4 pomme_sprite_rects[];
}};

// `Direction` order (down, up, north, south, west, east), after 0 = none.
const vec3 POMME_FACE_NORMALS[7] = vec3[7](
    vec3(0.0, 1.0, 0.0),
    vec3(0.0, -1.0, 0.0),
    vec3(0.0, 1.0, 0.0),
    vec3(0.0, 0.0, -1.0),
    vec3(0.0, 0.0, 1.0),
    vec3(-1.0, 0.0, 0.0),
    vec3(1.0, 0.0, 0.0));
"
    )
}

const TERRAIN_SETUP: &str = "
    vec3 local = vec3(pomme_in_pos_xy, pomme_in_pos_z) * POMME_POS_RANGE - POMME_POS_BIAS;
    vec3 rel = pomme_section_origin() - pomme_eye.xyz + local;
    pomme_Vertex = vec4(rel, 1.0);
    uint sprite = pomme_in_sprite & ((1u << POMME_SPRITE_BITS) - 1u);
    pomme_Normal = POMME_FACE_NORMALS[min(pomme_in_sprite >> POMME_SPRITE_BITS, 6u)];
    uint mask = (1u << POMME_UV_BITS) - 1u;
    vec2 sprite_uv = vec2(pomme_in_uv & mask, (pomme_in_uv >> POMME_UV_BITS) & mask) / POMME_UV_SCALE;
    uvec4 rect = pomme_sprite_rects[sprite];
    vec2 atlas_uv = (vec2(rect.xy) + sprite_uv * vec2(rect.zw)) / pomme_atlas_size.xy;
    pomme_MultiTexCoord0 = vec4(atlas_uv, 0.0, 1.0);
    pomme_MultiTexCoord1 = vec4(round(pomme_in_light_tint.r * 255.0), float(pomme_in_uv >> 24), 0.0, 1.0);
    // TODO: vanilla's face shade is baked in here; Iris drops it for packs.
    pomme_Color = vec4(pomme_in_light_tint.gba, 1.0);
";

const ENTITY_INPUTS: &str = "
layout(location = 0) in vec3 pomme_in_position;
layout(location = 1) in vec2 pomme_in_tex_coords;
layout(location = 2) in vec4 pomme_in_normal;
layout(location = 3) in vec4 pomme_in_model_0;
layout(location = 4) in vec4 pomme_in_model_1;
layout(location = 5) in vec4 pomme_in_model_2;
layout(location = 6) in vec4 pomme_in_model_3;
layout(location = 7) in vec4 pomme_in_tint;
layout(location = 8) in vec4 pomme_in_overlay;
layout(location = 9) in vec4 pomme_in_uv;

mat4 pomme_model() {
    return mat4(pomme_in_model_0, pomme_in_model_1, pomme_in_model_2, pomme_in_model_3);
}
";

const BLOCK_ENTITY_INPUTS: &str = "
layout(location = 0) in vec3 pomme_in_position;
layout(location = 1) in vec2 pomme_in_tex_coords;
layout(location = 2) in vec4 pomme_in_normal;

layout(push_constant) uniform PommeDraw {
    mat4 pomme_draw_model;
    vec4 pomme_in_tint;
    vec4 pomme_in_overlay;
    vec4 pomme_in_uv;
};

mat4 pomme_model() {
    return pomme_draw_model;
}
";

/// An entity's model-view: `pomme_model()` reads the instance attributes or
/// the block-entity push constants.
const ENTITY_MODEL_VIEW: &str = "pomme_view * pomme_eye_offset() * pomme_model()";

const ENTITY_SETUP: &str = "
    pomme_Vertex = vec4(pomme_in_position, 1.0);
    pomme_Normal = pomme_in_normal.xyz;
    pomme_Color = pomme_in_tint;
    pomme_MultiTexCoord0 = vec4(pomme_in_tex_coords + pomme_in_uv.xy, 0.0, 1.0);
    // Emissive layers come unlit; Iris draws them full bright.
    pomme_MultiTexCoord1 = pomme_in_uv.w < 0.0
        ? vec4(240.0, 240.0, 0.0, 1.0)
        : pomme_light_coords(pomme_in_uv.w);
";

/// The sky push block, both stages: `SkyPush` on the Rust side.
const SKY_DRAW: &str = "
layout(push_constant) uniform PommeSky {
    mat4 pomme_sky_pose;
    vec4 pomme_sky_color;
    uint pomme_sky_stage;
};
";

const SKY_INPUTS: &str = "
layout(location = 0) in vec3 pomme_in_position;
layout(location = 1) in vec2 pomme_in_tex_coords;
";

// Vanilla's sky formats carry no normal or light; the sunset fan's
// vertex alpha ramps from its centre (`build_sunrise_fan`'s uv.x).
const SKY_SETUP: &str = "
    pomme_Vertex = vec4(pomme_in_position, 1.0);
    pomme_Normal = vec3(0.0, 0.0, 1.0);
    pomme_Color = pomme_sky_color;
    // MC_RENDER_STAGE_SUNSET
    if (pomme_sky_stage == 2u) {
        pomme_Color.a *= pomme_in_tex_coords.x;
    }
    pomme_MultiTexCoord0 = vec4(pomme_in_tex_coords, 0.0, 1.0);
    pomme_MultiTexCoord1 = vec4(240.0, 240.0, 0.0, 1.0);
";

const FULL_SCREEN_SETUP: &str = "
    vec2 uv = vec2((gl_VertexIndex << 1) & 2, gl_VertexIndex & 2);
    pomme_Vertex = vec4(uv, 0.0, 1.0);
    pomme_Normal = vec3(0.0, 0.0, 1.0);
    pomme_Color = vec4(1.0);
    pomme_MultiTexCoord0 = vec4(uv, 0.0, 1.0);
    pomme_MultiTexCoord1 = vec4(240.0, 240.0, 0.0, 1.0);
";
