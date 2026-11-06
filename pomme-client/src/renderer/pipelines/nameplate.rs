use glam::DVec3;

use crate::renderer::chunk::block_lighter::LightLevels;
use crate::renderer::pipelines::world_text::{
    MAX_WORLD_TEXT_VERTS, WorldTextVertices, push_quad, push_spans, spans_width,
};
use crate::ui::font::GlyphMap;
use crate::ui::text::{TextSpan, font_spans};

pub const NAME_TAG_SCALE: f32 = 0.025;

/// A name tag to draw: vanilla `SubmitNodeCollection.submitNameTag`.
#[derive(Clone, Debug)]
pub struct NameplateRenderInfo {
    /// Its anchor, the middle of the text's top edge.
    pub position: DVec3,
    pub spans: Vec<TextSpan>,
    /// A sneaking entity's tag, drawn faint and only where not hidden.
    pub discrete: bool,
    /// The entity's light (`getPackedLightCoords`).
    pub light: LightLevels,
}

/// A see-through tag's dimmed copy with its backdrop, drawn over terrain,
/// then the depth-tested text.
pub fn push_nameplates(
    verts: &mut WorldTextVertices,
    camera: &crate::renderer::camera::Camera,
    camera_render_pos: DVec3,
    anchor: DVec3,
    glyph_map: &GlyphMap,
    nameplates: &[NameplateRenderInfo],
) {
    let (right, up) = camera.billboard_axes();
    let forward = camera.forward();
    let away = forward * 0.002;

    for plate in nameplates {
        if verts.len() + 128 > MAX_WORLD_TEXT_VERTS {
            break;
        }
        let to_cam = (camera_render_pos - plate.position).as_vec3();
        if to_cam.dot(forward) >= 0.0 {
            continue;
        }

        let spans = font_spans(&plate.spans);
        let text_w = spans_width(&spans, glyph_map);
        if text_w <= 0.0 {
            continue;
        }
        let start_x = -text_w / 2.0;
        let center = (plate.position - anchor).as_vec3();
        // Text-space (x right, y down) to world space.
        let at = |x: f32, y: f32| center + (right * x - up * y) * NAME_TAG_SCALE;

        let light = plate.light.lane();
        // `LightCoordsUtil.lightCoordsWithEmission(light, 2)`.
        let emissive = LightLevels::new(plate.light.sky(), plate.light.block().max(2)).lane();

        // `Font`'s background: one pixel around the text, a line high.
        let push_bg = |verts: &mut Vec<_>, alpha: f32| {
            let corners = [
                at(start_x - 1.0, -1.0) + away,
                at(start_x + text_w + 1.0, -1.0) + away,
                at(start_x + text_w + 1.0, 9.0) + away,
                at(start_x - 1.0, 9.0) + away,
            ];
            push_quad(
                verts,
                corners,
                [0.0; 4],
                0.0,
                -1.0,
                [0.0, 0.0, 0.0, alpha],
                light,
            );
        };
        let push_text = |verts: &mut Vec<_>, alpha: f32, light: f32| {
            let faded = |span: &TextSpan| {
                let mut color = span.color;
                color[3] *= alpha;
                color
            };
            push_spans(verts, glyph_map, &spans, start_x, &at, &faded, light);
        };

        // `SubmitNodeCollection.submitNameTag` at the default text
        // background opacity, 0.25, so text alpha 0.5.
        // TODO: the Text Background option, once Pomme has it.
        if plate.discrete {
            push_bg(&mut verts.depth_tested, 0.25);
            push_text(&mut verts.depth_tested, 0.5, light);
        } else {
            push_bg(&mut verts.see_through, 0.25);
            push_text(&mut verts.see_through, 0.5, light);
            push_text(&mut verts.depth_tested, 1.0, emissive);
        }
    }
}
