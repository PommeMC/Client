use std::{sync::Arc, time::Instant};
use winit::window::Window;

use crate::{
    net::connection::ConnectionHandle,
    renderer::Renderer,
    window::{FpsCounter, game::GameState, state_slot::StateSlot},
};

#[derive(PartialEq)]
pub enum ConnectionPhase {
    Connecting,
    Loading,
}

pub enum AppPhase {
    Setup {
        quick_access_multiplayer: Option<String>,
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
    },
    InGame {
        gfx: Gfx,
        connection: ConnectionHandle,
        game: GameState,
    },
}

impl AppPhase {
    pub fn gfx_ref(&self) -> Option<&Gfx> {
        match self {
            AppPhase::Setup { .. } => None,
            AppPhase::InMenu { gfx, .. } => Some(gfx),
            AppPhase::Connecting { gfx, .. } => Some(gfx),
            AppPhase::InGame { gfx, .. } => Some(gfx),
        }
    }

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
    pub fn gfx_ref(&self) -> Option<&Gfx> {
        self.get().gfx_ref()
    }

    pub fn gfx_mut(&mut self) -> Option<&mut Gfx> {
        self.get_mut().gfx_mut()
    }
}

pub struct Gfx {
    // Keep Renderer above Window, it must be dropped first
    pub renderer: Renderer,
    pub window: Arc<Window>,
    pub last_frame: Instant,
    pub fps_counter: FpsCounter,
}

pub struct Panorama {
    scroll: f32,
}

impl Panorama {
    pub fn new() -> Self {
        Self { scroll: 0.0 }
    }

    pub fn update(&mut self, dt: f32) {
        self.scroll += dt * 0.00556;
        if self.scroll > 1.0 {
            self.scroll -= 1.0;
        }
    }

    #[must_use]
    pub fn scroll(&self) -> f32 {
        self.scroll
    }
}
