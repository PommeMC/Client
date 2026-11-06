//! Sign text: vanilla `AbstractSignRenderer.submitSignText` with
//! `StandingSignRenderer`'s and `HangingSignRenderer`'s text transforms.

use glam::{DVec3, Mat4, Vec3};

use crate::position::BlockPos;
use crate::renderer::chunk::block_lighter::LightLevels;
use crate::renderer::pipelines::world_text::{
    MAX_WORLD_TEXT_VERTS, WorldTextVertices, push_spans, spans_advance, spans_width,
};
use crate::ui::common::rgb;
use crate::ui::font::GlyphMap;
use crate::ui::text::{TextSpan, font_spans, format_component_spans};
use crate::world::block_entity::{SignBlock, SignText};

/// A sign to draw (vanilla `SignRenderState`).
pub struct SignRenderInfo {
    pub pos: BlockPos,
    pub block: SignBlock,
    pub light: LightLevels,
    /// Front, then back.
    pub faces: [SignFace; 2],
}

/// One face's text, styled over its ink colour.
pub struct SignFace {
    lines: [Vec<TextSpan>; 4],
    glowing: bool,
    /// The dark colour drawn around glowing text, when shown.
    outline: Option<[f32; 4]>,
}

impl SignFace {
    /// `outline_visible` is `AbstractSignRenderer.isOutlineVisible`.
    pub fn new(text: &SignText, outline_visible: bool) -> Self {
        let ink = rgb(text.ink_color());
        Self {
            lines: std::array::from_fn(|i| format_component_spans(&text.messages[i], ink)),
            glowing: text.has_glowing_text,
            outline: (text.has_glowing_text && (text.is_black() || outline_visible))
                .then(|| rgb(text.dark_color())),
        }
    }
}

/// The eight one-pixel offsets `Font`'s outline mode draws behind the text.
const OUTLINE_OFFSETS: [(f32, f32); 8] = [
    (-1.0, -1.0),
    (0.0, -1.0),
    (1.0, -1.0),
    (-1.0, 0.0),
    (1.0, 0.0),
    (-1.0, 1.0),
    (0.0, 1.0),
    (1.0, 1.0),
];

pub fn push_signs(
    verts: &mut WorldTextVertices,
    anchor: DVec3,
    glyph_map: &GlyphMap,
    signs: &[SignRenderInfo],
) {
    let verts = &mut verts.depth_tested;
    for sign in signs {
        let origin = Mat4::from_translation((sign.pos.corner() - anchor).as_vec3());
        let block = sign.block;
        let line_height = block.text_line_height();
        let midpoint = 4.0 * line_height / 2.0;
        for (face, front) in sign.faces.iter().zip([true, false]) {
            let m = origin * text_transformation(block, front);
            let light = if face.glowing {
                LightLevels::FULL_BRIGHT
            } else {
                sign.light
            }
            .lane();
            for (i, line) in face.lines.iter().enumerate() {
                if verts.len() + 128 > MAX_WORLD_TEXT_VERTS {
                    return;
                }
                let line = font_spans(line);
                let width = |spans: &[TextSpan]| spans_advance(spans, glyph_map);
                let shown = crate::ui::chat::wrap_spans(&line, block.max_text_line_width(), &width)
                    .swap_remove(0);
                let x1 = -((spans_width(&shown, glyph_map) as i32 / 2) as f32);
                let y = i as f32 * line_height - midpoint;
                // The outline copies first, then the text over them.
                let outline = face
                    .outline
                    .into_iter()
                    .flat_map(|color| OUTLINE_OFFSETS.map(|offset| (offset, Some(color))));
                for ((dx, dy), color) in outline.chain([((0.0, 0.0), None)]) {
                    let at =
                        |x: f32, ty: f32| m.transform_point3(Vec3::new(x + dx, y + ty + dy, 0.0));
                    let color = |span: &TextSpan| color.unwrap_or(span.color);
                    push_spans(verts, glyph_map, &shown, x1, &at, &color, light);
                }
            }
        }
    }
}

/// `StandingSignRenderer.textTransformation` and
/// `HangingSignRenderer.textTransformation`, block-local.
fn text_transformation(block: SignBlock, front: bool) -> Mat4 {
    let flip = if front {
        Mat4::IDENTITY
    } else {
        Mat4::from_rotation_y(180f32.to_radians())
    };
    let yaw = Mat4::from_rotation_y((-block.angle).to_radians());
    let (base, offset, scale) = if block.hanging {
        (
            Mat4::from_translation(Vec3::new(0.5, 0.9375, 0.5))
                * yaw
                * Mat4::from_translation(Vec3::new(0.0, -0.3125, 0.0)),
            Vec3::new(0.0, -0.32, 0.073),
            0.0140625,
        )
    } else {
        let wall = if block.wall {
            Mat4::from_translation(Vec3::new(0.0, -0.3125, -0.4375))
        } else {
            Mat4::IDENTITY
        };
        (
            Mat4::from_translation(Vec3::splat(0.5)) * yaw * wall,
            Vec3::new(0.0, 0.33333334, 0.046666667),
            0.010416667,
        )
    };
    base * flip * Mat4::from_translation(offset) * Mat4::from_scale(Vec3::new(scale, -scale, scale))
}
