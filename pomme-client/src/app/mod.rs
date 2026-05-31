pub mod core;
pub mod input;
pub mod phases;
pub mod state_slot;

use std::mem::ManuallyDrop;
use std::sync::Arc;
use std::time::Instant;

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
const DEFAULT_RENDER_DISTANCE: u32 = 12;
const POSITION_SEND_INTERVAL: u32 = 20;
const POSITION_THRESHOLD_SQ: f64 = 4.0e-8;

pub struct App {
    phase: StateSlot<AppPhase>,
    core: AppCore,
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
                pending_skin_uuid: Some(user.uuid),
            }),
            core: AppCore::new(version, data_dirs, tokio_rt, presence, user),
        }
    }

    pub fn run(&mut self) -> Result<(), WindowError> {
        let event_loop = EventLoop::new()?;
        event_loop.run_app(self)?;
        Ok(())
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
                            mut gfx,
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
                                } else if game.chat.is_open() {
                                    match code {
                                        KeyCode::Escape => {
                                            game.chat.close();
                                            self.core
                                                .apply_cursor_grab(&gfx.window, Some(&mut game));
                                        }
                                        KeyCode::F3 => game.show_debug = !game.show_debug,
                                        _ => self.core.input.on_menu_key_event(&event),
                                    }
                                } else if game.creative_inventory_open {
                                    match code {
                                        KeyCode::Escape | KeyCode::KeyE => {
                                            game.creative_inventory_open = false;
                                            self.core
                                                .apply_cursor_grab(&gfx.window, Some(&mut game));
                                        }
                                        KeyCode::F3 => game.show_debug = !game.show_debug,
                                        _ => self.core.input.on_menu_key_event(&event),
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
                                        KeyCode::Escape if !game.dead => {
                                            if game.inventory_open {
                                                game.inventory_open = false;
                                            } else {
                                                game.paused = !game.paused;
                                            }
                                            self.core
                                                .apply_cursor_grab(&gfx.window, Some(&mut game));
                                        }
                                        KeyCode::KeyE
                                            if !game.paused
                                                && !game.dead
                                                && game.player.game_mode != 3 =>
                                        {
                                            if game.player.game_mode == 1 {
                                                game.creative_inventory_open = true;
                                            } else {
                                                game.inventory_open = !game.inventory_open;
                                            }
                                            self.core
                                                .apply_cursor_grab(&gfx.window, Some(&mut game));
                                        }
                                        KeyCode::KeyT if !game.paused && !game.gui_open() => {
                                            game.chat.open();
                                            self.core
                                                .apply_cursor_grab(&gfx.window, Some(&mut game));
                                        }
                                        KeyCode::Slash if !game.paused && !game.gui_open() => {
                                            game.chat.open_with_slash();
                                            self.core
                                                .apply_cursor_grab(&gfx.window, Some(&mut game));
                                        }
                                        KeyCode::F3 => {
                                            game.show_debug = !game.show_debug;
                                        }
                                        KeyCode::KeyG
                                            if self.core.input.key_pressed(KeyCode::F3) =>
                                        {
                                            game.show_chunk_borders = !game.show_chunk_borders;
                                        }
                                        KeyCode::F5 => {
                                            gfx.renderer.cycle_camera_mode();
                                        }
                                        _ => {}
                                    }
                                }
                            }

                            if !game.paused && !game.chat.is_open() && !game.gui_open() {
                                self.core.input.on_key_event(&event);
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
                    AppPhase::InGame { game, .. } if !game.gui_open() => {
                        self.core.input.on_scroll(scroll)
                    }
                    AppPhase::InGame { game, .. } if game.creative_inventory_open => {
                        self.core.input.on_menu_scroll(scroll);
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

            WindowEvent::RedrawRequested => {
                if matches!(self.phase.get(), AppPhase::Setup { .. }) {
                    return;
                }

                let dt = if let Some(app_rt) = self.phase.gfx_mut() {
                    let now = Instant::now();
                    let dt = now.duration_since(app_rt.last_frame).as_secs_f32().min(0.1);

                    app_rt.last_frame = now;
                    app_rt.fps_counter.update(dt);

                    dt
                } else {
                    0.0
                };

                let core = &mut self.core;
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
                        let update_result = update_game(core, dt, &mut gfx, &connection, &mut game);

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

                if let Some(gfx) = self.phase.gfx_mut() {
                    if !gfx.window.is_visible().unwrap_or(true) {
                        gfx.window.set_visible(true);
                    }
                    gfx.window.request_redraw();
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
