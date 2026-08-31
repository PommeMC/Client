//! Title / subtitle overlay: the big centered fade-in text driven by the
//! `/title` command (vanilla `Hud.extractTitle` and the title fields around
//! it).

use crate::renderer::pipelines::menu_overlay::MenuElement;
use crate::ui::common::FONT_SIZE;
use crate::ui::text::{TextSpan, with_alpha};

pub struct TitleState {
    title: Option<Vec<TextSpan>>,
    subtitle: Option<Vec<TextSpan>>,
    /// Remaining ticks; inactive at <= 0.
    title_time: i32,
    fade_in: i32,
    stay: i32,
    fade_out: i32,
}

impl Default for TitleState {
    fn default() -> Self {
        Self {
            title: None,
            subtitle: None,
            title_time: 0,
            fade_in: 10,
            stay: 70,
            fade_out: 20,
        }
    }
}

impl TitleState {
    fn total_time(&self) -> i32 {
        self.fade_in + self.stay + self.fade_out
    }

    pub fn set_title(&mut self, spans: Vec<TextSpan>) {
        self.title = Some(spans);
        self.title_time = self.total_time();
    }

    /// Stashes only; a subtitle never shows without an active title.
    pub fn set_subtitle(&mut self, spans: Vec<TextSpan>) {
        self.subtitle = Some(spans);
    }

    /// Negative fields keep their current value; an active countdown restarts
    /// at the new total (vanilla `Hud.setTimes`).
    pub fn set_times(&mut self, fade_in: i32, stay: i32, fade_out: i32) {
        if fade_in >= 0 {
            self.fade_in = fade_in;
        }
        if stay >= 0 {
            self.stay = stay;
        }
        if fade_out >= 0 {
            self.fade_out = fade_out;
        }
        if self.title_time > 0 {
            self.title_time = self.total_time();
        }
    }

    pub fn clear(&mut self, reset_times: bool) {
        if reset_times {
            *self = Self::default();
        } else {
            self.title = None;
            self.subtitle = None;
            self.title_time = 0;
        }
    }

    pub fn tick(&mut self) {
        if self.title_time > 0 {
            self.title_time -= 1;
            if self.title_time <= 0 {
                self.title = None;
                self.subtitle = None;
            }
        }
    }

    pub fn build(
        &self,
        elements: &mut Vec<MenuElement>,
        screen_w: f32,
        screen_h: f32,
        gs: f32,
        partial_tick: f32,
    ) {
        let Some(title) = &self.title else { return };
        if self.title_time <= 0 {
            return;
        }

        let t = self.title_time as f32 - partial_tick;
        let mut alpha = 255;
        if self.title_time > self.fade_out + self.stay {
            alpha = if self.fade_in > 0 {
                ((self.total_time() as f32 - t) * 255.0 / self.fade_in as f32) as i32
            } else {
                255
            };
        }
        if self.title_time <= self.fade_out {
            alpha = if self.fade_out > 0 {
                (t * 255.0 / self.fade_out as f32) as i32
            } else {
                0
            };
        }
        let alpha = alpha.clamp(0, 255) as f32 / 255.0;
        if alpha <= 0.0 {
            return;
        }

        let cx = (screen_w / 2.0).round();
        let cy = (screen_h / 2.0).round();
        // Vanilla draws at local y -10 under a 4x scale about the center, and
        // the subtitle at local y 5 under a 2x scale.
        let mut push = |spans: &[TextSpan], y_off: f32, size: f32| {
            elements.push(MenuElement::McText {
                x: cx,
                y: cy + y_off * gs,
                spans: with_alpha(spans, alpha),
                scale: FONT_SIZE * gs * size,
                centered: true,
                shadow: true,
            });
        };
        push(title, -40.0, 4.0);
        if let Some(subtitle) = &self.subtitle {
            push(subtitle, 10.0, 2.0);
        }
    }
}
