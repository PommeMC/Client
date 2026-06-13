use std::ops::Add;
use std::sync::Arc;
use std::time::Instant;

use azalea_protocol::packets::game::{
    ServerboundClientCommand, ServerboundGamePacket, s_client_command,
};
use glam::dvec3;
use winit::keyboard::KeyCode;
use winit::window::{CursorGrabMode, Window};

use crate::app::input::InputState;
use crate::app::phases::ConnectionPhase;
use crate::app::phases::in_game::GameState;
use crate::app::{POSITION_SEND_INTERVAL, POSITION_THRESHOLD_SQ};
use crate::assets::AssetIndex;
use crate::dirs::DataDirs;
use crate::discord::DiscordPresence;
use crate::entity::components::{LookDirection, Position, Velocity};
use crate::net::NetworkEvent;
use crate::net::connection::ConnectionHandle;
use crate::physics::movement;
use crate::player::LocalPlayer;
use crate::renderer::Renderer;
use crate::resource_pack::ResourcePackManager;
use crate::ui::menu::{MainMenu, MenuInput};
use crate::user::UserData;
use crate::world::chunk::{ChunkStore, mesh_neighborhood};

pub struct PendingPackDownload {
    pub id: uuid::Uuid,
    pub required: bool,
    pub hash: String,
    pub handle: std::thread::JoinHandle<PackDownloadResult>,
}

pub type PackDownloadResult = Result<std::path::PathBuf, crate::resource_pack::PackError>;

/// Queues `pos` and its already-loaded mesh neighborhood (de-duplicated): when
/// `pos` changes, every chunk whose mesh samples it must re-mesh too, else
/// their shared border keeps the stale full-bright edge light.
fn enqueue_with_neighbors(
    out: &mut Vec<azalea_core::position::ChunkPos>,
    store: &ChunkStore,
    pos: azalea_core::position::ChunkPos,
) {
    for p in mesh_neighborhood(pos) {
        if (p == pos || store.get_chunk(&p).is_some()) && !out.contains(&p) {
            out.push(p);
        }
    }
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

#[derive(Default, PartialEq)]
pub struct PlayerInputState {
    forward: bool,
    backward: bool,
    left: bool,
    right: bool,
    jump: bool,
    shift: bool,
    sprint: bool,
}

pub struct AppCore {
    pub user: UserData,
    pub presence: Option<DiscordPresence>,
    pub display_mode: DisplayMode,
    pub input: InputState,
    pub menu: MainMenu,
    pub tokio_rt: Arc<tokio::runtime::Runtime>,
    pub data_dirs: DataDirs,
    pub version: String,
    pub resource_packs: ResourcePackManager,
    pub pending_pack_download: Option<PendingPackDownload>,
    pub asset_index: Option<AssetIndex>,
    pub audio: crate::audio::AudioEngine,
    pub tick_accumulator: f32,
    pub time_tick_accumulator: f32,
}

impl AppCore {
    pub fn new(
        version: String,
        data_dirs: DataDirs,
        tokio_rt: Arc<tokio::runtime::Runtime>,
        presence: Option<DiscordPresence>,
        user: UserData,
    ) -> Self {
        let resource_packs = ResourcePackManager::new(&data_dirs.game_dir);

        let menu = MainMenu::new(
            &data_dirs.game_dir,
            Arc::clone(&tokio_rt),
            user.username.clone(),
        );

        let asset_index =
            AssetIndex::load(&data_dirs.indexes_dir, &data_dirs.objects_dir, &version);

        let audio = crate::audio::AudioEngine::new(
            &data_dirs.jar_assets_dir,
            asset_index.clone(),
            menu.category_volumes(),
        );

        Self {
            user,
            presence,
            display_mode: DisplayMode::Windowed,
            input: InputState::new(),
            menu,
            tokio_rt,
            data_dirs,
            version,
            resource_packs,
            pending_pack_download: None,
            asset_index,
            audio,
            tick_accumulator: 0.0,
            time_tick_accumulator: 0.0,
        }
    }

    pub fn build_menu_input(&mut self) -> MenuInput {
        MenuInput {
            cursor: self.input.cursor_pos(),
            clicked: self.input.left_just_pressed(),
            mouse_held: self.input.left_held(),
            typed_chars: self.input.drain_typed_chars(),
            backspace: self.input.backspace_pressed(),
            enter: self.input.enter_pressed(),
            escape: self.input.escape_pressed(),
            tab: self.input.tab_pressed(),
            f5: self.input.f5_pressed(),
            select_all: self.input.select_all_pressed(),
            copy: self.input.copy_pressed(),
            cut: self.input.cut_pressed(),
            undo: self.input.undo_pressed(),
            scroll_delta: self.input.consume_menu_scroll(),
        }
    }

    pub fn apply_display_mode(&mut self, window: &Window) {
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

    pub fn apply_cursor_grab(&self, window: &Window, game: Option<&mut GameState>) {
        let captured =
            game.is_some_and(|g| g.input_live() && !g.dead && self.input.is_cursor_captured());
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

    pub fn send_respawn(&mut self, connection: &ConnectionHandle, game: &mut GameState) {
        connection
            .packet_tx
            .send(ServerboundGamePacket::ClientCommand(
                ServerboundClientCommand {
                    action: s_client_command::Action::PerformRespawn,
                },
            ));

        game.death_confirm = false;
        game.respawn_sent = true;
    }

    pub fn send_chat_message(&self, connection: &ConnectionHandle, msg: String) {
        let _ = connection.chat_tx.try_send(msg);
    }

    pub fn drain_network_events(
        &mut self,
        connection: &ConnectionHandle,
        mut connect_phase: Option<&mut ConnectionPhase>,
        renderer: &mut Renderer,
        window: &Window,
        game: &mut GameState,
    ) -> Option<String> {
        let rx = &connection.event_rx;

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
                    if let Some(state) = connect_phase.as_deref_mut() {
                        tracing::info!("Connected to server");
                        *state = ConnectionPhase::Loading;
                    } else {
                        tracing::warn!("Unexpected NetworkEvent::Connected, skipping");
                    }
                }
                NetworkEvent::BiomeColors { colors } => {
                    tracing::info!("Received {} biome climate entries", colors.len());
                    game.biome_climate = Arc::new(colors);
                    game.mesh_dispatcher
                        .set_biome_climate(Arc::clone(&game.biome_climate));
                }
                NetworkEvent::DimensionInfo { height, min_y } => {
                    tracing::info!("Dimension: height={height}, min_y={min_y}");
                    game.chunk_store =
                        ChunkStore::new_with_dimension(self.menu.render_distance, height, min_y);
                    game.position_set = false;
                    game.player_loaded_sent = false;

                    renderer.clear_chunk_meshes();
                    game.mesh_dispatcher =
                        renderer.create_mesh_dispatcher(Arc::clone(&game.biome_climate), None);
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
                    if let Err(e) = game.chunk_store.load_chunk(pos, &data, &heightmaps) {
                        tracing::error!("Failed to load chunk [{}, {}]: {e}", pos.x, pos.z);
                        continue;
                    }
                    game.chunk_store.store_light(
                        pos,
                        &sky_light,
                        &block_light,
                        &sky_y_mask,
                        &block_y_mask,
                    );
                    enqueue_with_neighbors(&mut chunks_to_mesh, &game.chunk_store, pos);
                }
                NetworkEvent::ChunkUnloaded { pos } => {
                    game.chunk_store.unload_chunk(&pos);
                    game.block_entity_anim.drop_chunk(pos.x, pos.z);
                    game.meshed_lod.remove(&pos);

                    renderer.remove_chunk_mesh(&pos);
                }
                NetworkEvent::ChunkCacheCenter { x, z } => {
                    tracing::debug!("Chunk cache center: [{x}, {z}]");
                    game.chunk_store
                        .set_center(azalea_core::position::ChunkPos::new(x, z));
                }
                NetworkEvent::PlayerPosition { change, relative } => {
                    fn resolve<T: Add<Output = T>>(base: T, is_relative: bool, value: T) -> T {
                        if is_relative { base + value } else { value }
                    }

                    let new_position = Position::new(
                        resolve(game.player.position.x, relative.x, change.pos.x),
                        resolve(game.player.position.y, relative.y, change.pos.y),
                        resolve(game.player.position.z, relative.z, change.pos.z),
                    );

                    let new_look_dir = LookDirection::new(
                        resolve(
                            game.player.look_dir.y_rot_deg(),
                            relative.y_rot,
                            change.look_direction.y_rot(),
                        ),
                        resolve(
                            game.player.look_dir.x_rot_deg(),
                            relative.x_rot,
                            change.look_direction.x_rot(),
                        ),
                    );

                    let new_velocity = {
                        let mut new_velocity = game.player.velocity;
                        if relative.rotate_delta {
                            let x_rot_delta =
                                game.player.look_dir.x_rot_deg() - new_look_dir.x_rot_deg();
                            let y_rot_delta =
                                game.player.look_dir.y_rot_deg() - new_look_dir.y_rot_deg();

                            new_velocity = new_velocity
                                .x_rot(x_rot_delta.to_radians() as f64)
                                .y_rot(y_rot_delta.to_radians() as f64);
                        }
                        Velocity::new(
                            resolve(new_velocity.x, relative.delta_x, change.delta.x),
                            resolve(new_velocity.y, relative.delta_y, change.delta.y),
                            resolve(new_velocity.z, relative.delta_z, change.delta.z),
                        )
                    };

                    game.player.position = new_position;
                    game.player.prev_position = game.player.position;
                    game.player.velocity = new_velocity;
                    game.player.look_dir = new_look_dir;
                    game.player.prev_look_dir = game.player.look_dir;
                    game.interaction.on_teleport();

                    let to_chunk_coord = |v: f64| (v.floor() as i32).div_euclid(16);
                    game.chunk_store
                        .set_center(azalea_core::position::ChunkPos::new(
                            to_chunk_coord(new_position.x),
                            to_chunk_coord(new_position.z),
                        ));

                    renderer.reset_camera(new_position, new_look_dir);

                    if !game.position_set {
                        game.position_set = true;
                        tracing::info!(
                            "Player position set to ({:.1}, {:.1}, {:.1})",
                            new_position.x,
                            new_position.y,
                            new_position.z
                        );
                    }

                    connection.packet_tx.send(ServerboundGamePacket::MovePlayerPosRot(
                        azalea_protocol::packets::game::s_move_player_pos_rot::ServerboundMovePlayerPosRot {
                            pos: new_position.into(),
                            look_direction: new_look_dir.into(),
                            flags: azalea_protocol::common::movements::MoveFlags {
                                on_ground: false,
                                horizontal_collision: false,
                            },
                        },
                    ));
                }
                NetworkEvent::PlayerHealth {
                    health,
                    food,
                    saturation,
                } => {
                    game.player.health = health;
                    game.player.food = food;
                    game.player.saturation = saturation;
                    if health > 0.0 && game.dead {
                        game.dead = false;
                        self.apply_cursor_grab(window, Some(game));
                    } else if health <= 0.0 && !game.dead {
                        game.dead = true;
                        game.death_message = String::new();
                        game.death_instant = Instant::now();
                        game.death_confirm = false;
                        game.respawn_sent = false;

                        let _ = window.set_cursor_grab(CursorGrabMode::None);
                        window.set_cursor_visible(true);
                    }
                }
                NetworkEvent::PlayerExperience { progress, level } => {
                    game.player.experience_progress = progress;
                    game.player.experience_level = level;
                }
                NetworkEvent::EntityArmorUpdate { entity_id, armor } => {
                    if entity_id == game.player.entity_id {
                        game.player.armor = armor;
                    }
                }
                NetworkEvent::InventoryContent { items } => {
                    game.player.inventory.set_contents(items);
                }
                NetworkEvent::InventorySlot { index, item } => {
                    game.player.inventory.set_slot(index as usize, item);
                }
                NetworkEvent::ChatMessage { spans } => {
                    game.chat.push_message(spans);
                }
                NetworkEvent::CommandTree { tree } => {
                    game.command_tree = Some(tree);
                }
                NetworkEvent::BlockUpdate { pos, state } => {
                    if game.interaction.update_known_server_state(&pos, state) {
                        continue;
                    }
                    game.chunk_store.set_block_state(pos.x, pos.y, pos.z, state);
                    let chunk_pos = azalea_core::position::ChunkPos::new(
                        pos.x.div_euclid(16),
                        pos.z.div_euclid(16),
                    );
                    // TODO(chunk-light): route edge edits through enqueue_with_neighbors so the
                    // neighbor's border lighting refreshes too (gate on edge-proximity to avoid
                    // churn).
                    chunks_to_mesh.push(chunk_pos);
                }
                NetworkEvent::SectionBlocksUpdate { updates } => {
                    for (pos, state) in updates {
                        if game.interaction.update_known_server_state(&pos, state) {
                            continue;
                        }
                        game.chunk_store.set_block_state(pos.x, pos.y, pos.z, state);
                        let chunk_pos = azalea_core::position::ChunkPos::new(
                            pos.x.div_euclid(16),
                            pos.z.div_euclid(16),
                        );
                        // TODO(chunk-light): same neighbor re-mesh as BlockUpdate above for edge
                        // edits.
                        if !chunks_to_mesh.contains(&chunk_pos) {
                            chunks_to_mesh.push(chunk_pos);
                        }
                    }
                }
                NetworkEvent::BlockEntitySync { chunk_pos, entries } => {
                    game.chunk_store.block_entities.retain(|p, _| {
                        p.x.div_euclid(16) != chunk_pos.x || p.z.div_euclid(16) != chunk_pos.z
                    });
                    game.block_entity_anim.drop_chunk(chunk_pos.x, chunk_pos.z);
                    for (pos, kind, nbt) in entries {
                        game.chunk_store.block_entities.insert(
                            pos,
                            crate::world::block_entity::StoredBlockEntity { kind, nbt },
                        );
                    }
                }
                NetworkEvent::BlockEntityUpdate { pos, kind, nbt } => match nbt {
                    Some(nbt) => {
                        let chunk_pos = azalea_core::position::ChunkPos::new(
                            pos.x.div_euclid(16),
                            pos.z.div_euclid(16),
                        );
                        if game.chunk_store.get_chunk(&chunk_pos).is_some() {
                            game.chunk_store.block_entities.insert(
                                pos,
                                crate::world::block_entity::StoredBlockEntity { kind, nbt },
                            );
                        }
                    }
                    None => {
                        game.chunk_store.block_entities.remove(&pos);
                    }
                },
                NetworkEvent::BlockEvent {
                    pos,
                    action_id,
                    action_parameter,
                } => {
                    // Action 1 for chest/shulker = open-viewer count.
                    if action_id == 1 {
                        game.block_entity_anim.set_open_count(pos, action_parameter);
                    }
                }
                NetworkEvent::PlaySound {
                    sound,
                    category,
                    pos,
                    volume,
                    pitch,
                    seed,
                } => {
                    self.audio
                        .play_world_sound(&sound, category, pos, volume, pitch, seed);
                }
                NetworkEvent::PlayEntitySound {
                    sound,
                    category,
                    entity_id,
                    volume,
                    pitch,
                    seed,
                } => {
                    let pos = (entity_id == game.player.entity_id)
                        .then_some(game.player.position + dvec3(0.0, 1.0, 0.0))
                        .or_else(|| game.entity_store.living.get(&entity_id).map(|e| e.position));

                    if let Some(pos) = pos {
                        self.audio
                            .play_world_sound(&sound, category, pos, volume, pitch, seed);
                    }
                }
                NetworkEvent::GameModeChanged { game_mode } => {
                    tracing::info!("Game mode changed to {game_mode}");
                    game.player.game_mode = game_mode;
                    if game.inventory_open || game.creative_inventory_open {
                        match game_mode {
                            1 => {
                                game.inventory_open = false;
                                game.creative_inventory_open = true;
                            }
                            3 => {
                                game.inventory_open = false;
                                game.creative_inventory_open = false;
                                self.apply_cursor_grab(window, Some(game));
                            }
                            _ => {
                                game.inventory_open = true;
                                game.creative_inventory_open = false;
                            }
                        }
                    }
                }
                NetworkEvent::ServerViewDistance { distance } => {
                    tracing::info!("Server view distance: {distance}");
                    game.server_render_distance = distance;
                }
                NetworkEvent::ServerSimulationDistance { distance } => {
                    tracing::info!("Server simulation distance: {distance}");
                    game.server_simulation_distance = distance;
                }
                NetworkEvent::BlockChangedAck { seq } => {
                    if let Some(snap) = game.interaction.acknowledge(
                        seq,
                        &game.chunk_store,
                        game.player.position.into(),
                        &mut chunks_to_mesh,
                    ) {
                        game.player.position = snap.into();
                        game.player.prev_position = game.player.position;
                    }
                }
                NetworkEvent::TimeUpdate {
                    game_time,
                    day_time,
                } => {
                    game.sky_state.game_time = game_time;
                    if let Some(dt) = day_time {
                        game.sky_state.day_time = dt;
                    }
                }
                NetworkEvent::WeatherUpdate { event, param } => {
                    // Mirrors vanilla ClientPacketListener.handleGameEvent: the
                    // server drives the level, the client just applies it.
                    use azalea_protocol::packets::game::c_game_event::EventType;
                    match event {
                        EventType::StartRaining => game.sky_state.rain_level = 0.0,
                        EventType::StopRaining => game.sky_state.rain_level = 1.0,
                        EventType::RainLevelChange => game.sky_state.rain_level = param,
                        EventType::ThunderLevelChange => game.sky_state.thunder_level = param,
                        _ => {}
                    }
                }
                NetworkEvent::EntitySpawned {
                    id,
                    entity_type,
                    position,
                    velocity,
                    y_rot_deg,
                    x_rot_deg,
                    head_y_rot_deg,
                } => {
                    if crate::entity::is_living_mob(&entity_type) {
                        game.entity_store.spawn_living(
                            id,
                            entity_type,
                            position,
                            LookDirection::new(head_y_rot_deg, x_rot_deg),
                            y_rot_deg,
                            head_y_rot_deg,
                        );
                    }
                    if entity_type == azalea_registry::builtin::EntityKind::Item {
                        game.item_entity_store.spawn_item(id, position, velocity);
                    }
                }
                NetworkEvent::EntityMoved { id, dx, dy, dz } => {
                    game.entity_store.move_living_delta(id, dx, dy, dz);
                    game.item_entity_store.move_delta(id, dx, dy, dz);
                }
                NetworkEvent::EntityMovedRotated {
                    id,
                    dx,
                    dy,
                    dz,
                    y_rot_deg,
                    x_rot_deg,
                } => {
                    game.entity_store.move_living_delta(id, dx, dy, dz);
                    game.entity_store
                        .update_living_rotation(id, y_rot_deg, x_rot_deg);
                    game.item_entity_store.move_delta(id, dx, dy, dz);
                }
                NetworkEvent::EntityTeleported {
                    id,
                    position,
                    y_rot_deg,
                    x_rot_deg,
                } => {
                    game.entity_store.teleport_living(id, position);
                    game.entity_store
                        .update_living_rotation(id, y_rot_deg, x_rot_deg);
                    game.item_entity_store.teleport(id, position);
                }
                NetworkEvent::EntitiesRemoved { ids } => {
                    for id in &ids {
                        game.entity_store.remove_living(*id);
                    }
                    game.item_entity_store.remove(&ids);
                }
                NetworkEvent::EntityHeadRotation {
                    id,
                    head_y_rot_deg: head_y_rot,
                } => {
                    game.entity_store.update_head_rotation(id, head_y_rot);
                }
                NetworkEvent::EntityItemData {
                    id,
                    item_name,
                    count,
                } => {
                    let is_block_model = renderer.ensure_item_mesh(&item_name);

                    game.item_entity_store
                        .set_item_data(id, item_name, count, is_block_model);
                }
                NetworkEvent::EntityBabyFlag { id, is_baby } => {
                    game.entity_store.set_baby(id, is_baby);
                }
                NetworkEvent::SheepWoolData { id, color, sheared } => {
                    game.entity_store.set_sheep_wool(id, color, sheared);
                }
                NetworkEvent::SheepEatStart { id } => {
                    game.entity_store.start_sheep_eat(id);
                }
                NetworkEvent::CowVariant { id, variant } => {
                    game.entity_store.set_cow_variant(id, variant);
                }
                NetworkEvent::EntityCustomName { id, name } => {
                    game.entity_store.set_custom_name(id, name);
                }
                NetworkEvent::ItemPickedUp {
                    item_id,
                    collector_id,
                } => {
                    let target_pos = game
                        .entity_store
                        .living
                        .get(&collector_id)
                        .map(|e| e.position + dvec3(0.0, 0.81, 0.0))
                        .unwrap_or_else(|| {
                            Position::new(
                                game.player.position.x,
                                game.player.position.y + 0.81,
                                game.player.position.z,
                            )
                        });
                    game.item_entity_store.pickup(item_id, target_pos);
                }
                NetworkEvent::PlayerLogin { entity_id } => {
                    game.player.entity_id = entity_id;
                }
                NetworkEvent::PlayerScore { entity_id, score } => {
                    if entity_id == game.player.entity_id {
                        game.player.score = score;
                    }
                }
                NetworkEvent::PlayerDied { message } => {
                    game.dead = true;
                    game.death_message = message;
                    game.death_instant = Instant::now();
                    game.death_confirm = false;
                    game.respawn_sent = false;

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
                    self.pending_pack_download = Some(PendingPackDownload {
                        id,
                        required,
                        hash: hash.clone(),
                        handle: std::thread::spawn(move || {
                            ResourcePackManager::download_server_pack(&cache_dir, &url, &hash)
                        }),
                    });
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
                    game.tab_list.clear();
                }
                NetworkEvent::PlayerInfoUpdate { actions, entries } => {
                    game.tab_list.apply_update(&actions, &entries);
                }
                NetworkEvent::PlayerInfoRemove { uuids } => {
                    game.tab_list.remove(&uuids);
                }
                NetworkEvent::TabListHeaderFooter { header, footer } => {
                    game.tab_list.set_header_footer(header, footer);
                }
            }
        }

        if let Some(pending) = &self.pending_pack_download
            && pending.handle.is_finished()
        {
            let pending = self.pending_pack_download.take().unwrap();
            let result = pending.handle.join();

            use azalea_protocol::packets::game::s_resource_pack;
            let action = match result {
                Err(_) => {
                    tracing::error!("Resource pack {} thread panicked", pending.id);
                    if pending.required {
                        disconnect_reason = Some(
                            "Required resource pack failed: thread panicked (internal error)"
                                .into(),
                        );
                    }
                    s_resource_pack::Action::FailedDownload
                }
                Ok(Err(e)) => {
                    tracing::error!("Resource pack {} failed: {e}", pending.id);
                    if pending.required {
                        disconnect_reason = Some(format!("Required resource pack failed: {e}"));
                    }
                    s_resource_pack::Action::FailedDownload
                }
                Ok(Ok(_path)) => {
                    self.resource_packs
                        .apply_server_pack(pending.id, &pending.hash);
                    tracing::info!("Resource pack {} loaded successfully", pending.id);
                    self.menu.reload_assets = true;
                    s_resource_pack::Action::SuccessfullyLoaded
                }
            };

            connection
                .packet_tx
                .send(ServerboundGamePacket::ResourcePack(
                    s_resource_pack::ServerboundResourcePack {
                        id: pending.id,
                        action,
                    },
                ));
            self.menu.active_packs = self.resource_packs.active_pack_info();
        }

        let player_chunk = azalea_core::position::ChunkPos::new(
            (game.player.position.x as i32).div_euclid(16),
            (game.player.position.z as i32).div_euclid(16),
        );
        for pos in chunks_to_mesh {
            let lod = chunk_lod(pos, player_chunk);
            game.meshed_lod.insert(pos, lod);
            game.mesh_dispatcher.enqueue(&game.chunk_store, pos, lod);
        }

        if player_chunk != game.last_player_chunk {
            game.last_player_chunk = player_chunk;
            for pos in game.chunk_store.loaded_positions() {
                let new_lod = chunk_lod(pos, player_chunk);
                let old_lod = game.meshed_lod.get(&pos).copied();
                if old_lod != Some(new_lod) {
                    game.meshed_lod.insert(pos, new_lod);
                    game.mesh_dispatcher
                        .enqueue(&game.chunk_store, pos, new_lod);
                }
            }
        }

        disconnect_reason
    }

    pub fn tick_physics(
        &mut self,
        renderer: &mut Renderer,
        connection: &ConnectionHandle,
        game: &mut GameState,
    ) {
        if game.dead {
            return;
        }

        // Open menus only release the keys; the simulation keeps ticking.
        let input_live = game.input_live();
        let neutral = InputState::released();
        let input = if input_live { &self.input } else { &neutral };

        game.player.prev_look_dir = game.player.look_dir;
        game.player.look_dir = renderer.camera_look_dir();

        game.player.prev_position = game.player.position;
        movement::tick(&mut game.player, input, &game.chunk_store);
        game.entity_store.tick_living();

        let dx = game.player.position.x - game.player.prev_position.x;
        let dz = game.player.position.z - game.player.prev_position.z;
        crate::entity::update_walk_animation(
            dx,
            dz,
            &mut game.player_walk_pos,
            &mut game.player_walk_speed,
            &mut game.player_prev_walk_speed,
        );

        renderer.set_base_fov(self.menu.fov as f32);
        renderer.update_fov_mod(compute_fov_modifier(&game.player));

        Self::send_input_packet(input, connection, game);
        self.send_sprint_command(connection, game);
        self.send_position_packet(connection, game);

        let eye_pos = game.player.eye_pos();
        game.interaction
            .update_target(eye_pos, game.player.look_dir, &game.chunk_store);

        let dirty = game.interaction.tick(
            input,
            &game.chunk_store,
            &connection.packet_tx,
            &self.audio,
            game.player.position.into(),
            game.player.on_ground,
            game.player.game_mode == 1,
        );
        for pos in dirty {
            game.mesh_dispatcher.enqueue(&game.chunk_store, pos, 0);
        }

        // Menus consume their own clicks later in the frame, so only clear
        // them when the simulation saw the live input.
        if input_live {
            self.input.clear_click_events();
        }
    }

    fn send_input_packet(input: &InputState, connection: &ConnectionHandle, game: &mut GameState) {
        let sender = &connection.packet_tx;
        let current = PlayerInputState {
            forward: input.key_pressed(KeyCode::KeyW),
            backward: input.key_pressed(KeyCode::KeyS),
            left: input.key_pressed(KeyCode::KeyA),
            right: input.key_pressed(KeyCode::KeyD),
            jump: input.key_pressed(KeyCode::Space),
            shift: input.key_pressed(KeyCode::ShiftLeft),
            sprint: game.player.sprinting,
        };

        if current != game.last_sent_input {
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
            game.last_sent_input = current;
        }
    }

    pub fn send_sprint_command(&self, connection: &ConnectionHandle, game: &mut GameState) {
        let sprinting = game.player.sprinting;
        if sprinting != game.was_sprinting {
            let sender = &connection.packet_tx;
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
            game.was_sprinting = sprinting;
        }
    }

    pub fn send_position_packet(&self, connection: &ConnectionHandle, game: &mut GameState) {
        let sender = &connection.packet_tx;
        use azalea_protocol::common::movements::MoveFlags;
        use azalea_protocol::packets::game::*;

        let pos = game.player.position;
        let look_dir = game.player.look_dir;

        let dx = pos.x - game.last_sent_pos.x;
        let dy = pos.y - game.last_sent_pos.y;
        let dz = pos.z - game.last_sent_pos.z;
        game.position_send_counter += 1;
        let pos_changed = dx * dx + dy * dy + dz * dz > POSITION_THRESHOLD_SQ
            || game.position_send_counter >= POSITION_SEND_INTERVAL;
        let rot_changed = (look_dir.y_rot_deg() - game.last_sent_look_dir.y_rot_deg()) != 0.0
            || (look_dir.x_rot_deg() - game.last_sent_look_dir.x_rot_deg()) != 0.0;

        let flags = MoveFlags {
            on_ground: game.player.on_ground,
            horizontal_collision: game.player.horizontal_collision,
        };

        if pos_changed && rot_changed {
            sender.send(ServerboundGamePacket::MovePlayerPosRot(
                ServerboundMovePlayerPosRot {
                    pos: pos.into(),
                    look_direction: look_dir.into(),
                    flags,
                },
            ));
        } else if pos_changed {
            sender.send(ServerboundGamePacket::MovePlayerPos(
                ServerboundMovePlayerPos {
                    pos: pos.into(),
                    flags,
                },
            ));
        } else if rot_changed {
            sender.send(ServerboundGamePacket::MovePlayerRot(
                ServerboundMovePlayerRot {
                    look_direction: look_dir.into(),
                    flags,
                },
            ));
        } else if game.player.on_ground != game.last_sent_on_ground
            || game.player.horizontal_collision != game.last_sent_horizontal_collision
        {
            sender.send(ServerboundGamePacket::MovePlayerStatusOnly(
                ServerboundMovePlayerStatusOnly { flags },
            ));
        }

        if pos_changed {
            game.last_sent_pos = pos;
            game.position_send_counter = 0;
        }
        if rot_changed {
            game.last_sent_look_dir = look_dir;
        }
        game.last_sent_on_ground = game.player.on_ground;
        game.last_sent_horizontal_collision = game.player.horizontal_collision;
    }
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
