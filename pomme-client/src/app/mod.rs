pub mod core;
pub mod input;
pub mod phases;
pub mod state_slot;

use std::mem::ManuallyDrop;
use std::sync::Arc;
use std::time::{Duration, Instant};

use thiserror::Error;
use winit::application::ApplicationHandler;
use winit::event::{DeviceEvent, DeviceId, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{Window, WindowId};

use crate::app::core::AppCore;
use crate::app::phases::connecting::{ConnectingUpdateResult, update_connecting};
use crate::app::phases::in_game::{GameState, GameUpdateResult, update_game};
use crate::app::phases::in_menu::{MenuUpdateResult, update_menu};
use crate::app::phases::{AppPhase, ConnectionPhase, FpsCounter, Gfx, Panorama};
use crate::app::state_slot::StateSlot;
use crate::dirs::DataDirs;
use crate::net::connection::{ConnectArgs, spawn_connection};
use crate::renderer::{self, Renderer};
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

const TICK_RATE: f32 = 1.0 / 20.0;
const TICK_RATE_MS: u32 = 1000 / 20;
const DEFAULT_RENDER_DISTANCE: u32 = 12;
const POSITION_SEND_INTERVAL: u32 = 20;
const POSITION_THRESHOLD_SQ: f64 = 4.0e-8;

pub struct App {
    phase: StateSlot<AppPhase>,
    core: AppCore,
    occluded: bool,
    fps_limiter: FramerateLimiter,
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

impl App {
    pub fn new(
        version: String,
        data_dirs: DataDirs,
        tokio_rt: Arc<tokio::runtime::Runtime>,
        presence: Option<crate::discord::DiscordPresence>,
        user: UserData,
        quick_access_multiplayer: Option<String>,
    ) -> Self {
        Self {
            phase: StateSlot::new(AppPhase::Setup {
                quick_access_multiplayer,
                pending_skin_uuid: user.has_profile.then_some(user.uuid),
            }),
            core: AppCore::new(version, data_dirs, tokio_rt, presence, user),
            occluded: false,
            fps_limiter: FramerateLimiter::new(),
        }
    }

    pub fn run(&mut self) -> Result<(), WindowError> {
        let event_loop = EventLoop::new()?;
        event_loop.run_app(self)?;
        Ok(())
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

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        self.phase.transition(|app| match app {
            AppPhase::Setup {
                quick_access_multiplayer,
                mut pending_skin_uuid,
            } => {
                let window_icon = {
                    let png =
                        include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/assets/icon.png"));
                    let img = image::load_from_memory(png).expect("failed to decode icon");
                    let rgba = img.to_rgba8();
                    let (w, h) = (rgba.width(), rgba.height());
                    winit::window::Icon::from_rgba(rgba.into_raw(), w, h).ok()
                };

                let window_attrs = Window::default_attributes()
                    .with_title("Pomme")
                    .with_inner_size(winit::dpi::LogicalSize::new(854, 480))
                    .with_visible(false)
                    .with_window_icon(window_icon);

                let window = match event_loop.create_window(window_attrs) {
                    Ok(w) => Arc::new(w),
                    Err(e) => {
                        tracing::error!("Failed to create window: {e}");
                        event_loop.exit();
                        return AppPhase::Setup {
                            quick_access_multiplayer,
                            pending_skin_uuid,
                        };
                    }
                };

                let mut renderer = match Renderer::new(
                    Arc::clone(&window),
                    &self.core.data_dirs.jar_assets_dir,
                    &self.core.asset_index,
                    &self.core.data_dirs.game_dir,
                    self.core.menu.vsync,
                ) {
                    Ok(r) => r,
                    Err(e) => {
                        tracing::error!("Failed to create renderer: {e}");
                        event_loop.exit();
                        return AppPhase::Setup {
                            quick_access_multiplayer,
                            pending_skin_uuid,
                        };
                    }
                };

                if let Some(p) = &mut self.core.presence {
                    p.set_in_menu(&self.core.version);
                }

                if let Some(uuid) = pending_skin_uuid.take() {
                    renderer.load_player_skin(&uuid, &self.core.tokio_rt);
                }

                self.core.apply_cursor_grab(&window, None);

                if let Some(server_ip) = quick_access_multiplayer {
                    let connection = spawn_connection(
                        &self.core.tokio_rt,
                        ConnectArgs {
                            server: server_ip,
                            username: self.core.user.username.clone(),
                            uuid: self.core.user.uuid,
                            access_token: self.core.user.access_token.clone(),
                            view_distance: self.core.menu.render_distance as u8,
                        },
                    );

                    let game = GameState::new(&renderer, &self.core.resource_packs);

                    let gfx = Gfx {
                        renderer: ManuallyDrop::new(renderer),
                        window,
                        last_frame: Instant::now(),
                        fps_counter: FpsCounter::new(),
                    };

                    AppPhase::Connecting {
                        gfx,
                        panorama: Panorama::new(),
                        connect_phase: ConnectionPhase::Connecting,
                        connection,
                        game,
                    }
                } else {
                    let gfx = Gfx {
                        renderer: ManuallyDrop::new(renderer),
                        window,
                        last_frame: Instant::now(),
                        fps_counter: FpsCounter::new(),
                    };

                    AppPhase::InMenu {
                        gfx,
                        panorama: Panorama::new(),
                    }
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
        match event {
            WindowEvent::CloseRequested | WindowEvent::Destroyed => {
                event_loop.exit();
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
                self.phase.transition(|mut app| {
                    if let Some(Gfx { window, .. }) = app.gfx_mut()
                        && event.state.is_pressed()
                        && let PhysicalKey::Code(KeyCode::F11) = event.physical_key
                    {
                        self.core.display_mode = self.core.display_mode.cycle();
                        self.core.menu.display_mode = self.core.display_mode;
                        self.core.apply_display_mode(window);
                    }

                    self.core.input.on_key_event(&event);

                    match app {
                        AppPhase::Setup { .. } => app,
                        AppPhase::InMenu { gfx, panorama } => {
                            self.core.input.on_menu_key_event(&event);
                            AppPhase::InMenu { gfx, panorama }
                        }
                        AppPhase::Connecting {
                            mut gfx,
                            panorama,
                            connect_phase,
                            connection,
                            game,
                        } => {
                            if event.state.is_pressed()
                                && let PhysicalKey::Code(KeyCode::Escape) = event.physical_key
                            {
                                gfx.renderer.clear_chunk_meshes();

                                if let Some(p) = &mut self.core.presence {
                                    p.set_in_menu(&self.core.version);
                                }

                                self.core.apply_cursor_grab(&gfx.window, None);

                                AppPhase::InMenu {
                                    gfx,
                                    panorama: Panorama::new(),
                                }
                            } else {
                                AppPhase::Connecting {
                                    gfx,
                                    panorama,
                                    connect_phase,
                                    connection,
                                    game,
                                }
                            }
                        }
                        AppPhase::InGame {
                            gfx,
                            connection,
                            mut game,
                        } => {
                            if event.state.is_pressed()
                                && !event.repeat
                                && let PhysicalKey::Code(code) = event.physical_key
                            {
                                if code == KeyCode::KeyH && self.core.input.key_pressed(KeyCode::F3)
                                {
                                    game.advanced_item_tooltips = !game.advanced_item_tooltips;
                                } else if game.options_from_game {
                                    let f3_held = self.core.input.key_pressed(KeyCode::F3);
                                    if !game.handle_debug_key(code, f3_held) {
                                        self.core.input.on_menu_key_event(&event);
                                    }
                                } else if game.chat.is_open() {
                                    match code {
                                        KeyCode::Escape => {
                                            game.chat.close();
                                            self.core
                                                .input
                                                .clear_action(crate::app::input::Action::OpenMenu);
                                            self.core
                                                .apply_cursor_grab(&gfx.window, Some(&mut game));
                                        }
                                        _ => {
                                            let f3_held = self.core.input.key_pressed(KeyCode::F3);
                                            if !game.handle_debug_key(code, f3_held) {
                                                self.core.input.on_menu_key_event(&event);
                                            }
                                        }
                                    }
                                } else if game.creative_inventory_open {
                                    match code {
                                        KeyCode::Escape => {
                                            game.close_creative_inventory();
                                            self.core
                                                .input
                                                .clear_action(crate::app::input::Action::OpenMenu);
                                            self.core
                                                .apply_cursor_grab(&gfx.window, Some(&mut game));
                                        }
                                        _ => {
                                            let f3_held = self.core.input.key_pressed(KeyCode::F3);
                                            if !game.handle_debug_key(code, f3_held) {
                                                self.core.input.on_menu_key_event(&event);
                                            }
                                        }
                                    }
                                } else if game.wants_text_input() {
                                    // A container text field (anvil rename) has
                                    // focus; keys type into it. Escape still
                                    // closes via the OpenMenu action.
                                    let f3_held = self.core.input.key_pressed(KeyCode::F3);
                                    if !game.handle_debug_key(code, f3_held) {
                                        self.core.input.on_menu_key_event(&event);
                                    }
                                } else {
                                    match code {
                                        KeyCode::Escape
                                            if game.death_confirm
                                                && game
                                                    .death_confirm_instant
                                                    .elapsed()
                                                    .as_secs_f32()
                                                    >= 1.0 =>
                                        {
                                            game.death_confirm = false;
                                            self.core.send_respawn(&connection, &mut game);
                                        }
                                        _ => {
                                            let f3_held = self.core.input.key_pressed(KeyCode::F3);
                                            game.handle_debug_key(code, f3_held);
                                        }
                                    }
                                }
                            }

                            AppPhase::InGame {
                                gfx,
                                connection,
                                game,
                            }
                        }
                    }
                });
            }

            WindowEvent::MouseWheel { delta, .. } => {
                let scroll = match delta {
                    winit::event::MouseScrollDelta::LineDelta(_, y) => y,
                    winit::event::MouseScrollDelta::PixelDelta(p) => p.y as f32,
                };
                match self.phase.get() {
                    AppPhase::InMenu { .. } | AppPhase::Connecting { .. } => {
                        self.core.input.on_menu_scroll(scroll);
                    }
                    AppPhase::InGame { game, .. }
                        if game.options_from_game || game.creative_inventory_open =>
                    {
                        self.core.input.on_menu_scroll(scroll);
                    }
                    // TODO: open chat should capture scroll (chat history scrolling)
                    AppPhase::InGame { game, .. } if game.input_live() => {
                        self.core.input.on_scroll(scroll)
                    }
                    _ => {}
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
                self.core.input.on_mouse_button(button, state);
            }

            WindowEvent::Occluded(occluded) => {
                self.occluded = occluded;
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
                            MenuUpdateResult::Connect { connect_args } => {
                                let connection = spawn_connection(&core.tokio_rt, connect_args);

                                let game = GameState::new(&gfx.renderer, &core.resource_packs);
                                core.apply_cursor_grab(&gfx.window, None);

                                AppPhase::Connecting {
                                    gfx,
                                    panorama,
                                    connect_phase: ConnectionPhase::Connecting,
                                    connection,
                                    game,
                                }
                            }
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
                    } => {
                        let update_result = update_connecting(
                            core,
                            dt,
                            &mut gfx,
                            &mut panorama,
                            &mut connect_phase,
                            &connection,
                            &mut game,
                        );

                        match update_result {
                            ConnectingUpdateResult::None => AppPhase::Connecting {
                                gfx,
                                panorama,
                                connect_phase,
                                connection,
                                game,
                            },
                            ConnectingUpdateResult::ManualDisconnect => {
                                gfx.renderer.clear_chunk_meshes();

                                if let Some(p) = &mut core.presence {
                                    p.set_in_menu(&core.version);
                                }
                                core.apply_cursor_grab(&gfx.window, None);

                                AppPhase::InMenu { gfx, panorama }
                            }
                            ConnectingUpdateResult::Disconnected { reason } => {
                                gfx.renderer.clear_chunk_meshes();
                                core.menu.show_disconnect(reason);

                                if let Some(p) = &mut core.presence {
                                    p.set_in_menu(&core.version);
                                }
                                core.apply_cursor_grab(&gfx.window, None);

                                AppPhase::InMenu { gfx, panorama }
                            }
                            ConnectingUpdateResult::JoinGame => {
                                if let Some(p) = &mut core.presence {
                                    p.playing_multiplayer(&core.version);
                                }
                                // In-game screens use the plain arrow like vanilla, not
                                // the pointer the branded menu may have left set.
                                gfx.window.set_cursor(winit::window::CursorIcon::Default);
                                core.apply_cursor_grab(&gfx.window, Some(&mut game));

                                AppPhase::InGame {
                                    gfx,
                                    connection,
                                    game,
                                }
                            }
                        }
                    }
                    AppPhase::InGame {
                        mut gfx,
                        connection,
                        mut game,
                    } => {
                        let update_result =
                            update_game(core, dt, raw_dt, &mut gfx, &connection, &mut game);

                        match update_result {
                            GameUpdateResult::None => AppPhase::InGame {
                                gfx,
                                connection,
                                game,
                            },
                            GameUpdateResult::ManualDisconnect => {
                                gfx.renderer.clear_chunk_meshes();

                                if let Some(p) = &mut core.presence {
                                    p.set_in_menu(&core.version);
                                }
                                core.apply_cursor_grab(&gfx.window, None);

                                AppPhase::InMenu {
                                    gfx,
                                    panorama: Panorama::new(),
                                }
                            }
                            GameUpdateResult::Disconnected { reason } => {
                                gfx.renderer.clear_chunk_meshes();
                                core.menu.show_disconnect(reason);

                                if let Some(p) = &mut core.presence {
                                    p.set_in_menu(&core.version);
                                }
                                core.apply_cursor_grab(&gfx.window, None);

                                AppPhase::InMenu {
                                    gfx,
                                    panorama: Panorama::new(),
                                }
                            }
                        }
                    }
                });

                let limit = self.effective_framerate_limit();
                if let Some(gfx) = self.phase.gfx_mut() {
                    if !gfx.window.is_visible().unwrap_or(true) {
                        gfx.window.set_visible(true);
                    }
                    gfx.window.request_redraw();
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
            && matches!(self.phase.get(), AppPhase::InGame { game,.. } if !game.paused && !game.dead && !game.gui_open() && !game.chat.is_open())
        {
            self.core.input.on_mouse_motion(delta);
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        event_loop.set_control_flow(winit::event_loop::ControlFlow::Poll);
    }
}
