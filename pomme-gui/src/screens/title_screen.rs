use crate::graphics::extractor::GuiGraphicsExtractor;
use crate::graphics::state::Rect;
use crate::screen::Screen;
use crate::types::Color;

#[derive(Default)]
pub struct TitleScreen {}

impl Screen for TitleScreen {
    fn extract_render_state(&mut self, graphics: &mut GuiGraphicsExtractor, alpha: f32) {
        graphics.push_rect(Rect {
            x: 0,
            y: 0,
            width: graphics.screen_width,
            height: graphics.screen_height,
            color: Color::WHITE,
        });
    }
}
