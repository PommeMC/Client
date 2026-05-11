use std::{sync::Arc, time::Instant};
use winit::window::Window;

use crate::{
    renderer::Renderer,
    window::{FpsCounter, state_slot::StateSlot},
};

pub struct Runtime {
    pub renderer: Box<Renderer>,
    pub window: Arc<Window>,
    pub last_frame: Instant,
    pub fps_counter: FpsCounter,
}

#[derive(PartialEq)]
pub enum ConnectingState {
    Connecting,
    Loading,
}

pub enum AppState {
    Setup,
    InMenu {
        runtime: Runtime,
    },
    Connecting {
        runtime: Runtime,
        state: ConnectingState,
    },
    InGame {
        runtime: Runtime,
    },
}

impl AppState {
    pub fn rt_ref(&self) -> Option<&Runtime> {
        match self {
            AppState::InMenu { runtime } => Some(runtime),
            AppState::Connecting { runtime, .. } => Some(runtime),
            AppState::InGame { runtime } => Some(runtime),
            AppState::Setup => None,
        }
    }

    pub fn rt_mut(&mut self) -> Option<&mut Runtime> {
        match self {
            AppState::InMenu { runtime } => Some(runtime),
            AppState::Connecting { runtime, .. } => Some(runtime),
            AppState::InGame { runtime } => Some(runtime),
            AppState::Setup => None,
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
