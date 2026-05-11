pub mod app;
pub mod input;
pub mod state_slot;

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
use crate::benchmark::Benchmark;
use crate::dirs::DataDirs;
use crate::discord::DiscordPresence;
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
use crate::user::UserData;
use crate::window::app::{AppState, ConnectingState, Runtime};
use crate::window::state_slot::StateSlot;
use crate::world::chunk::ChunkStore;

pub struct AppCtx {
    user: UserData,
    presence: Option<DiscordPresence>,
    display_mode: DisplayMode,
    input: InputState,
    menu: MainMenu,
    tokio_rt: Arc<tokio::runtime::Runtime>,

    net_events: Option<crossbeam_channel::Receiver<NetworkEvent>>,
    chat_sender: Option<crossbeam_channel::Sender<String>>,
    packet_sender: Option<crate::net::sender::PacketSender>,
    net_task: Option<tokio::task::JoinHandle<()>>,
    chunk_store: ChunkStore,
    entity_store: EntityStore,
    data_dirs: DataDirs,
    asset_index: Option<AssetIndex>,
    position_set: bool,
    player_loaded_sent: bool,
    version: String,
    player: LocalPlayer,
    tick_accumulator: f32,
    time_tick_accumulator: f32,
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
    tab_list: crate::player::tab_list::TabList,
    panorama_scroll: f32,
    interaction: InteractionState,
    sky_state: crate::renderer::SkyState,
    show_debug: bool,
    show_chunk_borders: bool,
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
    benchmark: Option<crate::benchmark::Benchmark>,
    benchmark_result: Option<crate::benchmark::BenchmarkResult>,
    last_player_chunk: azalea_core::position::ChunkPos,
    meshed_lod: std::collections::HashMap<azalea_core::position::ChunkPos, u32>,
}

impl AppCtx {
    pub fn new(
        connection: Option<crate::net::connection::ConnectionHandle>,
        version: String,
        data_dirs: DataDirs,
        tokio_rt: Arc<tokio::runtime::Runtime>,
        presence: Option<crate::discord::DiscordPresence>,
        user: UserData,
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

        let resource_packs = crate::resource_pack::ResourcePackManager::new(&data_dirs.game_dir);

        let username = user.username.clone();

        Self {
            user,
            presence,
            display_mode: DisplayMode::Windowed,
            input: InputState::new(),
            menu: MainMenu::new(&data_dirs.game_dir, Arc::clone(&tokio_rt), username),
            tokio_rt,

            net_events,
            chat_sender,
            packet_sender,
            net_task,
            chunk_store: ChunkStore::new(DEFAULT_RENDER_DISTANCE),
            entity_store: EntityStore::new(),
            asset_index: AssetIndex::load(&data_dirs.indexes_dir, &data_dirs.objects_dir, &version),
            position_set: false,
            player_loaded_sent: false,
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
            time_tick_accumulator: 0.0,
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
            tab_list: crate::player::tab_list::TabList::new(),
            panorama_scroll: 0.0,
            interaction: InteractionState::new(),
            sky_state: crate::renderer::SkyState::default_day(),
            show_debug: false,
            show_chunk_borders: false,
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
            benchmark: None,
            benchmark_result: None,
            last_player_chunk: azalea_core::position::ChunkPos::new(0, 0),
            meshed_lod: std::collections::HashMap::new(),
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

    fn apply_display_mode(&mut self, window: &Window) {
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

    fn apply_cursor_grab(&self, ingame_state: bool, window: &Window) {
        let captured = ingame_state
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

    fn send_chat_message(&self, msg: String) {
        if let Some(tx) = &self.chat_sender {
            let _ = tx.try_send(msg);
        }
    }

    fn drain_network_events(
        &mut self,
        renderer: &mut Renderer,
        window: &Window,
        mut connecting_state: Option<&mut ConnectingState>,
    ) -> Option<String> {
        let Some(rx) = &self.net_events else {
            return None;
        };
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
                    if let Some(state) = connecting_state.as_deref_mut() {
                        tracing::info!("Connected to server");
                        *state = ConnectingState::Loading;
                    } else {
                        tracing::warn!("Unexpected NetworkEvent::Connected, skipping");
                    }
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
                    self.player_loaded_sent = false;

                    renderer.clear_chunk_meshes();
                    self.mesh_dispatcher =
                        Some(renderer.create_mesh_dispatcher(self.biome_climate.clone(), None));
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
                    self.meshed_lod.remove(&pos);

                    renderer.remove_chunk_mesh(&pos);
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

                        renderer.set_camera_position(x, y, z, yaw, pitch);

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
                        self.apply_cursor_grab(connecting_state.is_none(), window);
                    } else if health <= 0.0 && !self.dead {
                        self.dead = true;
                        self.death_message = String::new();
                        self.death_instant = Instant::now();
                        self.death_confirm = false;
                        self.respawn_sent = false;

                        let _ = window.set_cursor_grab(CursorGrabMode::None);
                        window.set_cursor_visible(true);
                    }
                }
                NetworkEvent::PlayerExperience { progress, level } => {
                    self.player.experience_progress = progress;
                    self.player.experience_level = level;
                }
                NetworkEvent::EntityArmorUpdate { entity_id, armor } => {
                    if entity_id == self.player.entity_id {
                        self.player.armor = armor;
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
                    if let Some(dt) = day_time {
                        self.sky_state.day_time = dt;
                    }
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
                    let is_block_model = renderer.ensure_item_mesh(&item_name);

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

                    let _ = window.set_cursor_grab(CursorGrabMode::None);
                    window.set_cursor_visible(true);
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
                    self.tab_list.clear();
                }
                NetworkEvent::PlayerInfoUpdate { actions, entries } => {
                    self.tab_list.apply_update(&actions, &entries);
                }
                NetworkEvent::PlayerInfoRemove { uuids } => {
                    self.tab_list.remove(&uuids);
                }
                NetworkEvent::TabListHeaderFooter { header, footer } => {
                    self.tab_list.set_header_footer(header, footer);
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

        if let Some(dispatcher) = &self.mesh_dispatcher {
            let player_chunk = azalea_core::position::ChunkPos::new(
                (self.player.position.x as i32).div_euclid(16),
                (self.player.position.z as i32).div_euclid(16),
            );
            for pos in chunks_to_mesh {
                let lod = chunk_lod(pos, player_chunk);
                self.meshed_lod.insert(pos, lod);
                dispatcher.enqueue(&self.chunk_store, pos, lod);
            }

            if player_chunk != self.last_player_chunk {
                self.last_player_chunk = player_chunk;
                for pos in self.chunk_store.loaded_positions() {
                    let new_lod = chunk_lod(pos, player_chunk);
                    let old_lod = self.meshed_lod.get(&pos).copied();
                    if old_lod != Some(new_lod) {
                        self.meshed_lod.insert(pos, new_lod);
                        dispatcher.enqueue(&self.chunk_store, pos, new_lod);
                    }
                }
            }
        }

        disconnect_reason
    }

    fn tick_physics(&mut self, renderer: &mut Renderer) {
        if self.dead {
            return;
        }

        self.player.yaw = if renderer.is_first_person() {
            renderer.camera_yaw()
        } else {
            renderer.camera_yaw() + std::f32::consts::PI
        };
        self.player.pitch = renderer.camera_pitch();

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

        renderer.set_base_fov(self.menu.fov as f32);
        renderer.update_fov(compute_fov_modifier(&self.player));

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

#[derive(Error, Debug)]
pub enum WindowError {
    #[error("failed to create event loop: {0}")]
    EventLoop(#[from] winit::error::EventLoopError),

    #[error("failed to create window: {0}")]
    CreateWindow(#[from] winit::error::OsError),

    #[error("renderer error: {0}")]
    Renderer(#[from] crate::renderer::RendererError),
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
    state: StateSlot<AppState>,
    ctx: AppCtx,
}

struct PackDownloadResult {
    id: uuid::Uuid,
    hash: String,
    required: bool,
    result: Result<std::path::PathBuf, crate::resource_pack::PackError>,
}

pub struct FpsCounter {
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
        user: UserData,
    ) -> Self {
        Self {
            state: StateSlot::new(AppState::Setup),
            ctx: AppCtx::new(connection, version, data_dirs, tokio_rt, presence, user),
        }
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if !matches!(self.state.get(), AppState::Setup) {
            return;
        }

        let window_icon = {
            let png = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/assets/icon.png"));
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
            &self.ctx.data_dirs.jar_assets_dir,
            &self.ctx.asset_index,
            &self.ctx.data_dirs.game_dir,
        ) {
            Ok(r) => r,
            Err(e) => {
                tracing::error!("Failed to create renderer: {e}");
                event_loop.exit();
                return;
            }
        };

        if let Some(p) = &mut self.ctx.presence {
            p.set_in_menu(&self.ctx.version);
        }

        self.ctx.mesh_dispatcher =
            Some(renderer.create_mesh_dispatcher(self.ctx.biome_climate.clone(), None));
        if let Some(uuid) = self.ctx.pending_skin_uuid.take() {
            renderer.load_player_skin(&uuid, &self.ctx.tokio_rt);
        }

        self.ctx.apply_cursor_grab(false, &window);
        self.state.set(AppState::InMenu {
            runtime: Runtime {
                renderer: Box::new(renderer),
                window,
                last_frame: Instant::now(),
                fps_counter: FpsCounter::new(),
            },
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
                if let Some(app_rt) = self.state.rt_mut() {
                    app_rt.renderer.resize(new_size);
                }
            }
            WindowEvent::KeyboardInput { event, .. } => {
                self.state.transition(|mut app| {
                    if let Some(Runtime { window, .. }) = app.rt_mut()
                        && event.state.is_pressed()
                        && let PhysicalKey::Code(KeyCode::F11) = event.physical_key
                    {
                        self.ctx.display_mode = self.ctx.display_mode.cycle();
                        self.ctx.menu.display_mode = self.ctx.display_mode;
                        self.ctx.apply_display_mode(window);
                    }

                    match app {
                        AppState::Setup => app,
                        AppState::InMenu { runtime } => {
                            self.ctx.input.on_menu_key_event(&event);
                            AppState::InMenu { runtime }
                        }
                        AppState::Connecting {
                            runtime: mut app_rt,
                            state,
                        } => {
                            if event.state.is_pressed()
                                && let PhysicalKey::Code(KeyCode::Escape) = event.physical_key
                            {
                                let ctx = &mut self.ctx;
                                // TODO(proper-disconnect)

                                ctx.packet_sender = None;
                                ctx.chat_sender = None;
                                ctx.net_events = None;
                                if let Some(task) = ctx.net_task.take() {
                                    task.abort();
                                }
                                ctx.paused = false;
                                ctx.dead = false;
                                ctx.death_message = String::new();
                                ctx.position_set = false;
                                ctx.player_loaded_sent = false;
                                ctx.chunk_store = ChunkStore::new(ctx.menu.render_distance);
                                ctx.entity_store.clear();
                                ctx.item_entity_store.clear();

                                app_rt.renderer.clear_chunk_meshes();
                                ctx.mesh_dispatcher = Some(
                                    app_rt
                                        .renderer
                                        .create_mesh_dispatcher(ctx.biome_climate.clone(), None),
                                );

                                if let Some(p) = &mut ctx.presence {
                                    p.set_in_menu(&ctx.version);
                                }

                                self.ctx.apply_cursor_grab(false, &app_rt.window);

                                AppState::InMenu { runtime: app_rt }
                            } else {
                                AppState::Connecting {
                                    runtime: app_rt,
                                    state,
                                }
                            }
                        }
                        AppState::InGame {
                            runtime: mut app_rt,
                        } => {
                            if event.state.is_pressed()
                                && !event.repeat
                                && let PhysicalKey::Code(code) = event.physical_key
                            {
                                if self.ctx.chat.is_open() {
                                    match code {
                                        KeyCode::Escape => {
                                            self.ctx.chat.close();
                                            self.ctx.apply_cursor_grab(true, &app_rt.window);
                                        }
                                        KeyCode::F3 => self.ctx.show_debug = !self.ctx.show_debug,
                                        _ => self.ctx.input.on_menu_key_event(&event),
                                    }
                                } else {
                                    match code {
                                        KeyCode::Escape
                                            if self.ctx.death_confirm
                                                && self
                                                    .ctx
                                                    .death_confirm_instant
                                                    .elapsed()
                                                    .as_secs_f32()
                                                    >= 1.0 =>
                                        {
                                            self.ctx.death_confirm = false;
                                            self.ctx.send_respawn();
                                        }
                                        KeyCode::Escape if !self.ctx.dead => {
                                            if self.ctx.inventory_open {
                                                self.ctx.inventory_open = false;
                                            } else {
                                                self.ctx.paused = !self.ctx.paused;
                                            }
                                            self.ctx.apply_cursor_grab(true, &app_rt.window);
                                        }
                                        KeyCode::KeyE if !self.ctx.paused && !self.ctx.dead => {
                                            self.ctx.inventory_open = !self.ctx.inventory_open;
                                            self.ctx.apply_cursor_grab(true, &app_rt.window);
                                        }
                                        KeyCode::KeyT
                                            if !self.ctx.paused && !self.ctx.inventory_open =>
                                        {
                                            self.ctx.chat.open();
                                            self.ctx.apply_cursor_grab(true, &app_rt.window);
                                        }
                                        KeyCode::Slash
                                            if !self.ctx.paused && !self.ctx.inventory_open =>
                                        {
                                            self.ctx.chat.open_with_slash();
                                            self.ctx.apply_cursor_grab(true, &app_rt.window);
                                        }
                                        KeyCode::F3 => {
                                            self.ctx.show_debug = !self.ctx.show_debug;
                                        }
                                        KeyCode::KeyG
                                            if self.ctx.input.key_pressed(KeyCode::F3) =>
                                        {
                                            self.ctx.show_chunk_borders =
                                                !self.ctx.show_chunk_borders;
                                        }
                                        KeyCode::F5 => {
                                            app_rt.renderer.cycle_camera_mode();
                                        }
                                        _ => {}
                                    }
                                }
                            }

                            if !self.ctx.paused
                                && !self.ctx.chat.is_open()
                                && !self.ctx.inventory_open
                            {
                                self.ctx.input.on_key_event(&event);
                            }

                            AppState::InGame { runtime: app_rt }
                        }
                    }
                });
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let scroll = match delta {
                    winit::event::MouseScrollDelta::LineDelta(_, y) => y,
                    winit::event::MouseScrollDelta::PixelDelta(p) => p.y as f32,
                };
                match self.state.get() {
                    AppState::InMenu { .. }
                    | AppState::Connecting { .. }
                    | AppState::InGame { .. } => {
                        self.ctx.input.on_menu_scroll(scroll);
                    }
                    _ => self.ctx.input.on_scroll(scroll),
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.ctx
                    .input
                    .on_cursor_moved(position.x as f32, position.y as f32);
            }
            WindowEvent::MouseInput { state, button, .. }
                if matches!(
                    self.state.get(),
                    AppState::InMenu { .. } | AppState::Connecting { .. } | AppState::InGame { .. }
                ) || self.ctx.paused
                    || self.ctx.inventory_open
                    || self.ctx.input.is_cursor_captured() =>
            {
                self.ctx.input.on_mouse_button(button, state);
            }
            WindowEvent::RedrawRequested => {
                if matches!(self.state.get(), AppState::Setup) {
                    return;
                }

                let dt = if let Some(app_rt) = self.state.rt_mut() {
                    let now = Instant::now();
                    let dt = now.duration_since(app_rt.last_frame).as_secs_f32().min(0.1);

                    app_rt.last_frame = now;
                    app_rt.fps_counter.update(dt);

                    dt
                } else {
                    0.0
                };

                let ctx = &mut self.ctx;
                self.state.transition(|app| match app {
                    AppState::Setup => unreachable!(),
                    AppState::InMenu { runtime: mut app_rt } => {

                        ctx.panorama_scroll += dt * 0.00556;
                        if ctx.panorama_scroll > 1.0 {
                            ctx.panorama_scroll -= 1.0;
                        }

                        let sw = app_rt.renderer.screen_width() as f32;
                        let sh = app_rt.renderer.screen_height() as f32;

                        let menu_input = MenuInput {
                            cursor: ctx.input.cursor_pos(),
                            clicked: ctx.input.left_just_pressed(),
                            mouse_held: ctx.input.left_held(),
                            typed_chars: ctx.input.drain_typed_chars(),
                            backspace: ctx.input.backspace_pressed(),
                            enter: ctx.input.enter_pressed(),
                            escape: ctx.input.escape_pressed(),
                            tab: ctx.input.tab_pressed(),
                            f5: ctx.input.f5_pressed(),
                            scroll_delta: ctx.input.consume_menu_scroll(),
                        };

                        let result = ctx
                            .menu
                            .build(sw, sh, &menu_input, |t, s| app_rt.renderer.menu_text_width(t, s));
                        let action = result.action;

                        let cursor_icon = if result.cursor_pointer {
                            winit::window::CursorIcon::Pointer
                        } else {
                            winit::window::CursorIcon::Default
                        };
                        if ctx.input.cursor_moved_this_frame() {
                            app_rt.window.set_cursor(cursor_icon);
                        }

                        if ctx.menu.is_server_list_screen() && ctx.menu.favicons_changed() {
                            let favicons = ctx.menu.collect_favicons();
                            if !favicons.is_empty() {
                                app_rt.renderer.update_favicon_atlas(&favicons);
                            }
                        }

                        if let Err(e) = app_rt.renderer.render_menu(
                            &app_rt.window,
                            ctx.panorama_scroll,
                            result.blur,
                            result.elements,
                            ctx.input.cursor_pos(),
                            ctx.menu.is_main_screen(),
                        ) {
                            tracing::error!("Render error: {e}");
                        }

                        ctx.input.clear_click_events();

                        if ctx.menu.render_distance != ctx.last_render_distance {
                            ctx.sync_render_distance();
                        }

                        if ctx.menu.display_mode != ctx.display_mode {
                            ctx.display_mode = ctx.menu.display_mode;
                            ctx.apply_display_mode(&app_rt.window);
                        }

                        if ctx.menu.rescan_packs {
                            ctx.menu.rescan_packs = false;
                            ctx.resource_packs.scan_local_packs();
                            ctx.menu.available_packs =
                                ctx.resource_packs.available_local_packs().to_vec();
                            ctx.menu.active_packs = ctx.resource_packs.active_pack_info();
                        }

                        if let Some((name, enable)) = ctx.menu.pack_toggle.take() {
                            if enable {
                                ctx.resource_packs.enable_local_pack(&name);
                            } else {
                                ctx.resource_packs.disable_local_pack(&name);
                            }
                            ctx.menu.active_packs = ctx.resource_packs.active_pack_info();
                            ctx.menu.available_packs =
                                ctx.resource_packs.available_local_packs().to_vec();
                        }

                        if ctx.menu.reload_assets {
                            ctx.menu.reload_assets = false;
                            app_rt.renderer
                                .reload_assets(&ctx.data_dirs.game_dir, &ctx.resource_packs);
                            if let Some(ref mut dispatcher) = ctx.mesh_dispatcher {
                                *dispatcher = app_rt.renderer.create_mesh_dispatcher(
                                    ctx.biome_climate.clone(),
                                    Some(&ctx.resource_packs),
                                );
                                for pos in ctx.chunk_store.loaded_positions() {
                                    dispatcher.enqueue(&ctx.chunk_store, pos, 0);
                                }
                            }
                        }

                        if result.clicked_button {
                            app_rt.renderer.trigger_skin_swing();
                        }

                        match action {
                            MenuAction::Connect { server, username } => {
                                let uuid = ctx.user.uuid;
                                let access_token = ctx.user.access_token.clone();

                                let connect_args = crate::net::connection::ConnectArgs {
                                    server,
                                    username,
                                    uuid,
                                    access_token,
                                    view_distance: ctx.menu.render_distance as u8,
                                };

                                let handle = crate::net::connection::spawn_connection(&ctx.tokio_rt, connect_args);
                                ctx.net_events = Some(handle.events);
                                ctx.chat_sender = Some(handle.chat_tx);
                                ctx.packet_sender = Some(crate::net::sender::PacketSender::new(handle.packet_tx));
                                ctx.net_task = Some(handle.task);

                                ctx.apply_cursor_grab(false, &app_rt.window);

                                return AppState::Connecting {
                                    runtime: app_rt,
                                    state: ConnectingState::Connecting,
                                }
                            }
                            MenuAction::ChangeTheme(theme) => {
                                let panorama_dir = match theme {
                                    PanoramaTheme::Default => {
                                        ctx.data_dirs.jar_assets_dir.clone()
                                    }
                                    PanoramaTheme::Pomme => {
                                        ctx.data_dirs.pomme_assets_dir.join("panoramas")
                                    }
                                };
                                app_rt.renderer.reload_panorama(&panorama_dir, &ctx.asset_index);
                                ctx.menu.start_transition_open();
                            }
                            MenuAction::Quit => {
                                event_loop.exit();
                                return AppState::Setup;
                            }
                            MenuAction::None => {}
                        }

                        AppState::InMenu { runtime: app_rt }
                    }
                    AppState::Connecting {
                        runtime: mut app_rt,
                        state: mut connection_state,
                    } => {
                        let disconnect_reason = ctx.drain_network_events(&mut app_rt.renderer, &app_rt.window, Some(&mut connection_state));
                        if let Some(reason) = disconnect_reason {
                            // TODO(proper-disconnect)

                            ctx.packet_sender = None;
                            ctx.chat_sender = None;
                            ctx.net_events = None;
                            if let Some(task) = ctx.net_task.take() {
                                task.abort();
                            }
                            ctx.paused = false;
                            ctx.dead = false;
                            ctx.death_message = String::new();
                            ctx.position_set = false;
                            ctx.player_loaded_sent = false;
                            ctx.chunk_store = ChunkStore::new(ctx.menu.render_distance);
                            ctx.entity_store.clear();
                            ctx.item_entity_store.clear();

                            app_rt.renderer.clear_chunk_meshes();
                            ctx.mesh_dispatcher =
                                Some(app_rt.renderer.create_mesh_dispatcher(ctx.biome_climate.clone(), None));

                            ctx.menu.show_disconnect(reason);
                            if let Some(p) = &mut ctx.presence {
                                p.set_in_menu(&ctx.version);
                            }

                            ctx.apply_cursor_grab(false, &app_rt.window);

                            return AppState::InMenu { runtime: app_rt };
                        }

                        if matches!(connection_state, ConnectingState::Loading) {
                            if let Some(dispatcher) =
                                &ctx.mesh_dispatcher
                            {
                                for mesh in dispatcher.drain_results() {
                                    app_rt.renderer.upload_chunk_mesh(&mesh);
                                }
                            }

                            let ready = ctx.position_set
                                && (ctx.dead
                                    || app_rt.renderer.loaded_chunk_count() > 0);

                            // Mirror vanilla's `notifyPlayerLoaded`; servers gate
                            // per-player entity tracking on it.
                            if ready
                                && !ctx.player_loaded_sent
                                && let Some(sender) = &ctx.packet_sender
                            {
                                sender.send(ServerboundGamePacket::PlayerLoaded(
                                    azalea_protocol::packets::game::s_player_loaded::ServerboundPlayerLoaded,
                                ));
                                ctx.player_loaded_sent = true;
                            }

                            if ready {
                                if let Some(p) = &mut ctx.presence {
                                    p.playing_multiplayer(&ctx.version);
                                }
                                ctx.apply_cursor_grab(true, &app_rt.window);

                                return AppState::InGame { runtime: app_rt };
                            }
                        }

                        let status_text = match connection_state {
                            ConnectingState::Loading => "Loading terrain...",
                            ConnectingState::Connecting => "Connecting to the server...",
                        };

                        ctx.panorama_scroll += dt * 0.00556;
                        if ctx.panorama_scroll > 1.0 {
                            ctx.panorama_scroll -= 1.0;
                        }

                        let mut cancel = false;

                            let sw = app_rt.renderer.screen_width() as f32;
                            let sh = app_rt.renderer.screen_height() as f32;
                            let gs = hud::gui_scale(sw, sh, ctx.menu.gui_scale_setting);
                            let fs = 11.0 * gs;
                            let btn_h = 30.0 * gs;
                            let btn_w = 160.0 * gs;

                            let cx = sw / 2.0;
                            let cy = sh / 2.0;

                            let mut elements = Vec::new();
                            let clicked = ctx.input.left_just_pressed();
                            let cursor = ctx.input.cursor_pos();

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

                            ctx.input.clear_click_events();

                            if let Err(e) = app_rt.renderer.render_menu(
                                &app_rt.window,
                                ctx.panorama_scroll,
                                2.0,
                                elements,
                                ctx.input.cursor_pos(),
                                false,
                            ) {
                                tracing::error!("Render error: {e}");
                            }


                        if cancel {
                            // TODO(proper-disconnect)

                            ctx.packet_sender = None;
                            ctx.chat_sender = None;
                            ctx.net_events = None;
                            if let Some(task) = ctx.net_task.take() {
                                task.abort();
                            }
                            ctx.paused = false;
                            ctx.dead = false;
                            ctx.death_message = String::new();
                            ctx.position_set = false;
                            ctx.player_loaded_sent = false;
                            ctx.chunk_store = ChunkStore::new(ctx.menu.render_distance);
                            ctx.entity_store.clear();
                            ctx.item_entity_store.clear();

                            app_rt.renderer.clear_chunk_meshes();
                            ctx.mesh_dispatcher =
                                Some(app_rt.renderer.create_mesh_dispatcher(ctx.biome_climate.clone(), None));

                            if let Some(p) = &mut ctx.presence {
                                p.set_in_menu(&ctx.version);
                            }

                            ctx.apply_cursor_grab(false, &app_rt.window);

                            return AppState::InMenu { runtime: app_rt };
                        }

                        AppState::Connecting { runtime: app_rt, state: connection_state }
                    },
                    AppState::InGame {  runtime: mut app_rt } => {
                        let disconnect_reason = ctx.drain_network_events(&mut app_rt.renderer, &app_rt.window, None);
                        if let Some(reason) = disconnect_reason {
                            // TODO(proper-disconnect)

                            ctx.packet_sender = None;
                            ctx.chat_sender = None;
                            ctx.net_events = None;
                            if let Some(task) = ctx.net_task.take() {
                                task.abort();
                            }
                            ctx.paused = false;
                            ctx.dead = false;
                            ctx.death_message = String::new();
                            ctx.position_set = false;
                            ctx.player_loaded_sent = false;
                            ctx.chunk_store = ChunkStore::new(ctx.menu.render_distance);
                            ctx.entity_store.clear();
                            ctx.item_entity_store.clear();

                            app_rt.renderer.clear_chunk_meshes();
                            ctx.mesh_dispatcher =
                                Some(app_rt.renderer.create_mesh_dispatcher(ctx.biome_climate.clone(), None));

                            ctx.menu.show_disconnect(reason);
                            if let Some(p) = &mut ctx.presence {
                                p.set_in_menu(&ctx.version);
                            }

                            ctx.apply_cursor_grab(false, &app_rt.window);

                            return AppState::InMenu { runtime: app_rt };
                        }

                        if let Some(dispatcher) =
                            &ctx.mesh_dispatcher
                        {
                            for mesh in dispatcher.drain_results() {
                                app_rt.renderer.upload_chunk_mesh(&mesh);
                            }
                        }

                        // Sky time ticks unconditionally so it keeps flowing in menus;
                        // server SetTime packets reconcile drift.
                        ctx.time_tick_accumulator = (ctx.time_tick_accumulator + dt).min(1.0);
                        while ctx.time_tick_accumulator >= TICK_RATE {
                            ctx.sky_state.day_time = ctx.sky_state.day_time.wrapping_add(1);
                            ctx.sky_state.game_time = ctx.sky_state.game_time.wrapping_add(1);
                            ctx.time_tick_accumulator -= TICK_RATE;
                        }

                        if !ctx.paused && !ctx.inventory_open && !ctx.chat.is_open() {
                            app_rt.renderer.update_camera(&mut ctx.input);

                            ctx.tick_accumulator += dt;
                            while ctx.tick_accumulator >= TICK_RATE {
                                ctx.tick_physics(&mut app_rt.renderer);
                                ctx.item_entity_store.tick(
                                    |bx, by, bz| {
                                        !ctx.chunk_store.get_block_state(bx, by, bz).is_air()
                                    },
                                    |bx, by, bz| {
                                        block_friction(
                                            ctx.chunk_store.get_block_state(bx, by, bz),
                                        )
                                    },
                                );
                                ctx.tick_accumulator -= TICK_RATE;
                            }
                        }

                        let alpha = ctx.tick_accumulator / TICK_RATE;
                        let interp_pos = ctx.prev_player_pos.lerp(ctx.player.position, alpha);
                        let eye_pos = interp_pos + glam::Vec3::new(0.0, 1.62, 0.0);
                        let eye_pos_f64 = glam::DVec3::new(
                            eye_pos.x as f64,
                            eye_pos.y as f64,
                            eye_pos.z as f64,
                        );

                        if !ctx.paused && !ctx.inventory_open && !ctx.chat.is_open() {
                            let yaw = app_rt.renderer.camera_yaw();
                            let pitch = app_rt.renderer.camera_pitch();


                            ctx.interaction.update_target(
                                eye_pos,
                                yaw,
                                pitch,
                                &ctx.chunk_store,
                            );
                        }

                        let typed = ctx.input.drain_typed_chars();
                        let backspace = ctx.input.backspace_pressed();
                        let enter = ctx.input.enter_pressed();
                        if let Some(msg) = ctx.chat.handle_key_input(&typed, backspace, enter)
                        {
                            ctx.send_chat_message(msg);
                            ctx.apply_cursor_grab(true, &app_rt.window);
                        }

                        let mut close_inventory = false;
                        let mut pause_action = PauseAction::None;
                        let mut death_action = DeathAction::None;


                            app_rt.renderer.sync_camera_to_player(
                                eye_pos_f64,
                                app_rt.renderer.camera_yaw(),
                                app_rt.renderer.camera_pitch(),
                            );
                            app_rt.renderer.update_third_person_distance(eye_pos, &ctx.chunk_store);

                            let sw = app_rt.renderer.screen_width() as f32;
                            let sh = app_rt.renderer.screen_height() as f32;
                            let gs = hud::gui_scale(sw, sh, ctx.menu.gui_scale_setting);

                            let mut elements: Vec<MenuElement> = Vec::new();
                            let hide_cursor = !ctx.paused
                                && !ctx.dead
                                && !ctx.inventory_open
                                && !ctx.chat.is_open()
                                && ctx.input.is_cursor_captured();

                            let debug = if ctx.show_debug {
                                Some(hud::DebugInfo {
                                    fps: app_rt.fps_counter.display_fps,
                                    position: ctx.player.position,
                                    yaw: ctx.player.yaw,
                                    pitch: ctx.player.pitch,
                                    target_block: ctx.interaction.target.map(|t| {
                                        let state = ctx.chunk_store.get_block_state(
                                            t.block_pos.x,
                                            t.block_pos.y,
                                            t.block_pos.z,
                                        );
                                        let block: Box<dyn azalea_block::BlockTrait> =
                                            state.into();
                                        (t.block_pos, t.face, block.id().to_string())
                                    }),
                                    chunk_count: app_rt.renderer.loaded_chunk_count(),
                                    gpu_name: app_rt.renderer.gpu_name(),
                                    vulkan_version: app_rt.renderer.vulkan_version(),
                                    screen_w: app_rt.renderer.screen_width(),
                                    screen_h: app_rt.renderer.screen_height(),
                                    timings: Some(hud::FrameTimings {
                                        frame_ms: app_rt.renderer.last_timings.frame_ms,
                                        fence_ms: app_rt.renderer.last_timings.fence_ms,
                                        acquire_ms: app_rt.renderer.last_timings.acquire_ms,
                                        cull_ms: app_rt.renderer.last_timings.cull_ms,
                                        draw_ms: app_rt.renderer.last_timings.draw_ms,
                                        present_ms: app_rt.renderer.last_timings.present_ms,
                                    }),
                                })
                            } else {
                                None
                            };
                            hud::build_hud(
                                &mut elements,
                                sw,
                                sh,
                                ctx.input.selected_slot(),
                                ctx.player.health,
                                ctx.player.food,
                                ctx.player.armor,
                                ctx.player.air_supply,
                                ctx.player.eyes_in_water,
                                ctx.player.experience_level,
                                ctx.player.experience_progress,
                                ctx.player.game_mode,
                                ctx.player.inventory.hotbar_slots(),
                                app_rt.renderer.is_first_person(),
                                debug.as_ref(),
                                ctx.menu.gui_scale_setting,
                            );

                            if ctx.input.tab_held()
                                && !ctx.paused
                                && !ctx.inventory_open
                                && !ctx.chat.is_open()
                                && !ctx.dead
                            {
                                let r = &*app_rt.renderer;
                                crate::ui::player_tab::build_player_tab_overlay(
                                    &mut elements,
                                    sw,
                                    &ctx.tab_list,
                                    gs,
                                    &|t, s| r.menu_text_width(t, s),
                                );
                            }

                            if let Some(ref mut bench) = ctx.benchmark {
                                let entity_count = ctx.entity_store.living.len() as u32;
                                let done = bench.record_frame(
                                    dt * 1000.0,
                                    &app_rt.renderer.last_timings,
                                    app_rt.renderer.loaded_chunk_count(),
                                    entity_count,
                                );
                                let progress = bench.progress();
                                elements.push(MenuElement::Rect {
                                    x: sw * 0.25,
                                    y: 16.0,
                                    w: sw * 0.5,
                                    h: 8.0,
                                    corner_radius: 4.0,
                                    color: [1.0, 1.0, 1.0, 0.1],
                                });
                                elements.push(MenuElement::Rect {
                                    x: sw * 0.25,
                                    y: 16.0,
                                    w: sw * 0.5 * progress,
                                    h: 8.0,
                                    corner_radius: 4.0,
                                    color: [0.294, 0.871, 0.498, 0.8],
                                });
                                elements.push(MenuElement::Text {
                                    x: sw / 2.0,
                                    y: 28.0,
                                    text: format!("Benchmarking... {:.0}%", progress * 100.0),
                                    scale: 8.0 * gs,
                                    color: [1.0, 1.0, 1.0, 1.0],
                                    centered: true,
                                });
                                if done {
                                    let bench = ctx.benchmark.take().unwrap();
                                    ctx.benchmark_result =
                                        Some(bench.finish(&ctx.data_dirs.game_dir));
                                }
                            }

                            if let Some(ref result) = ctx.benchmark_result {
                                let fs = 8.0 * gs;
                                let cx = sw / 2.0;
                                let by = sh / 2.0 - 90.0;
                                common::push_overlay(&mut elements, sw, sh, 0.5);
                                elements.push(MenuElement::Text {
                                    x: cx,
                                    y: by,
                                    text: "Benchmark Complete".into(),
                                    scale: fs * 2.0,
                                    color: [1.0, 1.0, 1.0, 1.0],
                                    centered: true,
                                });
                                let lines = [
                                    format!("GPU: {}", result.gpu),
                                    format!(
                                        "{}x{} / RD {} / {} chunks / {} entities",
                                        result.resolution[0],
                                        result.resolution[1],
                                        result.render_distance,
                                        result.peak_chunk_count,
                                        result.peak_entity_count,
                                    ),
                                    format!("Avg FPS: {:.0}", result.avg_fps),
                                    format!(
                                        "Min: {:.0} / Max: {:.0}",
                                        result.min_fps, result.max_fps
                                    ),
                                    format!(
                                        "Frame: {:.2}ms / P1: {:.2}ms / P99: {:.2}ms",
                                        result.avg_frame_ms,
                                        result.p1_frame_ms,
                                        result.p99_frame_ms
                                    ),
                                    format!(
                                        "Fence: {:.2}ms / Cull: {:.2}ms / Draw: {:.2}ms",
                                        result.avg_fence_ms,
                                        result.avg_cull_ms,
                                        result.avg_draw_ms
                                    ),
                                    format!(
                                        "{} spikes (>{:.0}ms) - Saved to benchmark.json",
                                        result.spike_count, 8.0
                                    ),
                                ];
                                for (i, line) in lines.iter().enumerate() {
                                    elements.push(MenuElement::Text {
                                        x: cx,
                                        y: by + fs * 2.0 + 10.0 + i as f32 * (fs + 4.0),
                                        text: line.clone(),
                                        scale: fs,
                                        color: [0.8, 0.85, 0.9, 1.0],
                                        centered: true,
                                    });
                                }
                                if ctx.input.escape_pressed() ||  ctx.input.left_just_pressed()
                                {
                                    ctx.benchmark_result = None;
                                }
                            }

                            if ctx.options_from_game {
                                let menu_input = MenuInput {
                                    cursor: ctx.input.cursor_pos(),
                                    clicked: ctx.input.left_just_pressed(),
                                    mouse_held: ctx.input.left_held(),
                                    typed_chars: ctx.input.drain_typed_chars(),
                                    backspace: ctx.input.backspace_pressed(),
                                    enter: ctx.input.enter_pressed(),
                                    escape: ctx.input.escape_pressed(),
                                    tab: ctx.input.tab_pressed(),
                                    f5: ctx.input.f5_pressed(),
                                    scroll_delta: ctx.input.consume_menu_scroll(),
                                };
                                let r = &*app_rt.renderer;
                                let result = ctx
                                    .menu
                                    .build(sw, sh, &menu_input, |t, s| r.menu_text_width(t, s));
                                elements.extend(result.elements);
                                ctx.input.clear_click_events();
                            } else if ctx.dead {
                                let cursor = ctx.input.cursor_pos();
                                let clicked =
                                    ctx.input.left_just_pressed() && !ctx.respawn_sent;
                                death_action = if ctx.death_confirm {
                                    death::build_death_confirm(
                                        &mut elements,
                                        sw,
                                        sh,
                                        cursor,
                                        clicked,
                                        gs,
                                        ctx.death_confirm_instant.elapsed().as_secs_f32()
                                            >= 1.0,
                                    )
                                } else {
                                    let buttons_enabled = !ctx.respawn_sent
                                        && ctx.death_instant.elapsed().as_secs_f32() >= 1.0;
                                    let r = &*app_rt.renderer;
                                    death::build_death_screen(
                                        &mut elements,
                                        sw,
                                        sh,
                                        cursor,
                                        clicked,
                                        gs,
                                        &ctx.death_message,
                                        ctx.player.score,
                                        buttons_enabled,
                                        &|t, s| r.menu_text_width(t, s),
                                    )
                                };
                                ctx.input.clear_click_events();
                            } else if ctx.paused {
                                let cursor = ctx.input.cursor_pos();
                                let clicked = ctx.input.left_just_pressed();
                                pause_action = pause::build_pause_menu(
                                    &mut elements,
                                    sw,
                                    sh,
                                    cursor,
                                    clicked,
                                    gs,
                                );
                                ctx.input.clear_click_events();
                            }

                            if ctx.inventory_open {
                                let cursor = ctx.input.cursor_pos();
                                let clicked = ctx.input.left_just_pressed();
                                close_inventory = crate::ui::inventory::build_inventory(
                                    &mut elements,
                                    sw,
                                    sh,
                                    cursor,
                                    clicked,
                                    &ctx.player.inventory,
                                    gs,
                                );
                                ctx.input.clear_click_events();
                            }

                            ctx.chat.build(&mut elements, sh, gs, &|t, s| {
                                app_rt.renderer.menu_text_width(t, s)
                            });

                            let swing_progress = ctx
                                .interaction
                                .get_swing_progress(ctx.tick_accumulator / TICK_RATE);
                            let destroy_info = ctx.interaction.destroy_stage();

                            let alpha = ctx.tick_accumulator / TICK_RATE;
                            let mut entity_renders: Vec<EntityRenderInfo> = ctx
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

                            if !app_rt.renderer.is_first_person() {
                                let cam_yaw_deg = -app_rt.renderer.camera_yaw().to_degrees();
                                entity_renders.push(EntityRenderInfo {
                                    x: interp_pos.x as f64,
                                    y: interp_pos.y as f64,
                                    z: interp_pos.z as f64,
                                    yaw: cam_yaw_deg,
                                    pitch: app_rt.renderer.camera_pitch().to_degrees(),
                                    head_yaw: cam_yaw_deg,
                                    is_baby: false,
                                    walk_anim_pos: ctx.player_walk_pos
                                        - ctx.player_walk_speed * (1.0 - alpha),
                                    walk_anim_speed: (ctx.player_prev_walk_speed
                                        + (ctx.player_walk_speed
                                            - ctx.player_prev_walk_speed)
                                            * alpha)
                                        .min(1.0),
                                    entity_kind: azalea_registry::builtin::EntityKind::Player,
                                });
                            }

                            let sky_partial_tick =
                                (ctx.time_tick_accumulator / TICK_RATE).clamp(0.0, 1.0);
                            let sky = crate::renderer::SkyState {
                                day_time: ctx.sky_state.day_time,
                                game_time: ctx.sky_state.game_time,
                                rain_level: ctx.sky_state.rain_level,
                                partial_tick: sky_partial_tick,
                            };
                            if ctx.show_chunk_borders {
                                app_rt.renderer.update_chunk_borders(
                                    ctx.chunk_store.min_y(),
                                    ctx.chunk_store.min_y() + ctx.chunk_store.height() as i32,
                                );
                            }

                            let cam_pos = glam::DVec3::new(
                                ctx.player.position.x as f64,
                                ctx.player.position.y as f64,
                                ctx.player.position.z as f64,
                            );
                            let partial_tick = ctx.tick_accumulator / TICK_RATE;
                            let item_renders = build_item_render_infos(
                                &ctx.item_entity_store,
                                &ctx.chunk_store,
                                cam_pos,
                                partial_tick,
                            );

                            if let Err(e) = app_rt.renderer.render_world(
                                &app_rt.window,
                                hide_cursor,
                                elements,
                                swing_progress,
                                destroy_info,
                                ctx.show_chunk_borders,
                                sky,
                                &entity_renders,
                                &item_renders,
                            ) {
                                tracing::error!("Render error: {e}");
                            }


                        if close_inventory {
                            ctx.inventory_open = false;
                            ctx.apply_cursor_grab(true, &app_rt.window);
                        }

                        match death_action {
                            DeathAction::Respawn => {
                                ctx.death_confirm = false;
                                ctx.send_respawn();
                            }
                            DeathAction::TitleScreen => {

                                    // TODO(proper-disconnect)

                                    ctx.packet_sender = None;
                                    ctx.chat_sender = None;
                                    ctx.net_events = None;
                                    if let Some(task) = ctx.net_task.take() {
                                        task.abort();
                                    }
                                    ctx.paused = false;
                                    ctx.dead = false;
                                    ctx.death_message = String::new();
                                    ctx.position_set = false;
                                    ctx.player_loaded_sent = false;
                                    ctx.chunk_store = ChunkStore::new(ctx.menu.render_distance);
                                    ctx.entity_store.clear();
                                    ctx.item_entity_store.clear();

                                    app_rt.renderer.clear_chunk_meshes();
                                    ctx.mesh_dispatcher =
                                        Some(app_rt.renderer.create_mesh_dispatcher(ctx.biome_climate.clone(), None));

                                    if let Some(p) = &mut ctx.presence {
                                        p.set_in_menu(&ctx.version);
                                    }

                                    ctx.apply_cursor_grab(false, &app_rt.window);

                                    return AppState::InMenu { runtime: app_rt };

                            }
                            DeathAction::ShowConfirm => {
                                ctx.death_confirm = true;
                                ctx.death_confirm_instant = Instant::now();
                            }
                            DeathAction::None => {}
                        }

                        match pause_action {
                            PauseAction::Resume => {
                                ctx.paused = false;
                                ctx.apply_cursor_grab(true, &app_rt.window);
                            }
                            PauseAction::Options => {
                                ctx.menu.open_options();
                                ctx.options_from_game = true;
                                ctx.paused = false;
                                ctx.apply_cursor_grab(true, &app_rt.window);
                            }
                            PauseAction::Disconnect => {
                                    // TODO(proper-disconnect)

                                    ctx.packet_sender = None;
                                    ctx.chat_sender = None;
                                    ctx.net_events = None;
                                    if let Some(task) = ctx.net_task.take() {
                                        task.abort();
                                    }
                                    ctx.paused = false;
                                    ctx.dead = false;
                                    ctx.death_message = String::new();
                                    ctx.position_set = false;
                                    ctx.player_loaded_sent = false;
                                    ctx.chunk_store = ChunkStore::new(ctx.menu.render_distance);
                                    ctx.entity_store.clear();
                                    ctx.item_entity_store.clear();

                                    app_rt.renderer.clear_chunk_meshes();
                                    ctx.mesh_dispatcher =
                                        Some(app_rt.renderer.create_mesh_dispatcher(ctx.biome_climate.clone(), None));

                                    if let Some(p) = &mut ctx.presence {
                                        p.set_in_menu(&ctx.version);
                                    }

                                    ctx.apply_cursor_grab(false, &app_rt.window);

                                    return AppState::InMenu { runtime: app_rt };

                            }
                            PauseAction::Benchmark => {
                                ctx.benchmark = Some(Benchmark::new(
                                    app_rt.renderer.gpu_name(),
                                    app_rt.renderer.screen_width(),
                                    app_rt.renderer.screen_height(),
                                    ctx.menu.render_distance,
                                ));
                                ctx.benchmark_result = None;
                                ctx.paused = false;
                                ctx.apply_cursor_grab(true, &app_rt.window);
                            }
                            PauseAction::None => {}
                        }

                        if ctx.options_from_game {
                            if ctx.menu.render_distance != ctx.last_render_distance {
                                ctx.sync_render_distance();
                            }
                            if !ctx.menu.is_options_screen() {
                                ctx.options_from_game = false;
                                ctx.paused = true;
                                ctx.apply_cursor_grab(true, &app_rt.window);
                            }
                        }

                        AppState::InGame { runtime: app_rt }
                    },
                });

                if let Some(app_rt) = &mut self.state.rt_mut() {
                    if !app_rt.window.is_visible().unwrap_or(true) {
                        app_rt.window.set_visible(true);
                    }
                    app_rt.window.request_redraw();
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
            && self.ctx.input.is_cursor_captured()
            && !self.ctx.paused
            && !self.ctx.dead
            && !self.ctx.inventory_open
            && !self.ctx.chat.is_open()
        {
            self.ctx.input.on_mouse_motion(delta);
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        event_loop.set_control_flow(winit::event_loop::ControlFlow::Poll);
    }
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
    user: UserData,
    presence: Option<crate::discord::DiscordPresence>,
) -> Result<(), WindowError> {
    let event_loop = EventLoop::new()?;
    let mut app = App::new(connection, version, data_dirs, tokio_rt, presence, user);

    event_loop.run_app(&mut app)?;
    Ok(())
}
