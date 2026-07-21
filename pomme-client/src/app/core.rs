use std::collections::HashMap;
use std::ops::Add;
use std::sync::Arc;
use std::time::Instant;

use azalea_core::position::ChunkSectionPos;
use azalea_protocol::packets::game::{
    ServerboundClientCommand, ServerboundGamePacket, s_client_command, s_client_tick_end,
};
use glam::{FloatExt, dvec3};
use winit::keyboard::KeyCode;
use winit::window::{CursorGrabMode, Window};

use crate::app::input::{Action, InputState, STICK_MOVEMENT_THRESHOLD};
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
use crate::world::chunk::ChunkStore;

pub struct PendingPackDownload {
    pub id: uuid::Uuid,
    pub required: bool,
    pub hash: String,
    pub handle: std::thread::JoinHandle<PackDownloadResult>,
}
pub type PackDownloadResult = Result<std::path::PathBuf, crate::resource_pack::PackError>;
struct PlayerSkinResult {
    uuid: uuid::Uuid,
    textures: Option<String>,
    result: Result<crate::renderer::SkinData, String>,
}

/// Applies a server-driven block change: block-entity sync, prediction
/// absorption, the block + light write, and the remesh cascade. The block
/// entity syncs even when a pending prediction absorbs the update (the state
/// is applied later in `acknowledge`), so e.g. a chest placed where a break
/// was just predicted still gets its entry.
fn apply_server_block(
    game: &mut GameState,
    priority_remesh: &mut Vec<(azalea_core::position::ChunkPos, i32)>,
    pos: azalea_core::position::BlockPos,
    state: azalea_block::BlockState,
) {
    crate::world::block_entity::sync_block_entity(&mut game.chunk_store.block_entities, pos, state);
    if game.interaction.update_known_server_state(&pos, state) {
        return;
    }
    crate::world::light::set_block_and_light(
        &game.chunk_store,
        &mut game.light_engine,
        pos.x,
        pos.y,
        pos.z,
        state,
    );
    dirty_sections_for_block(
        priority_remesh,
        pos.x,
        pos.y,
        pos.z,
        game.chunk_store.min_y(),
        game.chunk_store.section_count(),
    );
}

/// Queues a column's packet light for the per-tick apply. Chunk loads enable
/// the column, standalone light updates are corrections.
fn queue_light_apply(
    game: &mut GameState,
    pos: azalea_core::position::ChunkPos,
    light: &crate::net::PacketLightData,
    enable: bool,
) {
    let count = game.light_engine.light_section_count();
    game.light_engine
        .queue_task(crate::world::light::LightTask::ApplyLight {
            pos: (pos.x, pos.z),
            sky: crate::world::light::section_entries(
                count,
                &light.sky_y_mask,
                &light.empty_sky_y_mask,
                &light.sky_updates[..],
            ),
            block: crate::world::light::section_entries(
                count,
                &light.block_y_mask,
                &light.empty_block_y_mask,
                &light.block_updates[..],
            ),
            enable,
        });
}
/// Mirror of vanilla `LevelExtractor.setBlockDirty`: a block at (x,y,z) dirties
/// its own 16³ section plus any neighbour section it touches when on a boundary
/// (the 3×3×3-block cascade → up to a few sections). Pushes deduped
/// `(column, section_index)` keys.
fn dirty_sections_for_block(
    out: &mut Vec<(azalea_core::position::ChunkPos, i32)>,
    x: i32,
    y: i32,
    z: i32,
    min_y: i32,
    section_count: i32,
) {
    for bz in (z - 1)..=(z + 1) {
        for bx in (x - 1)..=(x + 1) {
            for by in (y - 1)..=(y + 1) {
                let si = (by - min_y).div_euclid(16);
                if si < 0 || si >= section_count {
                    continue;
                }
                let col =
                    azalea_core::position::ChunkPos::new(bx.div_euclid(16), bz.div_euclid(16));
                let key = (col, si);
                if !out.contains(&key) {
                    out.push(key);
                }
            }
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
    player_skin_tx: crossbeam_channel::Sender<PlayerSkinResult>,
    player_skin_rx: crossbeam_channel::Receiver<PlayerSkinResult>,
    requested_player_skins: HashMap<uuid::Uuid, Option<String>>,
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
            version.clone(),
            user.access_token.clone(),
        );
        let asset_index =
            AssetIndex::load(&data_dirs.indexes_dir, &data_dirs.objects_dir, &version);
        let audio = crate::audio::AudioEngine::new(
            &data_dirs.jar_assets_dir,
            asset_index.clone(),
            menu.category_volumes(),
        );
        let (player_skin_tx, player_skin_rx) = crossbeam_channel::unbounded();
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
            player_skin_tx,
            player_skin_rx,
            requested_player_skins: HashMap::new(),
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
    fn queue_player_skin(&mut self, uuid: uuid::Uuid, textures: Option<String>) {
        if self.requested_player_skins.get(&uuid) == Some(&textures) {
            return;
        }
        self.requested_player_skins.insert(uuid, textures.clone());

        // Name-derived (v3) UUIDs from offline-mode servers have no Mojang
        // profile to fetch; keep the default skin.
        if textures.is_none() && uuid.get_version_num() == 3 {
            return;
        }

        let tx = self.player_skin_tx.clone();
        let requested_textures = textures.clone();
        self.tokio_rt.spawn(async move {
            let result = if let Some(textures) = textures {
                crate::renderer::fetch_skin_texture_from_profile_property(&textures).await
            } else {
                let uuid_str = uuid.to_string().replace('-', "");
                crate::renderer::fetch_skin_texture(&uuid_str).await
            };
            let _ = tx.send(PlayerSkinResult {
                uuid,
                textures: requested_textures,
                result,
            });
        });
    }
    fn drain_player_skin_results(&mut self, renderer: &mut Renderer) {
        while let Ok(skin) = self.player_skin_rx.try_recv() {
            if self.requested_player_skins.get(&skin.uuid) != Some(&skin.textures) {
                continue;
            }
            match skin.result {
                Ok(data) => {
                    renderer.update_player_entity_skin(&skin.uuid, &data);
                }
                Err(e) => {
                    tracing::warn!("Failed to load entity player skin for {}: {e}", skin.uuid)
                }
            }
        }
    }
    fn remove_player_skin(&mut self, renderer: &mut Renderer, uuid: &uuid::Uuid) {
        self.requested_player_skins.remove(uuid);
        renderer.remove_player_entity_skin(uuid);
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

        // Phase timers for the chunk-load benchmark's worst-frame breakdown.
        let t_net = std::time::Instant::now();

        // Block edits go on the priority lane so they apply instantly even while
        // chunks stream in, instead of starving behind the load backlog.
        let mut priority_remesh: Vec<(azalea_core::position::ChunkPos, i32)> = Vec::new();
        let mut disconnect_reason: Option<String> = None;
        let mut processed = 0u32;
        self.drain_player_skin_results(renderer);
        // Empty the channel before applying (see `GameState::pending_events`
        // for why nothing may stay in it); only the applying is budgeted, and
        // leftovers keep their order for next frame.
        while let Ok(event) = rx.try_recv() {
            game.pending_events.push_back(event);
        }
        const NET_DRAIN_BUDGET_SECS: f32 = 0.003;
        // Record the worst single apply for the bench breakdown, timed by
        // boundary stamps so the loop stays at one clock read per event.
        let mut worst_event = "";
        let mut worst_event_secs = 0.0f32;
        let mut event_start = t_net.elapsed().as_secs_f32();
        while processed < 4096 && event_start < NET_DRAIN_BUDGET_SECS {
            let Some(event) = game.pending_events.pop_front() else {
                break;
            };
            processed += 1;
            let event_kind = event.kind();
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
                    game.meshing
                        .set_biome_climate(Arc::clone(&game.biome_climate));
                }
                NetworkEvent::DimensionInfo {
                    height,
                    min_y,
                    has_skylight,
                } => {
                    tracing::info!(
                        "Dimension: height={height}, min_y={min_y}, skylight={has_skylight}"
                    );
                    game.chunk_store =
                        ChunkStore::new_with_dimension(self.menu.render_distance, height, min_y);
                    game.light_engine =
                        crate::world::light::LevelLightEngine::new(height, min_y, has_skylight);
                    game.position_set = false;
                    game.player_loaded_sent = false;
                    // Login/respawn recreate vanilla's LocalPlayer, resetting
                    // the XP display sentinel; waypoints persist.
                    game.xp_display_start_tick = i64::MIN;

                    renderer.clear_chunk_meshes();
                    // The server's protocol may have switched the block-table
                    // id space; the dispatcher's registry clone inherits the
                    // rebuilt tables.
                    renderer.rebuild_block_state_tables();
                    game.meshing.recreate_dispatcher(
                        renderer,
                        Arc::clone(&game.chunk_store.shared),
                        Arc::clone(&game.biome_climate),
                    );
                }
                NetworkEvent::ChunkLoaded {
                    pos,
                    chunk,
                    light,
                    sky_sources,
                } => {
                    game.chunk_store.insert_chunk(pos, *chunk);
                    game.light_engine.on_chunk_loaded(
                        &game.chunk_store,
                        (pos.x, pos.z),
                        sky_sources,
                    );
                    // The column meshes once its queued light applies (vanilla
                    // schedules the rebuild from enableChunkLight, not here).
                    queue_light_apply(game, pos, &light, true);
                }
                NetworkEvent::LightUpdate { pos, light } => {
                    queue_light_apply(game, pos, &light, false);
                }
                NetworkEvent::ChunkUnloaded { pos } => {
                    game.chunk_store.unload_chunk(&pos);
                    game.light_engine.on_chunk_unloaded((pos.x, pos.z));
                    game.light_engine
                        .queue_task(crate::world::light::LightTask::Remove {
                            pos: (pos.x, pos.z),
                        });
                    game.block_entity_anim.drop_chunk(pos.x, pos.z);
                    game.meshing.on_chunk_unload(pos);
                    renderer.remove_chunk_mesh(&pos);
                }
                NetworkEvent::ChunkCacheCenter { x, z } => {
                    tracing::debug!("Chunk cache center: [{x}, {z}]");
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
                    if progress != game.player.experience_progress {
                        // Vanilla LocalPlayer.setExperienceValues: the first
                        // change after (re)spawn only arms the sentinel and
                        // doesn't yet prioritize the XP bar.
                        game.xp_display_start_tick = if game.xp_display_start_tick == i64::MIN {
                            i64::MIN + 1
                        } else {
                            game.tick_count as i64
                        };
                    }
                    game.player.experience_progress = progress;
                    game.player.experience_level = level;
                }
                NetworkEvent::Waypoint {
                    operation,
                    waypoint,
                } => {
                    game.waypoints.apply(operation, waypoint);
                }
                NetworkEvent::EntityArmorUpdate { entity_id, armor } => {
                    if entity_id == game.player.entity_id {
                        game.player.armor = armor;
                    }
                }
                NetworkEvent::ContainerContent {
                    container_id,
                    items,
                    carried,
                    state_id,
                } => {
                    // State ids are per-menu (vanilla scopes them to the menu
                    // the packet addresses); the rendered carried stack is the
                    // open menu's, so an inventory sync must not clobber it.
                    if container_id == 0 {
                        game.player.inventory.set_contents(items);
                        game.sync_container_from_inventory();
                        game.inventory_state_id = state_id;
                        if game.open_container.is_none() {
                            game.cursor_item = carried;
                        }
                    } else if game.open_menu_id() == Some(container_id) {
                        for (i, item) in items.into_iter().enumerate() {
                            game.set_menu_slot(i, item);
                        }
                        game.cursor_item = carried;
                        game.set_container_state_id(state_id);
                    }
                }
                NetworkEvent::CursorItem { item } => {
                    game.cursor_item = item;
                }
                NetworkEvent::Registries(registries) => {
                    game.registries = registries;
                }
                NetworkEvent::ContainerSlot {
                    container_id,
                    index,
                    item,
                    state_id,
                } => {
                    // Direct inventory updates (-2) carry no menu state id.
                    if container_id == 0 || container_id == -2 {
                        game.player.inventory.set_slot(index as usize, item);
                        game.sync_container_from_inventory();
                        if container_id == 0 {
                            game.inventory_state_id = state_id;
                        }
                    } else if game.open_menu_id() == Some(container_id) {
                        game.set_menu_slot(index as usize, item);
                        game.set_container_state_id(state_id);
                    }
                }
                NetworkEvent::ContainerData {
                    container_id,
                    id,
                    value,
                } => {
                    if let Some(c) = &mut game.open_container
                        && c.id == container_id
                        && let Some(d) = c.data.get_mut(id as usize)
                    {
                        *d = value as i16;
                    }
                }
                NetworkEvent::OpenScreen {
                    container_id,
                    menu_type,
                    title,
                } => {
                    use azalea_inventory::ItemStack;
                    use azalea_registry::builtin::MenuKind;

                    use crate::app::phases::in_game::ContainerScreen;
                    use crate::ui::furnace::FurnaceVariant;
                    let screen = match menu_type {
                        MenuKind::Crafting => Some(ContainerScreen::CraftingTable),
                        MenuKind::Furnace => {
                            Some(ContainerScreen::Furnace(FurnaceVariant::Furnace))
                        }
                        MenuKind::BlastFurnace => {
                            Some(ContainerScreen::Furnace(FurnaceVariant::BlastFurnace))
                        }
                        MenuKind::Smoker => Some(ContainerScreen::Furnace(FurnaceVariant::Smoker)),
                        MenuKind::Generic9x1 => Some(ContainerScreen::Chest { rows: 1 }),
                        MenuKind::Generic9x2 => Some(ContainerScreen::Chest { rows: 2 }),
                        MenuKind::Generic9x3 => Some(ContainerScreen::Chest { rows: 3 }),
                        MenuKind::Generic9x4 => Some(ContainerScreen::Chest { rows: 4 }),
                        MenuKind::Generic9x5 => Some(ContainerScreen::Chest { rows: 5 }),
                        MenuKind::Generic9x6 => Some(ContainerScreen::Chest { rows: 6 }),
                        MenuKind::ShulkerBox => Some(ContainerScreen::ShulkerBox),
                        MenuKind::Anvil => Some(ContainerScreen::Anvil),
                        MenuKind::Enchantment => Some(ContainerScreen::Enchantment),
                        _ => None,
                    };
                    if let Some(screen) = screen {
                        // Vanilla setScreen replaces whatever screen is up,
                        // including the pause menu.
                        game.paused = false;
                        game.inventory_open = false;
                        game.close_creative_inventory();
                        game.inv_drag = None;
                        game.inv_last_click = None;
                        game.open_container = Some(crate::app::phases::in_game::OpenContainer {
                            id: container_id,
                            title,
                            screen,
                            slots: vec![ItemStack::Empty; screen.click_kind().slot_count()],
                            data: [0; 10],
                            anvil: (screen == ContainerScreen::Anvil)
                                .then(crate::ui::anvil::AnvilState::new),
                            enchant: (screen == ContainerScreen::Enchantment)
                                .then(crate::ui::enchantment::EnchantState::new),
                            state_id: 0,
                        });
                        game.sync_container_from_inventory();
                        // The new menu replaces any previous one server-side;
                        // don't send a close for the replaced menu (the server
                        // would apply it to this one).
                        game.container_was_open = Some(container_id);
                        self.apply_cursor_grab(window, Some(game));
                    } else {
                        // TODO: render the remaining menu screens (chest,
                        // brewing stand, ...). Until then tell the server we
                        // closed the menu so its container state stays
                        // consistent.
                        use azalea_protocol::packets::game::s_container_close::ServerboundContainerClose;
                        connection
                            .packet_tx
                            .send(ServerboundGamePacket::ContainerClose(
                                ServerboundContainerClose { container_id },
                            ));
                    }
                }
                NetworkEvent::ContainerClosed => {
                    // Vanilla closes whatever menu is open regardless of the
                    // packet's container id.
                    game.close_menu();
                    // The server initiated the close; don't echo one back.
                    game.container_was_open = None;
                    self.apply_cursor_grab(window, Some(game));
                }
                NetworkEvent::ChatMessage { spans } => {
                    game.chat.push_message(spans);
                }
                NetworkEvent::CommandTree { tree } => {
                    game.command_tree = Some(tree);
                }
                NetworkEvent::CommandSuggestions { id, start, options } => {
                    game.chat.apply_server_suggestions(id, start, options);
                }
                NetworkEvent::BlockUpdate { pos, state } => {
                    apply_server_block(game, &mut priority_remesh, pos, state);
                }
                NetworkEvent::SectionBlocksUpdate { updates } => {
                    for (pos, state) in updates {
                        apply_server_block(game, &mut priority_remesh, pos, state);
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
                        if game.chunk_store.shared.has_chunk(chunk_pos) {
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
                                game.close_creative_inventory();
                                self.apply_cursor_grab(window, Some(game));
                            }
                            _ => {
                                game.inventory_open = true;
                                game.close_creative_inventory();
                            }
                        }
                    }
                }
                NetworkEvent::PlayerAbilitiesChanged { flying } => {
                    game.player.flying = flying;
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
                    let mut ack_dirty: Vec<azalea_core::position::BlockPos> = Vec::new();
                    let snap = game.interaction.acknowledge(
                        seq,
                        &game.chunk_store,
                        game.player.position.into(),
                        &mut ack_dirty,
                    );
                    if let Some(snap) = snap {
                        game.player.position = snap.into();
                        game.player.prev_position = game.player.position;
                    }
                    let min_y = game.chunk_store.min_y();
                    let n = game.chunk_store.section_count();
                    for b in ack_dirty {
                        game.light_engine
                            .on_block_dirty(&game.chunk_store, b.x, b.y, b.z);
                        dirty_sections_for_block(&mut priority_remesh, b.x, b.y, b.z, min_y, n);
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
                    uuid,
                    entity_type,
                    position,
                    velocity,
                    y_rot_deg,
                    x_rot_deg,
                    head_y_rot_deg,
                } => {
                    if crate::entity::is_living_mob(&entity_type) {
                        let player_uuid = (entity_type
                            == azalea_registry::builtin::EntityKind::Player)
                            .then_some(uuid);
                        game.entity_store.spawn_living(
                            id,
                            entity_type,
                            position,
                            LookDirection::new(head_y_rot_deg, x_rot_deg),
                            y_rot_deg,
                            player_uuid,
                        );
                        if let Some(uuid) = player_uuid {
                            let textures = game
                                .tab_list
                                .players
                                .get(&uuid)
                                .and_then(|p| p.textures.clone());
                            self.queue_player_skin(uuid, textures);
                        }
                    }
                    if entity_type == azalea_registry::builtin::EntityKind::Item {
                        game.item_entity_store.spawn_item(id, position, velocity);
                    }
                }
                NetworkEvent::EntityMoved {
                    id,
                    dx,
                    dy,
                    dz,
                    on_ground,
                } => {
                    game.entity_store.move_living_delta(id, dx, dy, dz);
                    game.item_entity_store.move_delta(id, dx, dy, dz, on_ground);
                }
                NetworkEvent::EntityMovedRotated {
                    id,
                    dx,
                    dy,
                    dz,
                    y_rot_deg,
                    x_rot_deg,
                    on_ground,
                } => {
                    game.entity_store.move_living_delta(id, dx, dy, dz);
                    game.entity_store
                        .update_living_rotation(id, y_rot_deg, x_rot_deg);
                    game.item_entity_store.move_delta(id, dx, dy, dz, on_ground);
                }
                NetworkEvent::EntityMotion { id, velocity } => {
                    game.item_entity_store.set_motion(id, velocity);
                }
                NetworkEvent::EntityTeleported {
                    id,
                    position,
                    velocity,
                    y_rot_deg,
                    x_rot_deg,
                    on_ground,
                } => {
                    game.entity_store.teleport_living(id, position);
                    game.entity_store
                        .update_living_rotation(id, y_rot_deg, x_rot_deg);
                    game.item_entity_store
                        .teleport(id, position, velocity, on_ground);
                }
                NetworkEvent::LevelEvent {
                    event_type,
                    pos,
                    data,
                } => {
                    // Vanilla `LevelEventHandler` case 2001 (block break).
                    // The server excludes the breaking player from the
                    // broadcast; the local break's effects come from
                    // `predict_destroy`. TODO: the other level events.
                    if event_type == 2001
                        && let Some(state) = crate::world::block::try_state(data)
                    {
                        if !crate::world::block::is_air(state) {
                            crate::player::interaction::play_break_sound(&self.audio, state, pos);
                        }
                        game.particle_store.add_destroy_block_effect(
                            pos,
                            state,
                            renderer.registry(),
                            &game.chunk_store,
                            &game.biome_climate,
                        );
                    }
                }
                NetworkEvent::LevelParticles {
                    kind,
                    override_limiter,
                    pos,
                    x_dist,
                    y_dist,
                    z_dist,
                    max_speed,
                    count,
                } => {
                    game.particle_store.add_particles_from_packet(
                        kind,
                        override_limiter,
                        pos,
                        glam::dvec3(x_dist as f64, y_dist as f64, z_dist as f64),
                        max_speed as f64,
                        count,
                        renderer.camera_render_position(),
                    );
                }
                NetworkEvent::EntitiesRemoved { ids } => {
                    for id in &ids {
                        if let Some(entity) = game.entity_store.remove_living(*id)
                            && let Some(uuid) = entity.player_uuid
                            && !game.entity_store.has_player_uuid(&uuid)
                        {
                            self.remove_player_skin(renderer, &uuid);
                        }
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
                    item_id,
                    count,
                } => {
                    let mesh = renderer.ensure_item_mesh(&item_name);
                    game.item_entity_store.set_item_data(
                        id,
                        item_name,
                        item_id,
                        count,
                        mesh.is_block_model,
                        mesh.min_y,
                        mesh.z_size,
                    );
                }
                NetworkEvent::EntityBabyFlag { id, is_baby } => {
                    game.entity_store.set_baby(id, is_baby);
                }
                NetworkEvent::EntityPose { id, is_crouching } => {
                    game.entity_store.set_crouching(id, is_crouching);
                }
                NetworkEvent::SheepWoolData { id, color, sheared } => {
                    game.entity_store.set_sheep_wool(id, color, sheared);
                }
                NetworkEvent::SheepEatStart { id } => {
                    game.entity_store.start_sheep_eat(id);
                }
                NetworkEvent::FinishUseItem { id } => {
                    // Vanilla sends event 9 only to the eater; remote players'
                    // eating effects come from each client simulating their use
                    // ticks off entity flags (TODO, with third-person items).
                    if id == game.player.entity_id {
                        game.interaction.complete_using(
                            &self.audio,
                            &mut game.particle_store,
                            &game.chunk_store,
                            game.player.position.into(),
                            game.player.eye_pos().into(),
                            game.player.look_dir,
                        );
                    }
                }
                NetworkEvent::CowVariant { id, variant } => {
                    game.entity_store.set_cow_variant(id, variant);
                }
                NetworkEvent::VillagerData {
                    id,
                    kind,
                    profession,
                    level,
                } => {
                    game.entity_store
                        .set_villager_data(id, kind, profession, level);
                }
                NetworkEvent::VillagerUnhappy { id, counter } => {
                    game.entity_store.set_villager_unhappy(id, counter);
                }
                NetworkEvent::EntityCustomName { id, name } => {
                    game.entity_store.set_custom_name(id, name);
                }
                NetworkEvent::EntityAggressive { id, aggressive } => {
                    game.entity_store.set_aggressive(id, aggressive);
                }
                NetworkEvent::EntitySwing { id } => {
                    game.entity_store.start_swing(id);
                }
                NetworkEvent::CreeperPowered { id, powered } => {
                    game.entity_store.set_powered(id, powered);
                }
                NetworkEvent::EntityDamaged { id } => {
                    game.entity_store.mark_hurt(id);
                }
                NetworkEvent::ItemPickedUp {
                    item_id,
                    collector_id,
                    amount,
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
                    if let Some(item_pos) =
                        game.item_entity_store.pickup(item_id, target_pos, amount)
                    {
                        // Vanilla plays this client-side in handleTakeItemEntity.
                        self.audio.play_world_sound(
                            &crate::audio::SoundRef::Event("entity.item.pickup".to_string()),
                            crate::audio::CATEGORY_PLAYERS,
                            item_pos,
                            0.2,
                            (fastrand::f32() - fastrand::f32()) * 1.4 + 2.0,
                            fastrand::u64(..),
                        );
                    }
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
                    self.requested_player_skins.clear();
                    renderer.clear_player_entity_skins();
                }
                NetworkEvent::PlayerInfoUpdate { actions, entries } => {
                    if actions.add_player {
                        for entry in &entries {
                            self.queue_player_skin(entry.uuid, entry.textures.clone());
                        }
                    } else {
                        for entry in entries.iter().filter(|e| e.textures.is_some()) {
                            self.queue_player_skin(entry.uuid, entry.textures.clone());
                        }
                    }
                    game.tab_list.apply_update(&actions, &entries);
                }
                NetworkEvent::PlayerInfoRemove { uuids } => {
                    for uuid in &uuids {
                        if !game.entity_store.has_player_uuid(uuid) {
                            self.remove_player_skin(renderer, uuid);
                        }
                    }
                    game.tab_list.remove(&uuids);
                }
                NetworkEvent::TabListHeaderFooter { header, footer } => {
                    game.tab_list.set_header_footer(header, footer);
                }
            }
            let event_end = t_net.elapsed().as_secs_f32();
            let event_secs = event_end - event_start;
            if event_secs > worst_event_secs {
                worst_event_secs = event_secs;
                worst_event = event_kind;
            }
            event_start = event_end;
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
        let player_chunk = game.player_chunk();
        let min_y_section = game.chunk_store.min_section_y();
        // Edits mesh the affected section(s) immediately on the priority lane,
        // ungated by visibility.
        for &(col, si) in &priority_remesh {
            let spos = ChunkSectionPos::new(col.x, min_y_section + si, col.z);
            game.enqueue_section_edit(spos, chunk_lod(col, player_chunk));
        }
        let ms = |t: std::time::Instant| t.elapsed().as_secs_f32() * 1000.0;
        game.last_update_phases.net_decode_ms = ms(t_net);
        game.last_update_phases.net_worst_event_ms = worst_event_secs * 1000.0;
        game.last_update_phases.net_worst_event = worst_event;

        // Enqueue everything that needs meshing; newly lit columns marked
        // their dirty bits in GameState::update_light. Visibility itself is
        // GPU-side (the Hi-Z pass), so no CPU visibility refresh runs here.
        let t_rescan = std::time::Instant::now();
        game.rescan_mesh_jobs(
            player_chunk,
            &renderer.camera_frustum_planes(),
            renderer.camera_render_position(),
        );
        game.last_update_phases.rescan_ms = ms(t_rescan);

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
        // Open menus only release the keys; the simulation keeps ticking. The
        // chunk-load benchmark also freezes the player so every run measures the
        // same fixed origin.
        let input_live = game.input_live() && game.chunk_load_bench.is_none();
        let neutral = InputState::released();
        let input = if input_live { &self.input } else { &neutral };
        game.player.prev_look_dir = game.player.look_dir;
        game.player.look_dir = renderer.camera_look_dir();
        game.player.prev_position = game.player.position;
        if game.chunk_load_bench.is_some() {
            game.player.velocity = crate::entity::components::Velocity::new(0.0, 0.0, 0.0);
        }
        movement::tick(
            &mut game.player,
            input,
            &game.chunk_store,
            game.interaction.use_speed_multiplier(),
            game.interaction.slow_due_to_using_item(),
        );
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
        game.player.tick_bob(dx, dz);
        renderer.set_base_fov(self.menu.fov as f32);
        let fov_effect_scale = self.menu.fov_effect();
        renderer.update_fov_mod(compute_fov_modifier(&game.player, fov_effect_scale));
        // Vanilla modifyFovBasedOnDeathOrFluid: narrow FOV underwater, unsmoothed.
        // TODO: lava camera fluid (no eyes_in_lava) and the death-animation factor.
        renderer.set_fluid_fov_factor(if game.player.eyes_in_water {
            1.0_f32.lerp(0.857_142_87, fov_effect_scale)
        } else {
            1.0
        });
        Self::send_input_packet(input, connection, game);
        self.send_sprint_command(connection, game);
        self.send_position_packet(connection, game);
        let eye_pos = game.player.eye_pos();
        game.interaction.update_target(
            eye_pos,
            game.player.look_dir,
            &game.chunk_store,
            &game.entity_store,
            crate::player::is_creative(game.player.game_mode),
        );
        let held_stack = match game
            .player
            .inventory
            .hotbar_slots()
            .get(input.selected_slot() as usize)
        {
            Some(azalea_inventory::ItemStack::Present(data)) if data.count > 0 => Some(data),
            _ => None,
        };
        let place_block = held_stack.and_then(|data| {
            let name = crate::player::inventory::item_resource_name(data.kind);
            renderer.registry().placeable_block_for_item(&name)
        });
        let hands_empty = held_stack.is_none() && game.player.inventory.offhand().is_empty();
        let dirty = game.interaction.tick(
            input,
            &game.chunk_store,
            &connection.packet_tx,
            &self.audio,
            game.player.position.into(),
            game.player.eye_pos().into(),
            game.player.look_dir,
            game.player.on_ground,
            crate::player::is_creative(game.player.game_mode),
            game.player.food,
            input.selected_slot(),
            held_stack,
            place_block,
            hands_empty,
            &mut crate::player::interaction::BreakEffects {
                particles: &mut game.particle_store,
                registry: renderer.registry(),
                biome_climate: &game.biome_climate,
            },
        );
        if !dirty.is_empty() {
            let min_y = game.chunk_store.min_y();
            let n = game.chunk_store.section_count();
            let mut sections: Vec<(azalea_core::position::ChunkPos, i32)> = Vec::new();
            for b in dirty {
                // Light lands in this frame's update_light, matching vanilla's
                // prediction timing (setBlockState queues; the per-frame
                // ClientLevel.update drains).
                game.light_engine
                    .on_block_dirty(&game.chunk_store, b.x, b.y, b.z);
                dirty_sections_for_block(&mut sections, b.x, b.y, b.z, min_y, n);
            }
            // Player edits are always adjacent (lod 0) and mesh on this
            // thread so they show this frame, like vanilla's compileSync.
            let min_y_section = min_y.div_euclid(16);
            for (col, si) in sections {
                let spos = ChunkSectionPos::new(col.x, min_y_section + si, col.z);
                game.mesh_edit_now(spos, 0);
            }
        }
        // Menus consume their own clicks later in the frame, so only clear
        // them when the simulation saw the live input.
        if input_live {
            self.input.clear_just_pressed_actions();
        }
        // Marks the end of the client tick (1.21.2+). Must be the last packet of
        // the tick: servers and anti-cheat batch our movement between these to
        // tick-align it, so omitting it makes them reject/rubber-band movement.
        connection
            .packet_tx
            .send(ServerboundGamePacket::ClientTickEnd(
                s_client_tick_end::ServerboundClientTickEnd,
            ));
    }
    fn send_input_packet(input: &InputState, connection: &ConnectionHandle, game: &mut GameState) {
        let sender = &connection.packet_tx;
        let analog_move = input.get_gamepad_left_analog().unwrap_or(glam::Vec2::ZERO);
        let current = PlayerInputState {
            forward: input.key_pressed(KeyCode::KeyW) || analog_move.y > STICK_MOVEMENT_THRESHOLD,
            backward: input.key_pressed(KeyCode::KeyS) || analog_move.y < -STICK_MOVEMENT_THRESHOLD,
            left: input.key_pressed(KeyCode::KeyA) || analog_move.x > STICK_MOVEMENT_THRESHOLD,
            right: input.key_pressed(KeyCode::KeyD) || analog_move.x < -STICK_MOVEMENT_THRESHOLD,
            jump: input.performing_action(Action::Jump),
            shift: input.performing_action(Action::Sneak),
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
pub(crate) fn chunk_lod(
    pos: azalea_core::position::ChunkPos,
    player: azalea_core::position::ChunkPos,
) -> u32 {
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
/// Vanilla `AbstractClientPlayer.getFieldOfViewModifier`. `effect_scale` is the
/// `fovEffectScale` accessibility value (1.0 = full effect).
fn compute_fov_modifier(player: &LocalPlayer, effect_scale: f32) -> f32 {
    let mut modifier = 1.0;
    if player.flying {
        modifier *= 1.1;
    }
    // Vanilla's speedFactor is MOVEMENT_SPEED / walkingSpeed; with Pomme's
    // client-side speed model that reduces to sprint ? 1.3 : 1.0.
    // TODO: drive from the MOVEMENT_SPEED attribute so Speed/Slowness potions
    // and gear modifiers affect FOV too.
    // TODO: bow-draw narrowing and spyglass scoping (need item-use-duration state).
    let speed_factor: f32 = if player.sprinting { 1.3 } else { 1.0 };
    modifier *= (speed_factor + 1.0) / 2.0;
    1.0_f32.lerp(modifier, effect_scale)
}
