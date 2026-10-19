//! What runs on the boot screen before the normal startup: updating a
//! release build to the newest one, then the first-run download of the
//! version's assets (`pomme_install`).

use std::path::PathBuf;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use winit::event_loop::ActiveEventLoop;
use winit::window::Window;

use crate::app::core::AppCore;
use crate::renderer::boot::BootScreen;

#[derive(Clone)]
enum State {
    /// The window isn't up yet, so the job hasn't started.
    Waiting,
    Running(pomme_install::Progress),
    Failed,
    Done,
    /// The update is in; this binary hands over to it.
    Updated(PathBuf),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Job {
    Update,
    Assets,
}

/// What the startup does after a frame of the boot screen.
pub enum Step {
    Running,
    /// Everything is in; the normal startup takes over the window.
    Done(Arc<Window>),
    /// A newer release replaced this one and started in its place.
    Relaunched,
}

pub struct Install {
    job: Job,
    needs_assets: bool,
    state: Arc<Mutex<State>>,
    window: Option<Arc<Window>>,
    screen: Option<BootScreen>,
}

impl Install {
    /// The boot screen's work, or `None` when the normal startup can go
    /// straight ahead. The job starts once the window is up.
    pub fn new(core: &AppCore, update: bool) -> Option<Self> {
        let needs_assets = !core.data_dirs.is_installed(&core.version);
        let job = match (update, needs_assets) {
            (true, _) => Job::Update,
            (false, true) => Job::Assets,
            (false, false) => return None,
        };
        Some(Self {
            job,
            needs_assets,
            state: Arc::new(Mutex::new(State::Waiting)),
            window: None,
            screen: None,
        })
    }

    fn start(&self, core: &AppCore) {
        *self.state.lock().unwrap() = State::Running(Default::default());
        let state = Arc::clone(&self.state);
        let report = move |progress| *state.lock().unwrap() = State::Running(progress);
        let job: Pin<Box<dyn Future<Output = State> + Send>> = match self.job {
            Job::Update => Box::pin(async move {
                match crate::update::run(&report).await {
                    Ok(Some(exe)) => State::Updated(exe),
                    Ok(None) => State::Done,
                    Err(reason) => {
                        tracing::warn!("Skipping the update: {reason}");
                        State::Done
                    }
                }
            }),
            Job::Assets => {
                let dirs = core.data_dirs.clone();
                let version = core.version.clone();
                Box::pin(async move {
                    let result = pomme_install::install(
                        crate::net::http::client(),
                        &dirs.assets_dir,
                        &dirs.versions_dir,
                        &version,
                        &report,
                    )
                    .await;
                    result.map_or_else(
                        |reason| {
                            tracing::warn!("Asset download failed: {reason}");
                            State::Failed
                        },
                        |()| State::Done,
                    )
                })
            }
        };
        let state = Arc::clone(&self.state);
        let task = core.tokio_rt.spawn(async move {
            let next = job.await;
            *state.lock().unwrap() = next;
        });
        // A panic would otherwise leave the screen waiting forever.
        let state = Arc::clone(&self.state);
        core.tokio_rt.spawn(async move {
            if task.await.is_err() {
                *state.lock().unwrap() = State::Failed;
            }
        });
    }

    /// Brings the window up, or back after Android took it, with its screen,
    /// and starts the job the first time. A download that failed while the
    /// app was away (Android freezes a backgrounded app, dropping its
    /// connections) picks up again.
    pub fn resume(&mut self, event_loop: &ActiveEventLoop, core: &AppCore) {
        self.retry(core);
        let window = match self.window.take() {
            Some(window) => window,
            None => match super::create_window(event_loop, core) {
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
        if matches!(*self.state.lock().unwrap(), State::Waiting | State::Failed) {
            self.start(core);
        }
    }

    pub fn frame(&mut self, core: &AppCore) -> Step {
        let state = self.state.lock().unwrap().clone();
        let (progress, status) = match state {
            State::Waiting => (0.0, String::new()),
            State::Running(p) => (
                if p.total == 0 {
                    0.0
                } else {
                    p.done as f32 / p.total as f32
                },
                match self.job {
                    Job::Update => format!("Updating Pomme: {}", p.status),
                    Job::Assets => format!("Minecraft {}: {}", core.version, p.status),
                },
            ),
            State::Failed => (
                0.0,
                "Download failed. Check your connection, then tap or click to retry.".to_owned(),
            ),
            State::Updated(exe) => match crate::update::relaunch(&exe) {
                Ok(()) => return Step::Relaunched,
                Err(e) => {
                    // This build still runs fine from memory, so it carries
                    // on as if there were no update.
                    tracing::error!("Couldn't start the updated client: {e}");
                    *self.state.lock().unwrap() = State::Done;
                    (0.0, String::new())
                }
            },
            State::Done if self.job == Job::Update && self.needs_assets => {
                self.job = Job::Assets;
                self.start(core);
                (0.0, String::new())
            }
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
