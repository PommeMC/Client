pub mod input;

use azalea_protocol::packets::game::ServerboundGamePacket;
use input::InputState;
use std::sync::Arc;
use std::time::Instant;
use thiserror::Error;
use winit::application::ApplicationHandler;
use winit::event::{DeviceEvent, DeviceId, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{CursorGrabMode, Window, WindowId};

use crate::assets::AssetIndex;
use crate::dirs::DataDirs;
use crate::entity::{EntityStore, ItemEntityStore};
use crate::net::NetworkEvent;
use crate::physics::movement;
use crate::player::LocalPlayer;
use crate::player::interaction::InteractionState;
use crate::renderer::Renderer;
use crate::renderer::chunk::mesher::MeshDispatcher;
use crate::renderer::pipelines::entity_renderer::EntityRenderInfo;
use crate::renderer::pipelines::menu_overlay::MenuElement;
use crate::ui::chat::ChatState;
use crate::ui::common::{self, WHITE};
use crate::ui::death::{self, DeathAction};
use crate::ui::hud;
use crate::ui::menu::{MainMenu, MenuAction, MenuInput, PanoramaTheme};
use crate::ui::pause::{self, PauseAction};
use crate::world::chunk::ChunkStore;

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
    Loading,
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

#[derive(Clone, Copy, PartialEq)]
pub enum DisplayMode {
    Windowed,
    Borderless,
    Fullscreen,
}

impl DisplayMode {
    pub fn cycle(self) -> Self {
        match self {
            Self::Windowed => Self::Borderless,
            Self::Borderless => Self::Fullscreen,
            Self::Fullscreen => Self::Windowed,
        }
    }
}

struct App {
    presence: Option<crate::discord::DiscordPresence>,
    display_mode: DisplayMode,
    window: Option<Arc<Window>>,
    renderer: Option<Renderer>,
    input: InputState,
    last_frame: Option<Instant>,
    net_events: Option<crossbeam_channel::Receiver<NetworkEvent>>,
    chat_sender: Option<crossbeam_channel::Sender<String>>,
    packet_sender: Option<crate::net::sender::PacketSender>,
    net_task: Option<tokio::task::JoinHandle<()>>,
    chunk_store: ChunkStore,
    entity_store: EntityStore,
    data_dirs: DataDirs,
    asset_index: Option<AssetIndex>,
    position_set: bool,
    state: GameState,
    menu: MainMenu,
    version: String,
    tokio_rt: Arc<tokio::runtime::Runtime>,
    player: LocalPlayer,
    tick_accumulator: f32,
    prev_player_pos: glam::Vec3,
    biome_climate:
        Arc<std::collections::HashMap<u32, crate::renderer::chunk::mesher::BiomeClimate>>,
    player_walk_pos: f32,
    player_walk_speed: f32,
    player_prev_walk_speed: f32,
    mesh_dispatcher: Option<MeshDispatcher>,
    paused: bool,
    dead: bool,
    death_message: String,
    death_instant: Instant,
    death_confirm: bool,
    death_confirm_instant: Instant,
    respawn_sent: bool,
    inventory_open: bool,
    chat: ChatState,
    panorama_scroll: f32,
    interaction: InteractionState,
    sky_state: crate::renderer::SkyState,
    show_debug: bool,
    show_chunk_borders: bool,
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
    pending_skin_uuid: Option<uuid::Uuid>,
    item_entity_store: ItemEntityStore,
    resource_packs: crate::resource_pack::ResourcePackManager,
    pending_pack_download: Option<std::thread::JoinHandle<PackDownloadResult>>,
}

struct PackDownloadResult {
    id: uuid::Uuid,
    hash: String,
    required: bool,
    result: Result<std::path::PathBuf, crate::resource_pack::PackError>,
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
        version: String,
        data_dirs: DataDirs,
        tokio_rt: Arc<tokio::runtime::Runtime>,
        presence: Option<crate::discord::DiscordPresence>,
    ) -> Self {
        let (net_events, chat_sender, packet_sender, net_task) = match connection {
            Some(handle) => (
                Some(handle.events),
                Some(handle.chat_tx),
                Some(crate::net::sender::PacketSender::new(handle.packet_tx)),
                Some(handle.task),
            ),
            None => (None, None, None, None),
        };
        let state = if net_events.is_some() {
            GameState::Connecting
        } else {
            GameState::Menu
        };

        let resource_packs = crate::resource_pack::ResourcePackManager::new(&data_dirs.game_dir);
        Self {
            presence,
            display_mode: DisplayMode::Windowed,
            window: None,
            renderer: None,
            input: InputState::new(),
            last_frame: None,
            net_events,
            chat_sender,
            packet_sender,
            net_task,
            chunk_store: ChunkStore::new(DEFAULT_RENDER_DISTANCE),
            entity_store: EntityStore::new(),
            asset_index: AssetIndex::load(&data_dirs.indexes_dir, &data_dirs.objects_dir, &version),
            position_set: false,
            state,
            menu: MainMenu::new(&data_dirs.game_dir, Arc::clone(&tokio_rt)),
            tokio_rt,
            data_dirs,
            version,
            options_from_game: false,
            last_render_distance: DEFAULT_RENDER_DISTANCE,
            server_render_distance: 0,
            server_simulation_distance: 0,
            pending_skin_uuid: None,
            item_entity_store: ItemEntityStore::new(),
            player: LocalPlayer::new(),
            tick_accumulator: 0.0,
            prev_player_pos: glam::Vec3::ZERO,
            biome_climate: Arc::new(std::collections::HashMap::new()),
            player_walk_pos: 0.0,
            player_walk_speed: 0.0,
            player_prev_walk_speed: 0.0,
            mesh_dispatcher: None,
            paused: false,
            dead: false,
            death_message: String::new(),
            death_instant: Instant::now(),
            death_confirm: false,
            death_confirm_instant: Instant::now(),
            respawn_sent: false,
            inventory_open: false,
            chat: ChatState::new(),
            panorama_scroll: 0.0,
            interaction: InteractionState::new(),
            sky_state: crate::renderer::SkyState::default_day(),
            show_debug: false,
            show_chunk_borders: false,
            fps_counter: FpsCounter::new(),
            last_sent_input: PlayerInputState::default(),
            last_sent_pos: glam::Vec3::ZERO,
            last_sent_yaw: 0.0,
            last_sent_pitch: 0.0,
            last_sent_on_ground: false,
            last_sent_horizontal_collision: false,
            was_sprinting: false,
            position_send_counter: 0,
            resource_packs,
            pending_pack_download: None,
        }
    }

    fn sync_render_distance(&mut self) {
        let rd = self.menu.render_distance;
        self.last_render_distance = rd;
        tracing::info!("Render distance changed to {rd}");
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
            && !self.dead
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

    fn apply_display_mode(&self) {
        let Some(window) = &self.window else { return };
        match self.display_mode {
            DisplayMode::Windowed => {
                window.set_fullscreen(None);
                window.set_decorations(true);
            }
            DisplayMode::Borderless => {
                window.set_fullscreen(Some(winit::window::Fullscreen::Borderless(None)));
            }
            DisplayMode::Fullscreen => {
                let monitor = window.current_monitor();
                let video_mode = monitor.and_then(|m| {
                    m.video_modes().max_by_key(|v| {
                        (v.refresh_rate_millihertz(), v.size().width, v.size().height)
                    })
                });
                if let Some(mode) = video_mode {
                    window.set_fullscreen(Some(winit::window::Fullscreen::Exclusive(mode)));
                } else {
                    window.set_fullscreen(Some(winit::window::Fullscreen::Borderless(None)));
                }
            }
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
        self.net_task = Some(handle.task);
        self.state = GameState::Connecting;
        self.apply_cursor_grab();
    }

    fn send_respawn(&mut self) {
        if let Some(sender) = &self.packet_sender {
            sender.send(ServerboundGamePacket::ClientCommand(
                azalea_protocol::packets::game::s_client_command::ServerboundClientCommand {
                    action:
                        azalea_protocol::packets::game::s_client_command::Action::PerformRespawn,
                },
            ));
        }
        self.death_confirm = false;
        self.respawn_sent = true;
    }

    fn disconnect_to_menu(&mut self, reason: Option<String>) {
        self.packet_sender = None;
        self.chat_sender = None;
        self.net_events = None;
        if let Some(task) = self.net_task.take() {
            task.abort();
        }
        self.state = GameState::Menu;
        self.paused = false;
        self.dead = false;
        self.death_message = String::new();
        self.position_set = false;
        self.chunk_store = ChunkStore::new(self.menu.render_distance);
        self.entity_store.clear();
        self.item_entity_store.clear();
        if let Some(renderer) = &mut self.renderer {
            renderer.clear_chunk_meshes();
            self.mesh_dispatcher =
                Some(renderer.create_mesh_dispatcher(self.biome_climate.clone(), None));
        }
        if let Some(reason) = reason {
            self.menu.show_disconnect(reason);
        }
        if let Some(p) = &mut self.presence {
            p.set_in_menu(&self.version);
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
            if processed > 4096 {
                break;
            }
            match event {
                NetworkEvent::Connected => {
                    tracing::info!("Connected to server");
                    self.state = GameState::Loading;
                }
                NetworkEvent::BiomeColors { colors } => {
                    tracing::info!("Received {} biome climate entries", colors.len());
                    self.biome_climate = Arc::new(colors);
                    if let Some(dispatcher) = &mut self.mesh_dispatcher {
                        dispatcher.set_biome_climate(self.biome_climate.clone());
                    }
                }
                NetworkEvent::DimensionInfo { height, min_y } => {
                    tracing::info!("Dimension: height={height}, min_y={min_y}");
                    self.chunk_store =
                        ChunkStore::new_with_dimension(self.menu.render_distance, height, min_y);
                    self.position_set = false;
                    if let Some(renderer) = &mut self.renderer {
                        renderer.clear_chunk_meshes();
                        self.mesh_dispatcher =
                            Some(renderer.create_mesh_dispatcher(self.biome_climate.clone(), None));
                    }
                }
                NetworkEvent::ChunkLoaded {
                    pos,
                    data,
                    heightmaps,
                    sky_light,
                    block_light,
                    sky_y_mask,
                    block_y_mask,
                } => {
                    if let Err(e) = self.chunk_store.load_chunk(pos, &data, &heightmaps) {
                        tracing::error!("Failed to load chunk [{}, {}]: {e}", pos.x, pos.z);
                        continue;
                    }
                    self.chunk_store.store_light(
                        pos,
                        &sky_light,
                        &block_light,
                        &sky_y_mask,
                        &block_y_mask,
                    );
                    chunks_to_mesh.push(pos);
                }
                NetworkEvent::ChunkUnloaded { pos } => {
                    self.chunk_store.unload_chunk(&pos);
                    if let Some(renderer) = &mut self.renderer {
                        renderer.remove_chunk_mesh(&pos);
                    }
                }
                NetworkEvent::ChunkCacheCenter { x, z } => {
                    tracing::debug!("Chunk cache center: [{x}, {z}]");
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
                        tracing::info!("Player position set to ({x:.1}, {y:.1}, {z:.1})");
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
                    if health > 0.0 && self.dead {
                        self.dead = false;
                        self.apply_cursor_grab();
                    } else if health <= 0.0 && !self.dead {
                        self.dead = true;
                        self.death_message = String::new();
                        self.death_instant = Instant::now();
                        self.death_confirm = false;
                        self.respawn_sent = false;
                        if let Some(window) = &self.window {
                            let _ = window.set_cursor_grab(CursorGrabMode::None);
                            window.set_cursor_visible(true);
                        }
                    }
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
                    tracing::info!("Game mode changed to {game_mode}");
                    self.player.game_mode = game_mode;
                }
                NetworkEvent::ServerViewDistance { distance } => {
                    tracing::info!("Server view distance: {distance}");
                    self.server_render_distance = distance;
                }
                NetworkEvent::ServerSimulationDistance { distance } => {
                    tracing::info!("Server simulation distance: {distance}");
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
                NetworkEvent::EntitySpawned {
                    id,
                    entity_type,
                    x,
                    y,
                    z,
                    yaw,
                    pitch,
                    head_yaw,
                    velocity,
                } => {
                    if crate::entity::is_living_mob(&entity_type) {
                        self.entity_store.spawn_living(
                            id,
                            entity_type,
                            glam::DVec3::new(x, y, z),
                            yaw,
                            pitch,
                            head_yaw,
                        );
                    }
                    if entity_type == azalea_registry::builtin::EntityKind::Item {
                        let pos = glam::DVec3::new(x, y, z);
                        let vel = glam::DVec3::new(velocity[0], velocity[1], velocity[2]);
                        self.item_entity_store.spawn_item(id, pos, vel);
                    }
                }
                NetworkEvent::EntityMoved { id, dx, dy, dz } => {
                    self.entity_store.move_living_delta(id, dx, dy, dz);
                    self.item_entity_store.move_delta(id, dx, dy, dz);
                }
                NetworkEvent::EntityMovedRotated {
                    id,
                    dx,
                    dy,
                    dz,
                    yaw,
                    pitch,
                } => {
                    self.entity_store.move_living_delta(id, dx, dy, dz);
                    self.entity_store.update_living_rotation(id, yaw, pitch);
                    self.item_entity_store.move_delta(id, dx, dy, dz);
                }
                NetworkEvent::EntityTeleported {
                    id,
                    x,
                    y,
                    z,
                    yaw,
                    pitch,
                } => {
                    self.entity_store.teleport_living(id, x, y, z);
                    self.entity_store.update_living_rotation(id, yaw, pitch);
                    self.item_entity_store
                        .teleport(id, glam::DVec3::new(x, y, z));
                }
                NetworkEvent::EntitiesRemoved { ids } => {
                    for id in &ids {
                        self.entity_store.remove_living(*id);
                    }
                    self.item_entity_store.remove(&ids);
                }
                NetworkEvent::EntityHeadRotation { id, head_yaw } => {
                    self.entity_store.update_head_rotation(id, head_yaw);
                }
                NetworkEvent::EntityItemData {
                    id,
                    item_name,
                    count,
                } => {
                    let is_block_model = self
                        .renderer
                        .as_mut()
                        .map(|r| r.ensure_item_mesh(&item_name))
                        .unwrap_or(false);
                    self.item_entity_store
                        .set_item_data(id, item_name, count, is_block_model);
                }
                NetworkEvent::EntityBabyFlag { id, is_baby } => {
                    self.entity_store.set_baby(id, is_baby);
                }
                NetworkEvent::ItemPickedUp {
                    item_id,
                    collector_id,
                } => {
                    let target_pos = self
                        .entity_store
                        .living
                        .get(&collector_id)
                        .map(|e| e.position + glam::DVec3::new(0.0, 0.81, 0.0))
                        .unwrap_or_else(|| {
                            glam::DVec3::new(
                                self.player.position.x as f64,
                                self.player.position.y as f64 + 0.81,
                                self.player.position.z as f64,
                            )
                        });
                    self.item_entity_store.pickup(item_id, target_pos);
                }
                NetworkEvent::PlayerLogin { entity_id } => {
                    self.player.entity_id = entity_id;
                }
                NetworkEvent::PlayerScore { entity_id, score } => {
                    if entity_id == self.player.entity_id {
                        self.player.score = score;
                    }
                }
                NetworkEvent::PlayerDied { message } => {
                    self.dead = true;
                    self.death_message = message;
                    self.death_instant = Instant::now();
                    self.death_confirm = false;
                    self.respawn_sent = false;
                    if let Some(window) = &self.window {
                        let _ = window.set_cursor_grab(CursorGrabMode::None);
                        window.set_cursor_visible(true);
                    }
                }
                NetworkEvent::ResourcePackPush {
                    id,
                    url,
                    hash,
                    required,
                } => {
                    tracing::info!("Resource pack push: {id} url={url} required={required}");
                    let cache_dir = self.resource_packs.server_cache_dir().to_path_buf();
                    self.pending_pack_download = Some(std::thread::spawn(move || {
                        let result =
                            crate::resource_pack::ResourcePackManager::download_server_pack(
                                &cache_dir, &url, &hash,
                            );
                        PackDownloadResult {
                            id,
                            hash,
                            required,
                            result,
                        }
                    }));
                }
                NetworkEvent::ResourcePackPop { id } => {
                    if let Some(id) = id {
                        self.resource_packs.remove_server_pack(&id);
                    } else {
                        self.resource_packs.clear_server_packs();
                    }
                    self.menu.active_packs = self.resource_packs.active_pack_info();
                    self.menu.reload_assets = true;
                }
                NetworkEvent::Disconnected { reason } => {
                    tracing::warn!("Disconnected: {reason}");
                    disconnect_reason = Some(reason);
                }
            }
        }

        if let Some(handle) = &self.pending_pack_download
            && handle.is_finished()
        {
            let handle = self.pending_pack_download.take().unwrap();
            let dl = handle.join().expect("pack download thread panicked");
            use azalea_protocol::packets::game::s_resource_pack;
            let action = match &dl.result {
                Ok(_) => {
                    self.resource_packs.apply_server_pack(dl.id, &dl.hash);
                    tracing::info!("Resource pack {} loaded successfully", dl.id);
                    self.menu.reload_assets = true;
                    s_resource_pack::Action::SuccessfullyLoaded
                }
                Err(e) => {
                    tracing::error!("Resource pack {} failed: {e}", dl.id);
                    if dl.required {
                        disconnect_reason = Some(format!("Required resource pack failed: {e}"));
                    }
                    s_resource_pack::Action::FailedDownload
                }
            };
            if let Some(sender) = &self.packet_sender {
                sender.send(ServerboundGamePacket::ResourcePack(
                    s_resource_pack::ServerboundResourcePack { id: dl.id, action },
                ));
            }
            self.menu.active_packs = self.resource_packs.active_pack_info();
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
        if self.dead {
            return;
        }
        if let Some(renderer) = &self.renderer {
            self.player.yaw = if renderer.is_first_person() {
                renderer.camera_yaw()
            } else {
                renderer.camera_yaw() + std::f32::consts::PI
            };
            self.player.pitch = renderer.camera_pitch();
        }

        self.prev_player_pos = self.player.position;
        movement::tick(&mut self.player, &self.input, &self.chunk_store);
        self.entity_store.tick_living();

        let dx = (self.player.position.x - self.prev_player_pos.x) as f64;
        let dz = (self.player.position.z - self.prev_player_pos.z) as f64;
        crate::entity::update_walk_animation(
            dx,
            dz,
            &mut self.player_walk_pos,
            &mut self.player_walk_speed,
            &mut self.player_prev_walk_speed,
        );

        if let Some(renderer) = &mut self.renderer {
            renderer.set_base_fov(self.menu.fov as f32);
            renderer.update_fov(compute_fov_modifier(&self.player));
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
                    id: azalea_core::entity_id::MinecraftEntityId(0),
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
                ServerboundMovePlayerPosRot {
                    pos: net_pos,
                    look_direction: look,
                    flags,
                },
            ));
        } else if pos_changed {
            sender.send(ServerboundGamePacket::MovePlayerPos(
                ServerboundMovePlayerPos {
                    pos: net_pos,
                    flags,
                },
            ));
        } else if rot_changed {
            sender.send(ServerboundGamePacket::MovePlayerRot(
                ServerboundMovePlayerRot {
                    look_direction: look,
                    flags,
                },
            ));
        } else if self.player.on_ground != self.last_sent_on_ground
            || self.player.horizontal_collision != self.last_sent_horizontal_collision
        {
            sender.send(ServerboundGamePacket::MovePlayerStatusOnly(
                ServerboundMovePlayerStatusOnly { flags },
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

        let window_icon = {
            let png = include_bytes!("../../assets/icon.png");
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
                return;
            }
        };

        let mut renderer = match Renderer::new(
            Arc::clone(&window),
            &self.data_dirs.jar_assets_dir,
            &self.asset_index,
            &self.data_dirs.game_dir,
        ) {
            Ok(r) => r,
            Err(e) => {
                tracing::error!("Failed to create renderer: {e}");
                event_loop.exit();
                return;
            }
        };

        if let Some(p) = &mut self.presence {
            p.set_in_menu(&self.version);
        }

        self.mesh_dispatcher =
            Some(renderer.create_mesh_dispatcher(self.biome_climate.clone(), None));
        if let Some(uuid) = self.pending_skin_uuid.take() {
            renderer.load_player_skin(&uuid, &self.tokio_rt);
        }
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
                if event.state.is_pressed()
                    && let PhysicalKey::Code(KeyCode::F11) = event.physical_key
                {
                    self.display_mode = self.display_mode.cycle();
                    self.menu.display_mode = self.display_mode;
                    self.apply_display_mode();
                }
                match self.state {
                    GameState::Menu => self.input.on_menu_key_event(&event),
                    GameState::Connecting | GameState::Loading => {
                        if event.state.is_pressed()
                            && let PhysicalKey::Code(KeyCode::Escape) = event.physical_key
                        {
                            self.disconnect_to_menu(None);
                        }
                    }
                    GameState::InGame => {
                        if event.state.is_pressed()
                            && !event.repeat
                            && let PhysicalKey::Code(code) = event.physical_key
                        {
                            if self.chat.is_open() {
                                match code {
                                    KeyCode::Escape => {
                                        self.chat.close();
                                        self.apply_cursor_grab();
                                    }
                                    KeyCode::F3 => self.show_debug = !self.show_debug,
                                    _ => self.input.on_menu_key_event(&event),
                                }
                            } else {
                                match code {
                                    KeyCode::Escape
                                        if self.death_confirm
                                            && self
                                                .death_confirm_instant
                                                .elapsed()
                                                .as_secs_f32()
                                                >= 1.0 =>
                                    {
                                        self.death_confirm = false;
                                        self.send_respawn();
                                    }
                                    KeyCode::Escape if !self.dead => {
                                        if self.inventory_open {
                                            self.inventory_open = false;
                                        } else {
                                            self.paused = !self.paused;
                                        }
                                        self.apply_cursor_grab();
                                    }
                                    KeyCode::KeyE if !self.paused && !self.dead => {
                                        self.inventory_open = !self.inventory_open;
                                        self.apply_cursor_grab();
                                    }
                                    KeyCode::KeyT if !self.paused && !self.inventory_open => {
                                        self.chat.open();
                                        self.apply_cursor_grab();
                                    }
                                    KeyCode::Slash if !self.paused && !self.inventory_open => {
                                        self.chat.open_with_slash();
                                        self.apply_cursor_grab();
                                    }
                                    KeyCode::F3 => {
                                        self.show_debug = !self.show_debug;
                                    }
                                    KeyCode::KeyG if self.input.key_pressed(KeyCode::F3) => {
                                        self.show_chunk_borders = !self.show_chunk_borders;
                                    }
                                    KeyCode::F5 => {
                                        if let Some(r) = &mut self.renderer {
                                            r.cycle_camera_mode();
                                        }
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
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let scroll = match delta {
                    winit::event::MouseScrollDelta::LineDelta(_, y) => y,
                    winit::event::MouseScrollDelta::PixelDelta(p) => p.y as f32,
                };
                if matches!(
                    self.state,
                    GameState::Menu | GameState::Connecting | GameState::Loading
                ) {
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
                if matches!(
                    self.state,
                    GameState::Menu | GameState::Connecting | GameState::Loading
                ) || self.paused
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
                            self.panorama_scroll += dt * 0.00556;
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

                                if self.menu.is_server_list_screen() && self.menu.favicons_changed()
                                {
                                    let favicons = self.menu.collect_favicons();
                                    if !favicons.is_empty() {
                                        renderer.update_favicon_atlas(&favicons);
                                    }
                                }

                                if let Err(e) = renderer.render_menu(
                                    window,
                                    self.panorama_scroll,
                                    result.blur,
                                    result.elements,
                                    self.input.cursor_pos(),
                                    self.menu.is_main_screen(),
                                ) {
                                    tracing::error!("Render error: {e}");
                                }

                                self.input.clear_click_events();

                                if self.menu.render_distance != self.last_render_distance {
                                    self.sync_render_distance();
                                }

                                if self.menu.display_mode != self.display_mode {
                                    self.display_mode = self.menu.display_mode;
                                    self.apply_display_mode();
                                }

                                if self.menu.rescan_packs {
                                    self.menu.rescan_packs = false;
                                    self.resource_packs.scan_local_packs();
                                    self.menu.available_packs =
                                        self.resource_packs.available_local_packs().to_vec();
                                    self.menu.active_packs = self.resource_packs.active_pack_info();
                                }

                                if let Some((name, enable)) = self.menu.pack_toggle.take() {
                                    if enable {
                                        self.resource_packs.enable_local_pack(&name);
                                    } else {
                                        self.resource_packs.disable_local_pack(&name);
                                    }
                                    self.menu.active_packs = self.resource_packs.active_pack_info();
                                    self.menu.available_packs =
                                        self.resource_packs.available_local_packs().to_vec();
                                }

                                if self.menu.reload_assets {
                                    self.menu.reload_assets = false;
                                    if let Some(renderer) = &mut self.renderer {
                                        renderer.reload_assets(
                                            &self.data_dirs.game_dir,
                                            &self.resource_packs,
                                        );
                                        if let Some(ref mut dispatcher) = self.mesh_dispatcher {
                                            *dispatcher = renderer.create_mesh_dispatcher(
                                                self.biome_climate.clone(),
                                                Some(&self.resource_packs),
                                            );
                                            for pos in self.chunk_store.loaded_positions() {
                                                dispatcher.enqueue(&self.chunk_store, pos, 0);
                                            }
                                        }
                                    }
                                }

                                if result.clicked_button
                                    && let Some(renderer) = &mut self.renderer
                                {
                                    renderer.trigger_skin_swing();
                                }

                                match action {
                                    MenuAction::Connect { server, username } => {
                                        self.connect_to_server(server, username);
                                    }
                                    MenuAction::ChangeTheme(theme) => {
                                        if let Some(renderer) = &mut self.renderer {
                                            let panorama_dir = match theme {
                                                PanoramaTheme::Default => {
                                                    self.data_dirs.jar_assets_dir.clone()
                                                }
                                                PanoramaTheme::Pomme => self
                                                    .data_dirs
                                                    .pomme_assets_dir
                                                    .join("panoramas"),
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
                        GameState::Connecting | GameState::Loading => {
                            self.drain_network_events();
                            if matches!(self.state, GameState::Menu) {
                                break 'redraw;
                            }

                            if matches!(self.state, GameState::Loading) {
                                if let (Some(dispatcher), Some(renderer)) =
                                    (&self.mesh_dispatcher, &mut self.renderer)
                                {
                                    for mesh in dispatcher.drain_results() {
                                        renderer.upload_chunk_mesh(&mesh);
                                    }
                                }

                                let ready = self.position_set
                                    && (self.dead
                                        || self
                                            .renderer
                                            .as_ref()
                                            .is_some_and(|r| r.loaded_chunk_count() > 0));

                                if ready {
                                    if let Some(p) = &mut self.presence {
                                        p.playing_multiplayer(&self.version);
                                    }
                                    self.state = GameState::InGame;
                                    self.apply_cursor_grab();
                                    break 'redraw;
                                }
                            }

                            let status_text = if matches!(self.state, GameState::Loading) {
                                "Loading terrain..."
                            } else {
                                "Connecting to the server..."
                            };

                            self.panorama_scroll += dt * 0.00556;
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
                                let clicked = self.input.left_just_pressed();
                                let cursor = self.input.cursor_pos();

                                elements.push(MenuElement::Text {
                                    x: cx,
                                    y: cy - fs,
                                    text: status_text.into(),
                                    scale: fs,
                                    color: WHITE,
                                    centered: true,
                                });

                                let btn_y = cy + fs;
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
                                    false,
                                ) {
                                    tracing::error!("Render error: {e}");
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
                                    self.item_entity_store.tick(
                                        |bx, by, bz| {
                                            !self.chunk_store.get_block_state(bx, by, bz).is_air()
                                        },
                                        |bx, by, bz| {
                                            block_friction(
                                                self.chunk_store.get_block_state(bx, by, bz),
                                            )
                                        },
                                    );
                                    self.tick_accumulator -= TICK_RATE;
                                }
                            }

                            let alpha = self.tick_accumulator / TICK_RATE;
                            let interp_pos = self.prev_player_pos.lerp(self.player.position, alpha);
                            let eye_pos = interp_pos + glam::Vec3::new(0.0, 1.62, 0.0);
                            let eye_pos_f64 = glam::DVec3::new(
                                eye_pos.x as f64,
                                eye_pos.y as f64,
                                eye_pos.z as f64,
                            );

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
                            let mut death_action = DeathAction::None;

                            if let (Some(renderer), Some(window)) =
                                (&mut self.renderer, &self.window)
                            {
                                renderer.sync_camera_to_player(
                                    eye_pos_f64,
                                    renderer.camera_yaw(),
                                    renderer.camera_pitch(),
                                );
                                renderer.update_third_person_distance(eye_pos, &self.chunk_store);

                                let sw = renderer.screen_width() as f32;
                                let sh = renderer.screen_height() as f32;
                                let gs = hud::gui_scale(sw, sh, self.menu.gui_scale_setting);

                                let mut elements: Vec<MenuElement> = Vec::new();
                                let hide_cursor = !self.paused
                                    && !self.dead
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
                                            frame_ms: renderer.last_timings.frame_ms,
                                            fence_ms: renderer.last_timings.fence_ms,
                                            acquire_ms: renderer.last_timings.acquire_ms,
                                            cull_ms: renderer.last_timings.cull_ms,
                                            draw_ms: renderer.last_timings.draw_ms,
                                            present_ms: renderer.last_timings.present_ms,
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
                                    self.player.inventory.hotbar_slots(),
                                    renderer.is_first_person(),
                                    debug.as_ref(),
                                    self.menu.gui_scale_setting,
                                );

                                if self.options_from_game {
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
                                    let r = &*renderer;
                                    let result = self
                                        .menu
                                        .build(sw, sh, &menu_input, |t, s| r.menu_text_width(t, s));
                                    elements.extend(result.elements);
                                    self.input.clear_click_events();
                                } else if self.dead {
                                    let cursor = self.input.cursor_pos();
                                    let clicked =
                                        self.input.left_just_pressed() && !self.respawn_sent;
                                    death_action = if self.death_confirm {
                                        death::build_death_confirm(
                                            &mut elements,
                                            sw,
                                            sh,
                                            cursor,
                                            clicked,
                                            gs,
                                            self.death_confirm_instant.elapsed().as_secs_f32()
                                                >= 1.0,
                                        )
                                    } else {
                                        let buttons_enabled = !self.respawn_sent
                                            && self.death_instant.elapsed().as_secs_f32() >= 1.0;
                                        let r = &*renderer;
                                        death::build_death_screen(
                                            &mut elements,
                                            sw,
                                            sh,
                                            cursor,
                                            clicked,
                                            gs,
                                            &self.death_message,
                                            self.player.score,
                                            buttons_enabled,
                                            &|t, s| r.menu_text_width(t, s),
                                        )
                                    };
                                    self.input.clear_click_events();
                                } else if self.paused {
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

                                let alpha = self.tick_accumulator / TICK_RATE;
                                let mut entity_renders: Vec<EntityRenderInfo> = self
                                    .entity_store
                                    .living
                                    .values()
                                    .map(|e| {
                                        let pos = e.prev_position.lerp(e.position, alpha as f64);
                                        let body_yaw = e.prev_body_yaw
                                            + (e.body_yaw - e.prev_body_yaw) * alpha;
                                        let head_yaw = e.prev_head_yaw
                                            + (e.head_yaw - e.prev_head_yaw) * alpha;
                                        EntityRenderInfo {
                                            x: pos.x,
                                            y: pos.y,
                                            z: pos.z,
                                            yaw: body_yaw,
                                            pitch: e.prev_pitch + (e.pitch - e.prev_pitch) * alpha,
                                            head_yaw,
                                            is_baby: e.is_baby,
                                            walk_anim_pos: {
                                                let scale = if e.is_baby { 3.0 } else { 1.0 };
                                                (e.walk_anim_pos
                                                    - e.walk_anim_speed * (1.0 - alpha))
                                                    * scale
                                            },
                                            walk_anim_speed: (e.prev_walk_anim_speed
                                                + (e.walk_anim_speed - e.prev_walk_anim_speed)
                                                    * alpha)
                                                .min(1.0),
                                            entity_kind: e.entity_type,
                                        }
                                    })
                                    .collect();

                                if !renderer.is_first_person() {
                                    let cam_yaw_deg = -renderer.camera_yaw().to_degrees();
                                    entity_renders.push(EntityRenderInfo {
                                        x: interp_pos.x as f64,
                                        y: interp_pos.y as f64,
                                        z: interp_pos.z as f64,
                                        yaw: cam_yaw_deg,
                                        pitch: renderer.camera_pitch().to_degrees(),
                                        head_yaw: cam_yaw_deg,
                                        is_baby: false,
                                        walk_anim_pos: self.player_walk_pos
                                            - self.player_walk_speed * (1.0 - alpha),
                                        walk_anim_speed: (self.player_prev_walk_speed
                                            + (self.player_walk_speed
                                                - self.player_prev_walk_speed)
                                                * alpha)
                                            .min(1.0),
                                        entity_kind: azalea_registry::builtin::EntityKind::Player,
                                    });
                                }

                                let sky = crate::renderer::SkyState {
                                    day_time: self.sky_state.day_time,
                                    game_time: self.sky_state.game_time,
                                    rain_level: self.sky_state.rain_level,
                                };
                                if self.show_chunk_borders {
                                    renderer.update_chunk_borders(
                                        self.chunk_store.min_y(),
                                        self.chunk_store.min_y() + self.chunk_store.height() as i32,
                                    );
                                }

                                let cam_pos = glam::DVec3::new(
                                    self.player.position.x as f64,
                                    self.player.position.y as f64,
                                    self.player.position.z as f64,
                                );
                                let partial_tick = self.tick_accumulator / TICK_RATE;
                                let item_renders = build_item_render_infos(
                                    &self.item_entity_store,
                                    &self.chunk_store,
                                    cam_pos,
                                    partial_tick,
                                );

                                if let Err(e) = renderer.render_world(
                                    window,
                                    hide_cursor,
                                    elements,
                                    swing_progress,
                                    destroy_info,
                                    self.show_chunk_borders,
                                    sky,
                                    &entity_renders,
                                    &item_renders,
                                ) {
                                    tracing::error!("Render error: {e}");
                                }
                            }

                            if close_inventory {
                                self.inventory_open = false;
                                self.apply_cursor_grab();
                            }

                            match death_action {
                                DeathAction::Respawn => {
                                    self.death_confirm = false;
                                    self.send_respawn();
                                }
                                DeathAction::TitleScreen => {
                                    self.disconnect_to_menu(None);
                                }
                                DeathAction::ShowConfirm => {
                                    self.death_confirm = true;
                                    self.death_confirm_instant = Instant::now();
                                }
                                DeathAction::None => {}
                            }

                            match pause_action {
                                PauseAction::Resume => {
                                    self.paused = false;
                                    self.apply_cursor_grab();
                                }
                                PauseAction::Options => {
                                    self.menu.open_options();
                                    self.options_from_game = true;
                                    self.paused = false;
                                    self.apply_cursor_grab();
                                }
                                PauseAction::Disconnect => {
                                    self.disconnect_to_menu(None);
                                }
                                PauseAction::None => {}
                            }

                            if self.options_from_game {
                                if self.menu.render_distance != self.last_render_distance {
                                    self.sync_render_distance();
                                }
                                if !self.menu.is_options_screen() {
                                    self.options_from_game = false;
                                    self.paused = true;
                                    self.apply_cursor_grab();
                                }
                            }
                        }
                    }
                } // 'redraw

                if let Some(window) = &self.window {
                    if !window.is_visible().unwrap_or(true) {
                        window.set_visible(true);
                    }
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
        if let DeviceEvent::MouseMotion { delta } = event
            && self.input.is_cursor_captured()
            && !self.paused
            && !self.dead
            && !self.inventory_open
            && !self.chat.is_open()
        {
            self.input.on_mouse_motion(delta);
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        event_loop.set_control_flow(winit::event_loop::ControlFlow::Poll);
    }
}

pub struct LaunchAuth {
    pub username: String,
    pub uuid: uuid::Uuid,
    pub access_token: String,
}

fn compute_fov_modifier(player: &LocalPlayer) -> f32 {
    let base_walk_speed = 0.1;
    let mut speed = base_walk_speed;
    if player.sprinting {
        speed *= 1.3;
    }

    let mut modifier = (speed / base_walk_speed + 1.0) / 2.0;

    if player.game_mode == 1 && player.sprinting {
        modifier *= 1.1;
    }

    modifier
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

fn block_friction(state: azalea_block::BlockState) -> f32 {
    let block: Box<dyn azalea_block::BlockTrait> = state.into();
    match block.id() {
        "ice" | "packed_ice" | "frosted_ice" => 0.98,
        "blue_ice" => 0.989,
        "slime_block" => 0.8,
        _ => 0.6,
    }
}

fn stack_render_count(count: i32) -> usize {
    if count <= 1 {
        1
    } else if count <= 16 {
        2
    } else if count <= 32 {
        3
    } else if count <= 48 {
        4
    } else {
        5
    }
}

fn seeded_rand(state: &mut u32) -> f32 {
    *state = state.wrapping_mul(1103515245).wrapping_add(12345);
    ((*state >> 16) & 0x7FFF) as f32 / 0x7FFF as f32
}

fn get_entity_light(chunk_store: &ChunkStore, pos: glam::DVec3) -> f32 {
    use crate::renderer::chunk::mesher::LIGHT_TABLE;
    let bx = pos.x.floor() as i32;
    let by = pos.y.floor() as i32;
    let bz = pos.z.floor() as i32;
    let level = chunk_store
        .get_sky_light(bx, by, bz)
        .max(chunk_store.get_block_light(bx, by, bz));
    LIGHT_TABLE[level as usize]
}

fn build_item_render_infos(
    entity_store: &crate::entity::ItemEntityStore,
    chunk_store: &ChunkStore,
    camera_pos: glam::DVec3,
    partial_tick: f32,
) -> Vec<crate::renderer::pipelines::item_entity::ItemRenderInfo> {
    let mut infos = Vec::new();
    for item in entity_store.visible_items(camera_pos, 64.0) {
        let age_f = item.age as f32 + partial_tick;
        let bob = (age_f / 10.0 + item.bob_offset).sin() * 0.1 + 0.1;
        let spin = age_f / 20.0 + item.bob_offset;
        let lerped = item.prev_position.lerp(item.position, partial_tick as f64);
        let pos = lerped.as_vec3();
        let light = get_entity_light(chunk_store, lerped);
        let copies = stack_render_count(item.count);

        // Vanilla GROUND display transform: blocks scale=0.25, flat items scale=0.5
        // Hover = bob + (-boundingBox.minY) + 0.0625
        // Block model: minY after scale = -0.5 * 0.25 = -0.125 → hover = bob + 0.1875
        // Flat item: minY after scale = -0.5 * 0.5 = -0.25 → hover = bob + 0.3125
        let (scale, hover_y) = if item.is_block_model {
            (0.25, bob + 0.1875)
        } else {
            (0.5, bob + 0.3125)
        };

        let base = glam::Mat4::from_translation(pos + glam::Vec3::new(0.0, hover_y, 0.0))
            * glam::Mat4::from_rotation_y(spin);

        let mut rng_state = (item.bob_offset * 1000.0) as u32;
        if item.is_block_model {
            for i in 0..copies {
                let copy_offset = if i == 0 {
                    glam::Mat4::IDENTITY
                } else {
                    let rx = seeded_rand(&mut rng_state) * 0.3 - 0.15;
                    let ry = seeded_rand(&mut rng_state) * 0.3 - 0.15;
                    let rz = seeded_rand(&mut rng_state) * 0.3 - 0.15;
                    glam::Mat4::from_translation(glam::Vec3::new(rx, ry, rz))
                };
                let model = base * copy_offset * glam::Mat4::from_scale(glam::Vec3::splat(scale));
                infos.push(crate::renderer::pipelines::item_entity::ItemRenderInfo {
                    item_name: item.item_name.clone(),
                    model_matrix: model,
                    light,
                });
            }
        } else {
            let depth = 1.0 / 16.0 * scale;
            let z_step = depth * 1.5;
            let z_start = -(z_step * (copies - 1) as f32 / 2.0);
            for i in 0..copies {
                let z_offset = z_start + z_step * i as f32;
                let copy_offset = if i == 0 {
                    glam::Mat4::from_translation(glam::Vec3::new(0.0, 0.0, z_offset))
                } else {
                    let rx = (seeded_rand(&mut rng_state) * 2.0 - 1.0) * 0.15 * 0.5;
                    let ry = (seeded_rand(&mut rng_state) * 2.0 - 1.0) * 0.15 * 0.5;
                    glam::Mat4::from_translation(glam::Vec3::new(rx, ry, z_offset))
                };
                let model = base * copy_offset * glam::Mat4::from_scale(glam::Vec3::splat(scale));
                infos.push(crate::renderer::pipelines::item_entity::ItemRenderInfo {
                    item_name: item.item_name.clone(),
                    model_matrix: model,
                    light,
                });
            }
        }
    }

    for pickup in entity_store.active_pickups(partial_tick) {
        let pos = pickup.position.as_vec3();
        let light = get_entity_light(chunk_store, pickup.position);
        let age_f = pickup.age as f32 + partial_tick;
        let spin = age_f / 20.0 + pickup.bob_offset;
        let scale = if pickup.is_block_model { 0.25 } else { 0.5 };
        let model = glam::Mat4::from_translation(pos)
            * glam::Mat4::from_rotation_y(spin)
            * glam::Mat4::from_scale(glam::Vec3::splat(scale));
        infos.push(crate::renderer::pipelines::item_entity::ItemRenderInfo {
            item_name: pickup.item_name,
            model_matrix: model,
            light,
        });
    }

    infos
}

pub fn run(
    connection: Option<crate::net::connection::ConnectionHandle>,
    version: String,
    data_dirs: DataDirs,
    tokio_rt: Arc<tokio::runtime::Runtime>,
    auth: Option<LaunchAuth>,
    presence: Option<crate::discord::DiscordPresence>,
) -> Result<(), WindowError> {
    let event_loop = EventLoop::new()?;
    let mut app = App::new(connection, version, data_dirs, tokio_rt, presence);
    if let Some(auth) = auth {
        app.pending_skin_uuid = Some(auth.uuid);
        app.menu
            .set_launch_auth(auth.username, auth.uuid, auth.access_token);
    }

    event_loop.run_app(&mut app)?;
    Ok(())
}
