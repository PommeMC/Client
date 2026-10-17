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

pub struct FpsCounter {
    frame_count: u32,
    elapsed: f32,
    display_fps: u32,
}

impl FpsCounter {
    pub const fn new() -> Self {
        Self {
            frame_count: 0,
            elapsed: 0.0,
            display_fps: 0,
        }
    }

    pub const fn update(&mut self, dt: f32) {
        self.frame_count += 1;
        self.elapsed += dt;
        if self.elapsed >= 1.0 {
            self.display_fps = self.frame_count;
            self.frame_count = 0;
            self.elapsed -= 1.0;
        }
    }

    #[inline]
    #[must_use]
    pub const fn display_fps(&self) -> u32 {
        self.display_fps
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
        quick_access_multiplayer: Option<String>,
        pending_skin_uuid: Option<uuid::Uuid>,
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

/// Where a save leads, so closing the window can wait for it too.
#[derive(Clone, Copy, PartialEq)]
pub enum AfterSaving {
    Menu,
    Quit,
}

impl AppPhase {
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
    let mut elements = Vec::new();

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

    if let Err(e) =
        gfx.renderer
            .render_menu(&gfx.window, panorama.scroll(), 2.0, elements, cursor, false)
    {
        tracing::error!("Render error: {e}");
    }

    clicked
}
