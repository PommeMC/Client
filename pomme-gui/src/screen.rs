use crate::graphics::extractor::GuiGraphicsExtractor;

pub trait Screen {
    fn extract_render_state(&mut self, graphics: &mut GuiGraphicsExtractor, alpha: f32);
}
