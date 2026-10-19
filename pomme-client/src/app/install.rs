//! The first-run download of the version's assets (`pomme_install`) and its
//! screen, run before the normal startup can build anything from them.

use std::sync::{Arc, Mutex};

use winit::event_loop::ActiveEventLoop;
use winit::window::Window;

use crate::app::core::AppCore;
use crate::renderer::boot::BootScreen;

enum State {
    Running(pomme_install::Progress),
    Failed,
    Done,
}

/// What the startup does after a frame of the download.
pub enum Step {
    Running,
    /// The assets are in; the normal startup takes over the window.
    Done(Arc<Window>),
}

pub struct Install {
    state: Arc<Mutex<State>>,
    window: Option<Arc<Window>>,
    screen: Option<BootScreen>,
}

impl Install {
    pub fn start(core: &AppCore) -> Self {
        let install = Self {
            state: Arc::new(Mutex::new(State::Running(Default::default()))),
            window: None,
            screen: None,
        };
        install.download(core);
        install
    }

    fn download(&self, core: &AppCore) {
        *self.state.lock().unwrap() = State::Running(Default::default());
        let state = Arc::clone(&self.state);
        let dirs = core.data_dirs.clone();
        let version = core.version.clone();
        let task = core.tokio_rt.spawn(async move {
            let result = pomme_install::install(
                crate::net::http::client(),
                &dirs.assets_dir,
                &dirs.versions_dir,
                &version,
                &|progress| *state.lock().unwrap() = State::Running(progress),
            )
            .await;
            *state.lock().unwrap() = match result {
                Ok(()) => State::Done,
                Err(reason) => {
                    tracing::warn!("Asset download failed: {reason}");
                    State::Failed
                }
            };
        });
        // A panic would otherwise leave the screen waiting forever.
        let state = Arc::clone(&self.state);
        core.tokio_rt.spawn(async move {
            if task.await.is_err() {
                *state.lock().unwrap() = State::Failed;
            }
        });
    }

    /// Brings the window up, or back after Android took it, with its screen.
    /// A download that failed while the app was away (Android freezes a
    /// backgrounded app, dropping its connections) picks up again.
    pub fn resume(&mut self, event_loop: &ActiveEventLoop, core: &AppCore) {
        self.retry(core);
        let window = match self.window.take() {
            Some(window) => window,
            None => match super::create_window(event_loop, core.display_mode) {
                Ok(window) => window,
                Err(e) => {
                    tracing::error!("Failed to create window: {e}");
                    event_loop.exit();
                    return;
                }
            },
        };
        match BootScreen::new(&window, core.font_sources(), core.menu.vsync) {
            Ok(screen) => self.screen = Some(screen),
            Err(e) => {
                tracing::error!("Failed to create the download screen: {e}");
                event_loop.exit();
            }
        }
        window.request_redraw();
        self.window = Some(window);
    }

    #[cfg(target_os = "ios")]
    pub fn window(&self) -> Option<&Window> {
        self.window.as_deref()
    }

    pub fn suspend(&mut self) {
        self.screen = None;
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        if let Some(screen) = &mut self.screen
            && let Err(e) = screen.resize(width, height)
        {
            tracing::warn!("Download screen resize failed: {e}");
        }
    }

    /// A click or tap retries a failed download.
    pub fn retry(&self, core: &AppCore) {
        if matches!(*self.state.lock().unwrap(), State::Failed) {
            self.download(core);
        }
    }

    pub fn frame(&mut self, version: &str) -> Step {
        let (progress, status) = match &*self.state.lock().unwrap() {
            State::Running(p) => (
                if p.total == 0 {
                    0.0
                } else {
                    p.done as f32 / p.total as f32
                },
                format!("Minecraft {version}: {}", p.status),
            ),
            State::Failed => (
                0.0,
                "Download failed. Check your connection, then tap or click to retry.".to_owned(),
            ),
            State::Done => {
                // The full renderer makes its own surface on this window.
                self.screen = None;
                if let Some(window) = self.window.take() {
                    return Step::Done(window);
                }
                return Step::Running;
            }
        };
        if let Some(screen) = &mut self.screen
            && let Err(e) = screen.draw(progress, &status)
        {
            tracing::warn!("Download screen draw failed: {e}");
        }
        if let Some(window) = &self.window {
            window.request_redraw();
        }
        Step::Running
    }
}
