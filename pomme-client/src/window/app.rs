use std::{sync::Arc, time::Instant};
use winit::window::Window;

use crate::{
    net::connection::ConnectionHandle,
    renderer::Renderer,
    window::{FpsCounter, state_slot::StateSlot},
};

#[derive(PartialEq)]
pub enum ConnectingState {
    Connecting,
    Loading,
}

pub enum AppState {
    Setup {
        quick_access_multiplayer: Option<String>,
    },
    InMenu {
        runtime: Runtime,
    },
    Connecting {
        runtime: Runtime,
        connect_state: ConnectingState,
        connection: ConnectionHandle,
    },
    InGame {
        runtime: Runtime,
        connection: ConnectionHandle,
    },
}

impl AppState {
    pub fn rt_ref(&self) -> Option<&Runtime> {
        match self {
            AppState::Setup { .. } => None,
            AppState::InMenu { runtime } => Some(runtime),
            AppState::Connecting { runtime, .. } => Some(runtime),
            AppState::InGame { runtime, .. } => Some(runtime),
        }
    }

    pub fn rt_mut(&mut self) -> Option<&mut Runtime> {
        match self {
            AppState::Setup { .. } => None,
            AppState::InMenu { runtime } => Some(runtime),
            AppState::Connecting { runtime, .. } => Some(runtime),
            AppState::InGame { runtime, .. } => Some(runtime),
        }
    }
}

impl StateSlot<AppState> {
    pub fn rt_ref(&self) -> Option<&Runtime> {
        self.get().rt_ref()
    }

    pub fn rt_mut(&mut self) -> Option<&mut Runtime> {
        self.get_mut().rt_mut()
    }
}

pub struct Runtime {
    pub renderer: Box<Renderer>,
    pub window: Arc<Window>,
    pub last_frame: Instant,
    pub fps_counter: FpsCounter,
}
