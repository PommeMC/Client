use crate::graphics::extractor::GuiGraphicsExtractor;
use crate::graphics::state::GuiRenderState;
use crate::screen::Screen;
use crate::screens::title_screen::TitleScreen;
use crate::types::{Position, Size};

pub mod graphics;
pub mod screen;
pub mod screens;
pub mod types;

pub struct Gui {
    screen_size: Size,
    gui_scale_setting: u32,
    enforce_unicode: bool,
    gui_scale: i32,
    scaled_screen_size: Size,

    gui_render_state: GuiRenderState,

    screen: Option<Box<dyn Screen>>,
}

impl Gui {
    const MIN_WIDTH: i32 = 320;
    const MIN_HEIGHT: i32 = 240;

    pub fn new(
        screen_size: impl Into<Size>,
        gui_scale_setting: u32,
        enforce_unicode: bool,
    ) -> Self {
        let screen_size = screen_size.into();
        let gui_scale = Self::calculate_gui_scale(screen_size, gui_scale_setting, enforce_unicode);
        let scaled_screen_size = Self::calculate_scaled_screen_size(gui_scale, screen_size);

        Self {
            screen_size,
            gui_scale_setting,
            enforce_unicode,
            gui_scale,
            scaled_screen_size,

            gui_render_state: GuiRenderState::default(),

            screen: Some(Box::new(TitleScreen::default())),
        }
    }

    pub fn resize(&mut self, screen_size: impl Into<Size>) {
        self.screen_size = screen_size.into();
        self.recalculate();
    }

    /// TODO: Call when the user changes the gui scale setting (0 = auto).
    fn set_gui_scale_setting(&mut self, gui_scale_setting: u32) {
        self.gui_scale_setting = gui_scale_setting;
        self.recalculate();
    }

    /// TODO: Call when Unicode font enforcement is toggled.
    fn set_enforce_unicode(&mut self, enforce_unicode: bool) {
        self.enforce_unicode = enforce_unicode;
        self.recalculate();
    }

    fn set_screen(&mut self, screen: Option<impl Screen + 'static>) {
        self.screen = screen.map(|s| Box::new(s) as Box<dyn Screen>);
        self.recalculate();
    }

    pub fn extract_render_state(&mut self, mouse_pos: impl Into<Position>) -> &mut GuiRenderState {
        self.gui_render_state.clear();
        if let Some(screen) = self.screen.as_deref_mut() {
            let mut graphics = GuiGraphicsExtractor::new(
                &mut self.gui_render_state,
                self.gui_scale,
                mouse_pos.into(),
                self.scaled_screen_size,
            );
            screen.extract_render_state(&mut graphics, 1.0);
        }
        &mut self.gui_render_state
    }

    /// Recomputes `gui_scale` and `scaled_screen_size` from current state.
    fn recalculate(&mut self) {
        self.gui_scale = Self::calculate_gui_scale(
            self.screen_size,
            self.gui_scale_setting,
            self.enforce_unicode,
        );
        self.scaled_screen_size =
            Self::calculate_scaled_screen_size(self.gui_scale, self.screen_size);
    }

    fn calculate_max_gui_scale(screen_size: Size) -> i32 {
        (screen_size.width / Self::MIN_WIDTH)
            .min(screen_size.height / Self::MIN_HEIGHT)
            .max(1)
    }

    fn calculate_gui_scale(
        screen_size: Size,
        gui_scale_setting: u32,
        enforce_unicode: bool,
    ) -> i32 {
        let max = Self::calculate_max_gui_scale(screen_size);
        let mut scale = if gui_scale_setting == 0 {
            max
        } else {
            (gui_scale_setting as i32).min(max)
        };

        if enforce_unicode && scale % 2 != 0 {
            scale += 1;
        }

        scale.min(max)
    }

    fn calculate_scaled_screen_size(gui_scale: i32, screen_size: Size) -> Size {
        let scale = gui_scale as f64;
        let width = (screen_size.width as f64 / scale).ceil() as i32;
        let height = (screen_size.height as f64 / scale).ceil() as i32;
        Size::new(width, height)
    }
}
