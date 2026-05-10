use std::sync::Arc;
use winit::window::Window;

use crate::renderer::Renderer;

pub struct Runtime {
    pub renderer: Box<Renderer>,
    pub window: Arc<Window>,
}

#[derive(PartialEq, Eq)]
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
    Quitting,
}
