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
use crate::ui::common::{self, WHITE};
use crate::ui::hud;
use crate::ui::menu::{MainMenu, MenuAction, MenuInput, PanoramaTheme};
use crate::ui::pause::{self, PauseAction};
use crate::world::chunk::ChunkStore;
use azalea_protocol::packets::game::ServerboundGamePacket;
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
    Connecting,
    InGame,
}

const TICK_RATE: f32 = 1.0 / 20.0;
const DEFAULT_RENDER_DISTANCE: u32 = 12;
const POSITION_SEND_INTERVAL: u32 = 20;
const POSITION_THRESHOLD_SQ: f64 = 4.0e-8;

#[derive(Default, PartialEq)]
struct PlayerInputState {
    forward: bool,
    backward: bool,
    left: bool,
    right: bool,
    jump: bool,
    shift: bool,
    sprint: bool,
}

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
    sky_state: crate::renderer::SkyState,
    show_debug: bool,
    fps_counter: FpsCounter,
    last_sent_input: PlayerInputState,
    last_sent_pos: glam::Vec3,
    last_sent_yaw: f32,
    last_sent_pitch: f32,
    last_sent_on_ground: bool,
    last_sent_horizontal_collision: bool,
    was_sprinting: bool,
    position_send_counter: u32,
    options_from_game: bool,
    last_render_distance: u32,
    server_render_distance: u32,
    server_simulation_distance: u32,
}

struct FpsCounter {
    frame_count: u32,
    elapsed: f32,
    display_fps: u32,
}

impl FpsCounter {
    fn new() -> Self {
        Self {
            frame_count: 0,
            elapsed: 0.0,
            display_fps: 0,
        }
    }

    fn update(&mut self, dt: f32) {
        self.frame_count += 1;
        self.elapsed += dt;
        if self.elapsed >= 1.0 {
            self.display_fps = self.frame_count;
            self.frame_count = 0;
            self.elapsed -= 1.0;
        }
    }
}

impl App {
    fn new(
        connection: Option<crate::net::connection::ConnectionHandle>,
        assets_dir: std::path::PathBuf,
        game_dir: std::path::PathBuf,
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
            GameState::Connecting
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
            chunk_store: ChunkStore::new(DEFAULT_RENDER_DISTANCE),
            asset_index: crate::assets::AssetIndex::load(&assets_dir),
            assets_dir,
            game_dir: game_dir.clone(),
            position_set: false,
            state,
            menu: MainMenu::new(&game_dir, Arc::clone(&tokio_rt)),
            tokio_rt,
            options_from_game: false,
            last_render_distance: DEFAULT_RENDER_DISTANCE,
            server_render_distance: 0,
            server_simulation_distance: 0,
            player: LocalPlayer::new(),
            tick_accumulator: 0.0,
            prev_player_pos: glam::Vec3::ZERO,
            mesh_dispatcher: None,
            paused: false,
            inventory_open: false,
            chat: ChatState::new(),
            panorama_scroll: 0.0,
            interaction: InteractionState::new(),
            sky_state: crate::renderer::SkyState::default_day(),
            show_debug: false,
            fps_counter: FpsCounter::new(),
            last_sent_input: PlayerInputState::default(),
            last_sent_pos: glam::Vec3::ZERO,
            last_sent_yaw: 0.0,
            last_sent_pitch: 0.0,
            last_sent_on_ground: false,
            last_sent_horizontal_collision: false,
            was_sprinting: false,
            position_send_counter: 0,
        }
    }

    fn sync_render_distance(&mut self) {
        let rd = self.menu.render_distance;
        self.last_render_distance = rd;
        log::info!("Render distance changed to {rd}");
        if let Some(sender) = &self.packet_sender {
            use azalea_entity::HumanoidArm;
            use azalea_protocol::common::client_information::*;
            sender.send(ServerboundGamePacket::ClientInformation(
                azalea_protocol::packets::game::s_client_information::ServerboundClientInformation {
                    client_information: ClientInformation {
                        language: "en_us".into(),
                        view_distance: rd as u8,
                        chat_visibility: ChatVisibility::Full,
                        chat_colors: true,
                        model_customization: ModelCustomization {
                            cape: true,
                            jacket: true,
                            left_sleeve: true,
                            right_sleeve: true,
                            left_pants: true,
                            right_pants: true,
                            hat: true,
                        },
                        main_hand: HumanoidArm::Right,
                        text_filtering_enabled: false,
                        allows_listing: true,
                        particle_status: ParticleStatus::All,
                    },
                },
            ));
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
            let _ = window
                .set_cursor_grab(CursorGrabMode::Locked)
                .or_else(|_| window.set_cursor_grab(CursorGrabMode::Confined));
            window.set_cursor_visible(false);
        } else {
            let _ = window.set_cursor_grab(CursorGrabMode::None);
            window.set_cursor_visible(true);
        }
    }

    fn connect_to_server(&mut self, server: String, username: String) {
        let (uuid, access_token) = match self.menu.auth_account() {
            Some(account) => (account.uuid, Some(account.access_token.clone())),
            None => (uuid::Uuid::nil(), None),
        };
        let connect_args = crate::net::connection::ConnectArgs {
            server,
            username,
            uuid,
            access_token,
            view_distance: self.menu.render_distance as u8,
        };

        let handle = crate::net::connection::spawn_connection(&self.tokio_rt, connect_args);
        self.net_events = Some(handle.events);
        self.chat_sender = Some(handle.chat_tx);
        self.packet_sender = Some(crate::net::sender::PacketSender::new(handle.packet_tx));
        self.state = GameState::Connecting;
        self.apply_cursor_grab();
    }

    fn disconnect_to_menu(&mut self, reason: Option<String>) {
        self.net_events = None;
        self.chat_sender = None;
        self.packet_sender = None;
        self.state = GameState::Menu;
        self.paused = false;
        self.position_set = false;
        self.chunk_store = ChunkStore::new(self.menu.render_distance);
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
        let mut processed = 0u32;

        while let Ok(event) = rx.try_recv() {
            processed += 1;
            if processed > 512 {
                break;
            }
            match event {
                NetworkEvent::Connected => {
                    log::info!("Connected to server");
                    self.state = GameState::InGame;
                    self.apply_cursor_grab();
                }
                NetworkEvent::DimensionInfo { height, min_y } => {
                    log::info!("Dimension: height={height}, min_y={min_y}");
                    self.chunk_store =
                        ChunkStore::new_with_dimension(self.menu.render_distance, height, min_y);
                    if let Some(renderer) = &mut self.renderer {
                        renderer.clear_chunk_meshes();
                        self.mesh_dispatcher = Some(renderer.create_mesh_dispatcher());
                    }
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
                    log::debug!("Chunk cache center: [{x}, {z}]");
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
                    self.chunk_store
                        .set_center(azalea_core::position::ChunkPos::new(
                            (x as i32).div_euclid(16),
                            (z as i32).div_euclid(16),
                        ));
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
                NetworkEvent::PlayerHealth {
                    health,
                    food,
                    saturation,
                } => {
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
                    if self.interaction.has_pending_prediction(&pos) {
                        continue;
                    }
                    self.chunk_store.set_block_state(pos.x, pos.y, pos.z, state);
                    let chunk_pos = azalea_core::position::ChunkPos::new(
                        pos.x.div_euclid(16),
                        pos.z.div_euclid(16),
                    );
                    chunks_to_mesh.push(chunk_pos);
                }
                NetworkEvent::SectionBlocksUpdate { updates } => {
                    for (pos, state) in updates {
                        self.chunk_store.set_block_state(pos.x, pos.y, pos.z, state);
                        let chunk_pos = azalea_core::position::ChunkPos::new(
                            pos.x.div_euclid(16),
                            pos.z.div_euclid(16),
                        );
                        if !chunks_to_mesh.contains(&chunk_pos) {
                            chunks_to_mesh.push(chunk_pos);
                        }
                    }
                }
                NetworkEvent::GameModeChanged { game_mode } => {
                    log::info!("Game mode changed to {game_mode}");
                    self.player.game_mode = game_mode;
                }
                NetworkEvent::ServerViewDistance { distance } => {
                    log::info!("Server view distance: {distance}");
                    self.server_render_distance = distance;
                }
                NetworkEvent::ServerSimulationDistance { distance } => {
                    log::info!("Server simulation distance: {distance}");
                    self.server_simulation_distance = distance;
                }
                NetworkEvent::BlockChangedAck { seq } => {
                    self.interaction.acknowledge(seq);
                }
                NetworkEvent::TimeUpdate {
                    game_time,
                    day_time,
                } => {
                    self.sky_state.game_time = game_time;
                    self.sky_state.day_time = day_time;
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
            let player_chunk = azalea_core::position::ChunkPos::new(
                (self.player.position.x as i32).div_euclid(16),
                (self.player.position.z as i32).div_euclid(16),
            );
            for pos in chunks_to_mesh {
                let lod = chunk_lod(pos, player_chunk);
                dispatcher.enqueue(&self.chunk_store, pos, lod);
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

        if let Some(renderer) = &mut self.renderer {
            renderer.update_fov(self.player.sprinting);
        }

        if self.packet_sender.is_some() {
            self.send_input_packet();
            self.send_sprint_command();
            self.send_position_packet();
        }

        if !self.paused && !self.inventory_open && !self.chat.is_open() {
            let eye_pos = self.player.position + glam::Vec3::new(0.0, 1.62, 0.0);
            self.interaction.update_target(
                eye_pos,
                self.player.yaw,
                self.player.pitch,
                &self.chunk_store,
            );

            let dirty = self.interaction.tick(
                &self.input,
                &self.chunk_store,
                self.packet_sender.as_ref(),
                self.player.on_ground,
                self.player.game_mode == 1,
            );
            if let Some(dispatcher) = &self.mesh_dispatcher {
                for pos in dirty {
                    dispatcher.enqueue(&self.chunk_store, pos, 0);
                }
            }

            self.input.clear_click_events();
        }
    }

    fn send_input_packet(&mut self) {
        let sender = self.packet_sender.as_ref().unwrap();
        let current = PlayerInputState {
            forward: self.input.key_pressed(KeyCode::KeyW),
            backward: self.input.key_pressed(KeyCode::KeyS),
            left: self.input.key_pressed(KeyCode::KeyA),
            right: self.input.key_pressed(KeyCode::KeyD),
            jump: self.input.key_pressed(KeyCode::Space),
            shift: self.input.key_pressed(KeyCode::ShiftLeft),
            sprint: self.player.sprinting,
        };

        if current != self.last_sent_input {
            sender.send(ServerboundGamePacket::PlayerInput(
                azalea_protocol::packets::game::s_player_input::ServerboundPlayerInput {
                    forward: current.forward,
                    backward: current.backward,
                    left: current.left,
                    right: current.right,
                    jump: current.jump,
                    shift: current.shift,
                    sprint: current.sprint,
                },
            ));
            self.last_sent_input = current;
        }
    }

    fn send_sprint_command(&mut self) {
        let sprinting = self.player.sprinting;
        if sprinting != self.was_sprinting {
            let sender = self.packet_sender.as_ref().unwrap();
            let action = if sprinting {
                azalea_protocol::packets::game::s_player_command::Action::StartSprinting
            } else {
                azalea_protocol::packets::game::s_player_command::Action::StopSprinting
            };
            sender.send(ServerboundGamePacket::PlayerCommand(
                azalea_protocol::packets::game::s_player_command::ServerboundPlayerCommand {
                    id: azalea_world::MinecraftEntityId(0),
                    action,
                    data: 0,
                },
            ));
            self.was_sprinting = sprinting;
        }
    }

    fn send_position_packet(&mut self) {
        let sender = self.packet_sender.as_ref().unwrap();
        use azalea_protocol::common::movements::MoveFlags;
        use azalea_protocol::packets::game::*;

        let pos = self.player.position;
        let yaw = self.player.yaw.to_degrees();
        let pitch = self.player.pitch.to_degrees();

        let dx = (pos.x - self.last_sent_pos.x) as f64;
        let dy = (pos.y - self.last_sent_pos.y) as f64;
        let dz = (pos.z - self.last_sent_pos.z) as f64;
        self.position_send_counter += 1;
        let pos_changed = dx * dx + dy * dy + dz * dz > POSITION_THRESHOLD_SQ
            || self.position_send_counter >= POSITION_SEND_INTERVAL;
        let rot_changed =
            (yaw - self.last_sent_yaw) != 0.0 || (pitch - self.last_sent_pitch) != 0.0;

        let flags = MoveFlags {
            on_ground: self.player.on_ground,
            horizontal_collision: self.player.horizontal_collision,
        };

        let net_pos = azalea_core::position::Vec3 {
            x: pos.x as f64,
            y: pos.y as f64,
            z: pos.z as f64,
        };
        let look = azalea_entity::LookDirection::new(yaw, pitch);

        if pos_changed && rot_changed {
            sender.send(ServerboundGamePacket::MovePlayerPosRot(
                s_move_player_pos_rot::ServerboundMovePlayerPosRot {
                    pos: net_pos,
                    look_direction: look,
                    flags,
                },
            ));
        } else if pos_changed {
            sender.send(ServerboundGamePacket::MovePlayerPos(
                s_move_player_pos::ServerboundMovePlayerPos {
                    pos: net_pos,
                    flags,
                },
            ));
        } else if rot_changed {
            sender.send(ServerboundGamePacket::MovePlayerRot(
                s_move_player_rot::ServerboundMovePlayerRot {
                    look_direction: look,
                    flags,
                },
            ));
        } else if self.player.on_ground != self.last_sent_on_ground
            || self.player.horizontal_collision != self.last_sent_horizontal_collision
        {
            sender.send(ServerboundGamePacket::MovePlayerStatusOnly(
                s_move_player_status_only::ServerboundMovePlayerStatusOnly { flags },
            ));
        }

        if pos_changed {
            self.last_sent_pos = pos;
            self.position_send_counter = 0;
        }
        if rot_changed {
            self.last_sent_yaw = yaw;
            self.last_sent_pitch = pitch;
        }
        self.last_sent_on_ground = self.player.on_ground;
        self.last_sent_horizontal_collision = self.player.horizontal_collision;
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }

        let window_attrs = Window::default_attributes()
            .with_title("POMC")
            .with_inner_size(winit::dpi::LogicalSize::new(854, 480))
            .with_visible(false);

        let window = match event_loop.create_window(window_attrs) {
            Ok(w) => Arc::new(w),
            Err(e) => {
                log::error!("Failed to create window: {e}");
                event_loop.exit();
                return;
            }
        };

        let renderer = match Renderer::new(
            Arc::clone(&window),
            &self.assets_dir,
            &self.asset_index,
            &self.game_dir,
        ) {
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
                } else if matches!(self.state, GameState::Connecting) {
                    if event.state.is_pressed() {
                        if let PhysicalKey::Code(KeyCode::Escape) = event.physical_key {
                            self.disconnect_to_menu(None);
                        }
                    }
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
                                KeyCode::KeyE if !self.paused && !self.chat.is_open() => {
                                    self.inventory_open = !self.inventory_open;
                                    self.apply_cursor_grab();
                                }
                                KeyCode::KeyT | KeyCode::Enter
                                    if !self.paused
                                        && !self.chat.is_open()
                                        && !self.inventory_open =>
                                {
                                    self.chat.open();
                                    self.apply_cursor_grab();
                                }
                                KeyCode::Slash
                                    if !self.paused
                                        && !self.chat.is_open()
                                        && !self.inventory_open =>
                                {
                                    self.chat.open_with_slash();
                                    self.apply_cursor_grab();
                                }
                                KeyCode::F3 => {
                                    self.show_debug = !self.show_debug;
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
                if matches!(self.state, GameState::Menu | GameState::Connecting) {
                    self.input.on_menu_scroll(scroll);
                } else if !self.inventory_open {
                    self.input.on_scroll(scroll);
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.input
                    .on_cursor_moved(position.x as f32, position.y as f32);
            }
            WindowEvent::MouseInput { state, button, .. }
                if matches!(self.state, GameState::Menu | GameState::Connecting)
                    || self.paused
                    || self.inventory_open
                    || self.input.is_cursor_captured() =>
            {
                self.input.on_mouse_button(button, state);
            }
            WindowEvent::RedrawRequested => {
                let now = Instant::now();
                let dt = self
                    .last_frame
                    .map(|last| now.duration_since(last).as_secs_f32())
                    .unwrap_or(0.0)
                    .min(0.1);
                self.last_frame = Some(now);
                self.fps_counter.update(dt);

                'redraw: {
                    match self.state {
                        GameState::Menu => {
                            self.panorama_scroll += dt * 0.01;
                            if self.panorama_scroll > 1.0 {
                                self.panorama_scroll -= 1.0;
                            }

                            if let (Some(renderer), Some(window)) =
                                (&mut self.renderer, &self.window)
                            {
                                let sw = renderer.screen_width() as f32;
                                let sh = renderer.screen_height() as f32;

                                let menu_input = MenuInput {
                                    cursor: self.input.cursor_pos(),
                                    clicked: self.input.left_just_pressed(),
                                    mouse_held: self.input.left_held(),
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
                                if self.input.cursor_moved_this_frame() {
                                    window.set_cursor(cursor_icon);
                                }

                                if let Err(e) = renderer.render_menu(
                                    window,
                                    self.panorama_scroll,
                                    result.blur,
                                    result.elements,
                                    self.input.cursor_pos(),
                                ) {
                                    log::error!("Render error: {e}");
                                }

                                self.input.clear_click_events();

                                if self.menu.render_distance != self.last_render_distance {
                                    self.sync_render_distance();
                                }

                                if self.options_from_game && !self.menu.is_options_screen() {
                                    self.state = GameState::InGame;
                                    self.paused = true;
                                    self.options_from_game = false;
                                    self.apply_cursor_grab();
                                    break 'redraw;
                                }

                                if result.clicked_button {
                                    if let Some(renderer) = &mut self.renderer {
                                        renderer.trigger_skin_swing();
                                    }
                                }

                                match action {
                                    MenuAction::Connect { server, username } => {
                                        self.connect_to_server(server, username);
                                    }
                                    MenuAction::ChangeTheme(theme) => {
                                        if let Some(renderer) = &mut self.renderer {
                                            let panorama_dir = match theme {
                                                PanoramaTheme::Default => self.assets_dir.clone(),
                                                PanoramaTheme::Pomc => {
                                                    self.game_dir.join("pomc_panorama")
                                                }
                                            };
                                            renderer
                                                .reload_panorama(&panorama_dir, &self.asset_index);
                                        }
                                        self.menu.start_transition_open();
                                    }
                                    MenuAction::Quit => {
                                        event_loop.exit();
                                        return;
                                    }
                                    MenuAction::None => {}
                                }
                            }
                        }
                        GameState::Connecting => {
                            self.drain_network_events();

                            self.panorama_scroll += dt * 0.01;
                            if self.panorama_scroll > 1.0 {
                                self.panorama_scroll -= 1.0;
                            }

                            let mut cancel = false;

                            if let (Some(renderer), Some(window)) =
                                (&mut self.renderer, &self.window)
                            {
                                let sw = renderer.screen_width() as f32;
                                let sh = renderer.screen_height() as f32;
                                let gs = hud::gui_scale(sw, sh, self.menu.gui_scale_setting);
                                let fs = 11.0 * gs;
                                let btn_h = 30.0 * gs;
                                let btn_w = 160.0 * gs;

                                let cx = sw / 2.0;
                                let cy = sh / 2.0;

                                let mut elements = Vec::new();

                                elements.push(MenuElement::Text {
                                    x: cx,
                                    y: cy - fs,
                                    text: "Connecting to the server...".into(),
                                    scale: fs,
                                    color: WHITE,
                                    centered: true,
                                });

                                let btn_y = cy + fs;
                                let clicked = self.input.left_just_pressed();
                                let cursor = self.input.cursor_pos();
                                if common::push_button(
                                    &mut elements,
                                    cursor,
                                    cx - btn_w / 2.0,
                                    btn_y,
                                    btn_w,
                                    btn_h,
                                    gs,
                                    fs,
                                    "Cancel",
                                    true,
                                ) && clicked
                                {
                                    cancel = true;
                                }

                                self.input.clear_click_events();

                                if let Err(e) = renderer.render_menu(
                                    window,
                                    self.panorama_scroll,
                                    2.0,
                                    elements,
                                    self.input.cursor_pos(),
                                ) {
                                    log::error!("Render error: {e}");
                                }
                            }

                            if cancel {
                                self.disconnect_to_menu(None);
                            }
                        }
                        GameState::InGame => {
                            self.drain_network_events();
                            if !matches!(self.state, GameState::InGame) {
                                break 'redraw;
                            }

                            let t_mesh = std::time::Instant::now();
                            if let (Some(dispatcher), Some(renderer)) =
                                (&self.mesh_dispatcher, &mut self.renderer)
                            {
                                for mesh in dispatcher.drain_results() {
                                    renderer.upload_chunk_mesh(&mesh);
                                }
                            }
                            let mesh_upload_ms = t_mesh.elapsed().as_secs_f32() * 1000.0;
                            if let Some(renderer) = &mut self.renderer {
                                renderer.last_timings.mesh_upload_ms = mesh_upload_ms;
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
                                self.interaction.update_target(
                                    eye_pos,
                                    yaw,
                                    pitch,
                                    &self.chunk_store,
                                );
                            }

                            let typed = self.input.drain_typed_chars();
                            let backspace = self.input.backspace_pressed();
                            let enter = self.input.enter_pressed();
                            if let Some(msg) = self.chat.handle_key_input(&typed, backspace, enter)
                            {
                                self.send_chat_message(msg);
                                self.apply_cursor_grab();
                            }

                            let mut close_inventory = false;
                            let mut pause_action = PauseAction::None;

                            if let (Some(renderer), Some(window)) =
                                (&mut self.renderer, &self.window)
                            {
                                renderer.sync_camera_to_player(
                                    eye_pos,
                                    renderer.camera_yaw(),
                                    renderer.camera_pitch(),
                                );

                                let sw = renderer.screen_width() as f32;
                                let sh = renderer.screen_height() as f32;
                                let gs = hud::gui_scale(sw, sh, self.menu.gui_scale_setting);

                                let mut elements: Vec<MenuElement> = Vec::new();
                                let hide_cursor = !self.paused
                                    && !self.inventory_open
                                    && !self.chat.is_open()
                                    && self.input.is_cursor_captured();

                                let debug = if self.show_debug {
                                    Some(hud::DebugInfo {
                                        fps: self.fps_counter.display_fps,
                                        position: self.player.position,
                                        yaw: self.player.yaw,
                                        pitch: self.player.pitch,
                                        target_block: self.interaction.target.map(|t| {
                                            let state = self.chunk_store.get_block_state(
                                                t.block_pos.x,
                                                t.block_pos.y,
                                                t.block_pos.z,
                                            );
                                            let block: Box<dyn azalea_block::BlockTrait> =
                                                state.into();
                                            (t.block_pos, t.face, block.id().to_string())
                                        }),
                                        chunk_count: renderer.loaded_chunk_count(),
                                        gpu_name: renderer.gpu_name(),
                                        vulkan_version: renderer.vulkan_version(),
                                        screen_w: renderer.screen_width(),
                                        screen_h: renderer.screen_height(),
                                        timings: Some(hud::FrameTimings {
                                            mesh_upload_ms: renderer.last_timings.mesh_upload_ms,
                                            cull_ms: renderer.last_timings.cull_ms,
                                            draw_ms: renderer.last_timings.draw_ms,
                                            overlay_ms: renderer.last_timings.overlay_ms,
                                            frame_ms: renderer.last_timings.frame_ms,
                                        }),
                                    })
                                } else {
                                    None
                                };
                                hud::build_hud(
                                    &mut elements,
                                    sw,
                                    sh,
                                    self.input.selected_slot(),
                                    self.player.health,
                                    self.player.food,
                                    self.player.air_supply,
                                    debug.as_ref(),
                                    self.menu.gui_scale_setting,
                                );

                                if self.paused {
                                    let cursor = self.input.cursor_pos();
                                    let clicked = self.input.left_just_pressed();
                                    pause_action = pause::build_pause_menu(
                                        &mut elements,
                                        sw,
                                        sh,
                                        cursor,
                                        clicked,
                                        gs,
                                    );
                                    self.input.clear_click_events();
                                }

                                if self.inventory_open {
                                    let cursor = self.input.cursor_pos();
                                    let clicked = self.input.left_just_pressed();
                                    close_inventory = crate::ui::inventory::build_inventory(
                                        &mut elements,
                                        sw,
                                        sh,
                                        cursor,
                                        clicked,
                                        &self.player.inventory,
                                        gs,
                                    );
                                    self.input.clear_click_events();
                                }

                                self.chat.build(&mut elements, sh, gs, &|t, s| {
                                    renderer.menu_text_width(t, s)
                                });

                                let swing_progress = self
                                    .interaction
                                    .get_swing_progress(self.tick_accumulator / TICK_RATE);
                                let destroy_info = self.interaction.destroy_stage();

                                let sky = crate::renderer::SkyState {
                                    day_time: self.sky_state.day_time,
                                    game_time: self.sky_state.game_time,
                                    rain_level: self.sky_state.rain_level,
                                };
                                if let Err(e) = renderer.render_world(
                                    window,
                                    hide_cursor,
                                    elements,
                                    swing_progress,
                                    destroy_info,
                                    sky,
                                ) {
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
                                PauseAction::Options => {
                                    self.menu.open_options();
                                    self.state = GameState::Menu;
                                    self.options_from_game = true;
                                    self.apply_cursor_grab();
                                }
                                PauseAction::Disconnect => {
                                    self.disconnect_to_menu(None);
                                }
                                PauseAction::None => {}
                            }
                        }
                    }
                } // 'redraw

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
            if self.input.is_cursor_captured()
                && !self.paused
                && !self.inventory_open
                && !self.chat.is_open()
            {
                self.input.on_mouse_motion(delta);
            }
        }
    }
}

pub struct LaunchAuth {
    pub username: String,
    pub uuid: uuid::Uuid,
    pub access_token: String,
}

fn chunk_lod(pos: azalea_core::position::ChunkPos, player: azalea_core::position::ChunkPos) -> u32 {
    let dx = (pos.x - player.x).unsigned_abs();
    let dz = (pos.z - player.z).unsigned_abs();
    let dist = dx.max(dz);
    if dist <= 8 {
        0
    } else if dist <= 16 {
        1
    } else {
        2
    }
}

pub fn run(
    connection: Option<crate::net::connection::ConnectionHandle>,
    assets_dir: std::path::PathBuf,
    game_dir: std::path::PathBuf,
    tokio_rt: Arc<tokio::runtime::Runtime>,
    auth: Option<LaunchAuth>,
) -> Result<(), WindowError> {
    let event_loop = EventLoop::new()?;
    let mut app = App::new(connection, assets_dir, game_dir, tokio_rt);
    if let Some(auth) = auth {
        app.menu
            .set_launch_auth(auth.username, auth.uuid, auth.access_token);
    }
    event_loop.run_app(&mut app)?;
    Ok(())
}
