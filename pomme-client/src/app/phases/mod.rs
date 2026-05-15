use std::mem::ManuallyDrop;
use std::sync::Arc;
use std::time::Instant;

use winit::window::Window;

use crate::app::phases::in_game::GameState;
use crate::app::state_slot::StateSlot;
use crate::net::connection::ConnectionHandle;
use crate::renderer::Renderer;

pub mod connecting;
pub mod in_game;
pub mod in_menu;

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
    },
    InGame {
        gfx: Gfx,
        connection: ConnectionHandle,
        game: GameState,
    },
}

impl AppPhase {
    pub fn gfx_mut(&mut self) -> Option<&mut Gfx> {
        match self {
            AppPhase::Setup { .. } => None,
            AppPhase::InMenu { gfx, .. } => Some(gfx),
            AppPhase::Connecting { gfx, .. } => Some(gfx),
            AppPhase::InGame { gfx, .. } => Some(gfx),
        }
    }
}

impl StateSlot<AppPhase> {
    pub fn gfx_mut(&mut self) -> Option<&mut Gfx> {
        self.get_mut().gfx_mut()
    }
}
