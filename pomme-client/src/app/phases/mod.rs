use std::collections::VecDeque;
use std::mem::ManuallyDrop;
use std::sync::Arc;
use std::time::Instant;

use winit::window::Window;

use crate::app::core::AppCore;
use crate::app::phases::in_game::GameState;
use crate::app::state_slot::StateSlot;
use crate::net::connection::ConnectionHandle;
use crate::renderer::Renderer;
use crate::renderer::pipelines::menu_overlay::MenuElement;
use crate::singleplayer::World;
use crate::ui::common;

pub mod connecting;
pub mod in_game;
pub mod in_menu;
pub mod saving;

pub struct Gfx {
    // Renderer must be dropped before window, as it holds Vulkan resources that require the window
    // surface to still be alive during cleanup. We do that manually in the `Drop` impl of this
    // struct.
    pub renderer: ManuallyDrop<Renderer>,
    // Window does not require `ManuallyDrop` because it is dropped normally after the renderer by
    // the compiler.
    pub window: Arc<Window>,
    pub last_frame: Instant,
    pub fps_counter: FpsCounter,
}

impl Drop for Gfx {
    fn drop(&mut self) {
        // SAFETY: called inside `drop`, so no code can access `renderer` after this.
        unsafe {
            ManuallyDrop::drop(&mut self.renderer);
        }
    }
}

pub struct Panorama {
    scroll: f32,
}

impl Panorama {
    pub const fn new() -> Self {
        Self { scroll: 0.0 }
    }

    #[inline]
    pub const fn update(&mut self, dt: f32) {
        self.scroll += dt * 0.00556;
        if self.scroll > 1.0 {
            self.scroll -= 1.0;
        }
    }

    #[inline]
    #[must_use]
    pub const fn scroll(&self) -> f32 {
        self.scroll
    }
}

const FPS_WINDOW_SECS: f32 = 2.0;

pub struct FpsCounter {
    frame_count: u32,
    elapsed: f32,
    display_fps: u32,
    window_time: f32,
    frame_times: VecDeque<f32>,
    scratch: Vec<f32>,
    low_1pct_fps: u32,
    low_01pct_fps: u32,
}

impl FpsCounter {
    pub fn new() -> Self {
        Self {
            frame_count: 0,
            elapsed: 0.0,
            display_fps: 0,
            window_time: 0.0,
            frame_times: VecDeque::with_capacity(512),
            scratch: Vec::with_capacity(512),
            low_1pct_fps: 0,
            low_01pct_fps: 0,
        }
    }

    pub fn update(&mut self, dt: f32) {
        self.frame_count += 1;
        self.elapsed += dt;

        self.window_time += dt;
        self.frame_times.push_back(dt);
        while self.window_time > FPS_WINDOW_SECS && self.frame_times.len() > 1 {
            if let Some(old) = self.frame_times.pop_front() {
                self.window_time -= old;
            }
        }

        if self.elapsed >= 1.0 {
            self.display_fps = self.frame_count;
            self.frame_count = 0;
            self.elapsed -= 1.0;
            self.recompute_lows();
        }
    }

    fn recompute_lows(&mut self) {
        let count = self.frame_times.len();
        if count == 0 {
            self.low_1pct_fps = self.display_fps;
            self.low_01pct_fps = self.display_fps;
            return;
        }

        self.scratch.clear();
        self.scratch.extend(&self.frame_times);
        self.scratch.sort_unstable_by(|a, b| b.total_cmp(a));

        let count_1pct = ((count as f64 * 0.01).ceil() as usize).max(1).min(count);
        let sum_1pct: f32 = self.scratch[..count_1pct].iter().sum();
        let avg_1pct = sum_1pct / count_1pct as f32;
        self.low_1pct_fps = if avg_1pct > 0.0 {
            ((1.0 / avg_1pct).round() as u32).min(self.display_fps)
        } else {
            0
        };

        let count_01pct = ((count as f64 * 0.001).ceil() as usize).max(1).min(count);
        let sum_01pct: f32 = self.scratch[..count_01pct].iter().sum();
        let avg_01pct = sum_01pct / count_01pct as f32;
        self.low_01pct_fps = if avg_01pct > 0.0 {
            ((1.0 / avg_01pct).round() as u32).min(self.low_1pct_fps)
        } else {
            0
        };
    }

    #[inline]
    #[must_use]
    pub const fn display_fps(&self) -> u32 {
        self.display_fps
    }

    #[inline]
    #[must_use]
    pub const fn stats(&self) -> (u32, u32, u32) {
        (self.display_fps, self.low_1pct_fps, self.low_01pct_fps)
    }
}

impl Default for FpsCounter {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(PartialEq)]
pub enum ConnectionPhase {
    /// Waiting on the integrated server. Singleplayer only.
    StartingWorld,
    Connecting,
    Loading,
}

pub enum AppPhase {
    Setup {
        quick_play: Option<crate::app::QuickPlay>,
        pending_skin_uuid: Option<uuid::Uuid>,
        /// The window and its renderer, once `resumed` has started loading
        /// them.
        loading: Option<Box<Loading>>,
    },
    InMenu {
        gfx: Gfx,
        panorama: Panorama,
    },
    Connecting {
        gfx: Gfx,
        panorama: Panorama,
        connect_phase: ConnectionPhase,
        connection: ConnectionHandle,
        game: GameState,
        /// The integrated server, when this is a singleplayer session.
        world: Option<World>,
    },
    InGame {
        gfx: Gfx,
        connection: ConnectionHandle,
        game: GameState,
        world: Option<World>,
    },
    /// Waiting on the integrated server to finish saving. Singleplayer only.
    SavingWorld {
        gfx: Gfx,
        panorama: Panorama,
        world: World,
        then: AfterSaving,
    },
}

/// A window showing the splash while its renderer loads.
pub struct Loading {
    /// Declared before `window`, so its Vulkan resources go before the
    /// surface's window does.
    pub boot: crate::renderer::RendererBoot,
    pub window: Arc<Window>,
    pub next_frame: Instant,
}

/// Where a save leads, so closing the window can wait for it too.
#[derive(Clone, Copy, PartialEq)]
pub enum AfterSaving {
    Menu,
    Quit,
}

impl AppPhase {
    /// In a world with no screen, chat or pause menu over it.
    pub fn world_has_pointer(&self) -> bool {
        matches!(self, AppPhase::InGame { game, .. } if game.world_has_pointer())
    }

    pub fn gfx_mut(&mut self) -> Option<&mut Gfx> {
        match self {
            AppPhase::Setup { .. } => None,
            AppPhase::InMenu { gfx, .. } => Some(gfx),
            AppPhase::Connecting { gfx, .. } => Some(gfx),
            AppPhase::InGame { gfx, .. } => Some(gfx),
            AppPhase::SavingWorld { gfx, .. } => Some(gfx),
        }
    }
}

impl StateSlot<AppPhase> {
    pub fn gfx_mut(&mut self) -> Option<&mut Gfx> {
        self.get_mut().gfx_mut()
    }
}

/// Panorama, blur and one centred line. With a button this is vanilla's
/// `ConnectScreen` (status at `height / 2 - 50`, the button at
/// `height / 4 + 132`); without, `GenericMessageScreen` (the line at
/// `height / 2 - lineHeight / 2`). Returns whether the button was clicked.
pub fn draw_status(
    core: &mut AppCore,
    dt: f32,
    gfx: &mut Gfx,
    panorama: &mut Panorama,
    text: &str,
    button: Option<&str>,
) -> bool {
    panorama.update(dt);

    let sw = gfx.renderer.screen_width() as f32;
    let sh = gfx.renderer.screen_height() as f32;
    let gs = core.menu.gui_scale(sw, sh);
    let fs = common::FONT_SIZE * gs;
    let cx = sw / 2.0;
    let cy = sh / 2.0;

    let cursor = core.input.cursor_pos();
    let mut elements = vec![MenuElement::BlurBackground];

    elements.push(MenuElement::Text {
        x: cx,
        y: cy - if button.is_some() { 50.0 } else { 4.0 } * gs,
        text: text.into(),
        scale: fs,
        color: common::WHITE,
        centered: true,
    });

    let mut clicked = false;
    if let Some(label) = button {
        clicked = common::push_button(
            &mut elements,
            cursor,
            cx - 100.0 * gs,
            sh / 4.0 + 132.0 * gs,
            200.0 * gs,
            common::BTN_H * gs,
            gs,
            fs,
            label,
            true,
        ) && core.input.left_just_pressed();
    }

    core.input.clear_just_pressed_actions();

    if let Err(e) = gfx
        .renderer
        .render_menu(&gfx.window, panorama.scroll(), elements, cursor, None)
    {
        tracing::error!("Render error: {e}");
    }

    clicked
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fps_counter_calculates_average_and_lows() {
        let mut counter = FpsCounter::new();
        for _ in 0..99 {
            counter.update(0.01);
        }
        counter.update(0.011);
        assert_eq!(counter.display_fps(), 100);
        assert_eq!(counter.stats(), (100, 91, 91));
    }

    #[test]
    fn fps_counter_detects_frametime_stutters() {
        let mut counter = FpsCounter::new();
        for _ in 0..99 {
            counter.update(0.009);
        }
        counter.update(0.109);
        let (fps, low_1, low_01) = counter.stats();
        assert_eq!(fps, 100);
        assert!(low_1 < 100);
        assert!(low_01 <= low_1);
    }
}
