pub mod input;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use thiserror::Error;
use winit::application::ApplicationHandler;
use winit::event::{DeviceEvent, DeviceId, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{CursorGrabMode, Window, WindowId};

use crate::net::NetworkEvent;
use crate::physics::movement;
use crate::player::interaction::InteractionState;
use crate::player::LocalPlayer;
use crate::renderer::chunk::mesher::MeshDispatcher;
use crate::renderer::pipelines::menu_overlay::MenuElement;
use crate::renderer::Renderer;
use crate::ui::chat::ChatState;
use crate::ui::hud;
use crate::ui::menu::{MainMenu, MenuAction, MenuInput, PanoramaTheme};
use crate::ui::pause::{self, PauseAction};
use crate::world::chunk::ChunkStore;
use input::InputState;

#[derive(Error, Debug)]
pub enum WindowError {
    #[error("failed to create event loop: {0}")]
    EventLoop(#[from] winit::error::EventLoopError),

    #[error("failed to create window: {0}")]
    CreateWindow(#[from] winit::error::OsError),

    #[error("renderer error: {0}")]
    Renderer(#[from] crate::renderer::RendererError),
}

enum GameState {
    Menu,
    InGame,
}

const TICK_RATE: f32 = 1.0 / 20.0;
const VIEW_DISTANCE: u32 = 8;

struct App {
    window: Option<Arc<Window>>,
    renderer: Option<Renderer>,
    input: InputState,
    last_frame: Option<Instant>,
    net_events: Option<crossbeam_channel::Receiver<NetworkEvent>>,
    chat_sender: Option<crossbeam_channel::Sender<String>>,
    packet_sender: Option<crate::net::sender::PacketSender>,
    chunk_store: ChunkStore,
    assets_dir: PathBuf,
    game_dir: PathBuf,
    asset_index: Option<crate::assets::AssetIndex>,
    position_set: bool,
    state: GameState,
    menu: MainMenu,
    tokio_rt: Arc<tokio::runtime::Runtime>,
    player: LocalPlayer,
    tick_accumulator: f32,
    prev_player_pos: glam::Vec3,
    mesh_dispatcher: Option<MeshDispatcher>,
    paused: bool,
    inventory_open: bool,
    chat: ChatState,
    panorama_scroll: f32,
    interaction: InteractionState,
}

impl App {
    fn new(
        connection: Option<crate::net::connection::ConnectionHandle>,
        assets_dir: PathBuf,
        game_dir: PathBuf,
        tokio_rt: Arc<tokio::runtime::Runtime>,
    ) -> Self {
        let (net_events, chat_sender, packet_sender) = match connection {
            Some(handle) => (
                Some(handle.events),
                Some(handle.chat_tx),
                Some(crate::net::sender::PacketSender::new(handle.packet_tx)),
            ),
            None => (None, None, None),
        };
        let state = if net_events.is_some() {
            GameState::InGame
        } else {
            GameState::Menu
        };

        Self {
            window: None,
            renderer: None,
            input: InputState::new(),
            last_frame: None,
            net_events,
            chat_sender,
            packet_sender,
            chunk_store: ChunkStore::new(VIEW_DISTANCE),
            asset_index: crate::assets::AssetIndex::load(&assets_dir),
            assets_dir,
            game_dir: game_dir.clone(),
            position_set: false,
            state,
            menu: MainMenu::new(&game_dir, Arc::clone(&tokio_rt)),
            tokio_rt,
            player: LocalPlayer::new(),
            tick_accumulator: 0.0,
            prev_player_pos: glam::Vec3::ZERO,
            mesh_dispatcher: None,
            paused: false,
            inventory_open: false,
            chat: ChatState::new(),
            panorama_scroll: 0.0,
            interaction: InteractionState::new(),
        }
    }

    fn apply_cursor_grab(&self) {
        let Some(window) = &self.window else { return };
        let captured = matches!(self.state, GameState::InGame)
            && !self.paused
            && !self.inventory_open
            && !self.chat.is_open()
            && self.input.is_cursor_captured();
        if captured {
            let _ = window.set_cursor_grab(CursorGrabMode::Locked)
                .or_else(|_| window.set_cursor_grab(CursorGrabMode::Confined));
            window.set_cursor_visible(false);
        } else {
            let _ = window.set_cursor_grab(CursorGrabMode::None);
            window.set_cursor_visible(true);
        }
    }

    fn connect_to_server(&mut self, server: String, username: String) {
        let connect_args = crate::net::connection::ConnectArgs {
            server,
            username,
            uuid: uuid::Uuid::nil(),
            access_token: None,
        };

        let handle = crate::net::connection::spawn_connection(&self.tokio_rt, connect_args);
        self.net_events = Some(handle.events);
        self.chat_sender = Some(handle.chat_tx);
        self.packet_sender = Some(crate::net::sender::PacketSender::new(handle.packet_tx));
        self.state = GameState::InGame;
        self.apply_cursor_grab();
    }

    fn disconnect_to_menu(&mut self, reason: Option<String>) {
        self.net_events = None;
        self.chat_sender = None;
        self.packet_sender = None;
        self.state = GameState::Menu;
        self.paused = false;
        self.position_set = false;
        self.chunk_store = ChunkStore::new(VIEW_DISTANCE);
        if let Some(renderer) = &mut self.renderer {
            renderer.clear_chunk_meshes();
            self.mesh_dispatcher = Some(renderer.create_mesh_dispatcher());
        }
        if let Some(reason) = reason {
            self.menu.show_disconnect(reason);
        }
        self.apply_cursor_grab();
    }

    fn send_chat_message(&self, msg: String) {
        if let Some(tx) = &self.chat_sender {
            let _ = tx.try_send(msg);
        }
    }

    fn drain_network_events(&mut self) {
        let Some(rx) = &self.net_events else { return };
        let mut chunks_to_mesh = Vec::new();
        let mut disconnect_reason: Option<String> = None;

        while let Ok(event) = rx.try_recv() {
            match event {
                NetworkEvent::Connected => {
                    log::info!("Connected to server");
                }
                NetworkEvent::ChunkLoaded {
                    pos,
                    data,
                    heightmaps,
                } => {
                    if let Err(e) = self.chunk_store.load_chunk(pos, &data, &heightmaps) {
                        log::error!("Failed to load chunk [{}, {}]: {e}", pos.x, pos.z);
                        continue;
                    }
                    chunks_to_mesh.push(pos);
                }
                NetworkEvent::ChunkUnloaded { pos } => {
                    self.chunk_store.unload_chunk(&pos);
                    if let Some(renderer) = &mut self.renderer {
                        renderer.remove_chunk_mesh(&pos);
                    }
                }
                NetworkEvent::ChunkCacheCenter { x, z } => {
                    self.chunk_store
                        .set_center(azalea_core::position::ChunkPos::new(x, z));
                }
                NetworkEvent::PlayerPosition {
                    x,
                    y,
                    z,
                    yaw,
                    pitch,
                    ..
                } => {
                    if !self.position_set {
                        self.player.position = glam::Vec3::new(x as f32, y as f32, z as f32);
                        self.player.yaw = yaw.to_radians();
                        self.player.pitch = pitch.to_radians();
                        self.prev_player_pos = self.player.position;
                        if let Some(renderer) = &mut self.renderer {
                            renderer.set_camera_position(x, y, z, yaw, pitch);
                        }
                        self.position_set = true;
                        log::info!("Player position set to ({x:.1}, {y:.1}, {z:.1})");
                    }
                }
                NetworkEvent::PlayerHealth { health, food, saturation } => {
                    self.player.health = health;
                    self.player.food = food;
                    self.player.saturation = saturation;
                }
                NetworkEvent::InventoryContent { items } => {
                    self.player.inventory.set_contents(items);
                }
                NetworkEvent::InventorySlot { index, item } => {
                    self.player.inventory.set_slot(index as usize, item);
                }
                NetworkEvent::ChatMessage { text } => {
                    self.chat.push_message(text);
                }
                NetworkEvent::BlockUpdate { pos, state } => {
                    self.chunk_store.set_block_state(pos.x, pos.y, pos.z, state);
                    let chunk_pos = azalea_core::position::ChunkPos::new(
                        pos.x.div_euclid(16), pos.z.div_euclid(16),
                    );
                    chunks_to_mesh.push(chunk_pos);
                }
                NetworkEvent::SectionBlocksUpdate { updates } => {
                    for (pos, state) in updates {
                        self.chunk_store.set_block_state(pos.x, pos.y, pos.z, state);
                        let chunk_pos = azalea_core::position::ChunkPos::new(
                            pos.x.div_euclid(16), pos.z.div_euclid(16),
                        );
                        if !chunks_to_mesh.contains(&chunk_pos) {
                            chunks_to_mesh.push(chunk_pos);
                        }
                    }
                }
                NetworkEvent::Disconnected { reason } => {
                    log::warn!("Disconnected: {reason}");
                    disconnect_reason = Some(reason);
                }
            }
        }

        if let Some(reason) = disconnect_reason {
            self.disconnect_to_menu(Some(reason));
            return;
        }

        if let Some(dispatcher) = &self.mesh_dispatcher {
            for pos in chunks_to_mesh {
                dispatcher.enqueue(&self.chunk_store, pos);
            }
        }
    }

    fn tick_physics(&mut self) {
        if let Some(renderer) = &self.renderer {
            self.player.yaw = renderer.camera_yaw();
            self.player.pitch = renderer.camera_pitch();
        }

        self.prev_player_pos = self.player.position;
        movement::tick(&mut self.player, &self.input, &self.chunk_store);
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }

        let window_attrs = Window::default_attributes()
            .with_title("POMC")
            .with_inner_size(winit::dpi::LogicalSize::new(854, 480));

        let window = match event_loop.create_window(window_attrs) {
            Ok(w) => Arc::new(w),
            Err(e) => {
                log::error!("Failed to create window: {e}");
                event_loop.exit();
                return;
            }
        };

        let renderer = match Renderer::new(Arc::clone(&window), &self.assets_dir, &self.asset_index) {
            Ok(r) => r,
            Err(e) => {
                log::error!("Failed to create renderer: {e}");
                event_loop.exit();
                return;
            }
        };

        self.mesh_dispatcher = Some(renderer.create_mesh_dispatcher());
        self.renderer = Some(renderer);
        self.window = Some(window);
        self.apply_cursor_grab();
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: WindowId,
        event: WindowEvent,
    ) {
        match event {
            WindowEvent::CloseRequested => {
                event_loop.exit();
            }
            WindowEvent::Resized(new_size) => {
                if let Some(renderer) = &mut self.renderer {
                    renderer.resize(new_size);
                }
            }
            WindowEvent::KeyboardInput { event, .. } => {
                if matches!(self.state, GameState::Menu) {
                    self.input.on_menu_key_event(&event);
                } else if matches!(self.state, GameState::InGame) {
                    if self.chat.is_open() {
                        self.input.on_menu_key_event(&event);
                    } else if event.state.is_pressed() {
                        if let PhysicalKey::Code(code) = event.physical_key {
                            match code {
                                KeyCode::Escape => {
                                    if self.inventory_open {
                                        self.inventory_open = false;
                                    } else {
                                        self.paused = !self.paused;
                                    }
                                    self.apply_cursor_grab();
                                }
                                KeyCode::KeyE
                                    if !self.paused && !self.chat.is_open() =>
                                {
                                    self.inventory_open = !self.inventory_open;
                                    self.apply_cursor_grab();
                                }
                                KeyCode::KeyT | KeyCode::Enter
                                    if !self.paused && !self.chat.is_open() && !self.inventory_open =>
                                {
                                    self.chat.open();
                                    self.apply_cursor_grab();
                                }
                                KeyCode::Slash if !self.paused && !self.chat.is_open() && !self.inventory_open => {
                                    self.chat.open_with_slash();
                                    self.apply_cursor_grab();
                                }
                                _ => {}
                            }
                        }
                    }
                    if !self.paused && !self.chat.is_open() && !self.inventory_open {
                        self.input.on_key_event(&event);
                    }
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let scroll = match delta {
                    winit::event::MouseScrollDelta::LineDelta(_, y) => y,
                    winit::event::MouseScrollDelta::PixelDelta(p) => p.y as f32,
                };
                if matches!(self.state, GameState::Menu) {
                    self.input.on_menu_scroll(scroll);
                } else if !self.inventory_open {
                    self.input.on_scroll(scroll);
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.input.on_cursor_moved(position.x as f32, position.y as f32);
            }
            WindowEvent::MouseInput { state, button, .. } => {
                if matches!(self.state, GameState::Menu)
                    || self.paused
                    || self.inventory_open
                {
                    self.input.on_mouse_button(button, state);
                } else if self.input.is_cursor_captured() {
                    self.input.on_mouse_button(button, state);
                }
            }
            WindowEvent::RedrawRequested => {
                let now = Instant::now();
                let dt = self
                    .last_frame
                    .map(|last| now.duration_since(last).as_secs_f32())
                    .unwrap_or(0.0)
                    .min(0.1);
                self.last_frame = Some(now);

                match self.state {
                    GameState::Menu => {
                        self.panorama_scroll += dt * 0.01;
                        if self.panorama_scroll > 1.0 {
                            self.panorama_scroll -= 1.0;
                        }

                        if let (Some(renderer), Some(window)) = (&mut self.renderer, &self.window) {
                            let sw = renderer.screen_width() as f32;
                            let sh = renderer.screen_height() as f32;

                            let menu_input = MenuInput {
                                cursor: self.input.cursor_pos(),
                                clicked: self.input.left_just_pressed(),
                                typed_chars: self.input.drain_typed_chars(),
                                backspace: self.input.backspace_pressed(),
                                enter: self.input.enter_pressed(),
                                escape: self.input.escape_pressed(),
                                tab: self.input.tab_pressed(),
                                f5: self.input.f5_pressed(),
                                scroll_delta: self.input.consume_menu_scroll(),
                            };

                            let result = self.menu.build(sw, sh, &menu_input, |t, s| {
                                renderer.menu_text_width(t, s)
                            });
                            let action = result.action;

                            let cursor_icon = if result.cursor_pointer {
                                winit::window::CursorIcon::Pointer
                            } else {
                                winit::window::CursorIcon::Default
                            };
                            window.set_cursor(cursor_icon);

                            if let Err(e) = renderer.render_menu(window, self.panorama_scroll, result.blur, result.elements) {
                                log::error!("Render error: {e}");
                            }

                            self.input.clear_click_events();

                            match action {
                                MenuAction::Connect { server, username } => {
                                    self.connect_to_server(server, username);
                                }
                                MenuAction::ChangeTheme(theme) => {
                                    if let Some(renderer) = &mut self.renderer {
                                        let panorama_dir = match theme {
                                            PanoramaTheme::Default => self.assets_dir.clone(),
                                            PanoramaTheme::Pomc => self.game_dir.join("pomc_panorama"),
                                        };
                                        renderer.reload_panorama(&panorama_dir, &self.asset_index);
                                    }
                                    self.menu.start_transition_open();
                                }
                                MenuAction::None => {}
                            }
                        }
                    }
                    GameState::InGame => {
                        self.drain_network_events();

                        if let (Some(dispatcher), Some(renderer)) =
                            (&self.mesh_dispatcher, &mut self.renderer)
                        {
                            for mesh in dispatcher.drain_results() {
                                renderer.upload_chunk_mesh(&mesh);
                            }
                        }

                        if !self.paused && !self.inventory_open && !self.chat.is_open() {
                            if let Some(renderer) = &mut self.renderer {
                                renderer.update_camera(&mut self.input);
                            }

                            self.tick_accumulator += dt;
                            while self.tick_accumulator >= TICK_RATE {
                                self.tick_physics();
                                self.tick_accumulator -= TICK_RATE;
                            }
                        }

                        let alpha = self.tick_accumulator / TICK_RATE;
                        let interp_pos = self.prev_player_pos.lerp(self.player.position, alpha);
                        let eye_pos = interp_pos + glam::Vec3::new(0.0, 1.62, 0.0);

                        if !self.paused && !self.inventory_open && !self.chat.is_open() {
                            let (yaw, pitch) = if let Some(r) = &self.renderer {
                                (r.camera_yaw(), r.camera_pitch())
                            } else {
                                (self.player.yaw, self.player.pitch)
                            };
                            self.interaction.update_target(eye_pos, yaw, pitch, &self.chunk_store);
                            let dirty = self.interaction.tick(
                                &self.input,
                                &self.chunk_store,
                                self.packet_sender.as_ref(),
                            );
                            if let Some(dispatcher) = &self.mesh_dispatcher {
                                for pos in dirty {
                                    dispatcher.enqueue(&self.chunk_store, pos);
                                }
                            }
                            self.input.clear_click_events();
                        }

                        // Handle chat input before borrowing renderer
                        let typed = self.input.drain_typed_chars();
                        let backspace = self.input.backspace_pressed();
                        let enter = self.input.enter_pressed();
                        if let Some(msg) = self.chat.handle_key_input(&typed, backspace, enter) {
                            self.send_chat_message(msg);
                            self.apply_cursor_grab();
                        }

                        // Handle inventory close before borrowing renderer
                        let mut close_inventory = false;
                        let mut pause_action = PauseAction::None;

                        if let (Some(renderer), Some(window)) = (&mut self.renderer, &self.window) {
                            renderer.sync_camera_to_player(
                                eye_pos,
                                renderer.camera_yaw(),
                                renderer.camera_pitch(),
                            );

                            let sw = renderer.screen_width() as f32;
                            let sh = renderer.screen_height() as f32;
                            let gs = hud::gui_scale(sw, sh);

                            let mut elements: Vec<MenuElement> = Vec::new();
                            let hide_cursor = !self.paused && !self.inventory_open
                                && !self.chat.is_open() && self.input.is_cursor_captured();

                            hud::build_hud(
                                &mut elements, sw, sh,
                                self.input.selected_slot(),
                                self.player.health,
                                self.player.food,
                            );

                            if self.paused {
                                let cursor = self.input.cursor_pos();
                                let clicked = self.input.left_just_pressed();
                                pause_action = pause::build_pause_menu(
                                    &mut elements, sw, sh, cursor, clicked, gs,
                                );
                                self.input.clear_click_events();
                            }

                            if self.inventory_open {
                                let cursor = self.input.cursor_pos();
                                let clicked = self.input.left_just_pressed();
                                close_inventory = crate::ui::inventory::build_inventory(
                                    &mut elements, sw, sh, cursor, clicked,
                                    &self.player.inventory, gs,
                                );
                                self.input.clear_click_events();
                            }

                            self.chat.build(&mut elements, sh, gs, &|t, s| {
                                renderer.menu_text_width(t, s)
                            });

                            if let Err(e) = renderer.render_world(window, hide_cursor, elements) {
                                log::error!("Render error: {e}");
                            }
                        }

                        if close_inventory {
                            self.inventory_open = false;
                            self.apply_cursor_grab();
                        }

                        match pause_action {
                            PauseAction::Resume => {
                                self.paused = false;
                                self.apply_cursor_grab();
                            }
                            PauseAction::Disconnect => {
                                self.disconnect_to_menu(None);
                            }
                            PauseAction::None => {}
                        }
                    }
                }

                if let Some(window) = &self.window {
                    window.request_redraw();
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
        if let DeviceEvent::MouseMotion { delta } = event {
            if self.input.is_cursor_captured() && !self.paused && !self.inventory_open && !self.chat.is_open() {
                self.input.on_mouse_motion(delta);
            }
        }
    }
}

pub fn run(
    connection: Option<crate::net::connection::ConnectionHandle>,
    assets_dir: PathBuf,
    game_dir: PathBuf,
    tokio_rt: Arc<tokio::runtime::Runtime>,
) -> Result<(), WindowError> {
    let event_loop = EventLoop::new()?;
    let mut app = App::new(connection, assets_dir, game_dir, tokio_rt);
    event_loop.run_app(&mut app)?;
    Ok(())
}
