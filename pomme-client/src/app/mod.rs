mod copper_golem;
pub mod core;
pub mod input;
mod install;
pub mod level_load;
pub mod phases;
mod sniffer;
pub mod state_slot;

use std::mem::ManuallyDrop;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use thiserror::Error;
use winit::application::ApplicationHandler;
use winit::event::{DeviceEvent, DeviceId, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::window::{Window, WindowId};

use crate::app::core::AppCore;
use crate::app::input::Action;
use crate::app::input::touch::TouchLayout;
use crate::app::phases::connecting::{ConnectingUpdateResult, update_connecting};
use crate::app::phases::in_game::{GameState, GameUpdateResult, start_chunk_load, update_game};
use crate::app::phases::in_menu::{self, MenuUpdateResult, update_menu};
use crate::app::phases::saving::update_saving;
use crate::app::phases::{AfterSaving, AppPhase, ConnectionPhase, FpsCounter, Gfx, Panorama};
use crate::app::state_slot::StateSlot;
use crate::dirs::DataDirs;
use crate::net::connection::{ConnectArgs, ConnectionHandle, spawn_connection};
use crate::renderer::util::MAX_RESOURCE_IMAGE_BYTES;
use crate::renderer::{self, Renderer};
use crate::singleplayer::World;
use crate::user::UserData;

#[derive(Error, Debug)]
pub enum WindowError {
    #[error("failed to create event loop: {0}")]
    EventLoop(#[from] winit::error::EventLoopError),

    #[error("failed to create window: {0}")]
    CreateWindow(#[from] winit::error::OsError),

    #[error("renderer error: {0}")]
    Renderer(#[from] renderer::RendererError),
}

pub(crate) const TICK_RATE: f32 = 1.0 / 20.0;
const TICK_RATE_MS: u32 = 1000 / 20;
const POSITION_SEND_INTERVAL: u32 = 20;
const POSITION_THRESHOLD_SQ: f64 = 4.0e-8;

pub struct App {
    phase: StateSlot<AppPhase>,
    core: AppCore,
    occluded: bool,
    /// In the background on a phone; the loop sleeps instead of polling.
    suspended: bool,
    fps_limiter: FramerateLimiter,
    /// The version's assets are still downloading; startup waits for them.
    install: Option<install::Install>,
    /// The download screen's window, for the normal startup to keep.
    installed_window: Option<Arc<Window>>,
    /// Whether the phone's soft keyboard was last asked for.
    #[cfg(mobile)]
    soft_keyboard: bool,
    #[cfg(feature = "test-harness")]
    harness: Option<crate::harness::Harness>,
    chunk_bench: Option<ChunkBenchLaunch>,
}

/// Where a `--chunk-bench` run is.
#[derive(Clone, Copy)]
enum ChunkBenchLaunch {
    /// The render distance to benchmark, until in game.
    Waiting(u32),
    Running,
    /// The process exit code.
    Ended(i32),
}

/// Port of vanilla `FramerateLimiter`: paces to a target fps by sleeping most
/// of the wait (with an adaptive margin for oversleep) then spinning the rest.
struct FramerateLimiter {
    last_frame: Instant,
    average_overshoot_ns: u64,
    last_limit: u32,
}

impl FramerateLimiter {
    const MAX_CURRENT_OVERSHOOT_NS: u64 = 25_000_000;
    const MAX_AVERAGE_OVERSHOOT_NS: u64 = 2_000_000;
    const SPIN_SAFETY_BUFFER_NS: u64 = 500_000;

    fn new() -> Self {
        Self {
            last_frame: Instant::now(),
            average_overshoot_ns: 0,
            last_limit: 0,
        }
    }

    fn limit_display_fps(&mut self, framerate_limit: u32) {
        let target_time =
            self.last_frame + Duration::from_nanos(1_000_000_000 / framerate_limit.max(1) as u64);
        if framerate_limit != self.last_limit {
            self.average_overshoot_ns = 0;
            self.last_limit = framerate_limit;
        }
        loop {
            let now = Instant::now();
            if now >= target_time {
                break;
            }
            let remaining_ns = (target_time - now).as_nanos() as u64;
            if remaining_ns > self.average_overshoot_ns + Self::SPIN_SAFETY_BUFFER_NS {
                let expected_ns =
                    remaining_ns - self.average_overshoot_ns - Self::SPIN_SAFETY_BUFFER_NS;
                let sleep_start = Instant::now();
                std::thread::sleep(Duration::from_nanos(expected_ns));
                let overshoot_ns =
                    (sleep_start.elapsed().as_nanos() as u64).saturating_sub(expected_ns);
                if overshoot_ns > 0 && overshoot_ns < Self::MAX_CURRENT_OVERSHOOT_NS {
                    self.average_overshoot_ns =
                        (0.1 * overshoot_ns as f64 + 0.9 * self.average_overshoot_ns as f64) as u64;
                    self.average_overshoot_ns = self
                        .average_overshoot_ns
                        .min(Self::MAX_AVERAGE_OVERSHOOT_NS);
                }
            } else {
                std::hint::spin_loop();
            }
        }
        self.last_frame = Instant::now();
    }
}

/// The tail of every exit from a world or server.
///
/// Takes the connection by value and drops it before the server is asked to
/// stop, in `Minecraft.disconnect`'s order. Dropping aborts the connection
/// task, which closes the pipe on a runtime thread, so the server may see the
/// cancel first; that is fine, since steel's `save_and_shutdown` disconnects
/// and persists every player still online before it saves the worlds.
fn leave_world(
    core: &mut AppCore,
    mut gfx: Gfx,
    panorama: Panorama,
    connection: ConnectionHandle,
    world: Option<World>,
    then: AfterSaving,
) -> AppPhase {
    drop(connection);
    core.audio.stop_all_sounds();
    core.return_to_menu(&mut gfx);

    match world {
        Some(world) => {
            world.begin_close();
            AppPhase::SavingWorld {
                gfx,
                panorama,
                world,
                then,
            }
        }
        None => AppPhase::InMenu { gfx, panorama },
    }
}

/// A launch straight into a world (vanilla `--quickPlay*`), skipping the
/// title screen.
pub enum QuickPlay {
    Multiplayer(String),
    /// A save folder name.
    Singleplayer(String),
}

/// Connects with `connect_args`; `world` is the integrated server for a
/// singleplayer launch.
fn start_connecting(
    core: &mut AppCore,
    gfx: Gfx,
    panorama: Panorama,
    connect_args: ConnectArgs,
    world: Option<World>,
) -> AppPhase {
    let connect_phase = if world.is_some() {
        ConnectionPhase::StartingWorld
    } else {
        ConnectionPhase::Connecting
    };
    // Capture the requested version before the connection task can negotiate
    // a different session protocol. The registry receives explicit metadata.
    let protocol = crate::version::selected_protocol();
    let block_definitions = Rc::clone(&core.block_definitions);
    let connection = spawn_connection(&core.tokio_rt, connect_args);
    let game = GameState::new(
        &*gfx.renderer,
        core.menu.render_distance,
        world.is_some(),
        core.menu.chat_options,
        core.menu.skin_options(),
        block_definitions,
        protocol,
    );
    core.apply_cursor_grab(&gfx.window, None);
    AppPhase::Connecting {
        gfx,
        panorama,
        connect_phase,
        connection,
        game,
        world,
    }
}

impl App {
    pub fn new(
        version: String,
        data_dirs: DataDirs,
        tokio_rt: Arc<tokio::runtime::Runtime>,
        presence: Option<crate::discord::DiscordPresence>,
        user: UserData,
        quick_play: Option<QuickPlay>,
        window_size: winit::dpi::LogicalSize<u32>,
    ) -> Self {
        let pending_skin_uuid = user.has_profile.then_some(user.uuid);
        let core = AppCore::new(version, data_dirs, tokio_rt, presence, user, window_size);
        Self {
            phase: StateSlot::new(AppPhase::Setup {
                quick_play,
                pending_skin_uuid,
            }),
            install: install::Install::new(&core, false),
            installed_window: None,
            core,
            occluded: false,
            suspended: true,
            #[cfg(mobile)]
            soft_keyboard: false,
            fps_limiter: FramerateLimiter::new(),
            #[cfg(feature = "test-harness")]
            harness: None,
            chunk_bench: None,
        }
    }

    /// A `--chunk-bench` run: borderless, as most players play, never paused
    /// for losing focus, and joining at the benchmark's render distance so an
    /// integrated server sends that far.
    pub fn with_chunk_bench(mut self, rd: Option<u32>) -> Self {
        if let Some(rd) = rd {
            self.core.menu.set_bench_render_distance(rd);
            self.core.pause_on_lost_focus = false;
            self.core
                .override_display_mode(core::DisplayMode::Borderless);
        }
        self.chunk_bench = rd.map(ChunkBenchLaunch::Waiting);
        self
    }

    /// The `--chunk-bench` run's exit code: 1 if the game quit before the run
    /// ended.
    pub fn chunk_bench_exit_code(&self) -> Option<i32> {
        self.chunk_bench.map(|launch| match launch {
            ChunkBenchLaunch::Ended(code) => code,
            _ => 1,
        })
    }

    /// Starts the `--chunk-bench` run once in game, and quits when it ends
    /// or the connection does.
    fn tick_chunk_bench(&mut self, event_loop: &ActiveEventLoop) {
        let Some(launch) = self.chunk_bench else {
            return;
        };
        let next = match (launch, self.phase.get_mut()) {
            (
                ChunkBenchLaunch::Waiting(rd),
                AppPhase::InGame {
                    gfx,
                    connection,
                    game,
                    ..
                },
            ) => {
                start_chunk_load(&mut self.core, gfx, connection, game, rd);
                ChunkBenchLaunch::Running
            }
            (ChunkBenchLaunch::Running, AppPhase::InGame { game, .. }) => {
                match (&game.chunk_load_bench, &game.chunk_load_result) {
                    (Some(_), _) => return,
                    (None, Some(_)) => ChunkBenchLaunch::Ended(0),
                    // Cancelled with Esc.
                    (None, None) => ChunkBenchLaunch::Ended(1),
                }
            }
            // The quick play failed to open or connect, or is held for a
            // sign-in that has nothing to restore.
            (ChunkBenchLaunch::Waiting(_), AppPhase::InMenu { .. })
                if self.core.deferred_quick_play.is_none()
                    || self.core.menu.no_account_to_restore() =>
            {
                ChunkBenchLaunch::Ended(1)
            }
            (ChunkBenchLaunch::Running, _) => ChunkBenchLaunch::Ended(1),
            _ => return,
        };
        self.chunk_bench = Some(next);
        if matches!(next, ChunkBenchLaunch::Ended(_)) {
            self.quit(event_loop);
        }
    }

    /// Closes the game, saving the world first in singleplayer.
    fn quit(&mut self, event_loop: &ActiveEventLoop) {
        // A world saves on the way out, so the window stays up for it
        // rather than vanishing while the process finishes writing.
        self.phase.transition(|app| match app {
            AppPhase::Connecting {
                gfx,
                panorama,
                connection,
                world: world @ Some(_),
                ..
            } => leave_world(
                &mut self.core,
                gfx,
                panorama,
                connection,
                world,
                AfterSaving::Quit,
            ),
            AppPhase::InGame {
                gfx,
                connection,
                world: world @ Some(_),
                ..
            } => leave_world(
                &mut self.core,
                gfx,
                Panorama::new(),
                connection,
                world,
                AfterSaving::Quit,
            ),
            AppPhase::SavingWorld {
                gfx,
                panorama,
                world,
                ..
            } => AppPhase::SavingWorld {
                gfx,
                panorama,
                world,
                then: AfterSaving::Quit,
            },
            app => {
                event_loop.exit();
                app
            }
        });
    }

    /// Updates a release build to the newest release before the menu.
    pub fn check_for_update(&mut self) {
        self.install = install::Install::new(&self.core, true);
    }

    /// No launcher passed an account; see `MainMenu::require_account`.
    /// Holds the menu, and any quick play, until an account is signed in,
    /// restoring the stored one in the background meanwhile.
    pub fn require_account(&mut self) {
        self.core
            .menu
            .require_account(&self.core.input.key_bindings);
        self.core.menu.restore_account();
    }

    #[cfg(feature = "test-harness")]
    pub fn with_harness(mut self, harness: Option<crate::harness::Harness>) -> Self {
        // A test window rarely has focus, so it must not pause on losing it.
        if harness.is_some() {
            self.core.pause_on_lost_focus = false;
        }
        self.harness = harness;
        self
    }

    /// The test-harness script's exit code, once it has finished.
    #[cfg(feature = "test-harness")]
    pub fn harness_exit_code(&self) -> Option<i32> {
        self.harness.as_ref()?.exit_code()
    }

    pub fn run(&mut self, event_loop: EventLoop<()>) -> Result<(), WindowError> {
        event_loop.run_app(self)?;
        Ok(())
    }

    /// A text field has keyboard focus: chat and the in-game fields, or a
    /// menu form's.
    #[cfg(mobile)]
    fn wants_text_input(&self) -> bool {
        match self.phase.get() {
            AppPhase::InGame { game, .. } if !game.paused => self.core.input.text_capture,
            _ => self.core.menu.text_field_focused(),
        }
    }

    fn world_has_pointer(&self) -> bool {
        self.phase.get().world_has_pointer()
    }

    /// The effective framerate cap, or `None` for uncapped, matching vanilla
    /// `FramerateLimitTracker`: occluded/iconified → 10, the title/menu (no
    /// world) → 60, otherwise the Max Framerate setting (uncapped at its top).
    fn effective_framerate_limit(&self) -> Option<u32> {
        if self.occluded {
            Some(10)
        } else if !matches!(self.phase.get(), AppPhase::InGame { .. }) {
            Some(60)
        } else {
            let max = self.core.menu.max_framerate;
            (max < crate::ui::menu::MAX_FRAMERATE_UNLIMITED).then_some(max)
        }
    }
}

/// The game window, born in the persisted display mode: the swapchain is
/// sized from `inner_size()` before the window is shown, so switching after
/// creation would leave it built for the windowed size.
fn create_window(
    event_loop: &ActiveEventLoop,
    core: &AppCore,
) -> Result<Arc<Window>, winit::error::OsError> {
    let window_icon = {
        crate::renderer::util::load_resource_image(
            &core.resources,
            &crate::resources::packs::POMME_ICON,
            MAX_RESOURCE_IMAGE_BYTES,
        )
        .map(|rgba| {
            let (w, h) = (rgba.width(), rgba.height());
            match winit::window::Icon::from_rgba(rgba.into_raw(), w, h) {
                Ok(icon) => Some(icon),
                Err(error) => {
                    tracing::warn!(%error, "Failed to create window icon");
                    None
                }
            }
        })
        .unwrap_or_else(|error| {
            tracing::warn!(%error, "Failed to load window icon");
            None
        })
    };
    let monitor = event_loop
        .primary_monitor()
        .or_else(|| event_loop.available_monitors().next());
    // A phone's window is the whole screen, shown from the start: iOS would
    // otherwise make it the desktop size, and it never reports visibility, so
    // the first frame couldn't show a hidden one.
    let mut window_attrs = Window::default_attributes()
        .with_title("Pomme")
        .with_fullscreen(core.display_mode.fullscreen_for(monitor))
        .with_visible(cfg!(mobile))
        .with_window_icon(window_icon);
    if cfg!(not(mobile)) {
        window_attrs = window_attrs.with_inner_size(core.window_size);
    }
    event_loop.create_window(window_attrs).map(Arc::new)
}

impl ApplicationHandler for App {
    fn exiting(&mut self, _event_loop: &ActiveEventLoop) {
        self.core.menu.restore_bench_render_distance();
        // Minecraft.exitWorldAndClose runs screen().removed() before close().
        self.core.menu.flush_settings(&self.core.input.key_bindings);
    }

    fn suspended(&mut self, _event_loop: &ActiveEventLoop) {
        self.suspended = true;
        if let Some(install) = &mut self.install {
            install.suspend();
        }
        if let Some(gfx) = self.phase.gfx_mut() {
            gfx.renderer.suspend();
        }
    }

    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        self.suspended = false;
        if let Some(install) = &mut self.install {
            install.resume(event_loop, &self.core);
            return;
        }
        if let Some(gfx) = self.phase.gfx_mut() {
            if let Err(e) = gfx.renderer.resume(&gfx.window) {
                tracing::error!("Failed to resume the renderer: {e}");
                event_loop.exit();
            }
            return;
        }
        self.phase.transition(|app| match app {
            AppPhase::Setup {
                quick_play,
                mut pending_skin_uuid,
            } => {
                // The first-run download's window, if it ran.
                let window = match self
                    .installed_window
                    .take()
                    .map_or_else(|| create_window(event_loop, &self.core), Ok)
                {
                    Ok(w) => w,
                    Err(e) => {
                        tracing::error!("Failed to create window: {e}");
                        event_loop.exit();
                        return AppPhase::Setup {
                            quick_play,
                            pending_skin_uuid,
                        };
                    }
                };

                let mut renderer = match Renderer::new(
                    Arc::clone(&window),
                    self.core.font_sources(),
                    &self.core.data_dirs.game_dir,
                    &self.core.data_dirs.cache_dir,
                    self.core.menu.vsync,
                ) {
                    Ok(r) => r,
                    Err(e) => {
                        tracing::error!("Failed to create renderer: {e}");
                        event_loop.exit();
                        return AppPhase::Setup {
                            quick_play,
                            pending_skin_uuid,
                        };
                    }
                };

                if let Some(p) = &mut self.core.presence {
                    p.set_in_menu(&self.core.version);
                }

                if let Some(uuid) = pending_skin_uuid.take() {
                    renderer.load_player_skin(&uuid, &self.core.tokio_rt);
                    self.core.menu.set_player_face(Some(&uuid));
                }

                self.core.apply_cursor_grab(&window, None);

                let gfx = Gfx {
                    renderer: ManuallyDrop::new(renderer),
                    window,
                    last_frame: Instant::now(),
                    fps_counter: FpsCounter::new(),
                };
                let panorama = Panorama::new();
                let launch = match quick_play {
                    Some(play) if self.core.menu.account_gated() => {
                        self.core.deferred_quick_play = Some(play);
                        MenuUpdateResult::None
                    }
                    Some(play) => in_menu::quick_play(&mut self.core, play),
                    None => MenuUpdateResult::None,
                };
                match launch {
                    MenuUpdateResult::Connect {
                        connect_args,
                        world,
                    } => start_connecting(&mut self.core, gfx, panorama, *connect_args, world),
                    _ => AppPhase::InMenu { gfx, panorama },
                }
            }
            _ => app,
        });
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: WindowId,
        event: WindowEvent,
    ) {
        if let Some(install) = &mut self.install {
            match event {
                WindowEvent::RedrawRequested => {
                    let step = install.frame(&mut self.core);
                    if matches!(step, install::Step::Relaunched) {
                        event_loop.exit();
                    }
                    if let install::Step::Done(window) = step {
                        self.install = None;
                        self.installed_window = Some(window);
                        // In the background there's no surface to build the
                        // renderer on; the real `resumed` builds it.
                        if !self.suspended {
                            self.resumed(event_loop);
                            // The first frame of the normal loop, which asks
                            // for each next one itself.
                            if let Some(gfx) = self.phase.gfx_mut() {
                                gfx.window.request_redraw();
                            }
                        }
                    }
                }
                WindowEvent::Resized(size) => install.resize(size.width, size.height),
                WindowEvent::MouseInput {
                    state: winit::event::ElementState::Pressed,
                    ..
                }
                | WindowEvent::Touch(winit::event::Touch {
                    phase: winit::event::TouchPhase::Started,
                    ..
                }) => install.retry(&self.core),
                WindowEvent::CloseRequested => event_loop.exit(),
                _ => {}
            }
            return;
        }
        let binding_capture = self.core.menu.binding_capture_active()
            && self.core.unfocused_since.is_none()
            && match self.phase.get() {
                AppPhase::InMenu { .. } => true,
                AppPhase::InGame { game, .. } => game.options_from_game && !game.dialog_open(),
                _ => false,
            };
        self.core.input.set_binding_capture(binding_capture);
        match event {
            WindowEvent::CloseRequested | WindowEvent::Destroyed => {
                self.quit(event_loop);
            }
            WindowEvent::Resized(new_size) => {
                if let Some(app_rt) = self.phase.gfx_mut() {
                    app_rt.renderer.resize(new_size);
                }
            }
            WindowEvent::ModifiersChanged(mods) => {
                self.core.input.set_modifiers(mods);
            }
            // TODO: Migrate _fully_ to Action system
            WindowEvent::KeyboardInput { event, .. } => {
                let key = input::key_code(&event);
                self.phase.transition(|mut app| {
                    if self.core.input.captures_key_event(&event) {
                        self.core.input.on_key_event(&event);
                        return app;
                    }
                    // F3 held suppresses global keys (vanilla
                    // handleGlobalKeyPress); press only, no repeat.
                    if let Some(Gfx { window, .. }) = app.gfx_mut()
                        && event.state.is_pressed()
                        && !event.repeat
                        && !self.core.input.action_held(Action::DebugModifier)
                        && let Some(code) = key
                        && self
                            .core
                            .input
                            .key_bindings
                            .matches_key(Action::ToggleFullscreen, code)
                    {
                        self.core.toggle_display_mode(window);
                    }
                    // F2 screenshot works in every phase, like vanilla.
                    // TODO: Ctrl+F2 panoramic screenshot
                    if let Some(Gfx { renderer, .. }) = app.gfx_mut()
                        && event.state.is_pressed()
                        && !event.repeat
                        && !self.core.input.action_held(Action::DebugModifier)
                        && let Some(code) = key
                        && self
                            .core
                            .input
                            .key_bindings
                            .matches_key(Action::Screenshot, code)
                    {
                        renderer.request_screenshot();
                    }

                    self.core.input.on_key_event(&event);

                    match app {
                        AppPhase::Setup { .. } | AppPhase::SavingWorld { .. } => app,
                        AppPhase::InMenu { gfx, panorama } => {
                            self.core.input.on_menu_key_event(&event);
                            AppPhase::InMenu { gfx, panorama }
                        }
                        AppPhase::Connecting {
                            gfx,
                            panorama,
                            connect_phase,
                            connection,
                            mut game,
                            world,
                        } => {
                            // A configuration-phase dialog replaces the
                            // connect screen and takes its keys.
                            if event.state.is_pressed()
                                && let Some(code) = key
                                && game.dialog_open()
                            {
                                crate::app::phases::in_game::server_dialog_key(
                                    code,
                                    &event,
                                    &mut self.core,
                                    &gfx.window,
                                    &connection,
                                    &mut game,
                                );
                                AppPhase::Connecting {
                                    gfx,
                                    panorama,
                                    connect_phase,
                                    connection,
                                    game,
                                    world,
                                }
                            } else if event.state.is_pressed()
                                && let Some(code) = key
                                && self
                                    .core
                                    .input
                                    .key_bindings
                                    .matches_key(Action::Close, code)
                            {
                                leave_world(
                                    &mut self.core,
                                    gfx,
                                    Panorama::new(),
                                    connection,
                                    world,
                                    AfterSaving::Menu,
                                )
                            } else {
                                AppPhase::Connecting {
                                    gfx,
                                    panorama,
                                    connect_phase,
                                    connection,
                                    game,
                                    world,
                                }
                            }
                        }
                        AppPhase::InGame {
                            gfx,
                            connection,
                            mut game,
                            world,
                        } => {
                            // No repeat filter: vanilla dispatches GLFW repeats
                            // to screens and debug chords alike.
                            if event.state.is_pressed()
                                && let Some(code) = key
                            {
                                if game.options_from_game {
                                    let f3_held =
                                        self.core.input.action_held(Action::DebugModifier);
                                    if !game.handle_debug_key(
                                        code,
                                        f3_held,
                                        &self.core.input.key_bindings,
                                        &connection,
                                    ) {
                                        self.core.input.on_menu_key_event(&event);
                                    }
                                } else if game.server_dialog.is_some()
                                    || (game.chat.has_pending_modal_prompt()
                                        && !game.chat.is_open())
                                {
                                    crate::app::phases::in_game::server_dialog_key(
                                        code,
                                        &event,
                                        &mut self.core,
                                        &gfx.window,
                                        &connection,
                                        &mut game,
                                    );
                                } else if game.chat.is_open() {
                                    match code {
                                        _ if self
                                            .core
                                            .input
                                            .key_bindings
                                            .matches_key(Action::Close, code) =>
                                        {
                                            let closed = game.escape_chat();
                                            self.core
                                                .input
                                                .clear_action(crate::app::input::Action::OpenMenu);
                                            if closed {
                                                self.core.apply_cursor_grab(
                                                    &gfx.window,
                                                    Some(&mut game),
                                                );
                                            }
                                        }
                                        _ => {
                                            let f3_held =
                                                self.core.input.action_held(Action::DebugModifier);
                                            if !game.handle_debug_key(
                                                code,
                                                f3_held,
                                                &self.core.input.key_bindings,
                                                &connection,
                                            ) {
                                                self.core.input.on_menu_key_event(&event);
                                            }
                                        }
                                    }
                                } else if game.creative_inventory_open {
                                    match code {
                                        _ if self
                                            .core
                                            .input
                                            .key_bindings
                                            .matches_key(Action::Close, code) =>
                                        {
                                            game.close_creative_inventory();
                                            self.core
                                                .input
                                                .clear_action(crate::app::input::Action::OpenMenu);
                                            self.core
                                                .apply_cursor_grab(&gfx.window, Some(&mut game));
                                        }
                                        _ => {
                                            let f3_held =
                                                self.core.input.action_held(Action::DebugModifier);
                                            if !game.handle_debug_key(
                                                code,
                                                f3_held,
                                                &self.core.input.key_bindings,
                                                &connection,
                                            ) {
                                                self.core.input.on_menu_key_event(&event);
                                            }
                                        }
                                    }
                                } else if game.wants_text_input() {
                                    // A container text field (anvil rename) has
                                    // focus; keys type into it. Escape still
                                    // closes via the OpenMenu action.
                                    let f3_held =
                                        self.core.input.action_held(Action::DebugModifier);
                                    if !game.handle_debug_key(
                                        code,
                                        f3_held,
                                        &self.core.input.key_bindings,
                                        &connection,
                                    ) {
                                        self.core.input.on_menu_key_event(&event);
                                    }
                                } else {
                                    match code {
                                        // F3+Esc pauses without opening the
                                        // menu (vanilla pauseGame(true)).
                                        _ if self
                                            .core
                                            .input
                                            .key_bindings
                                            .matches_key(Action::Close, code)
                                            && self
                                                .core
                                                .input
                                                .action_held(Action::DebugModifier)
                                            && game.input_live() =>
                                        {
                                            game.paused = true;
                                            game.pause_screen =
                                                crate::ui::pause::PauseScreen::Hidden;
                                            game.f3_chord_consumed = true;
                                            self.core
                                                .input
                                                .clear_action(crate::app::input::Action::OpenMenu);
                                            self.core
                                                .apply_cursor_grab(&gfx.window, Some(&mut game));
                                        }
                                        _ if self
                                            .core
                                            .input
                                            .key_bindings
                                            .matches_key(Action::Close, code)
                                            && game.death_confirm
                                            && crate::ui::death::buttons_ready(
                                                game.death_confirm_ticks,
                                            ) =>
                                        {
                                            game.death_confirm = false;
                                            self.core.send_respawn(&connection, &mut game);
                                        }
                                        _ => {
                                            let f3_held =
                                                self.core.input.action_held(Action::DebugModifier);
                                            if game.handle_debug_key(
                                                code,
                                                f3_held,
                                                &self.core.input.key_bindings,
                                                &connection,
                                            ) {
                                                self.core.input.unclick(code);
                                            }
                                        }
                                    }
                                }
                            } else if !event.state.is_pressed()
                                && let Some(code) = key
                                && (self
                                    .core
                                    .input
                                    .key_bindings
                                    .matches_key(Action::DebugModifier, code)
                                    || self
                                        .core
                                        .input
                                        .key_bindings
                                        .matches_key(Action::ToggleOverlay, code))
                            {
                                // Overlay toggles on F3 release, unless a chord
                                // consumed it; runs in every in-game sub-state.
                                let overlay = self
                                    .core
                                    .input
                                    .key_bindings
                                    .matches_key(Action::ToggleOverlay, code);
                                let modifier_held =
                                    self.core.input.action_held(Action::DebugModifier);
                                let modifier = self
                                    .core
                                    .input
                                    .key_bindings
                                    .matches_key(Action::DebugModifier, code);
                                game.handle_f3_release(
                                    overlay,
                                    modifier,
                                    modifier_held,
                                    &connection,
                                );
                            }

                            AppPhase::InGame {
                                gfx,
                                connection,
                                game,
                                world,
                            }
                        }
                    }
                });
            }

            WindowEvent::MouseWheel { delta, .. } => {
                // In GLFW's units, which vanilla reads: lines, with precise
                // (trackpad) deltas at a tenth of a pixel on macOS and Wayland.
                // TODO: vanilla then applies `mouseWheelSensitivity` and
                // `discreteMouseScroll` (MouseHandler.onScroll); pomme has
                // neither option, so these act as their defaults (1.0, off).
                let (scroll_x, scroll_y) = match delta {
                    winit::event::MouseScrollDelta::LineDelta(x, y) => (x, y),
                    winit::event::MouseScrollDelta::PixelDelta(p) => {
                        (p.x as f32 / 10.0, p.y as f32 / 10.0)
                    }
                };
                let scroll = scroll_y;
                match self.phase.get_mut() {
                    AppPhase::InMenu { .. } | AppPhase::Connecting { .. } => {
                        self.core.input.on_menu_scroll_xy(scroll_x, scroll_y);
                    }
                    AppPhase::InGame { game, .. }
                        if game.dialog_open()
                            || game.options_from_game
                            || game.creative_inventory_open
                            || game.inventory_open
                            || game.open_container.is_some() =>
                    {
                        self.core.input.on_menu_scroll_xy(scroll_x, scroll_y);
                    }
                    // Queued raw: ChatScreen routes it to the completion popup
                    // or the backlog, with Shift's slower multiplier.
                    AppPhase::InGame { game, .. } if game.chat.is_open() => {
                        self.core.input.on_menu_scroll_xy(0.0, scroll);
                    }
                    // Vanilla MouseHandler: a spectator's wheel moves the menu
                    // selection (sign-inverted) while it is open, and adjusts
                    // flying speed (local-only) otherwise.
                    AppPhase::InGame {
                        game, connection, ..
                    } if game.input_live()
                        && crate::player::is_spectator(game.player.game_mode) =>
                    {
                        if game.spectator.is_menu_active() {
                            // f32::signum maps 0.0 to 1.0; skip empty deltas.
                            if scroll != 0.0 {
                                game.spectator.on_mouse_scrolled(
                                    -(scroll.signum() as i32),
                                    &game.tab_list,
                                    &connection.packet_tx,
                                );
                            }
                        } else {
                            game.player.fly_speed =
                                (game.player.fly_speed + scroll * 0.005).clamp(0.0, 0.2);
                        }
                    }
                    AppPhase::InGame { game, .. } if game.input_live() => {
                        self.core.input.on_scroll(scroll)
                    }
                    _ => {}
                }
            }
            // Desktop touchscreens already drive the mouse through the OS.
            WindowEvent::Touch(touch) if cfg!(mobile) => {
                let gameplay = self.world_has_pointer();
                if let Some(gfx) = self.phase.gfx_mut() {
                    let width = gfx.renderer.screen_width() as f32;
                    let height = gfx.renderer.screen_height() as f32;
                    let layout = TouchLayout {
                        width,
                        height,
                        gs: self.core.menu.gui_scale(width, height),
                    };
                    self.core.input.on_touch(&touch, gameplay, &layout);
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.core
                    .input
                    .on_cursor_moved(position.x as f32, position.y as f32);
            }
            WindowEvent::MouseInput { state, button, .. }
                if matches!(
                    self.phase.get(),
                    AppPhase::InMenu { .. } | AppPhase::Connecting { .. }
                ) || matches!(self.phase.get(), AppPhase::InGame { game, .. } if game.paused || game.gui_open())
                    || self.core.input.is_cursor_captured() =>
            {
                // TODO: Investigate controller touchpad clicks arriving as mouse
                // input (DualShock/DualSense), including whether a separate gamepad
                // event is available and whether the pointer source can be identified.
                self.core.input.on_mouse_button(button, state);
            }

            WindowEvent::Occluded(occluded) => {
                self.occluded = occluded;
            }

            WindowEvent::Focused(focused) => {
                if !focused {
                    self.core.menu.end_binding_capture();
                    self.core.input.set_binding_capture(false);
                }
                self.core.unfocused_since = (!focused).then(Instant::now);
                // The window manager may silently drop a cursor lock on focus
                // change, so a quick refocus (under the pause-on-lost-focus
                // delay) re-issues the grab.
                self.core.invalidate_cursor_grab_state();
                if focused {
                    match self.phase.get_mut() {
                        AppPhase::Setup { .. } => {}
                        AppPhase::InMenu { gfx, .. }
                        | AppPhase::Connecting { gfx, .. }
                        | AppPhase::SavingWorld { gfx, .. } => {
                            self.core.apply_cursor_grab(&gfx.window, None);
                        }
                        AppPhase::InGame { gfx, game, .. } => {
                            self.core.apply_cursor_grab(&gfx.window, Some(game));
                        }
                    }
                }
            }

            WindowEvent::RedrawRequested => {
                if matches!(self.phase.get(), AppPhase::Setup { .. }) {
                    return;
                }

                // `dt` is clamped for physics; `raw_dt` is the unclamped interval the
                // benchmarks need.
                let (dt, raw_dt) = if let Some(app_rt) = self.phase.gfx_mut() {
                    let now = Instant::now();
                    let raw_dt = now.duration_since(app_rt.last_frame).as_secs_f32();
                    let dt = raw_dt.min(0.1);

                    app_rt.last_frame = now;
                    app_rt.fps_counter.update(dt);

                    (dt, raw_dt)
                } else {
                    (0.0, 0.0)
                };

                let core = &mut self.core;

                let should_apply_cursor_grab = core.input.update(&mut self.phase);
                if should_apply_cursor_grab
                    && let AppPhase::InGame { gfx, game, .. } = self.phase.get_mut()
                {
                    core.apply_cursor_grab(&gfx.window, Some(game));
                }

                self.phase.transition(|app| match app {
                    AppPhase::Setup { .. } => unreachable!(
                        "The function early returns above if the phase is AppPhase::Setup"
                    ),
                    AppPhase::InMenu {
                        mut gfx,
                        mut panorama,
                    } => {
                        let update_result = update_menu(core, dt, &mut gfx, &mut panorama);

                        match update_result {
                            MenuUpdateResult::None => AppPhase::InMenu { gfx, panorama },
                            MenuUpdateResult::Connect {
                                connect_args,
                                world,
                            } => start_connecting(core, gfx, panorama, *connect_args, world),
                            MenuUpdateResult::Quit => {
                                event_loop.exit();
                                AppPhase::InMenu { gfx, panorama }
                            }
                        }
                    }
                    AppPhase::Connecting {
                        mut gfx,
                        mut panorama,
                        mut connect_phase,
                        connection,
                        mut game,
                        mut world,
                    } => {
                        let update_result = update_connecting(
                            core,
                            dt,
                            &mut gfx,
                            &mut panorama,
                            &mut connect_phase,
                            &connection,
                            &mut game,
                            world.as_mut(),
                        );

                        match update_result {
                            ConnectingUpdateResult::None => AppPhase::Connecting {
                                gfx,
                                panorama,
                                connect_phase,
                                connection,
                                game,
                                world,
                            },
                            ConnectingUpdateResult::ManualDisconnect => leave_world(
                                core,
                                gfx,
                                panorama,
                                connection,
                                world,
                                AfterSaving::Menu,
                            ),
                            ConnectingUpdateResult::Disconnected { reason } => {
                                core.menu.show_disconnect(reason, &core.input.key_bindings);

                                leave_world(
                                    core,
                                    gfx,
                                    panorama,
                                    connection,
                                    world,
                                    AfterSaving::Menu,
                                )
                            }
                            ConnectingUpdateResult::JoinGame => {
                                if let Some(p) = &mut core.presence {
                                    if world.is_some() {
                                        p.playing_singleplayer(&core.version);
                                    } else {
                                        p.playing_multiplayer(&core.version);
                                    }
                                }
                                // In-game screens use the plain arrow like vanilla, not
                                // the pointer the branded menu may have left set.
                                gfx.window.set_cursor(winit::window::CursorIcon::Default);
                                core.apply_cursor_grab(&gfx.window, Some(&mut game));

                                AppPhase::InGame {
                                    gfx,
                                    connection,
                                    game,
                                    world,
                                }
                            }
                        }
                    }
                    AppPhase::InGame {
                        mut gfx,
                        connection,
                        mut game,
                        mut world,
                    } => {
                        let update_result = match world.as_mut().map(World::poll) {
                            Some(Err(reason)) => GameUpdateResult::Disconnected { reason },
                            _ => update_game(core, dt, raw_dt, &mut gfx, &connection, &mut game),
                        };

                        match update_result {
                            GameUpdateResult::None => AppPhase::InGame {
                                gfx,
                                connection,
                                game,
                                world,
                            },
                            GameUpdateResult::ManualDisconnect => leave_world(
                                core,
                                gfx,
                                Panorama::new(),
                                connection,
                                world,
                                AfterSaving::Menu,
                            ),
                            GameUpdateResult::Disconnected { reason } => {
                                core.menu.show_disconnect(reason, &core.input.key_bindings);

                                leave_world(
                                    core,
                                    gfx,
                                    Panorama::new(),
                                    connection,
                                    world,
                                    AfterSaving::Menu,
                                )
                            }
                        }
                    }
                    AppPhase::SavingWorld {
                        mut gfx,
                        mut panorama,
                        world,
                        then,
                    } => {
                        if update_saving(core, dt, &mut gfx, &mut panorama, &world) {
                            if then == AfterSaving::Quit {
                                event_loop.exit();
                            }
                            AppPhase::InMenu { gfx, panorama }
                        } else {
                            AppPhase::SavingWorld {
                                gfx,
                                panorama,
                                world,
                                then,
                            }
                        }
                    }
                });

                #[cfg(feature = "test-harness")]
                if let Some(harness) = &mut self.harness {
                    harness.tick(core, self.phase.get_mut());
                    if harness.exit_code().is_some() {
                        event_loop.exit();
                    }
                }

                core.input.end_frame();
                self.tick_chunk_bench(event_loop);

                // Desktop is left alone: allowing IME there moves typing from
                // key events to IME events.
                #[cfg(mobile)]
                {
                    let wanted = self.wants_text_input();
                    if wanted != self.soft_keyboard
                        && let Some(gfx) = self.phase.gfx_mut()
                    {
                        gfx.window.set_ime_allowed(wanted);
                        self.soft_keyboard = wanted;
                    }
                }

                let limit = self.effective_framerate_limit();
                if let Some(gfx) = self.phase.gfx_mut() {
                    if !gfx.window.is_visible().unwrap_or(true) {
                        gfx.window.set_visible(true);
                    }
                    gfx.window.request_redraw();
                    // Paced here so input arriving during the wait is pumped before the next frame.
                    gfx.renderer.pace_frame();
                }
                if let Some(fps) = limit {
                    self.fps_limiter.limit_display_fps(fps);
                }
            }
            _ => {}
        }
    }

    fn device_event(
        &mut self,
        _event_loop: &ActiveEventLoop,
        _device_id: DeviceId,
        event: DeviceEvent,
    ) {
        if let DeviceEvent::MouseMotion { delta } = event
            && self.core.input.is_cursor_captured()
            && self.world_has_pointer()
        {
            self.core.input.on_mouse_motion(delta);
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        // UIKit ignores a redraw asked for mid-draw, which is where each frame
        // asks for the next, so iOS asks again once drawing is done.
        #[cfg(target_os = "ios")]
        if !self.suspended {
            let window = match &self.install {
                Some(install) => install.window(),
                None => self.phase.gfx_mut().map(|gfx| &*gfx.window),
            };
            if let Some(window) = window {
                window.request_redraw();
            }
        }
        // iOS delivers each requested redraw on the next display refresh, and
        // polling would keep its run loop too busy to get there.
        let poll = !self.suspended && !cfg!(target_os = "ios");
        event_loop.set_control_flow(if poll {
            winit::event_loop::ControlFlow::Poll
        } else {
            winit::event_loop::ControlFlow::Wait
        });
    }
}
