use std::collections::{HashMap, HashSet};
use std::ops::Add;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Instant;

use azalea_protocol::packets::game::s_player_input::ServerboundPlayerInput;
use azalea_protocol::packets::game::{
    ServerboundClientCommand, ServerboundGamePacket, s_client_command, s_client_tick_end,
};
use glam::{FloatExt, dvec3};
use winit::keyboard::KeyCode;
use winit::monitor::MonitorHandle;
use winit::window::{CursorGrabMode, Fullscreen, Window};

use crate::app::input::{Action, InputState, STICK_MOVEMENT_THRESHOLD};
use crate::app::level_load::ReadyInputs;
use crate::app::phases::in_game::GameState;
use crate::app::phases::{ConnectionPhase, Gfx};
use crate::app::{POSITION_SEND_INTERVAL, POSITION_THRESHOLD_SQ};
use crate::attribute::{AttributeInstance, AttributeKind, AttributeMap};
use crate::dirs::DataDirs;
use crate::discord::DiscordPresence;
use crate::entity::components::{LookDirection, Position, Velocity};
use crate::net::connection::ConnectionHandle;
use crate::net::{DimensionInfo, NetworkEvent};
use crate::physics::movement;
use crate::player::LocalPlayer;
use crate::player::tab_list::{PlayerChatValidation, TabList};
use crate::renderer::Renderer;
use crate::renderer::chunk::mesh_source::MeshSource;
use crate::renderer::chunk::occlusion_graph::Update as VisUpdate;
use crate::resources::ResourceManager;
use crate::resources::managers::ReloadableResourceManager;
use crate::resources::pack_resources::PackType;
use crate::resources::packs::{
    BuiltinRepositorySource, FolderRepositorySource, PackEnablementConfig, PackId,
    PackLocationInfo, PackPosition, PackRepository, PhysicalPack, RegisteredRepositorySource,
    SymlinkPolicy, ThemeSpecification, download_server_pack,
};
use crate::ui::common::HeadProfile;
use crate::ui::menu::{
    CREDITS_KEY_CTRL_L, CREDITS_KEY_CTRL_R, CREDITS_KEY_SPACE, CREDITS_KEY_UP, MainMenu, MenuInput,
    PanoramaTheme,
};
use crate::ui::text::InlineObject;
use crate::user::UserData;
use crate::world::block::block_registry::BlockDefinitions;
use crate::world::chunk::ChunkStore;
use crate::world::environment::EnvironmentProbe;

pub struct PendingPackDownload {
    pub id: uuid::Uuid,
    pub required: bool,
    pub hash: String,
    pub handle: std::thread::JoinHandle<PackDownloadResult>,
}

pub type PackDownloadResult =
    Result<std::path::PathBuf, crate::resources::packs::PackDownloadError>;

#[derive(Clone, Debug, PartialEq, Eq)]
enum PlayerSkinSource {
    Uuid,
    Textures(String),
    Name(String),
}

/// Fetches the skin a source names; `uuid` is the profile id a `Uuid` source
/// looks up.
async fn fetch_skin(
    source: &PlayerSkinSource,
    uuid: Option<uuid::Uuid>,
) -> Result<crate::renderer::SkinData, String> {
    match source {
        PlayerSkinSource::Textures(textures) => {
            crate::renderer::fetch_skin_texture_from_profile_property(textures).await
        }
        PlayerSkinSource::Name(name) => crate::renderer::fetch_skin_texture_by_name(name).await,
        PlayerSkinSource::Uuid => {
            let uuid = uuid.unwrap_or_default().to_string().replace('-', "");
            crate::renderer::fetch_skin_texture(&uuid).await
        }
    }
}

struct PlayerSkinResult {
    uuid: uuid::Uuid,
    source: PlayerSkinSource,
    result: Result<crate::renderer::SkinData, String>,
}

enum HeadState {
    Pending,
    Ready {
        hat: Vec<u8>,
        base: Vec<u8>,
        /// The 64x64 skin, which a profiled player head item is drawn with.
        skin: Vec<u8>,
    },
    /// No skin to show: a static profile with no textures, or a lookup that
    /// failed. Cached so it isn't retried (and re-logged) every frame.
    // TODO: vanilla falls back to `DefaultPlayerSkin.get(uuid)`, which head
    // items draw (`HeadProfile::asset_skin`); chat head glyphs and entities
    // still draw wide Steve.
    Fallback,
}

struct HeadEntry {
    state: HeadState,
    last_used: Instant,
}

struct HeadResult {
    profile: HeadProfile,
    result: Result<crate::renderer::SkinData, String>,
}

/// A glyph the text drew: its tile (or an animated sprite's frames), and when
/// it was last drawn (`PlayerSkinRenderCache.CACHE_DURATION`).
struct InlineObjectEntry {
    content: InlineObjectContent,
    last_used: Instant,
}

enum InlineObjectContent {
    /// A stitched sprite's frames, and the one showing now.
    Sprite {
        frames: crate::ui::object_glyph::SpriteFrames,
        frame: usize,
    },
    /// A head: the profile whose skin it shows, and its layer.
    Head { profile: HeadProfile, hat: bool },
}

/// Vanilla `PlayerSkinRenderCache.CACHE_DURATION`.
const GLYPH_CACHE_DURATION: std::time::Duration = std::time::Duration::from_secs(5 * 60);
/// How long a frame spends on chunk packets before the rest wait for the
/// next.
const CHUNK_PACKET_BUDGET: std::time::Duration = std::time::Duration::from_millis(3);

/// Whether the rest of the queue may wait for the next frame: only once the
/// chunk budget is spent and while this frame owes no tick, so every packet
/// received before a tick is handled before it runs.
fn defer_packets(chunk_work: std::time::Duration, tick_accumulator: f32) -> bool {
    chunk_work >= CHUNK_PACKET_BUDGET && tick_accumulator < crate::app::TICK_RATE
}

/// Applies a server-driven block change: block-entity sync, prediction
/// absorption, the block + light write, and the remesh cascade. The block
/// entity syncs even when a pending prediction absorbs the update (the state
/// is applied later in `acknowledge`), so e.g. a chest placed where a break
/// was just predicted still gets its entry.
fn apply_server_block(
    game: &mut GameState,
    priority_remesh: &mut Vec<(crate::position::ChunkPos, i32)>,
    pos: crate::position::BlockPos,
    state: azalea_block::BlockState,
) {
    sync_block_entities(game, pos, state);
    if game.interaction.update_known_server_state(&pos, state) {
        return;
    }
    write_block(game, priority_remesh, pos, state);
}

/// Vanilla `Level.setBlock` from the client's own code, which no pending
/// prediction absorbs.
fn set_client_block(
    game: &mut GameState,
    priority_remesh: &mut Vec<(crate::position::ChunkPos, i32)>,
    pos: crate::position::BlockPos,
    state: azalea_block::BlockState,
) {
    sync_block_entities(game, pos, state);
    write_block(game, priority_remesh, pos, state);
}

fn sync_block_entities(
    game: &mut GameState,
    pos: crate::position::BlockPos,
    state: azalea_block::BlockState,
) {
    let block_state_data = game.block_registry.block_state_data(state);
    // Vanilla drops a moving block's entity once its cell holds anything else.
    if block_state_data.id() != "moving_piston" {
        game.block_entity_anim.pistons.remove(&pos);
    }
    crate::world::block_entity::sync_block_entity(
        &mut game.chunk_store.block_entities,
        pos,
        block_state_data,
    );
}

/// The block + light write and the remesh cascade.
fn write_block(
    game: &mut GameState,
    priority_remesh: &mut Vec<(crate::position::ChunkPos, i32)>,
    pos: crate::position::BlockPos,
    state: azalea_block::BlockState,
) {
    crate::world::light::set_block_and_light(
        &game.chunk_store,
        &mut game.light_engine,
        game.block_registry.block_state_table(),
        pos.x,
        pos.y,
        pos.z,
        state,
    );
    game.edited_columns
        .insert(crate::position::ChunkPos::containing(pos));
    dirty_sections_for_block(
        priority_remesh,
        pos.x,
        pos.y,
        pos.z,
        game.chunk_store.min_y(),
        game.chunk_store.section_count(),
    );
}

/// The client world as the piston code sees it. Destroyed blocks' effects
/// wait for the caller, which owns the audio and the particle registry.
struct ClientPistonLevel<'a> {
    game: &'a mut GameState,
    remesh: &'a mut Vec<(crate::position::ChunkPos, i32)>,
    destroyed: Vec<(crate::position::BlockPos, azalea_block::BlockState)>,
}

impl<'a> ClientPistonLevel<'a> {
    fn new(game: &'a mut GameState, remesh: &'a mut Vec<(crate::position::ChunkPos, i32)>) -> Self {
        Self {
            game,
            remesh,
            destroyed: Vec::new(),
        }
    }
}

impl crate::world::piston::PistonLevel for ClientPistonLevel<'_> {
    fn block_registry(&self) -> &crate::world::block::block_registry::BlockRegistry {
        &self.game.block_registry
    }

    fn block(&self, pos: crate::position::BlockPos) -> azalea_block::BlockState {
        self.game.chunk_store.get_block_state(pos.x, pos.y, pos.z)
    }

    fn set_block(&mut self, pos: crate::position::BlockPos, state: azalea_block::BlockState) {
        set_client_block(self.game, self.remesh, pos, state);
    }

    fn is_loaded(&self, pos: crate::position::BlockPos) -> bool {
        let chunk = crate::position::ChunkPos::containing(pos);
        self.game.chunk_store.get_chunk(&chunk).is_some()
    }

    fn min_y(&self) -> i32 {
        self.game.chunk_store.min_y()
    }

    fn max_y(&self) -> i32 {
        self.game.chunk_store.min_y() + self.game.chunk_store.height() as i32 - 1
    }

    fn pistons(&mut self) -> &mut crate::world::piston::MovingPistons {
        &mut self.game.block_entity_anim.pistons
    }

    fn destroy_effect(&mut self, pos: crate::position::BlockPos, state: azalea_block::BlockState) {
        self.destroyed.push((pos, state));
    }
}

/// Vanilla `PistonMovingBlockEntity.tick` for the client's moving blocks, at
/// the block-entity point of the game tick; their landings remesh at once.
pub(crate) fn tick_moving_pistons(game: &mut GameState) {
    let mut remesh = Vec::new();
    let mut level = ClientPistonLevel::new(game, &mut remesh);
    let tick = level.game.tick_count;
    crate::world::piston::tick(&mut level, |level, pos, piston, new_progress| {
        let game = &mut *level.game;
        // A passenger moves with its vehicle, which the server moves.
        if game.position_set && game.riding_vehicle_id.is_none() {
            // TODO: vanilla's client also moves other entities in the way;
            // the server's position updates do it here.
            let world = crate::physics::collision::CollisionWorld::with_block_entities(
                &game.chunk_store,
                game.block_registry.block_state_table(),
                &game.block_entity_anim,
            );
            movement::push_by_moving_piston(
                &mut game.player,
                world,
                game.block_registry.tags(),
                pos,
                piston,
                new_progress,
                tick,
            );
        }
    });
    enqueue_edits(game, &remesh);
}

/// Marks edited sections for remeshing as recompiles, ungated by visibility.
fn enqueue_edits(game: &mut GameState, sections: &[(crate::position::ChunkPos, i32)]) {
    for &(col, si) in sections {
        game.enqueue_section_edit(col, si);
    }
}

/// The texture of what `e` holds in its main hand, for its eating crumbs.
fn mouth_item_texture(e: &crate::entity::LivingEntity) -> Option<String> {
    let azalea_inventory::ItemStack::Present(held) =
        e.equipment(azalea_inventory::components::EquipmentSlot::Mainhand)?
    else {
        return None;
    };
    Some(format!(
        "item/{}",
        crate::player::inventory::item_resource_name(held.kind)
    ))
}

/// The client half of `Panda.addEatingParticles`, flagged by the entity tick
/// (the sound is the server's broadcast).
fn panda_eating_particles(game: &mut GameState, camera_pos: glam::DVec3) {
    for e in game.entity_store.living.values() {
        if e.entity_type != azalea_registry::builtin::EntityKind::Panda || !e.panda.eat_particles {
            continue;
        }
        let Some(texture) = mouth_item_texture(e) else {
            continue;
        };
        game.particle_store.add_panda_eating_particles(
            &texture,
            e.position.into(),
            glam::DVec3::from(e.eye_position(1.0)).y,
            e.look_dir.x_rot_deg(),
            e.look_dir.y_rot_deg(),
            e.body_y_rot_deg,
            camera_pos,
        );
    }
}

/// A sound following an entity, skipped while it's silent (vanilla
/// `EntityBoundSoundInstance.canPlaySound`); the playing instance's id.
#[allow(clippy::too_many_arguments)]
pub(super) fn play_entity_sound(
    audio: &mut crate::audio::AudioEngine,
    game: &mut GameState,
    entity_id: i32,
    sound: &crate::audio::SoundRef,
    category: u8,
    volume: f32,
    pitch: f32,
    seed: u64,
) -> Option<u64> {
    if game.silent_entities.contains(&entity_id) {
        return None;
    }
    let pos = (entity_id == game.player.entity_id)
        .then_some(game.player.position)
        .or_else(|| game.entity_positions.get(&entity_id).copied())?;
    audio.play_entity_sound(
        sound,
        category,
        crate::audio::EntitySoundTarget { id: entity_id, pos },
        volume,
        pitch,
        seed,
    )
}

/// Plays the sounds the player's moves queued this tick, its own and the
/// pistons' pushes alike, where it stands: vanilla `LocalPlayer.playSound`,
/// which plays them locally while the server sends them only to other
/// players.
pub(crate) fn play_movement_sounds(audio: &crate::audio::AudioEngine, game: &mut GameState) {
    let silent = game.silent_entities.contains(&game.player.entity_id);
    for queued in std::mem::take(&mut game.player.movement_sounds.pending) {
        if queued.unless_silent && silent {
            continue;
        }
        audio.play_world_sound(
            &crate::audio::SoundRef::event(queued.event),
            crate::audio::CATEGORY_PLAYERS,
            game.player.position,
            queued.volume,
            queued.pitch,
            fastrand::u64(..),
        );
    }
}

/// Vanilla `LevelEventHandler` case 2001: a broken block's sound and
/// particles.
fn block_break_effect(
    audio: &crate::audio::AudioEngine,
    game: &mut GameState,
    pos: crate::position::BlockPos,
    state: azalea_block::BlockState,
) {
    let block_state_data = game.block_registry.block_state_data(state);
    if !block_state_data.is_air() {
        crate::player::interaction::play_break_sound_for_id(
            audio,
            game.block_registry.block_state_table(),
            block_state_data.id(),
            pos,
        );
    }
    game.particle_store.add_destroy_block_effect(
        pos,
        state,
        game.block_registry.render_view(),
        &game.chunk_store,
        &game.biome_climate,
    );
}

/// The local player's box as a push target, gated on `LivingEntity.isPushable`
/// and `EntitySelector.NO_SPECTATORS`.
fn local_push_box(game: &GameState) -> Option<crate::physics::aabb::Aabb> {
    (game.player.health > 0.0
        && !crate::player::is_spectator(game.player.game_mode)
        && !movement::on_climbable(
            &game.player,
            &game.chunk_store,
            game.block_registry.block_state_table(),
            game.block_registry.tags(),
        ))
    .then(|| game.player.bounding_box())
}

/// Starts an entity's hurt animation, with a direction for the packets that
/// carry one. The local player lives outside the entity store.
fn hurt_entity(game: &mut GameState, id: i32, dir: Option<f32>) {
    if id != game.player.entity_id {
        game.entity_store.mark_hurt(id);
    } else if let Some(yaw) = dir {
        game.player.animate_hurt(yaw);
    } else {
        game.player.mark_hurt();
    }
}

/// The level's chunk, light and position updates, shared by the windowed
/// client and the headless chunk benchmark; the caller does any renderer work.
impl GameState {
    /// The distant mesher tints with the chunk mesher's colours, so it takes
    /// them again whenever the dispatcher or its biomes change.
    fn sync_distant_colors(&mut self) {
        self.distant
            .set_colors(self.mesh_dispatcher.distant_colors());
    }

    pub fn select_protocol(&mut self, protocol: i32) {
        let previous_slot = self.block_registry.block_state_table().table_slot();
        self.block_registry.select_protocol(protocol);
        if previous_slot != self.block_registry.block_state_table().table_slot() {
            self.mesh_dispatcher = self
                .mesh_dispatcher
                .recreate_for_version(self.block_registry.owned_render_view());
            self.sync_distant_colors();
            self.set_distant_cache_key();
            self.respawn_light(self.light_engine.has_sky());
        }
    }

    /// A fresh light engine for the chunk store's level, reading the current
    /// block table.
    fn respawn_light(&mut self, has_sky: bool) {
        self.light_engine = crate::world::light::LightWorker::spawn(
            self.chunk_store.height(),
            self.chunk_store.min_y(),
            has_sky,
            self.block_registry.block_state_table(),
        );
    }

    /// Distant terrain stores block states and biomes by id, so it keeps
    /// them apart per block table and biome registry.
    fn set_distant_cache_key(&mut self) {
        if let Some(biomes) = crate::net::connection::biome_registry(&self.registries) {
            self.distant.set_cache_key(
                self.block_registry.block_state_table().table_slot(),
                biomes.map.keys(),
            );
        }
    }

    /// Keeps `pos`'s column as distant terrain of the current dimension.
    pub fn keep_column(&self, pos: crate::position::ChunkPos) {
        self.distant
            .capture(&self.dimension, pos, &self.chunk_store);
    }

    /// Keeps the columns edited since the distant terrain took them.
    pub fn keep_edited_columns(&mut self) {
        for pos in std::mem::take(&mut self.edited_columns) {
            self.keep_column(pos);
        }
    }

    pub fn set_biome_climate(
        &mut self,
        colors: HashMap<u32, crate::renderer::chunk::mesher::BiomeClimate>,
    ) {
        tracing::info!("Received {} biome climate entries", colors.len());
        self.biome_climate = Arc::new(colors);
        self.mesh_dispatcher
            .set_biome_climate(Arc::clone(&self.biome_climate));
        self.sync_distant_colors();
    }

    /// Resets the level for a new dimension, with a mesh dispatcher for its
    /// shading. The caller clears the old meshes.
    pub fn enter_dimension(
        &mut self,
        dim: DimensionInfo,
        render_distance: u32,
        mesh_source: &impl MeshSource,
        resources: &dyn ResourceManager,
    ) {
        let DimensionInfo {
            height,
            min_y,
            has_skylight,
            cardinal_light,
            ambient_light,
            default_clock,
            flat,
            environment,
        } = dim;
        tracing::info!(
            "Dimension: height={height}, min_y={min_y}, skylight={has_skylight}, cardinal_light={cardinal_light:?}"
        );
        self.cardinal_light = cardinal_light;
        self.ambient_light = ambient_light;
        self.default_clock = default_clock;
        self.flat = flat;
        self.environment = *environment;
        // Vanilla `EnvironmentAttributeProbe.reset`: no biomes until the
        // next tick samples the new level's.
        self.biome_weights = Default::default();
        self.environment_probe = EnvironmentProbe::new(self.resolve_environment());
        // The old level's columns stay as its distant terrain; the name
        // still names it until `DimensionName`.
        self.keep_edited_columns();
        self.chunk_store = ChunkStore::new_with_dimension(render_distance, height, min_y);
        self.respawn_light(has_skylight);
        // The old level's block entities, moving blocks among them.
        self.block_entity_anim = Default::default();
        self.position_set = false;
        // Login/respawn recreate vanilla's LocalPlayer, resetting
        // the XP display sentinel; waypoints persist.
        self.xp_display_start_tick = i64::MIN;
        self.controlled_vehicle_id = None;
        self.riding_vehicle_id = None;
        self.player.jump_riding_ticks = 0;
        self.player.jump_riding_scale = 0.0;
        self.player.reset_hurt_state();
        // The meshes went with the old level, so their bookkeeping
        // has to go too, or the new level's camera section reads as
        // already meshed.
        self.forget_visibility(VisUpdate::Clear);
        self.compiled.clear();
        self.clear_visibility();
        self.mesh_dispatcher = mesh_source.create_mesh_dispatcher(
            self.block_registry.owned_render_view(),
            Arc::clone(&self.biome_climate),
            resources,
            cardinal_light,
        );
        self.sync_distant_colors();
        self.reset_mesh_columns();
    }

    pub fn load_chunk(
        &mut self,
        pos: crate::position::ChunkPos,
        data: &[u8],
        heightmaps: &[(crate::world::chunk::HeightmapKind, Box<[u64]>)],
        light: &crate::net::PacketLightData,
    ) {
        let biomes = crate::net::connection::biome_count(&self.registries);
        if let Err(e) = self.chunk_store.load_chunk(
            pos,
            data,
            heightmaps,
            biomes,
            self.block_registry.block_state_table(),
        ) {
            tracing::error!("Failed to load chunk [{}, {}]: {e}", pos.x, pos.z);
            return;
        }
        if let Some(chunk) = self.chunk_store.get_chunk(&pos) {
            self.light_engine.chunk_loaded((pos.x, pos.z), chunk);
        }
        self.walker.update(VisUpdate::Load(pos));
        // The column meshes once its queued light applies (vanilla
        // schedules the rebuild from enableChunkLight, not here).
        queue_light_apply(self, pos, light, true);
    }

    /// Vanilla `handleChunksBiomes`: the columns' new biomes, then every
    /// loaded column around them remeshes, since biome tints blend across
    /// column edges.
    pub fn replace_biomes(&mut self, chunks: &[(crate::position::ChunkPos, Box<[u8]>)]) {
        let biomes = crate::net::connection::biome_count(&self.registries);
        for (pos, data) in chunks {
            if let Err(e) = self.chunk_store.replace_biomes(*pos, data, biomes) {
                tracing::error!("Failed to replace biomes in [{}, {}]: {e}", pos.x, pos.z);
            }
        }
        for (pos, _) in chunks {
            for column in crate::world::chunk::column_neighborhood(*pos) {
                if self.chunk_store.get_chunk(&column).is_some() {
                    self.mark_column_dirty(column);
                }
            }
        }
    }

    pub fn apply_light_update(
        &mut self,
        pos: crate::position::ChunkPos,
        light: &crate::net::PacketLightData,
    ) {
        queue_light_apply(self, pos, light, false);
    }

    /// The caller removes the column's mesh.
    pub fn unload_chunk(&mut self, pos: crate::position::ChunkPos) {
        self.keep_column(pos);
        self.edited_columns.remove(&pos);
        self.chunk_store.unload_chunk(&pos);
        self.block_entity_anim.drop_chunk(pos.x, pos.z);
        self.light_engine.chunk_unloaded((pos.x, pos.z));
        self.forget_mesh_column(pos);
        self.compiled.remove(&pos);
        let sections = self.chunk_store.section_count();
        if let Some(mask) = self.in_view_masks.remove(&pos) {
            self.mesh_gate.remove(mask, sections as u32);
        }
        for si in 0..sections {
            self.section_vis_epoch.remove(&(pos, si));
        }
        self.walker.update(VisUpdate::Unload { pos, sections });
    }

    /// Vanilla `handleMovePlayer`. Returns whether the player moved, for the
    /// caller to reset the camera.
    pub fn apply_player_position(
        &mut self,
        connection: &ConnectionHandle,
        id: u32,
        change: &pomme_protocol::entity::PositionMoveRotation,
        relative: &pomme_protocol::entity::Relative,
    ) -> bool {
        let is_passenger = self.riding_vehicle_id.is_some();
        let correction =
            resolve_player_position_correction(&self.player, change, relative, is_passenger);
        if let Some(correction) = &correction {
            self.player.position = correction.position;
            self.player.prev_position = correction.prev_position;
            self.player.velocity = correction.velocity;
            self.player.look_dir = correction.look_dir;
            self.player.prev_look_dir = correction.prev_look_dir;

            let to_chunk_coord = |v: f64| (v.floor() as i32).div_euclid(16);
            self.chunk_store.set_center(crate::position::ChunkPos::new(
                to_chunk_coord(correction.position.x),
                to_chunk_coord(correction.position.z),
            ));

            if !self.position_set {
                self.position_set = true;
                tracing::info!(
                    "Player position set to ({:.1}, {:.1}, {:.1})",
                    correction.position.x,
                    correction.position.y,
                    correction.position.z
                );
            }
        }

        // Vanilla `handleMovePlayer` acks and echoes the pose even
        // when mounted (a 26.3 wire folds the two).
        connection.packet_tx.send(ServerboundGamePacket::AcceptTeleportation(
            azalea_protocol::packets::game::s_accept_teleportation::ServerboundAcceptTeleportation { id },
        ));
        connection
            .packet_tx
            .send(ServerboundGamePacket::MovePlayerPosRot(
            azalea_protocol::packets::game::s_move_player_pos_rot::ServerboundMovePlayerPosRot {
                pos: self.player.position.into(),
                look_direction: self.player.look_dir.into(),
                flags: Default::default(),
            },
        ));
        self.interaction.on_teleport();
        correction.is_some()
    }

    /// Vanilla `Options.setServerRenderDistance`, from the login and
    /// `SetChunkCacheRadius`; clamped to the chunk grid.
    pub fn set_server_view_distance(&mut self, distance: u32) {
        tracing::info!("Server view distance: {distance}");
        self.server_render_distance = distance.min(crate::world::chunk::MAX_VIEW_DISTANCE);
    }
}

/// Queues a column's packet light for the per-tick apply. Chunk loads enable
/// the column, standalone light updates are corrections.
fn queue_light_apply(
    game: &mut GameState,
    pos: crate::position::ChunkPos,
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
    out: &mut Vec<(crate::position::ChunkPos, i32)>,
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
                let col = crate::position::ChunkPos::new(bx.div_euclid(16), bz.div_euclid(16));
                let key = (col, si);
                if !out.contains(&key) {
                    out.push(key);
                }
            }
        }
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
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

    pub fn to_u8(self) -> u8 {
        match self {
            Self::Windowed => 0,
            Self::Borderless => 1,
            Self::Fullscreen => 2,
        }
    }

    pub fn from_u8(value: u8) -> Self {
        match value {
            1 => Self::Borderless,
            2 => Self::Fullscreen,
            _ => Self::Windowed,
        }
    }

    /// Shared by window creation and the runtime switch so the two can't drift.
    pub fn fullscreen_for(self, monitor: Option<MonitorHandle>) -> Option<Fullscreen> {
        match self {
            Self::Windowed => None,
            Self::Borderless => Some(Fullscreen::Borderless(None)),
            Self::Fullscreen => {
                let video_mode = monitor.and_then(|m| {
                    m.video_modes().max_by_key(|v| {
                        (v.refresh_rate_millihertz(), v.size().width, v.size().height)
                    })
                });
                Some(match video_mode {
                    Some(mode) => Fullscreen::Exclusive(mode),
                    None => Fullscreen::Borderless(None),
                })
            }
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

fn player_input_state(
    input: &InputState,
    analog_move: glam::Vec2,
    sprint_key_down: bool,
) -> PlayerInputState {
    PlayerInputState {
        forward: input.action_held(Action::Forward) || analog_move.y > STICK_MOVEMENT_THRESHOLD,
        backward: input.action_held(Action::Backward) || analog_move.y < -STICK_MOVEMENT_THRESHOLD,
        left: input.action_held(Action::StrafeLeft) || analog_move.x > STICK_MOVEMENT_THRESHOLD,
        right: input.action_held(Action::StrafeRight) || analog_move.x < -STICK_MOVEMENT_THRESHOLD,
        jump: input.performing_action(Action::Jump),
        shift: input.performing_action(Action::Sneak),
        sprint: sprint_key_down,
    }
}

/// `base + value` for a relative field, `value` for an absolute one.
fn resolve<T: Add<Output = T>>(base: T, is_relative: bool, value: T) -> T {
    if is_relative { base + value } else { value }
}

fn resolve_corrected_look(
    current: LookDirection,
    y_rot: f32,
    x_rot: f32,
    relative_y: bool,
    relative_x: bool,
) -> LookDirection {
    // Vanilla `PositionMoveRotation.calculateAbsolute` clamps pitch before
    // `Entity.setXRot` applies its own modulo and clamp.
    LookDirection::new(
        resolve(current.y_rot_deg(), relative_y, y_rot),
        resolve(current.x_rot_deg(), relative_x, x_rot).clamp(-90.0, 90.0),
    )
}

struct ResolvedPlayerCorrection {
    position: Position,
    prev_position: Position,
    velocity: Velocity,
    look_dir: LookDirection,
    prev_look_dir: LookDirection,
}

/// `ServerboundMoveVehiclePacket.fromEntity`.
fn move_vehicle_packet(vehicle: &crate::entity::LivingEntity) -> Vec<u8> {
    pomme_protocol::wire::encode_move_vehicle(
        *vehicle.position,
        vehicle.body_y_rot_deg,
        vehicle.look_dir.x_rot_deg(),
        vehicle.on_ground,
    )
}

fn resolve_player_position_correction(
    player: &LocalPlayer,
    change: &pomme_protocol::entity::PositionMoveRotation,
    relative: &pomme_protocol::entity::Relative,
    is_passenger: bool,
) -> Option<ResolvedPlayerCorrection> {
    if is_passenger {
        return None;
    }

    // Vanilla `setValuesFromPositionPacket` resolves the old pose through the
    // same `calculateAbsolute`.
    let resolve_position = |base: Position| {
        Position::new(
            resolve(base.x, relative.x, change.position.x),
            resolve(base.y, relative.y, change.position.y),
            resolve(base.z, relative.z, change.position.z),
        )
    };
    let resolve_look = |base| {
        resolve_corrected_look(
            base,
            change.y_rot,
            change.x_rot,
            relative.y_rot,
            relative.x_rot,
        )
    };
    let position = resolve_position(player.position);
    let prev_position = resolve_position(player.prev_position);
    let look_dir = resolve_look(player.look_dir);
    let prev_look_dir = resolve_look(player.prev_look_dir);

    let mut velocity = player.velocity;
    if relative.rotate_delta {
        let x_rot_delta = player.look_dir.x_rot_deg() - look_dir.x_rot_deg();
        let y_rot_delta = player.look_dir.y_rot_deg() - look_dir.y_rot_deg();
        velocity = velocity
            .x_rot(x_rot_delta.to_radians() as f64)
            .y_rot(y_rot_delta.to_radians() as f64);
    }
    velocity = Velocity::new(
        resolve(velocity.x, relative.delta_x, change.delta.x),
        resolve(velocity.y, relative.delta_y, change.delta.y),
        resolve(velocity.z, relative.delta_z, change.delta.z),
    );

    Some(ResolvedPlayerCorrection {
        position,
        prev_position,
        velocity,
        look_dir,
        prev_look_dir,
    })
}

fn serverbound_player_input(state: &PlayerInputState) -> ServerboundPlayerInput {
    ServerboundPlayerInput {
        forward: state.forward,
        backward: state.backward,
        left: state.left,
        right: state.right,
        jump: state.jump,
        shift: state.shift,
        sprint: state.sprint,
    }
}

// GLFW's key-repeat shape, so a held DPad steps like a held arrow key
// instead of once per rendered frame.
const MENU_REPEAT_DELAY: f32 = 0.4;
const MENU_REPEAT_INTERVAL: f32 = 1.0 / 30.0;

/// Edge-plus-repeat stepping for a held direction. The gamepad exposes level
/// state only, which would step once per frame and tie the rate to the
/// framerate.
#[derive(Default)]
struct RepeatStepper {
    dir: i32,
    countdown: f32,
}

impl RepeatStepper {
    fn step(&mut self, dir: i32, dt: f32) -> i32 {
        if dir != self.dir {
            self.dir = dir;
            self.countdown = MENU_REPEAT_DELAY;
            // The press edge steps immediately; a release returns 0.
            return dir;
        }
        if dir == 0 {
            return 0;
        }
        self.countdown -= dt;
        let mut steps = 0;
        // `dt` is clamped against stalls by the app loop, so this is bounded.
        while self.countdown <= 0.0 {
            self.countdown += MENU_REPEAT_INTERVAL;
            steps += 1;
        }
        steps * dir
    }
}

pub struct AppCore {
    pub user: UserData,
    pub presence: Option<DiscordPresence>,
    pub display_mode: DisplayMode,
    display_mode_override: Option<DisplayMode>,
    /// The windowed size the window opens at (`--width`/`--height`).
    pub window_size: winit::dpi::LogicalSize<u32>,
    pub input: InputState,
    pub menu: MainMenu,
    pub tokio_rt: Arc<tokio::runtime::Runtime>,
    pub data_dirs: DataDirs,
    pub version: String,
    builtin_resources: BuiltinRepositorySource,
    folder_resources: FolderRepositorySource,
    registered_resources: RegisteredRepositorySource,
    resource_repository: PackRepository,
    resources: ReloadableResourceManager,
    // The active view can advance before GPU refresh fails. Cache eligibility
    // follows this view, not the last completed consumer refresh.
    active_resource_ids: Vec<PackId>,
    active_custom_resource_ids: Vec<PackId>,
    // Unlike the manager's active IDs, these advance only after asset refresh succeeds.
    completed_custom_resource_ids: Vec<PackId>,
    completed_resource_packs: Vec<crate::resources::packs::Pack>,
    completed_server_resource_ids: HashSet<PackId>,
    resource_completion_revision: u64,
    asset_reload_revision: u64,
    server_resource_ids: HashMap<uuid::Uuid, PackId>,
    pub(crate) block_definitions: Rc<BlockDefinitions>,
    pub pending_pack_download: Option<PendingPackDownload>,
    pub audio: crate::audio::AudioEngine,
    pub tick_accumulator: f32,
    pub time_tick_accumulator: f32,
    /// Ticks the loading screen handed to the game phase: the join tick, which
    /// sent `player_loaded`, and the rest of its frame's.
    pub carried_ticks: u32,
    menu_dpad: RepeatStepper,
    /// When the window lost OS focus, for pause-on-lost-focus (vanilla
    /// `pauseIfInactive`); `None` while focused.
    pub unfocused_since: Option<Instant>,
    /// A quick play held back until the required sign-in is done.
    pub deferred_quick_play: Option<crate::app::QuickPlay>,
    /// Vanilla `options.pauseOnLostFocus`.
    pub pause_on_lost_focus: bool,
    /// A test-harness window: it opens without taking focus, and grabbing
    /// leaves the OS cursor alone, so tests run while the desktop is in use.
    pub test_window: bool,
    /// Vanilla `MouseHandler.mouseGrabbed`.
    mouse_grabbed: bool,
    /// The OS grab may no longer match `mouse_grabbed` (the window manager
    /// can drop a lock on focus change), so the next apply re-issues it.
    os_grab_stale: bool,
    player_skin_tx: crossbeam_channel::Sender<PlayerSkinResult>,
    player_skin_rx: crossbeam_channel::Receiver<PlayerSkinResult>,
    requested_player_skins: HashMap<uuid::Uuid, PlayerSkinSource>,
    /// 8x8 RGBA faces of fetched player skins, for the spectator menu's
    /// face atlas (the GPU-side skins keep no CPU pixels).
    player_faces: HashMap<uuid::Uuid, Vec<u8>>,
    player_faces_dirty: bool,
    /// Inline object glyphs (sprites and heads) the text drew recently,
    /// keyed as the text renderer looks them up.
    inline_objects: HashMap<String, InlineObjectEntry>,
    /// Head skins by profile identity, shared by a profile's hat and base
    /// glyphs and kept apart from the entity skin cache.
    player_heads: HashMap<HeadProfile, HeadEntry>,
    /// A head's skin arrived or left, so the renderer's copies need syncing.
    head_skins_dirty: bool,
    /// The skins from the game's assets heads draw (`HeadProfile::asset_skin`),
    /// by texture; `None` when the asset is missing or isn't a 64x64 skin.
    asset_head_skins: HashMap<String, Option<Vec<u8>>>,
    asset_head_skins_stale: bool,
    head_tx: crossbeam_channel::Sender<HeadResult>,
    head_rx: crossbeam_channel::Receiver<HeadResult>,
    /// A packed glyph's pixels changed (a head arrived), so the atlas needs
    /// rebuilding even though its key set is the same.
    inline_atlas_dirty: bool,
    /// Whether player faces are in the atlas as packed.
    player_faces_packed: bool,
    /// The inline-object keys currently packed.
    game_dynamic_atlas_keys: HashSet<String>,
}

/// Folds the credits roll's keys into a `CREDITS_KEY_*` mask. Both control
/// keys count separately in vanilla's `speedupModifiers`, so this can't go
/// through `InputState::ctrl_held`, which folds them into one winit modifier
/// bit.
fn credits_key_mask(mut is_set: impl FnMut(KeyCode) -> bool) -> u8 {
    [
        (KeyCode::ArrowUp, CREDITS_KEY_UP),
        (KeyCode::Space, CREDITS_KEY_SPACE),
        (KeyCode::ControlLeft, CREDITS_KEY_CTRL_L),
        (KeyCode::ControlRight, CREDITS_KEY_CTRL_R),
    ]
    .into_iter()
    .filter(|&(code, _)| is_set(code))
    .map(|(_, bit)| bit)
    .sum()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CursorOp {
    Keep,
    Grab,
    Release { center: bool },
}

/// One `MouseHandler.grabMouse`/`releaseMouse` call as a pure transition over
/// `(mouse_grabbed, os_grab_stale)`. Grabbing needs an active window and is a
/// no-op once grabbed; releasing is a no-op once released and only centers on
/// the grabbed -> released edge. A stale OS grab re-issues either call.
fn cursor_step(grabbed: bool, stale: bool, want: bool, focused: bool) -> (CursorOp, bool, bool) {
    if want {
        if !focused || (grabbed && !stale) {
            return (CursorOp::Keep, grabbed, stale);
        }
        (CursorOp::Grab, true, false)
    } else {
        if !grabbed && !stale {
            return (CursorOp::Keep, grabbed, stale);
        }
        (CursorOp::Release { center: grabbed }, false, false)
    }
}

/// `LocalPlayerResolver`: a `Dynamic` profile resolves through the tab list
/// first (by name, ignoring case), and only then through the Mojang API. A
/// `Static` profile resolves to itself.
fn resolve_head_profile(profile: HeadProfile, tab_list: &TabList) -> HeadProfile {
    if profile.is_static() {
        return profile;
    }
    let found = match (&profile.id, &profile.name) {
        (Some(id), _) => tab_list.players.get(id),
        (None, Some(name)) => tab_list
            .players
            .values()
            .find(|player| player.name.eq_ignore_ascii_case(name)),
        _ => None,
    };
    match found {
        Some(player) => HeadProfile {
            id: Some(player.uuid),
            name: Some(player.name.clone()),
            textures: player.textures.clone(),
            patch_texture: profile.patch_texture.clone(),
        },
        None => profile,
    }
}

impl AppCore {
    pub fn new(
        version: String,
        data_dirs: DataDirs,
        tokio_rt: Arc<tokio::runtime::Runtime>,
        presence: Option<DiscordPresence>,
        user: UserData,
        window_size: winit::dpi::LogicalSize<u32>,
    ) -> Self {
        let pack_type = PackType::ClientResources;
        let builtin_resources = BuiltinRepositorySource::new(None);
        let folder_resources = FolderRepositorySource::new(Vec::new(), None, pack_type, None);
        let registered_resources = RegisteredRepositorySource::new(pack_type, None);
        let resource_repository = PackRepository::new(vec![
            Box::new(builtin_resources.clone()),
            Box::new(folder_resources.clone()),
            Box::new(registered_resources.clone()),
        ]);
        let resources = ReloadableResourceManager::new(pack_type);

        let mut input = InputState::new();
        let menu = MainMenu::new(
            &data_dirs.game_dir,
            data_dirs.bedrock_assets_dir.clone(),
            Arc::clone(&tokio_rt),
            user.username.clone(),
            version.clone(),
            user.access_token.clone(),
            &mut input.key_bindings,
        );

        let display_mode = menu.display_mode;

        let audio = crate::audio::AudioEngine::new(&resources, menu.category_volumes());
        let (player_skin_tx, player_skin_rx) = crossbeam_channel::unbounded();
        let (head_tx, head_rx) = crossbeam_channel::unbounded();

        let mut core = Self {
            user,
            presence,
            display_mode,
            display_mode_override: None,
            window_size,
            input,
            menu,
            tokio_rt,
            data_dirs,
            version,
            builtin_resources,
            folder_resources,
            registered_resources,
            active_resource_ids: resource_repository.enabled_ids().to_vec(),
            active_custom_resource_ids: Vec::new(),
            completed_custom_resource_ids: Vec::new(),
            completed_resource_packs: Vec::new(),
            completed_server_resource_ids: HashSet::new(),
            resource_completion_revision: 0,
            asset_reload_revision: 0,
            server_resource_ids: HashMap::new(),
            resource_repository,
            resources,
            block_definitions: Rc::new(BlockDefinitions::new()),
            pending_pack_download: None,
            audio,
            tick_accumulator: 0.0,
            time_tick_accumulator: 0.0,
            carried_ticks: 0,
            menu_dpad: RepeatStepper::default(),
            unfocused_since: None,
            deferred_quick_play: None,
            pause_on_lost_focus: true,
            test_window: false,
            mouse_grabbed: false,
            os_grab_stale: false,
            player_skin_tx,
            player_skin_rx,
            requested_player_skins: HashMap::new(),
            player_faces: HashMap::new(),
            player_faces_dirty: false,
            inline_objects: HashMap::new(),
            player_heads: HashMap::new(),
            head_skins_dirty: false,
            asset_head_skins: HashMap::new(),
            asset_head_skins_stale: false,
            head_tx,
            head_rx,
            inline_atlas_dirty: false,
            player_faces_packed: false,
            game_dynamic_atlas_keys: HashSet::new(),
        };
        core.configure_resources();
        core
    }

    fn configure_resources(&mut self) {
        let policy_path = self.data_dirs.game_dir.join("allowed_symlinks.txt");
        let policy = SymlinkPolicy::load(&policy_path);
        self.folder_resources.set_symlink_policy(policy.clone());
        self.registered_resources.set_symlink_policy(policy);

        let root = self.data_dirs.game_dir.join("resourcepacks");
        if !root.is_dir()
            && let Err(error) = std::fs::create_dir_all(&root)
        {
            tracing::warn!(path = %root.display(), %error, "Cannot create resource pack directory");
        }
        let cache = root.join(".server_cache");
        self.folder_resources.set_paths(vec![root], Some(cache));
        self.set_resource_theme(self.menu.theme());
        self.set_resource_installation();
        self.reload_resources();
        self.complete_resource_selection();
        self.menu.pick_splash();
    }

    /// Points the vanilla pack at the version's install, once it's complete.
    fn set_resource_installation(&self) {
        let installation = self
            .data_dirs
            .is_installed(&self.version)
            .then(|| self.data_dirs.installation_location(&self.version));
        self.builtin_resources.set_installation(installation);
        let target = self.builtin_resources.target_format();
        self.folder_resources.set_target_format(target);
        self.registered_resources.set_target_format(target);
    }

    fn set_resource_theme(&self, theme: PanoramaTheme) {
        self.builtin_resources.set_bedrock_ui(
            self.data_dirs.bedrock_ui_dir(),
            crate::renderer::pipelines::menu_overlay::bedrock_ui_aliases(),
        );
        self.builtin_resources.set_theme(match theme {
            PanoramaTheme::Java => None,
            PanoramaTheme::Pomme => Some(ThemeSpecification::Pomme {
                root: self.data_dirs.pomme_assets_dir.clone(),
            }),
            PanoramaTheme::Bedrock => Some(ThemeSpecification::Bedrock {
                root: pomme_install::bedrock::dir(&self.data_dirs.bedrock_assets_dir)
                    .join("resource_pack"),
            }),
        });
    }

    pub(crate) fn resources(&self) -> &dyn crate::resources::ResourceManager {
        &self.resources
    }

    pub(crate) fn update_menu_music(&mut self, dt: f32) {
        self.audio.update_menu_music(dt);
    }

    pub(crate) fn reload_resource_theme(&mut self, theme: PanoramaTheme, renderer: &mut Renderer) {
        self.set_resource_theme(theme);
        self.reload_resources();
        renderer.reload_theme_assets(&self.resources);
        self.complete_resource_selection();
    }

    /// Activates the selection with nothing to prepare.
    fn reload_resources(&mut self) {
        self.activate_resource_selection(None, |_| Ok(()))
            .expect("activation with nothing to prepare can't fail");
    }

    /// Catalog/source changes are not rolled back. Manager preparation retains
    /// its old view on failure.
    fn activate_resource_selection<T>(
        &mut self,
        toggle: Option<(PackId, bool)>,
        prepare: impl FnOnce(&dyn crate::resources::ResourceManager) -> std::io::Result<T>,
    ) -> std::io::Result<T> {
        self.resource_repository.reload();
        self.resource_repository
            .set_enabled(&self.active_resource_ids);
        if let Some((id, enable)) = toggle {
            if enable {
                self.resource_repository.select_pack(&id);
            } else {
                self.resource_repository.disable_pack(&id);
            }
        }
        let opened = self.resource_repository.open_all_enabled();
        let menu = &mut self.menu;
        let audio = &mut self.audio;
        let prepared = self
            .resources
            .reload_with_preparation(opened, prepare, |resources| {
                crate::lang::load(resources);
                menu.load_splashes(resources);
                audio.reload_assets(resources);
            })?;
        self.asset_head_skins.clear();
        self.asset_head_skins_stale = true;
        self.head_skins_dirty = true;
        self.active_resource_ids = self.resource_repository.enabled_ids().to_vec();
        // Server packs are required like vanilla's, but still not the stock
        // assets the block cache was built from.
        let server_ids = &self.server_resource_ids;
        self.active_custom_resource_ids = self
            .resource_repository
            .enabled_packs()
            .filter(|pack| {
                !pack.enablement_config().required || server_ids.values().any(|id| id == pack.id())
            })
            .map(|pack| pack.id().clone())
            .collect();
        Ok(prepared)
    }

    pub(crate) fn resource_catalog(&self) -> &PackRepository {
        &self.resource_repository
    }

    pub(crate) fn completed_resource_packs(&self) -> &[crate::resources::packs::Pack] {
        &self.completed_resource_packs
    }

    pub(crate) fn completed_resource_ids(&self) -> &[PackId] {
        &self.completed_custom_resource_ids
    }

    pub(crate) fn is_server_resource_pack(&self, id: &PackId) -> bool {
        self.server_resource_ids.values().any(|server| server == id)
    }

    pub(crate) fn is_completed_server_resource_pack(&self, id: &PackId) -> bool {
        self.completed_server_resource_ids.contains(id)
    }

    pub(crate) fn resource_completion_revision(&self) -> u64 {
        self.resource_completion_revision
    }

    pub(crate) fn asset_reload_revision(&self) -> u64 {
        self.asset_reload_revision
    }

    pub(crate) fn rescan_resource_packs(&mut self) {
        self.resource_repository.reload();
    }

    fn complete_resource_selection(&mut self) {
        self.completed_custom_resource_ids
            .clone_from(&self.active_custom_resource_ids);
        // Network draining can complete one reload and then fail another before
        // consumers observe it. Keep the successful catalog and classification.
        self.completed_resource_packs = self
            .resource_repository
            .available_packs()
            .cloned()
            .collect();
        self.completed_server_resource_ids = self.server_resource_ids.values().cloned().collect();
        self.resource_completion_revision = self.resource_completion_revision.wrapping_add(1);
    }

    pub(crate) fn reload_resource_selection(
        &mut self,
        toggle: Option<(PackId, bool)>,
        renderer: &mut Renderer,
        game: Option<&mut GameState>,
    ) -> std::io::Result<()> {
        let prepared = self.activate_resource_selection(toggle, Renderer::prepare_resources)?;
        match game {
            Some(game) => self.reload_live_resource_assets(game, renderer, prepared),
            None => self.reload_pack_assets(renderer, prepared),
        }
        Ok(())
    }

    pub fn build_menu_input(&mut self, dt: f32) -> MenuInput {
        if self.input.binding_capture_active() {
            self.menu_dpad.step(0, dt);
            return MenuInput {
                cursor: self.input.cursor_pos(),
                scroll_delta: self.input.consume_menu_scroll(),
                dt,
                binding_capture: true,
                binding_presses: self.input.take_binding_presses(),
                ..MenuInput::default()
            };
        }
        let credits_keys_down = credits_key_mask(|code| self.input.key_pressed(code));
        let credits_keys_pressed = credits_key_mask(|code| self.input.key_just_pressed(code));
        let dpad_dir = i32::from(self.input.gamepad_button_down(gilrs::Button::DPadRight))
            - i32::from(self.input.gamepad_button_down(gilrs::Button::DPadLeft));
        let gamepad_steps = self.menu_dpad.step(dpad_dir, dt);
        MenuInput {
            cursor: self.input.cursor_pos(),
            clicked: self.input.left_just_pressed(),
            mouse_held: self.input.left_held(),
            events: self.input.drain_text_events(),
            shift: self.input.shift_held(),
            enter: self.input.enter_pressed(),
            escape: self.input.escape_pressed(),
            tab: self.input.tab_pressed(),
            f5: self.input.f5_pressed(),
            gamepad_steps,
            scroll_delta: self.input.consume_menu_scroll(),
            dt,
            credits_keys_down,
            credits_keys_pressed,
            ..MenuInput::default()
        }
    }

    /// Uses `mode` for the window and every sync after, whatever the saved
    /// one is.
    pub fn override_display_mode(&mut self, mode: DisplayMode) {
        self.display_mode = mode;
        self.display_mode_override = Some(mode);
    }

    /// The fullscreen key: cycles and saves the display mode, unless an
    /// override holds it.
    pub fn toggle_display_mode(&mut self, window: &Window) {
        if self.display_mode_override.is_some() {
            return;
        }
        let display_mode = self.display_mode.cycle();
        self.menu
            .set_display_mode(display_mode, &self.input.key_bindings);
        self.sync_display_mode(window);
    }

    pub fn sync_display_mode(&mut self, window: &Window) {
        let wanted = self.display_mode_override.unwrap_or(self.menu.display_mode);
        if wanted != self.display_mode {
            self.display_mode = wanted;
            self.apply_display_mode(window);
        }
    }

    pub fn apply_display_mode(&mut self, window: &Window) {
        window.set_fullscreen(self.display_mode.fullscreen_for(window.current_monitor()));
        if self.display_mode == DisplayMode::Windowed {
            window.set_decorations(true);
        }
    }

    pub fn invalidate_cursor_grab_state(&mut self) {
        self.os_grab_stale = true;
    }

    /// GLFW, under vanilla, lets a disabled cursor go while another window
    /// has focus; the refocus grab takes it back. winit doesn't, so the Start
    /// menu or an alt-tabbed window would find the cursor still hidden and
    /// clipped to this one.
    pub fn suspend_cursor_grab(&self, window: &Window) {
        if self.mouse_grabbed {
            self.free_os_cursor(window);
        }
    }

    pub fn apply_cursor_grab(&mut self, window: &Window, game: Option<&mut GameState>) {
        let captured =
            game.is_some_and(|g| g.input_live() && !g.dead && self.input.is_cursor_captured());
        if captured {
            self.grab_cursor(window);
        } else {
            self.release_cursor(window);
        }
    }

    /// Vanilla `MouseHandler.grabMouse`.
    fn grab_cursor(&mut self, window: &Window) {
        let op = self.step_cursor(true);
        if op == CursorOp::Grab {
            // Vanilla centers on grab too; warp before locking, which
            // freezes the position on some platforms.
            self.center_cursor(window);
            self.lock_os_cursor(window);
        }
    }

    fn step_cursor(&mut self, want: bool) -> CursorOp {
        let (op, grabbed, stale) = cursor_step(
            self.mouse_grabbed,
            self.os_grab_stale,
            want,
            self.unfocused_since.is_none(),
        );
        self.mouse_grabbed = grabbed;
        self.os_grab_stale = stale;
        op
    }

    /// The chunk radius to ask a server for, from the video settings.
    pub const fn view_distance(&self) -> u8 {
        self.menu.render_distance as u8
    }

    /// Every return from a world or server, before the title screen shows.
    /// Vanilla builds a fresh `TitleScreen` here, which rolls a new splash.
    pub fn return_to_menu(&mut self, gfx: &mut Gfx) {
        self.menu.restore_bench_render_distance();
        self.clear_server_resource_packs(&mut gfx.renderer);
        // Leaving the world still retires it if resource preparation failed.
        gfx.renderer.clear_chunk_meshes();
        gfx.renderer.reset_distant();
        if let Some(p) = &mut self.presence {
            p.set_in_menu(&self.version);
        }
        self.apply_cursor_grab(&gfx.window, None);
        self.menu.splash = None;
        self.menu.pick_splash();
    }

    /// Releases the cursor, warping it to the window center only when it was
    /// grabbed, like vanilla `MouseHandler.releaseMouse` (a screen opened from
    /// gameplay starts with a centered cursor).
    fn release_cursor(&mut self, window: &Window) {
        if let CursorOp::Release { center } = self.step_cursor(false) {
            self.free_os_cursor(window);
            if center {
                self.center_cursor(window);
            }
        }
    }

    fn lock_os_cursor(&self, window: &Window) {
        if !self.test_window {
            let _ = window
                .set_cursor_grab(CursorGrabMode::Locked)
                .or_else(|_| window.set_cursor_grab(CursorGrabMode::Confined));
            window.set_cursor_visible(false);
        }
    }

    fn free_os_cursor(&self, window: &Window) {
        if !self.test_window {
            let _ = window.set_cursor_grab(CursorGrabMode::None);
            window.set_cursor_visible(true);
        }
    }

    /// Never warps the OS pointer while another window has focus, or in a
    /// test window.
    fn center_cursor(&mut self, window: &Window) {
        let size = window.inner_size();
        let (x, y) = (size.width as f32 / 2.0, size.height as f32 / 2.0);
        if self.unfocused_since.is_none() && !self.test_window {
            let _ = window.set_cursor_position(winit::dpi::PhysicalPosition::new(x, y));
        }
        self.input.on_cursor_moved(x, y);
    }

    /// `Gui.setScreen` with a new screen: whatever is up goes, running its
    /// `removed()`, so an options screen saves and chat keeps its draft.
    fn replace_screen(&mut self, connection: &ConnectionHandle, game: &mut GameState) {
        game.close_sign_editor(&connection.packet_tx);
        game.paused = false;
        self.menu
            .screen_removed(game.options_from_game, &self.input.key_bindings);
        game.options_from_game = false;
        game.options_close_to_game = false;
        game.close_creative_inventory();
        game.chat
            .close(crate::ui::chat::ChatExitReason::Interrupted);
        game.game_mode_switcher = None;
        game.server_dialog = None;
    }

    pub(crate) fn open_death_screen(
        &mut self,
        connection: &ConnectionHandle,
        window: &Window,
        game: &mut GameState,
        message: Option<String>,
    ) {
        game.interaction
            .stop_destroying_for_screen(&connection.packet_tx);
        self.replace_screen(connection, game);
        game.close_menu();

        if let Some(message) = message {
            game.death_message = message;
        }
        game.reset_death_screen();
        game.death_screen_open = true;
        self.release_cursor(window);
    }

    pub fn send_respawn(&mut self, connection: &ConnectionHandle, game: &mut GameState) {
        if game.respawn_sent {
            return;
        }
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
        connection.packet_tx.send_chat(msg);
    }

    /// Queues a tab-list player's entity skin: the textures the server sent,
    /// else a session-server lookup by id.
    fn queue_player_skin(&mut self, uuid: uuid::Uuid, textures: Option<String>) {
        let source = textures
            .map(PlayerSkinSource::Textures)
            .unwrap_or(PlayerSkinSource::Uuid);
        if self.requested_player_skins.get(&uuid) == Some(&source) {
            return;
        }
        self.requested_player_skins.insert(uuid, source.clone());

        // Name-derived (v3) UUIDs from offline-mode servers have no Mojang
        // profile to fetch; keep the default skin.
        if matches!(source, PlayerSkinSource::Uuid) && uuid.get_version_num() == 3 {
            return;
        }

        let tx = self.player_skin_tx.clone();
        self.tokio_rt.spawn(async move {
            let result = fetch_skin(&source, Some(uuid)).await;
            let _ = tx.send(PlayerSkinResult {
                uuid,
                source,
                result,
            });
        });
    }

    fn drain_player_skin_results(&mut self, renderer: &mut Renderer) {
        while let Ok(skin) = self.player_skin_rx.try_recv() {
            if self.requested_player_skins.get(&skin.uuid) != Some(&skin.source) {
                continue;
            }
            match skin.result {
                Ok(data) => {
                    if let Some(face) = crate::renderer::pipelines::menu_overlay::extract_face_8x8(
                        &data.pixels,
                        data.width,
                        data.height,
                    ) {
                        self.player_faces.insert(skin.uuid, face);
                        self.player_faces_dirty = true;
                    }
                    renderer.update_player_entity_skin(&skin.uuid, &data);
                }
                Err(e) => {
                    tracing::warn!("Failed to load entity player skin for {}: {e}", skin.uuid)
                }
            }
        }
    }

    /// Loads what the text drew last frame into the shared glyph atlas and
    /// rebuilds it when its contents change. Player faces (spectator menu,
    /// tab list) share the same texture binding, so they are packed alongside
    /// while `faces_shown`.
    pub fn sync_game_dynamic_atlas(
        &mut self,
        game: &GameState,
        renderer: &mut Renderer,
        faces_shown: bool,
    ) {
        let now = Instant::now();
        self.drain_head_results();

        // Objects drawn last frame are the ones to keep loaded.
        let drawn: Vec<(String, InlineObject)> = renderer.drain_drawn_inline_objects().collect();
        for (key, object) in drawn {
            match self.inline_objects.get_mut(&key) {
                Some(entry) => entry.last_used = now,
                None => {
                    let content = self.load_inline_object(&object, game);
                    self.inline_objects.insert(
                        key,
                        InlineObjectEntry {
                            content,
                            last_used: now,
                        },
                    );
                    self.inline_atlas_dirty = true;
                }
            }
        }

        // `expireAfterAccess`: a glyph the text stopped drawing is dropped
        // once it has been unused for the cache duration.
        self.inline_objects
            .retain(|_, entry| now.duration_since(entry.last_used) < GLYPH_CACHE_DURATION);
        let heads = self.player_heads.len();
        self.player_heads
            .retain(|_, entry| now.duration_since(entry.last_used) < GLYPH_CACHE_DURATION);
        self.head_skins_dirty |= self.player_heads.len() != heads;
        if std::mem::take(&mut self.head_skins_dirty) {
            for profile in self.player_heads.keys() {
                let texture = profile.asset_skin();
                if !self.asset_head_skins.contains_key(texture.as_ref()) {
                    let skin = crate::assets::AssetId::parse(&texture)
                        .resource_id("textures", ".png")
                        .ok()
                        .and_then(|id| crate::renderer::util::load_asset_png(&self.resources, &id))
                        .filter(|&(_, width, height)| (width, height) == (64, 64))
                        .map(|(pixels, ..)| pixels);
                    self.asset_head_skins.insert(texture.into_owned(), skin);
                }
            }
            // Asset skins stay until disconnect or resource-view replacement;
            // downloaded profile skins survive resource reloads.
            let own = self
                .player_heads
                .iter()
                .filter_map(|(profile, entry)| match &entry.state {
                    HeadState::Ready { skin, .. } if profile.patch_texture.is_none() => {
                        Some((profile.skin_key(), skin.as_slice()))
                    }
                    _ => None,
                });
            let mut skins: Vec<(String, &[u8])> = own.collect();
            if std::mem::take(&mut self.asset_head_skins_stale) {
                renderer
                    .sync_head_skins(skins.iter().map(|(key, skin)| (key.as_str(), *skin)), true);
            }
            let assets = self.asset_head_skins.iter().filter_map(|(texture, skin)| {
                Some((HeadProfile::asset_skin_key(texture), skin.as_deref()?))
            });
            skins.extend(assets);
            renderer.sync_head_skins(skins.iter().map(|(key, skin)| (key.as_str(), *skin)), false);
        }
        for entry in self.inline_objects.values() {
            if let InlineObjectContent::Head { profile, .. } = &entry.content
                && let Some(head) = self.player_heads.get_mut(profile)
            {
                head.last_used = now;
            }
        }

        // A head with no skin yet draws the default head sprite, so it packs
        // nothing; the arrival sets the dirty flag.
        let head_ready = |profile: &HeadProfile| {
            matches!(
                self.player_heads.get(profile),
                Some(HeadEntry {
                    state: HeadState::Ready { .. },
                    ..
                })
            )
        };
        let packable = |entry: &InlineObjectEntry| match &entry.content {
            InlineObjectContent::Sprite { .. } => true,
            InlineObjectContent::Head { profile, .. } => head_ready(profile),
        };
        // The packed set is compared by its keys, without rebuilding the
        // tiles every frame.
        let wanted = self.inline_objects.values().filter(|e| packable(e)).count();
        let rebuild = self.inline_atlas_dirty
            || wanted != self.game_dynamic_atlas_keys.len()
            || self
                .inline_objects
                .iter()
                .any(|(key, entry)| packable(entry) && !self.game_dynamic_atlas_keys.contains(key))
            || faces_shown != self.player_faces_packed
            || (faces_shown && self.player_faces_dirty);
        if rebuild {
            let mut entries = Vec::<(String, Vec<u8>, u32)>::new();
            if faces_shown {
                entries.extend(
                    self.player_faces
                        .iter()
                        .map(|(uuid, face)| (uuid.to_string(), face.clone(), 8)),
                );
            }
            self.game_dynamic_atlas_keys.clear();
            for (key, entry) in &self.inline_objects {
                match &entry.content {
                    InlineObjectContent::Sprite { frames, .. } => {
                        // Every frame of an animation is packed; the base key
                        // points at the one showing.
                        if frames.animated() {
                            for frame in 0..frames.frame_count() {
                                entries.push((
                                    format!("{key}#{frame}"),
                                    frames.pixels(frame).to_vec(),
                                    frames.size,
                                ));
                            }
                        }
                        entries.push((key.clone(), frames.pixels(0).to_vec(), frames.size));
                        self.game_dynamic_atlas_keys.insert(key.clone());
                    }
                    InlineObjectContent::Head { profile, hat } => {
                        if let Some(HeadEntry {
                            state:
                                HeadState::Ready {
                                    hat: hat_face,
                                    base,
                                    ..
                                },
                            ..
                        }) = self.player_heads.get(profile)
                        {
                            let face = if *hat { hat_face } else { base };
                            entries.push((key.clone(), face.clone(), 8));
                            self.game_dynamic_atlas_keys.insert(key.clone());
                        }
                    }
                }
            }
            if !entries.is_empty() {
                renderer.update_face_atlas(&entries);
            }
            self.inline_atlas_dirty = false;
            self.player_faces_packed = faces_shown;
            if faces_shown {
                self.player_faces_dirty = false;
            }
        }

        // An animated sprite keeps every frame in the atlas; the key the text
        // draws with points at the one showing this tick.
        for (key, entry) in &mut self.inline_objects {
            if let InlineObjectContent::Sprite { frames, frame } = &mut entry.content
                && frames.animated()
            {
                let next = frames.frame_at(game.tick_count);
                if *frame != next || rebuild {
                    *frame = next;
                    renderer.set_inline_object_frame(key, &format!("{key}#{next}"));
                }
            }
        }
    }

    /// The tile (or frames) behind one inline object, starting a head fetch
    /// where one is needed.
    fn load_inline_object(
        &mut self,
        object: &InlineObject,
        game: &GameState,
    ) -> InlineObjectContent {
        match object {
            InlineObject::AtlasSprite { atlas, sprite } => {
                let frames =
                    crate::ui::object_glyph::load_atlas_sprite(&self.resources, atlas, sprite)
                        .unwrap_or_else(|| {
                            // A sprite the atlas doesn't have keeps the missing
                            // texture, as `TextureAtlas.missingSprite` does.
                            let (pixels, size) = crate::ui::object_glyph::missing_tile();
                            crate::ui::object_glyph::SpriteFrames::still(pixels, size)
                        });
                InlineObjectContent::Sprite { frames, frame: 0 }
            }
            InlineObject::Player {
                uuid,
                name,
                textures,
                hat,
            } => {
                let profile = resolve_head_profile(
                    HeadProfile {
                        id: *uuid,
                        name: name.clone(),
                        textures: textures.clone(),
                        patch_texture: None,
                    },
                    &game.tab_list,
                );
                self.request_head(profile.clone());
                InlineObjectContent::Head { profile, hat: *hat }
            }
        }
    }

    /// Starts the one fetch a profile needs; its hat and base glyphs share it.
    pub(crate) fn request_head(&mut self, profile: HeadProfile) {
        if let Some(entry) = self.player_heads.get_mut(&profile) {
            entry.last_used = Instant::now();
            return;
        }
        // Its default skin draws until its own arrives.
        self.head_skins_dirty = true;
        // A `Static` profile with no textures has nothing to fetch: vanilla's
        // `SkinManager.get` reads the profile's own property and falls back to
        // the default skin without a lookup.
        let source = match (&profile.textures, &profile.id, &profile.name) {
            (Some(textures), _, _) => Some(PlayerSkinSource::Textures(textures.clone())),
            _ if profile.is_static() => None,
            (_, Some(_), _) => Some(PlayerSkinSource::Uuid),
            (_, None, Some(name)) if crate::player::valid_player_name(name) => {
                Some(PlayerSkinSource::Name(name.clone()))
            }
            _ => None,
        };
        let Some(source) = source else {
            self.player_heads.insert(
                profile,
                HeadEntry {
                    state: HeadState::Fallback,
                    last_used: Instant::now(),
                },
            );
            return;
        };
        self.player_heads.insert(
            profile.clone(),
            HeadEntry {
                state: HeadState::Pending,
                last_used: Instant::now(),
            },
        );
        let id = profile.id;
        let tx = self.head_tx.clone();
        self.tokio_rt.spawn(async move {
            let result = fetch_skin(&source, id).await;
            let _ = tx.send(HeadResult { profile, result });
        });
    }

    fn drain_head_results(&mut self) {
        while let Ok(head) = self.head_rx.try_recv() {
            let Some(entry) = self.player_heads.get_mut(&head.profile) else {
                continue;
            };
            entry.state = match head.result {
                Ok(data) => {
                    use crate::renderer::pipelines::menu_overlay::{
                        extract_face_8x8, extract_face_8x8_with_hat,
                    };
                    match (
                        extract_face_8x8(&data.pixels, data.width, data.height),
                        extract_face_8x8_with_hat(&data.pixels, data.width, data.height, false),
                    ) {
                        (Some(hat), Some(base)) => {
                            self.head_skins_dirty = true;
                            HeadState::Ready {
                                hat,
                                base,
                                skin: data.pixels,
                            }
                        }
                        _ => HeadState::Fallback,
                    }
                }
                Err(e) => {
                    tracing::warn!("Failed to load head skin for {:?}: {e}", head.profile);
                    HeadState::Fallback
                }
            };
            self.inline_atlas_dirty = true;
        }
    }

    fn remove_player_skin(&mut self, renderer: &mut Renderer, uuid: &uuid::Uuid) {
        self.requested_player_skins.remove(uuid);
        let removed = self.player_faces.remove(uuid).is_some();
        if removed {
            self.player_faces_dirty = true;
        }
        renderer.remove_player_entity_skin(uuid);
    }

    fn reload_live_resource_assets(
        &mut self,
        game: &mut GameState,
        renderer: &mut Renderer,
        prepared: crate::renderer::PreparedResources,
    ) {
        // The shared atlas bakes UVs into chunk vertices, so a live pack swap
        // must retire chunk geometry before repacking the atlas. Recreate the
        // CPU mesher and particle atlas state from the same freshly-reloaded
        // registry/UV map, then let the normal rescan rebuild loaded columns.
        renderer.clear_chunk_meshes();
        self.reload_pack_assets(renderer, prepared);
        game.block_registry
            .replace_render_assets(renderer.shared_render_assets());

        let mesh_dispatcher = renderer.create_mesh_dispatcher(
            game.block_registry.owned_render_view(),
            Arc::clone(&game.biome_climate),
            self.resources(),
            game.cardinal_light,
        );
        let (grass, foliage, dry_foliage) = mesh_dispatcher.colormaps();
        game.mesh_dispatcher = mesh_dispatcher;
        game.sync_distant_colors();
        game.particle_store = crate::particle::ParticleStore::new(
            renderer.uv_map().clone(),
            grass,
            foliage,
            dry_foliage,
        );

        game.rebuild_meshes();
    }

    fn clear_server_ui(&mut self, game: &mut GameState, renderer: &mut Renderer) {
        game.tab_list.clear();
        game.scoreboard.clear();
        game.player.effects.clear();
        game.boss_bars.clear();
        game.toasts.clear();
        // Vanilla Hud.onDisconnected: clearTitles + resetTitleTimes.
        game.title.clear(true);
        game.subtitles.clear();
        self.requested_player_skins.clear();
        self.player_faces.clear();
        self.player_faces_dirty = false;
        self.inline_objects.clear();
        self.player_heads.clear();
        self.asset_head_skins.clear();
        self.head_skins_dirty = true;
        self.game_dynamic_atlas_keys.clear();
        renderer.clear_player_entity_skins();
    }

    pub fn clear_server_resource_packs(&mut self, renderer: &mut Renderer) {
        match self.remove_server_resource_packs(None, Renderer::prepare_resources) {
            Ok(Some(prepared)) => {
                renderer.clear_chunk_meshes();
                self.reload_pack_assets(renderer, prepared);
            }
            Ok(None) => {}
            Err(error) => tracing::warn!(%error, "Failed to clear server resource assets"),
        }
    }

    fn remove_server_resource_packs<T>(
        &mut self,
        uuid: Option<uuid::Uuid>,
        prepare: impl FnOnce(&dyn crate::resources::ResourceManager) -> std::io::Result<T>,
    ) -> std::io::Result<Option<T>> {
        let ids: Vec<_> = match uuid {
            Some(uuid) => self
                .server_resource_ids
                .remove(&uuid)
                .map(|id| (uuid, id))
                .into_iter()
                .collect(),
            None => self.server_resource_ids.drain().collect(),
        };
        if ids.is_empty() {
            return Ok(None);
        }
        let removed: Vec<_> = ids
            .into_iter()
            .map(|(uuid, id)| {
                let registration = self.registered_resources.take_registration(&id);
                (uuid, id, registration)
            })
            .collect();
        match self.activate_resource_selection(None, prepare) {
            Ok(prepared) => Ok(Some(prepared)),
            Err(error) => {
                // Positions were captured as each definition was removed.
                for (uuid, id, registration) in removed.into_iter().rev() {
                    if let Some(registration) = registration {
                        self.registered_resources.restore_registration(registration);
                    }
                    self.server_resource_ids.insert(uuid, id);
                }
                Err(error)
            }
        }
    }

    fn activate_server_resource_pack<T>(
        &mut self,
        uuid: uuid::Uuid,
        hash: &str,
        path: std::path::PathBuf,
        prepare: impl FnOnce(&dyn crate::resources::ResourceManager) -> std::io::Result<T>,
    ) -> std::io::Result<T> {
        let previous = self
            .server_resource_ids
            .get(&uuid)
            .and_then(|id| self.registered_resources.take_registration(id));
        let id = self.registered_resources.register(
            PackLocationInfo {
                id: uuid.to_string(),
                title: crate::chat_component::Component::text(hash),
                known_pack_info: None,
            },
            PhysicalPack::Directory(path),
            // Vanilla `DownloadedPackSource.DOWNLOADED_PACK_SELECTION`: pinned
            // above the player's packs, so one enabled later can't cover it.
            PackEnablementConfig {
                required: true,
                default_position: PackPosition::Top,
                fixed_position: true,
            },
        );
        let previous_id = self.server_resource_ids.insert(uuid, id.clone());
        let prepared = match self.activate_resource_selection(Some((id.clone(), true)), prepare) {
            Ok(prepared) => prepared,
            Err(error) => {
                self.registered_resources.unregister(&id);
                if let Some(previous) = previous {
                    self.registered_resources.restore_registration(previous);
                }
                if self.server_resource_ids.get(&uuid) == Some(&id) {
                    match previous_id {
                        Some(previous_id) => {
                            self.server_resource_ids.insert(uuid, previous_id);
                        }
                        None => {
                            self.server_resource_ids.remove(&uuid);
                        }
                    }
                }
                return Err(error);
            }
        };
        Ok(prepared)
    }

    pub(crate) fn font_sources(&self) -> crate::ui::font::FontSources<'_> {
        crate::ui::font::FontSources {
            resources: &self.resources,
            options: self.menu.font_options(),
        }
    }

    /// Rebuilds what was built from assets that didn't exist yet, once the
    /// first-run download has put them on disk. Only the install boot screen
    /// exists here; it is discarded before constructing the new Renderer.
    pub(crate) fn reload_installed_assets(&mut self) {
        self.set_resource_installation();
        self.set_resource_theme(self.menu.theme());
        self.reload_resources();
        self.menu.pick_splash();
        self.audio = crate::audio::AudioEngine::new(&self.resources, self.menu.category_volumes());
        self.complete_resource_selection();
    }

    pub(crate) fn use_vanilla_block_cache(&self) -> bool {
        self.active_custom_resource_ids.is_empty()
    }

    fn reload_pack_assets(
        &mut self,
        renderer: &mut Renderer,
        prepared: crate::renderer::PreparedResources,
    ) {
        renderer.reload_assets(self.font_sources(), prepared);
        // A pack swap changes what the sprites look like, so they reload.
        self.inline_objects
            .retain(|_, entry| matches!(entry.content, InlineObjectContent::Head { .. }));
        self.game_dynamic_atlas_keys.clear();
        self.asset_reload_revision = self.asset_reload_revision.wrapping_add(1);
        self.complete_resource_selection();
    }

    /// Vanilla `Options.updateFontOptions`: a Font Settings toggle applies at
    /// once, from the title screen or in game.
    pub fn apply_font_options(&mut self, renderer: &mut Renderer) {
        if std::mem::take(&mut self.menu.reload_fonts) {
            renderer.reload_fonts(self.font_sources());
        }
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
        let mut priority_remesh: Vec<(crate::position::ChunkPos, i32)> = Vec::new();
        let mut disconnect_reason: Option<String> = None;
        self.drain_player_skin_results(renderer);

        // Vanilla `PacketProcessor.processQueuedPackets` drains everything
        // before this frame's ticks. A departure: after `CHUNK_PACKET_BUDGET`
        // of chunk packets the rest wait, in order, for the next frame, so a
        // burst of chunks is a few short frames instead of one long one. Only
        // frames that owe no tick split it (`defer_packets`; callers add the
        // frame's time first), so they render and nothing more.
        // `ChunkBatchReceived` is answered on the network thread as vanilla
        // does, so this doesn't pace the server.
        let mut chunk_work = std::time::Duration::ZERO;
        while !game.hold_packets
            && !defer_packets(chunk_work, self.tick_accumulator)
            && let Ok(event) = rx.try_recv()
        {
            let chunk_packet = matches!(
                event,
                NetworkEvent::ChunkLoaded { .. }
                    | NetworkEvent::ChunkUnloaded { .. }
                    | NetworkEvent::LightUpdate { .. }
                    | NetworkEvent::ChunkBiomes { .. }
            )
            .then(std::time::Instant::now);
            match event {
                NetworkEvent::ProtocolSelected { protocol } => game.select_protocol(protocol),
                NetworkEvent::Connected => {
                    if let Some(state) = connect_phase.as_deref_mut() {
                        tracing::info!("Connected to server");
                        // Ticks restart from zero with the play state.
                        self.tick_accumulator = 0.0;
                        self.carried_ticks = 0;
                        *state = ConnectionPhase::Loading;
                    } else {
                        tracing::warn!("Unexpected NetworkEvent::Connected, skipping");
                    }
                }
                NetworkEvent::LevelChunksLoadStart => {
                    if let Some(tracker) = &mut game.level_load {
                        tracker.loading_packets_received();
                    }
                }
                NetworkEvent::BiomeColors { colors } => game.set_biome_climate(colors),
                NetworkEvent::DimensionInfo(dim) => {
                    game.enter_dimension(
                        dim,
                        self.menu.render_distance,
                        &*renderer,
                        self.resources(),
                    );
                    renderer.clear_chunk_meshes();
                    renderer.reset_distant();
                }
                NetworkEvent::DimensionName { name } => {
                    game.dimension = name;
                }
                NetworkEvent::ChunkLoaded {
                    pos,
                    data,
                    heightmaps,
                    light,
                } => {
                    game.load_chunk(pos, &data, &heightmaps, &light);
                }
                NetworkEvent::LightUpdate { pos, light } => game.apply_light_update(pos, &light),
                NetworkEvent::ChunkBiomes { chunks } => game.replace_biomes(&chunks),
                NetworkEvent::ChunkUnloaded { pos } => {
                    game.unload_chunk(pos);
                    renderer.remove_chunk_mesh(&pos);
                    renderer.set_distant_column_sections(pos, 0, game.chunk_store.min_section_y());
                }
                NetworkEvent::ChunkCacheCenter { x, z } => {
                    tracing::debug!("Chunk cache center: [{x}, {z}]");
                    game.chunk_store
                        .set_center(crate::position::ChunkPos::new(x, z));
                }
                NetworkEvent::Ping { id } => AppCore::send_pong(connection, id),
                NetworkEvent::PlayerPosition {
                    id,
                    change,
                    relative,
                } => {
                    if game.apply_player_position(connection, id, &change, &relative) {
                        // The camera is the eye, as `sync_camera_pos` keeps it
                        // every frame; the feet would seed it a block and a half
                        // low until the first in-game frame.
                        renderer.reset_camera(game.player.eye_pos(), game.player.look_dir);
                    }
                }
                NetworkEvent::PlayerRotation {
                    y_rot,
                    relative_y,
                    x_rot,
                    relative_x,
                } => {
                    let new_look_dir = resolve_corrected_look(
                        game.player.look_dir,
                        y_rot,
                        x_rot,
                        relative_y,
                        relative_x,
                    );

                    // Vanilla `handleRotatePlayer`: set, `setOldRot`, then ack.
                    game.player.look_dir = new_look_dir;
                    game.player.prev_look_dir = new_look_dir;
                    renderer.reset_camera(game.player.eye_pos(), new_look_dir);
                    connection.packet_tx.send(ServerboundGamePacket::MovePlayerRot(
                        azalea_protocol::packets::game::s_move_player_rot::ServerboundMovePlayerRot {
                            look_direction: new_look_dir.into(),
                            flags: Default::default(),
                        },
                    ));
                }
                NetworkEvent::PlayerHealth {
                    health,
                    food,
                    saturation,
                } => {
                    game.player.apply_server_health(health);
                    game.player.food = food;
                    game.player.saturation = saturation;
                    if health > 0.0 && game.dead {
                        game.dead = false;
                        game.player.reset_death_time();
                        game.reset_death_screen();
                        self.apply_cursor_grab(window, Some(game));
                    } else if health <= 0.0 && !game.dead {
                        game.dead = true;
                        game.player.reset_death_time();
                        if !game.death_screen_open {
                            game.death_message.clear();
                        }
                        // Vanilla container screens close themselves as soon as
                        // the local player is no longer alive.
                        game.close_menu();
                        game.close_creative_inventory();
                    }
                }
                NetworkEvent::SetPassengers {
                    vehicle,
                    passengers,
                } => {
                    let me = game.player.entity_id;
                    if passengers.contains(&me) {
                        // Vanilla `handleSetEntityPassengersPacket` on boarding:
                        // a boat turns the player its way, and the dismount
                        // hint shows.
                        if game.riding_vehicle_id != Some(vehicle) {
                            if let Some(boat) = game
                                .entity_store
                                .living
                                .get(&vehicle)
                                .filter(|v| crate::entity::is_boat(v.entity_type))
                            {
                                let look = LookDirection::new(
                                    boat.body_y_rot_deg,
                                    game.player.look_dir.x_rot_deg(),
                                );
                                game.player.look_dir = look;
                                game.player.prev_look_dir = look;
                                renderer.set_camera_look_dir(look);
                            }
                            let key = self.input.key_bindings.key_name(Action::Sneak);
                            let hint = crate::lang::translate("mount.onboard")
                                .unwrap_or("Press %s to Dismount")
                                .replace("%s", &key);
                            game.action_bar = Some((
                                vec![crate::ui::text::TextSpan::new(hint, [1.0; 4])],
                                game.tick_count,
                            ));
                        }
                        game.riding_vehicle_id = Some(vehicle);
                        // Only the first passenger controls (vanilla
                        // getControlledVehicle -> null otherwise).
                        game.controlled_vehicle_id =
                            (passengers.first() == Some(&me)).then_some(vehicle);
                    } else {
                        // Removed from this vehicle.
                        if game.riding_vehicle_id == Some(vehicle) {
                            game.riding_vehicle_id = None;
                        }
                        if game.controlled_vehicle_id == Some(vehicle) {
                            game.controlled_vehicle_id = None;
                        }
                    }
                    game.vehicle_passengers.insert(vehicle, passengers);
                }
                NetworkEvent::EntityEquipment { entity_id, slots } => {
                    if let Some(e) = game.entity_store.living.get_mut(&entity_id) {
                        for (slot, item) in slots {
                            e.set_equipment(slot, item);
                        }
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
                NetworkEvent::EntityAttributesUpdate {
                    entity_id,
                    snapshots,
                } => {
                    let dirty = |kind| snapshots.iter().any(|snapshot| snapshot.attribute == kind);
                    if entity_id == game.player.entity_id {
                        for snapshot in &snapshots {
                            if !game.player.attributes.apply_snapshot(snapshot) {
                                tracing::warn!(
                                    "Server tried to update unsupported player attribute {:?}",
                                    snapshot.attribute
                                );
                            }
                        }
                        sync_player_attribute_mirrors(&mut game.player, dirty);
                    }

                    if let Some(entity) = game.entity_store.living.get_mut(&entity_id) {
                        for snapshot in &snapshots {
                            entity.attributes.apply_snapshot_or_insert(snapshot);
                        }
                        sync_living_attribute_mirrors(
                            &entity.attributes,
                            &mut entity.health,
                            &mut entity.max_health,
                            &mut entity.camera_distance,
                            dirty,
                        );
                    }
                }
                NetworkEvent::UpdateMobEffect { entity_id, effect } => {
                    if entity_id == game.player.entity_id {
                        game.player.effects.update(effect);
                    }
                }
                NetworkEvent::RemoveMobEffect {
                    entity_id,
                    effect_id,
                } => {
                    if entity_id == game.player.entity_id {
                        game.player.effects.remove(effect_id);
                    }
                }
                NetworkEvent::ClearMobEffects => {
                    game.player.effects.clear();
                }
                NetworkEvent::ContainerContent {
                    container_id,
                    items,
                    carried,
                    state_id,
                } => {
                    game.drop_bundle_selection(container_id, None);
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
                    game.set_distant_cache_key();
                }
                NetworkEvent::DialogRegistry(registry) => {
                    game.dialog_registry = registry;
                }
                NetworkEvent::ItemTags(tags) => {
                    game.recipe_book.set_item_tags(tags);
                }
                NetworkEvent::BlockTags { tags } => {
                    game.block_registry.replace_tags(tags);
                }
                NetworkEvent::ContainerSlot {
                    container_id,
                    index,
                    item,
                    state_id,
                } => {
                    game.drop_bundle_selection(container_id, Some(index));
                    if container_id == 0 {
                        game.player.inventory.set_slot(index as usize, item);
                        game.sync_container_from_inventory();
                        game.inventory_state_id = state_id;
                    } else if game.open_menu_id() == Some(container_id) {
                        game.set_menu_slot(index as usize, item);
                        game.set_container_state_id(state_id);
                    }
                }
                NetworkEvent::PlayerInventorySlot { index, item } => {
                    if let Some(slot) =
                        crate::player::inventory::menu_slot_for_inventory_index(index)
                    {
                        game.drop_bundle_selection(0, Some(slot as u16));
                        game.player.inventory.set_slot(slot, item);
                        game.sync_container_from_inventory();
                    }
                }
                NetworkEvent::HeldSlot { slot } => {
                    self.input.set_selected_slot(slot);
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
                        MenuKind::Hopper => Some(ContainerScreen::Hopper),
                        MenuKind::Generic3x3 => Some(ContainerScreen::Dispenser),
                        MenuKind::BrewingStand => Some(ContainerScreen::BrewingStand),
                        MenuKind::Crafter3x3 => Some(ContainerScreen::Crafter),
                        _ => None,
                    };
                    if let Some(screen) = screen {
                        if game.inventory_open || game.recipe_book_ui.captures_typing() {
                            game.recipe_book_ui
                                .reset_for_closed_screen(&mut game.recipe_book);
                        }
                        self.replace_screen(connection, game);
                        game.inventory_open = false;
                        game.inv_drag = None;
                        game.inv_last_click = None;
                        game.open_container = Some(crate::app::phases::in_game::OpenContainer {
                            id: container_id,
                            title,
                            #[cfg(feature = "test-harness")]
                            menu: menu_type,
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
                        // TODO: render the remaining menu screens (beacon,
                        // loom, ...). Until then tell the server we
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
                // Vanilla `handleMoveVehicle`: the vehicle this client moves
                // snaps to the server's correction, which it confirms.
                NetworkEvent::MoveVehicle {
                    position,
                    y_rot_deg,
                    x_rot_deg,
                } => {
                    if let Some(id) = game.steered_boat()
                        && let Some(boat) = game.entity_store.living.get(&id)
                    {
                        let moved =
                            boat.client_position().distance(position) > f64::from(1.0e-5_f32);
                        if moved {
                            game.entity_store.snap_living(
                                id,
                                position.into(),
                                LookDirection::new(y_rot_deg, x_rot_deg),
                                game.entity_store.living[&id].velocity,
                            );
                        }
                        let boat = &game.entity_store.living[&id];
                        connection.packet_tx.send_raw(move_vehicle_packet(boat));
                    }
                }
                NetworkEvent::ContainerClosed => {
                    // Vanilla closes whatever menu is open regardless of the
                    // packet's container id, and with it any screen.
                    game.close_sign_editor(&connection.packet_tx);
                    game.close_menu();
                    // The server initiated the close; don't echo one back.
                    game.container_was_open = None;
                    self.apply_cursor_grab(window, Some(game));
                }
                NetworkEvent::ChatMessage {
                    component,
                    secure_component,
                    missing_profile_component,
                    signature,
                    sender_uuid,
                    signed_body,
                    source,
                    tag,
                } => {
                    use crate::ui::text::format_component_spans_with_bindings;

                    let only_secure = game.chat.only_secure();
                    let component = if only_secure {
                        secure_component.unwrap_or(component)
                    } else {
                        component
                    };
                    let spans = format_component_spans_with_bindings(
                        &component,
                        [1.0; 4],
                        &self.input.key_bindings,
                    );
                    tracing::info!("Chat: {}", crate::ui::text::span_text(&spans));
                    if let (Some(sender_uuid), Some(body)) = (sender_uuid, signed_body) {
                        let now_ms = crate::net::chat_security::now_ms();
                        let validation =
                            game.tab_list.players.get_mut(&sender_uuid).map(|player| {
                                player.validate_chat_message(
                                    &body,
                                    signature.as_ref(),
                                    game.server_enforces_secure_chat,
                                    now_ms,
                                )
                            });
                        match validation {
                            None | Some(PlayerChatValidation::Invalid) => {
                                if let Some(component) = missing_profile_component {
                                    let spans = format_component_spans_with_bindings(
                                        &component,
                                        [1.0; 4],
                                        &self.input.key_bindings,
                                    );
                                    game.chat.push_validation_error(spans, signature);
                                }
                            }
                            Some(validation) => {
                                // An accepted unsigned message loses its signature
                                // (vanilla `removeSignature`).
                                let signed = validation == PlayerChatValidation::Signed;
                                let signature = signature.filter(|_| signed);
                                if body.fully_filtered {
                                    game.chat.push_fully_filtered(signature);
                                } else {
                                    let tag = accepted_player_chat_tag(
                                        game.singleplayer && sender_uuid == self.user.uuid,
                                        signed,
                                        &body,
                                        now_ms,
                                        only_secure,
                                        &self.input.key_bindings,
                                    );
                                    game.chat
                                        .push_message_with_source(spans, signature, source, tag);
                                }
                            }
                        }
                    } else {
                        game.chat
                            .push_message_with_source(spans, signature, source, tag);
                    }
                }
                NetworkEvent::DeleteChatMessage { signature } => {
                    game.chat.ignore_pending(signature);
                    game.chat.delete_message(signature);
                }
                NetworkEvent::ActionBar { component } => {
                    let spans = crate::ui::text::format_component_spans_with_bindings(
                        &component,
                        [1.0; 4],
                        &self.input.key_bindings,
                    );
                    game.action_bar = Some((spans, game.tick_count));
                }
                NetworkEvent::ServerLinks { links } => {
                    game.server_links = links;
                }
                NetworkEvent::OpenSignEditor { pos, is_front_text } => {
                    if let Some(editor) = game.sign_editor_for(pos.into(), is_front_text) {
                        self.replace_screen(connection, game);
                        game.close_menu();
                        game.sign_editor = Some(editor);
                        self.apply_cursor_grab(window, Some(game));
                    }
                }
                NetworkEvent::ShowDialog { dialog } => {
                    // The screen under the dialog is vanilla's `previousScreen`:
                    // it keeps its state (an open container included) and comes
                    // back when the dialog closes.
                    // TODO: a sign editor under it sends its lines here, as
                    // `setScreen` runs its `removed()`, and again on closing.
                    if game.open_server_dialog(dialog) {
                        self.apply_cursor_grab(window, Some(game));
                    }
                }
                // `clearDialog` closes a dialog screen only; a
                // `WaitingForResponseScreen` stays up for its own Back button.
                NetworkEvent::ClearDialog => {
                    if game
                        .server_dialog
                        .as_ref()
                        .is_some_and(|dialog| dialog.is_dialog())
                    {
                        game.server_dialog = None;
                        self.apply_cursor_grab(window, Some(game));
                    }
                }
                NetworkEvent::BossBarUpdate { id, op } => {
                    game.boss_bars.apply(id, op);
                }
                NetworkEvent::AdvancementsUpdate(update) => {
                    game.toasts.apply_advancements(*update);
                }
                NetworkEvent::RecipeBookAdd { entries, replace } => {
                    game.recipe_book.apply_add(entries, replace);
                }
                NetworkEvent::RecipeBookRemove { ids } => {
                    game.recipe_book.remove(ids);
                }
                NetworkEvent::RecipeBookSettings(settings) => {
                    game.recipe_book.apply_settings(settings);
                }
                NetworkEvent::RecipeData(data) => {
                    game.recipe_book.data = data;
                }
                NetworkEvent::GhostRecipe(ghost) => {
                    use crate::app::phases::in_game::ContainerScreen;

                    let active_recipe_container = if game.inventory_open {
                        Some(0)
                    } else {
                        game.open_container.as_ref().and_then(|container| {
                            matches!(
                                container.screen,
                                ContainerScreen::CraftingTable | ContainerScreen::Furnace(_)
                            )
                            .then_some(container.id)
                        })
                    };
                    if active_recipe_container == Some(ghost.container_id) {
                        game.recipe_book.ghost_recipe = Some(*ghost);
                    }
                }
                NetworkEvent::RecipeToastAdd { entries } => {
                    game.toasts.add_recipes(entries);
                }
                NetworkEvent::TitleText { spans } => {
                    game.title.set_title(spans);
                }
                NetworkEvent::SubtitleText { spans } => {
                    game.title.set_subtitle(spans);
                }
                NetworkEvent::TitlesAnimation {
                    fade_in,
                    stay,
                    fade_out,
                } => {
                    game.title.set_times(fade_in, stay, fade_out);
                }
                NetworkEvent::ClearTitles { reset_times } => {
                    game.title.clear(reset_times);
                }
                NetworkEvent::ScoreboardObjective {
                    name,
                    display,
                    number_format,
                } => {
                    game.scoreboard.set_objective(name, display, number_format);
                }
                NetworkEvent::ScoreboardDisplay { name } => {
                    game.scoreboard.set_display(name);
                }
                NetworkEvent::ScoreboardScore {
                    owner,
                    objective,
                    score,
                    display,
                    number_format,
                } => {
                    game.scoreboard
                        .set_score(owner, objective, score, display, number_format);
                }
                NetworkEvent::ScoreboardReset { owner, objective } => {
                    game.scoreboard.reset_score(&owner, objective.as_deref());
                }
                NetworkEvent::ScoreboardTeam {
                    name,
                    display_name,
                    prefix,
                    suffix,
                    color,
                    fill_color,
                    collision_rule,
                    name_tag_visibility,
                    see_friendly_invisibles,
                    members,
                } => {
                    game.scoreboard.set_team(
                        name,
                        display_name,
                        prefix,
                        suffix,
                        color,
                        fill_color,
                        collision_rule,
                        name_tag_visibility,
                        see_friendly_invisibles,
                        members,
                    );
                }
                NetworkEvent::ScoreboardTeamMembers {
                    name,
                    members,
                    join,
                } => {
                    game.scoreboard.update_team_members(&name, members, join);
                }
                NetworkEvent::ScoreboardTeamRemoved { name } => {
                    game.scoreboard.remove_team(&name);
                }
                NetworkEvent::CommandTree { tree } => {
                    game.command_tree = Some(tree);
                }
                NetworkEvent::CommandSuggestions { id, start, options } => {
                    game.chat.apply_server_suggestions(
                        id,
                        start,
                        options,
                        game.command_tree.as_deref(),
                    );
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
                    game.chunk_store.block_entities.remove_column(chunk_pos);
                    game.block_entity_anim.drop_chunk(chunk_pos.x, chunk_pos.z);
                    for (pos, kind, nbt) in entries {
                        if kind == azalea_registry::builtin::BlockEntityKind::Piston {
                            let piston = crate::world::piston::MovingPiston::from_nbt(
                                &nbt,
                                game.block_registry.block_state_table(),
                            );
                            game.block_entity_anim.pistons.insert(pos, piston);
                        }
                        game.chunk_store.block_entities.insert(
                            pos,
                            crate::world::block_entity::StoredBlockEntity::new(kind, nbt),
                        );
                    }
                }
                NetworkEvent::BlockEntityUpdate { pos, kind, nbt } => {
                    let chunk_pos = crate::position::ChunkPos::containing(pos);
                    if game.chunk_store.get_chunk(&chunk_pos).is_some() {
                        game.chunk_store.block_entities.insert(
                            pos,
                            crate::world::block_entity::StoredBlockEntity::new(kind, nbt),
                        );
                    }
                }
                NetworkEvent::BlockEvent {
                    pos,
                    action_id,
                    action_parameter,
                } => {
                    // Vanilla `Level.blockEvent` hands the event to the block
                    // the client has there.
                    let state = game.chunk_store.get_block_state(pos.x, pos.y, pos.z);
                    if crate::world::piston::is_piston_base(
                        game.block_registry.block_state_table(),
                        state,
                    ) {
                        let mut level = ClientPistonLevel::new(game, &mut priority_remesh);
                        crate::world::piston::trigger_event(
                            &mut level,
                            pos,
                            action_id,
                            action_parameter,
                        );
                        for (pos, state) in std::mem::take(&mut level.destroyed) {
                            block_break_effect(&self.audio, game, pos, state);
                        }
                    } else if action_id == 1 {
                        // Chest/shulker: the open-viewer count.
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
                    play_entity_sound(
                        &mut self.audio,
                        game,
                        entity_id,
                        &sound,
                        category,
                        volume,
                        pitch,
                        seed,
                    );
                }
                NetworkEvent::StopSound { sound_id, category } => {
                    self.audio.stop_sounds(sound_id.as_deref(), category);
                }
                NetworkEvent::GameModeChanged {
                    game_mode,
                    previous,
                } => {
                    tracing::info!("Game mode changed to {game_mode}");
                    // Vanilla `setLocalMode`: an in-game change records the
                    // replaced mode; login/respawn set it from the packet.
                    match previous {
                        Some(p) => game.previous_game_mode = p,
                        None if game_mode != game.player.game_mode => {
                            game.previous_game_mode = Some(game.player.game_mode);
                        }
                        None => {}
                    }
                    game.player.game_mode = game_mode;
                    // Vanilla GameType.updatePlayerAbilities, applied locally on
                    // setLocalMode (no packet).
                    // TODO: the instabuild/invulnerable/may_build halves
                    match game_mode {
                        // Creative grants flight but leaves `flying` untouched.
                        1 => game.player.may_fly = true,
                        3 => {
                            game.player.may_fly = true;
                            game.player.flying = true;
                        }
                        _ => {
                            game.player.may_fly = false;
                            game.player.flying = false;
                        }
                    }
                    if game.inventory_open || game.creative_inventory_open {
                        if game.inventory_open && matches!(game_mode, 1 | 3) {
                            game.recipe_book_ui
                                .reset_for_closed_screen(&mut game.recipe_book);
                        }
                        match game_mode {
                            1 => {
                                game.inventory_open = false;
                                if !game.creative_inventory_open {
                                    game.open_creative_inventory();
                                }
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
                NetworkEvent::PlayerAbilitiesChanged {
                    flying,
                    can_fly,
                    flying_speed,
                    walking_speed,
                } => {
                    game.player.flying = flying;
                    game.player.may_fly = can_fly;
                    game.player.fly_speed = flying_speed;
                    game.player.walk_speed = walking_speed;
                }
                NetworkEvent::ServerViewDistance { distance } => {
                    game.set_server_view_distance(distance)
                }
                NetworkEvent::ServerSimulationDistance { distance } => {
                    tracing::info!("Server simulation distance: {distance}");
                    game.server_simulation_distance = distance;
                }
                NetworkEvent::BlockChangedAck { seq } => {
                    let mut ack_dirty: Vec<crate::position::BlockPos> = Vec::new();
                    let player_aabb = game.player.bounding_box();
                    let snap = game.interaction.acknowledge(
                        game.block_registry.block_state_table(),
                        seq,
                        &game.chunk_store,
                        player_aabb,
                        &mut ack_dirty,
                    );
                    if let Some(snap) = snap {
                        game.player.position = snap.into();
                        game.player.prev_position = game.player.position;
                    }
                    let min_y = game.chunk_store.min_y();
                    let n = game.chunk_store.section_count();
                    for b in ack_dirty {
                        game.light_engine.block_dirty(b.x, b.y, b.z);
                        dirty_sections_for_block(&mut priority_remesh, b.x, b.y, b.z, min_y, n);
                    }
                }
                NetworkEvent::TimeUpdate { game_time, clocks } => {
                    game.sky_state.game_time = game_time;
                    game.clocks.handle_updates(game_time, clocks);
                    game.sync_day_time();
                }
                NetworkEvent::WeatherUpdate(weather) => {
                    // Mirrors vanilla ClientPacketListener.handleGameEvent: the
                    // server drives the level, the client just applies it.
                    use crate::net::Weather;
                    match weather {
                        Weather::StartRaining => game.sky_state.rain_level = 0.0,
                        Weather::StopRaining => game.sky_state.rain_level = 1.0,
                        Weather::RainLevel(level) => game.sky_state.rain_level = level,
                        Weather::ThunderLevel(level) => game.sky_state.thunder_level = level,
                    }
                }
                NetworkEvent::EntitySpawned {
                    id,
                    uuid,
                    entity_type,
                    position,
                    velocity,
                    x_rot_deg,
                    y_rot_deg,
                    head_y_rot_deg,
                } => {
                    game.entity_positions.insert(id, position);
                    game.entity_kinds.insert(id, entity_type);
                    game.silent_entities.remove(&id);
                    if crate::entity::is_living_mob(&entity_type) {
                        // A boat isn't living: it has no head yaw, only its own.
                        let y_rot = if crate::entity::is_boat(entity_type) {
                            y_rot_deg
                        } else {
                            head_y_rot_deg
                        };
                        game.entity_store.spawn_living(
                            id,
                            entity_type,
                            position,
                            LookDirection::new(y_rot, x_rot_deg),
                            uuid,
                        );
                        if entity_type == azalea_registry::builtin::EntityKind::Player {
                            let textures = game
                                .tab_list
                                .players
                                .get(&uuid)
                                .and_then(|p| p.textures.clone());
                            self.queue_player_skin(uuid, textures);
                        }
                    }
                    if entity_type == azalea_registry::builtin::EntityKind::Item {
                        game.item_entity_store
                            .spawn_item(id, uuid, position, velocity);
                    }
                }
                NetworkEvent::EntityMoved {
                    id,
                    dx,
                    dy,
                    dz,
                    on_ground,
                } => {
                    // Vanilla `handleMoveEntity`: an entity this client moves
                    // only takes the position as the base later moves decode
                    // against.
                    if game.steered_boat() == Some(id) {
                        game.entity_store.shift_living_base(id, dvec3(dx, dy, dz));
                    } else {
                        game.entity_store
                            .move_living_delta(id, dx, dy, dz, on_ground);
                        game.item_entity_store.move_delta(id, dx, dy, dz, on_ground);
                        if let Some(pos) = game.entity_positions.get_mut(&id) {
                            *pos += dvec3(dx, dy, dz);
                            self.audio.update_entity_sound_position(id, *pos);
                        }
                    }
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
                    if game.steered_boat() == Some(id) {
                        game.entity_store.shift_living_base(id, dvec3(dx, dy, dz));
                    } else {
                        game.entity_store
                            .move_living_delta(id, dx, dy, dz, on_ground);
                        game.entity_store
                            .rotate_living(id, y_rot_deg, x_rot_deg, on_ground);
                        game.item_entity_store.move_delta(id, dx, dy, dz, on_ground);
                        if let Some(pos) = game.entity_positions.get_mut(&id) {
                            *pos += dvec3(dx, dy, dz);
                            self.audio.update_entity_sound_position(id, *pos);
                        }
                    }
                }
                NetworkEvent::EntityRotated {
                    id,
                    y_rot_deg,
                    x_rot_deg,
                    on_ground,
                } => {
                    if game.steered_boat() != Some(id) {
                        game.entity_store
                            .rotate_living(id, y_rot_deg, x_rot_deg, on_ground);
                    }
                }
                NetworkEvent::EntityMotion { id, velocity } => {
                    // Vanilla `Entity.lerpMotion` sets velocity; the local
                    // player lives outside the entity stores.
                    if id == game.player.entity_id {
                        game.player.velocity = velocity.into();
                    } else {
                        game.item_entity_store.set_motion(id, velocity);
                        game.entity_store.set_living_motion(id, velocity);
                    }
                }
                NetworkEvent::PlayerKnockback { delta } => {
                    // Vanilla `handleExplosion`: `LocalPlayer.addDeltaMovement`.
                    *game.player.velocity += delta;
                }
                NetworkEvent::ItemCooldown { group, duration } => {
                    game.interaction.item_cooldowns.set(group, duration);
                }
                NetworkEvent::EntityTeleported {
                    id,
                    position,
                    velocity,
                    y_rot_deg,
                    x_rot_deg,
                    on_ground,
                } => {
                    // `handleEntityPositionSync` (no velocity) only moves the
                    // base of an entity this client moves; a teleport still
                    // moves it.
                    if velocity.is_none() && game.steered_boat() == Some(id) {
                        game.entity_store.set_living_base(id, position);
                    } else {
                        game.entity_store.teleport_living(id, position, on_ground);
                        game.entity_store
                            .rotate_living(id, y_rot_deg, x_rot_deg, on_ground);
                        if let Some(velocity) = velocity {
                            game.entity_store.set_living_motion(id, velocity);
                        }
                        game.item_entity_store
                            .teleport(id, position, velocity, on_ground);
                        game.entity_positions.insert(id, position);
                        self.audio.update_entity_sound_position(id, position);
                    }
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
                    // 1010/1011 (jukebox start/stop) only notify nearby
                    // entities so far. TODO: the song itself and the
                    // now-playing HUD line.
                    if event_type == 1010 || event_type == 1011 {
                        game.entity_store
                            .set_record_playing_nearby(pos, event_type == 1010);
                    }
                    if event_type == 2001
                        && let Some(state) = game.block_registry.block_state_table().try_state(data)
                    {
                        block_break_effect(&self.audio, game, pos, state);
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
                        game.entity_positions.remove(id);
                        game.entity_kinds.remove(id);
                        game.silent_entities.remove(id);
                        self.audio.stop_entity_sounds(*id);
                    }
                    game.item_entity_store.remove(&ids);
                    if game.controlled_vehicle_id.is_some_and(|v| ids.contains(&v)) {
                        game.controlled_vehicle_id = None;
                    }
                    if game.riding_vehicle_id.is_some_and(|v| ids.contains(&v)) {
                        game.riding_vehicle_id = None;
                    }
                    for id in &ids {
                        game.vehicle_passengers.remove(id);
                    }
                    for passengers in game.vehicle_passengers.values_mut() {
                        passengers.retain(|p| !ids.contains(p));
                    }
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
                    damage,
                    count,
                    stack,
                } => {
                    renderer.ensure_item_mesh(&item_name);
                    game.item_entity_store
                        .set_item_data(id, item_name, item_id, damage, count, stack);
                }
                NetworkEvent::EntityData { id, index, value } => {
                    if index == 4
                        && let crate::entity::MetaValue::Bool(silent) = &value
                    {
                        if *silent {
                            game.silent_entities.insert(id);
                            self.audio.stop_entity_sounds(id);
                        } else {
                            game.silent_entities.remove(&id);
                        }
                    }
                    if id == game.player.entity_id {
                        use crate::entity::MetaValue::{Byte, Int};
                        match (index, &value) {
                            (0, Byte(flags)) => game.player.sync_shared_flags(*flags),
                            (1, Int(air)) => game.player.air_supply = *air,
                            (8, Byte(flags)) => {
                                game.interaction.sync_using_item_flag(flags & 1 != 0)
                            }
                            _ => {}
                        }
                    }
                    if index == 0
                        && let crate::entity::MetaValue::Byte(flags) = value
                    {
                        game.item_entity_store.set_shared_flags(id, flags);
                    }
                    game.entity_store.apply_entity_data(id, index, value);
                }
                NetworkEvent::EntityPose { id, is_crouching } => {
                    game.entity_store.set_crouching(id, is_crouching);
                }
                NetworkEvent::SetCamera { id } => {
                    if id == game.player.entity_id {
                        game.set_camera(None);
                    } else if game.entity_store.living.contains_key(&id) {
                        game.set_camera(Some(id));
                    }
                }
                NetworkEvent::EntitySleepingPos { id, pos } => {
                    game.entity_store.set_sleeping_pos(id, pos);
                    if id == game.player.entity_id {
                        game.player.set_sleeping_pos(pos);
                    }
                }
                NetworkEvent::EntityWakeUp { id } => {
                    // TODO: vanilla `stopSleeping` also stands a remote sleeper
                    // up beside the bed, facing it, with pitch 0.
                    game.entity_store.set_sleeping_pos(id, None);
                    if id == game.player.entity_id {
                        game.player.wake_up();
                    }
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
                            game.player.position.into(),
                            game.player.eye_pos().into(),
                            game.player.look_dir,
                        );
                    }
                }
                NetworkEvent::EntityVariant { id, kind, variant } => {
                    game.entity_store.set_variant(id, kind, variant);
                }
                NetworkEvent::WolfShaking { id, shaking } => {
                    game.entity_store.set_wolf_shaking(id, shaking);
                }
                NetworkEvent::RabbitJump { id } => {
                    game.entity_store.start_rabbit_jump(id);
                }
                NetworkEvent::SnifferDigging { id } => {
                    super::sniffer::start_digging_sound(&mut self.audio, game, id);
                }
                NetworkEvent::FoxEat { id } => {
                    if let Some(fox) = game.entity_store.living.get(&id)
                        && let Some(texture) = mouth_item_texture(fox)
                    {
                        game.particle_store.add_fox_eating_particles(
                            &texture,
                            fox.position.into(),
                            fox.look_dir,
                            renderer.camera_render_position(),
                        );
                    }
                }
                // `Armadillo.handleEntityEvent`: a local sound at its position.
                NetworkEvent::ArmadilloPeek { id } => {
                    if let Some(pos) = game.entity_store.armadillo_peek(id) {
                        self.audio.play_world_sound(
                            &crate::audio::SoundRef::event("entity.armadillo.peek"),
                            crate::audio::CATEGORY_NEUTRAL,
                            pos,
                            1.0,
                            1.0,
                            fastrand::u64(..),
                        );
                    }
                }
                NetworkEvent::GoatRam { id, lowering } => {
                    game.entity_store.set_goat_lowering_head(id, lowering);
                }
                NetworkEvent::SquidTentacleReset { id } => {
                    game.entity_store.squid_tentacle_reset(id);
                }
                NetworkEvent::GolemPunch { id } => {
                    game.entity_store.golem_punch(id);
                }
                NetworkEvent::GolemOfferFlower { id, offering } => {
                    game.entity_store.set_golem_offering_flower(id, offering);
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
                NetworkEvent::EntityCustomName { id, name, spans } => {
                    game.entity_store.set_custom_name(id, name, spans);
                }
                NetworkEvent::EntitySwing { id } => {
                    game.entity_store.start_swing(id);
                }
                NetworkEvent::EntityDamaged { id } => hurt_entity(game, id, None),
                NetworkEvent::HurtAnimation { id, yaw } => hurt_entity(game, id, Some(yaw)),
                NetworkEvent::EntityDied { id } => {
                    // TODO: vanilla's event 3 plays every living entity's
                    // getDeathSound client-side; pomme has no per-kind death
                    // sound table yet, so only the player's is played here.
                    if id == game.player.entity_id {
                        let pitch = (fastrand::f32() - fastrand::f32()) * 0.2 + 1.0;
                        self.audio.play_world_sound(
                            &crate::audio::SoundRef::event("entity.player.death"),
                            crate::audio::CATEGORY_PLAYERS,
                            game.player.position,
                            1.0,
                            pitch,
                            fastrand::u64(..),
                        );
                    }
                    game.entity_store.mark_dead(id);
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
                            &crate::audio::SoundRef::event("entity.item.pickup"),
                            crate::audio::CATEGORY_PLAYERS,
                            item_pos,
                            0.2,
                            (fastrand::f32() - fastrand::f32()) * 1.4 + 2.0,
                            fastrand::u64(..),
                        );
                    }
                }
                NetworkEvent::PlayerLogin {
                    entity_id,
                    hardcore,
                    show_death_screen,
                    online_mode,
                } => {
                    connection.packet_tx.chat_login(online_mode);
                    game.recipe_book_ui
                        .reset_for_closed_screen(&mut game.recipe_book);
                    game.recipe_book.reset_for_login();
                    game.player.entity_id = entity_id;
                    game.set_camera(None);
                    game.hardcore = hardcore;
                    game.online_mode = online_mode;
                    game.show_death_screen = show_death_screen;
                    // The fresh LocalPlayer starts from supplier defaults.
                    game.player.attributes = AttributeMap::player();
                    sync_player_attribute_mirrors(&mut game.player, |_| false);
                    // Vanilla `handleLogin` builds a new LocalPlayer on slot 0,
                    // with the server's SetHeldSlot to follow. Ours lives in the
                    // app-wide input state, so a reconnect would otherwise
                    // report the last session's slot.
                    self.input.set_selected_slot(0);
                    game.start_level_load();
                    // `startWaitingForNewLevel` swaps in the level-loading
                    // screen, replacing a configuration-phase dialog.
                    game.configuring = false;
                    game.server_dialog = None;
                    self.apply_cursor_grab(window, Some(game));
                }
                NetworkEvent::SecureChatEnforced { enforced } => {
                    // TODO: the `UNSECURE_SERVER_WARNING` toast, once per
                    // connection when this is false (`handleLogin`; 1.20.x
                    // `handleServerData`). 1.20.x sends a placeholder false
                    // from `login` first, so wait for its `server_data`.
                    game.server_enforces_secure_chat = enforced;
                }
                NetworkEvent::PlayerScore { entity_id, score } => {
                    if entity_id == game.player.entity_id {
                        game.player.score = score;
                    }
                }
                NetworkEvent::PlayerAbsorption {
                    entity_id,
                    absorption,
                } => {
                    if entity_id == game.player.entity_id {
                        game.player.absorption = absorption;
                    }
                }
                NetworkEvent::PlayerRespawned {
                    keep_entity_data,
                    keep_attribute_modifiers,
                } => {
                    if !keep_entity_data {
                        game.player.absorption = 0.0;
                    }
                    game.dead = false;
                    game.set_camera(None);
                    // `startWaitingForNewLevel` replaces an open dialog here too.
                    game.server_dialog = None;
                    game.start_level_load();
                    game.player.reset_for_respawn(keep_entity_data);
                    sync_respawn_attributes(&mut game.player, keep_attribute_modifiers);
                    game.interaction.reset_player_transients_for_respawn();
                    // A fresh LocalPlayer gets a fresh KeyboardInput and packet
                    // baselines even when bit 2 keeps its entity data, so a kept
                    // sprint flag re-sends START_SPRINTING next tick as vanilla does.
                    game.last_sent_input = PlayerInputState::default();
                    game.was_sprinting = false;
                    game.last_sent_pos = Position::default();
                    game.last_sent_look_dir = LookDirection::default();
                    game.last_sent_on_ground = false;
                    game.last_sent_horizontal_collision = false;
                    game.position_send_counter = 0;
                    game.reset_death_screen();
                    self.apply_cursor_grab(window, Some(game));
                }
                NetworkEvent::PlayerDied { player_id, message } => {
                    if player_id != game.player.entity_id {
                        continue;
                    }
                    match death_route(game.show_death_screen) {
                        DeathRoute::ShowDeathScreen => {
                            self.open_death_screen(connection, window, game, Some(message));
                        }
                        DeathRoute::Respawn => {
                            game.death_message = message;
                            game.reset_death_screen();
                            self.send_respawn(connection, game);
                        }
                    }
                }
                NetworkEvent::ResourcePackPush {
                    id,
                    url,
                    hash,
                    required,
                } => {
                    tracing::info!("Resource pack push: {id} url={url} required={required}");
                    let cache_dir = self.data_dirs.game_dir.join("resourcepacks/.server_cache");
                    self.pending_pack_download = Some(PendingPackDownload {
                        id,
                        required,
                        hash: hash.clone(),
                        handle: std::thread::spawn(move || {
                            download_server_pack(&cache_dir, &url, &hash)
                        }),
                    });
                }
                NetworkEvent::ResourcePackPop { id } => {
                    // A server may pop an id it already popped, or never pushed.
                    match self.remove_server_resource_packs(id, Renderer::prepare_resources) {
                        Ok(Some(prepared)) => {
                            self.reload_live_resource_assets(game, renderer, prepared);
                        }
                        Ok(None) => {}
                        Err(error) => {
                            tracing::warn!(%error, "Failed to pop server resource assets");
                        }
                    }
                }
                NetworkEvent::Reconfiguring => {
                    tracing::info!("Server re-entered configuration");
                    game.block_registry.clear_tags();
                    self.audio.stop_all_sounds();
                    game.entity_store = crate::entity::EntityStore::new();
                    game.item_entity_store = crate::entity::ItemEntityStore::new();
                    game.entity_positions.clear();
                    game.entity_kinds.clear();
                    game.vehicle_passengers.clear();
                    game.silent_entities.clear();
                    game.action_bar = None;
                    game.waypoints = crate::world::waypoints::WaypointMap::default();
                    // `ServerReconfigScreen` replaces any dialog; the server
                    // links carry over.
                    game.server_dialog = None;
                    game.configuring = true;
                    game.reset_sleep_for_level_teardown();
                    self.clear_server_ui(game, renderer);
                    self.apply_cursor_grab(window, Some(game));
                }
                NetworkEvent::Disconnected { reason } => {
                    tracing::warn!("Disconnected: {reason}");
                    disconnect_reason = Some(reason);
                    self.clear_server_ui(game, renderer);
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
            if let Some(started) = chunk_packet {
                chunk_work += started.elapsed();
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
                Ok(Ok(path)) => {
                    // Vanilla acknowledges SuccessfullyLoaded only after the
                    // resources have been applied. Rebuild the live renderer
                    // (including shared-atlas chunk geometry) before replying.
                    // A failed preflight leaves the pack unregistered and the
                    // old view active, as vanilla's `onFailure` drops it.
                    let reload = self
                        .activate_server_resource_pack(
                            pending.id,
                            &pending.hash,
                            path,
                            Renderer::prepare_resources,
                        )
                        .map(|prepared| {
                            self.reload_live_resource_assets(game, renderer, prepared);
                        });
                    match reload {
                        Ok(()) => {
                            tracing::info!("Resource pack {} loaded successfully", pending.id);
                            s_resource_pack::Action::SuccessfullyLoaded
                        }
                        Err(error) => {
                            tracing::error!(%error, id = %pending.id, "Resource pack reload failed");
                            if pending.required {
                                disconnect_reason =
                                    Some(format!("Required resource pack failed: {error}"));
                            }
                            s_resource_pack::Action::FailedReload
                        }
                    }
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
        }

        let player_chunk = game.player_chunk();
        enqueue_edits(game, &priority_remesh);

        // Refresh the frustum tiers (throttled to camera movement / new loads),
        // then enqueue everything that needs meshing — visible-first, with hidden
        // columns backfilled at a bounded rate so the world still completes.
        // New chunk loads mark themselves dirty when their queued light
        // applies (GameState::update_light), which raises this flag.
        let loads_happened = std::mem::take(&mut game.pending_load_rescan);
        let ms = |t: std::time::Instant| t.elapsed().as_secs_f32() * 1000.0;
        game.last_update_phases.net_decode_ms = ms(t_net);

        let t_vis = std::time::Instant::now();
        game.update_visibility(renderer, player_chunk, loads_happened);
        game.last_update_phases.visibility_ms = ms(t_vis);

        let t_rescan = std::time::Instant::now();
        game.rescan_mesh_jobs(self.menu.chunk_detail);
        game.last_update_phases.rescan_ms = ms(t_rescan);

        disconnect_reason
    }

    /// Vanilla `ClientPacketListener.tick`'s level-load half: advance the
    /// tracker, and the first tick the level is ready send `player_loaded`
    /// (`notifyPlayerLoaded`) and drop the tracker. Runs at the start of every
    /// client tick, in the loading phase as well as in game, so a respawn or a
    /// dimension change waits and reports again.
    pub fn tick_level_load(
        camera: glam::DVec3,
        connection: &ConnectionHandle,
        game: &mut GameState,
    ) {
        // Taken for the tick so the readiness inputs can borrow `game`; put
        // back below unless the level is ready, which is vanilla clearing
        // `levelLoadTracker`.
        let Some(mut tracker) = game.level_load.take() else {
            return;
        };
        let now = Instant::now();

        let min_y = game.chunk_store.min_y();
        let max_y = min_y + game.chunk_store.height() as i32 - 1;
        let outside_build_height = |y: i32| y < min_y || y > max_y;
        // Vanilla reads `gameRenderer.mainCamera().blockPosition()`.
        let camera_block = camera.floor().as_ivec3();

        tracker.tick_client_load(
            now,
            &ReadyInputs {
                // 26.3's `isLevelReady` checks only the camera's height.
                player_outside_build_height: crate::version::session_protocol() < 777
                    && outside_build_height(game.player.position.y.floor() as i32),
                camera_outside_build_height: outside_build_height(camera_block.y),
                spectator: crate::player::is_spectator(game.player.game_mode),
                alive: !game.dead,
                // Until the server's first position the camera sits at the
                // origin, whose section says nothing about where we spawn.
                player_section_ready: game.position_set && game.camera_section_ready(camera_block),
            },
        );

        if !tracker.is_level_ready(now) {
            game.level_load = Some(tracker);
            return;
        }

        game.client_loaded = true;
        connection
            .packet_tx
            .send(ServerboundGamePacket::PlayerLoaded(
                azalea_protocol::packets::game::s_player_loaded::ServerboundPlayerLoaded,
            ));
    }

    pub fn send_pong(connection: &ConnectionHandle, id: u32) {
        connection.packet_tx.send(ServerboundGamePacket::Pong(
            azalea_protocol::packets::game::s_pong::ServerboundPong { id },
        ));
    }

    /// Marks the end of the client tick (1.21.2+). Must be the last packet of
    /// the tick: servers and anti-cheat batch our movement between these to
    /// tick-align it, so omitting it makes them reject/rubber-band movement.
    /// Vanilla `Minecraft.tick` sends it for every tick a level exists,
    /// including the ones spent on the loading screen.
    pub fn send_client_tick_end(connection: &ConnectionHandle) {
        connection
            .packet_tx
            .send(ServerboundGamePacket::ClientTickEnd(
                s_client_tick_end::ServerboundClientTickEnd,
            ));
    }

    pub fn tick_physics(
        &mut self,
        renderer: &mut Renderer,
        connection: &ConnectionHandle,
        game: &mut GameState,
    ) {
        // `handleLogin` adds the local player before any remote entity, so
        // `ClientLevel.tickEntities` ticks it first and pushes land in next
        // tick's travel.
        // TODO: a same-dimension `handleRespawn` re-adds it after them.
        // A passenger ticks after its vehicle, so the player waits for the
        // remote entities when riding one it doesn't move itself.
        game.rider_yaw_turn = 0.0;
        if game.riding_vehicle_id.is_some() && game.steered_boat().is_none() {
            self.tick_remote_entities(game);
            self.tick_local_player(renderer, connection, game);
        } else {
            self.tick_local_player(renderer, connection, game);
            self.tick_remote_entities(game);
        }
        game.position_remote_riders();
        super::sniffer::tick(&mut self.audio, game, renderer.camera_render_position());
        super::copper_golem::tick(&self.audio, game);
        panda_eating_particles(game, renderer.camera_render_position());
    }

    /// `LivingEntity.pushEntities` from every remote living entity's tick,
    /// filtered by `EntitySelector.pushableBy` with the local player as target.
    fn tick_remote_entities(&self, game: &mut GameState) {
        let push_box = local_push_box(game);
        let local_name = self.user.username.as_str();
        let vehicle = game.riding_vehicle_id;
        let (scoreboard, tab_list) = (&game.scoreboard, &game.tab_list);
        let pusher_allowed = |id: i32, entity: &crate::entity::LivingEntity| {
            // `isPassengerOfSameVehicle`. TODO: co-passengers.
            if Some(id) == vehicle {
                return false;
            }
            let pusher_name = match entity.player_uuid {
                Some(uuid) => match tab_list.players.get(&uuid) {
                    // `Player.tick` sets `noPhysics` for spectators, so
                    // `Entity.push` returns before any impulse.
                    Some(info) if crate::player::is_spectator(info.game_mode) => return false,
                    Some(info) => info.name.clone(),
                    // TODO: keep the spawn-time profile name; a player
                    // dropped from the tab list falls back to no team.
                    None => return true,
                },
                None => entity.uuid.to_string(),
            };
            scoreboard.pushable_by(&pusher_name, local_name)
        };
        // Vanilla `Level.isThundering`.
        game.entity_store.thundering =
            game.environment.has_weather && f64::from(game.sky_state.thunder()) > 0.9;
        *game.player.velocity += game.entity_store.tick_living(
            &game.chunk_store,
            game.block_registry.block_state_table(),
            game.player.position,
            push_box,
            pusher_allowed,
            game.server_simulation_distance,
            game.controlled_vehicle_id,
        );
    }

    fn tick_local_player(
        &mut self,
        renderer: &mut Renderer,
        connection: &ConnectionHandle,
        game: &mut GameState,
    ) {
        if game.death_screen_open {
            if game.death_confirm {
                game.death_confirm_ticks = game.death_confirm_ticks.saturating_add(1);
            } else {
                game.death_screen_ticks = game.death_screen_ticks.saturating_add(1);
            }
        }

        // Vanilla ticks the camera FOV interpolation even while dead.
        renderer.set_base_fov(self.menu.fov as f32);
        let fov_effect_scale = self.menu.fov_effect();
        renderer.update_fov_mod(compute_fov_modifier(&game.player, fov_effect_scale));
        // TODO: lava camera fluid (no eyes_in_lava).
        renderer.set_fluid_fov_factor(if game.player.eyes_in_water {
            1.0_f32.lerp(0.857_142_87, fov_effect_scale)
        } else {
            1.0
        });

        // Vanilla `LocalPlayer.tick` returns immediately until the client has
        // loaded: no physics, no interaction, and no input, sprint or movement
        // packets while the level is still coming in.
        if !game.client_loaded {
            self.input.clear_click_counts();
            self.input.clear_just_pressed_actions();
            return;
        }

        // LocalPlayer.tickDeath removes the client player at tick 20; from then
        // on ClientLevel.tickEntities skips it entirely.
        if game.dead && game.player.death_animation_finished() {
            self.input.clear_click_counts();
            return;
        }

        // Vanilla ClientLevel snapshots old entity transform before every tick,
        // including dead-player ticks up through the removal tick.
        game.player.snapshot_render_state();
        if game.dead {
            game.player.tick_death();
            let removed_this_tick = game.player.death_animation_finished();
            let hand_stacks = game
                .player
                .inventory
                .hand_stacks(self.input.selected_slot())
                .map(Option::<&_>::cloned);
            game.interaction.tick_using_item(
                hand_stacks.each_ref().map(Option::as_ref),
                &connection.packet_tx,
                &self.audio,
                game.player.position.into(),
                game.player.eye_pos().into(),
                game.player.look_dir,
                &mut crate::player::interaction::BreakEffects {
                    particles: &mut game.particle_store,
                    registry: game.block_registry.render_view(),
                    biome_climate: &game.biome_climate,
                },
            );

            // LocalPlayer.tickDeath marks the player removed at exactly tick 20.
            // LivingEntity.tick then skips aiStep for that removal tick, while
            // LocalPlayer.tick still executes its post-super player state and
            // input/position packet tail once.
            if !removed_this_tick {
                let extra_boxes = game.collidable_entity_boxes();
                movement::tick_dead(
                    &mut game.player,
                    crate::physics::collision::CollisionWorld::with_block_entities(
                        &game.chunk_store,
                        game.block_registry.block_state_table(),
                        &game.block_entity_anim,
                    )
                    .with_extra_boxes(&extra_boxes),
                    game.block_registry.tags(),
                    game.riding_vehicle_id.is_some(),
                );
                crate::entity::stop_walk_animation(
                    &mut game.player_walk_pos,
                    &mut game.player_walk_speed,
                    &mut game.player_prev_walk_speed,
                );
                let dx = game.player.position.x - game.player.prev_position.x;
                let dz = game.player.position.z - game.player.prev_position.z;
                game.player.tick_bob(dx, dz, true);
                game.player
                    .tick_body_rotation(dx, dz, game.interaction.arm_swinging());
            }
            game.interaction
                .tick_dead_player_state(hand_stacks[0].as_ref(), !removed_this_tick);

            let neutral = InputState::released();
            Self::send_abilities_packet(connection, game);
            Self::send_input_packet(&neutral, connection, game);
            self.send_sprint_command(connection, game);
            self.send_position_packet(connection, game);

            // Q/F presses queued while dead must not fire on respawn.
            self.input.clear_click_counts();
            return;
        }

        // Open menus only release the keys; the simulation keeps ticking. The
        // chunk-load benchmark also freezes the player so every run measures the
        // same fixed origin.
        let input_live = game.input_live() && game.chunk_load_bench.is_none();

        // Vanilla Minecraft.handleKeybinds: the spectator-effect toggle,
        // offhand-swap then drop consume the queued key presses each tick;
        // spectators consume the last two without acting.
        if input_live {
            let spectator = game.player.game_mode == 3;
            while self
                .input
                .consume_click(Action::ToggleSpectatorShaderEffects)
            {
                game.spectator_effect_active = !game.spectator_effect_active;
            }
            while self.input.consume_click(Action::SwapOffhand) {
                if !spectator {
                    crate::player::interaction::send_swap_offhand(&connection.packet_tx);
                }
            }
            while self.input.consume_click(Action::DropItem) {
                let whole_stack = self.input.ctrl_held();
                if !spectator {
                    // Vanilla `LocalPlayer.drop` always sends the action packet;
                    // only the swing is gated on something actually dropping.
                    crate::player::interaction::send_drop(&connection.packet_tx, whole_stack);
                    if game
                        .player
                        .inventory
                        .remove_from_selected(self.input.selected_slot(), whole_stack)
                    {
                        crate::player::interaction::send_use_swing(
                            &connection.packet_tx,
                            crate::player::interaction::InteractionHand::MainHand,
                        );
                    }
                }
            }
            for slot in self.input.take_spectator_slot_presses() {
                if spectator {
                    game.spectator.on_hotbar_selected(
                        slot,
                        &game.tab_list,
                        &game.scoreboard,
                        &connection.packet_tx,
                    );
                }
            }
            while self.input.consume_click(Action::SpectatorHotbar) {
                if spectator {
                    game.spectator.on_hotbar_action_key(
                        &game.tab_list,
                        &game.scoreboard,
                        &connection.packet_tx,
                    );
                }
            }
        }
        // A press queued while a menu or chat was open must not fire later.
        self.input.clear_click_counts();

        let neutral = InputState::released();
        let input = if input_live { &self.input } else { &neutral };

        // Vanilla LocalPlayer.aiStep ride-jump charge. `was_jump_pressed`
        // still holds last tick's key here (movement::tick overwrites it
        // below), matching vanilla's wasJumping sampled before input.tick().
        // Equine jump cooldown is always 0, so the cooldown gate collapses
        // into the vehicle check.
        let jump_held = input.performing_action(Action::Jump);
        if game.riding_jumpable_vehicle() {
            let p = &mut game.player;
            if p.jump_riding_ticks < 0 {
                p.jump_riding_ticks += 1;
                if p.jump_riding_ticks == 0 {
                    p.jump_riding_scale = 0.0;
                }
            }
            if p.was_jump_pressed && !jump_held {
                p.jump_riding_ticks = -10;
                // Vanilla also calls vehicle.onPlayerJump() for client horse
                // physics; pomme has no vehicle physics, the server moves us.
                use azalea_protocol::packets::game::s_player_command as cmd;
                connection
                    .packet_tx
                    .send(ServerboundGamePacket::PlayerCommand(
                        cmd::ServerboundPlayerCommand {
                            id: azalea_core::entity_id::MinecraftEntityId(p.entity_id),
                            action: cmd::Action::StartRidingJump,
                            data: (p.jump_riding_scale * 100.0).floor() as u32,
                        },
                    ));
            } else if !p.was_jump_pressed && jump_held {
                p.jump_riding_ticks = 0;
                p.jump_riding_scale = 0.0;
            } else if p.was_jump_pressed {
                p.jump_riding_ticks += 1;
                p.jump_riding_scale = if p.jump_riding_ticks < 10 {
                    p.jump_riding_ticks as f32 * 0.1
                } else {
                    0.8 + 2.0 / (p.jump_riding_ticks - 9) as f32 * 0.1
                };
            }
        } else {
            // Vanilla keeps jumpRidingTicks; only the scale resets.
            game.player.jump_riding_scale = 0.0;
        }

        let camera_look = renderer.camera_look_dir();
        game.player.look_dir = if game.player.is_sleeping() {
            // `LivingEntity.tick` forces xRot to 0 while sleeping.
            LookDirection::new(camera_look.y_rot_deg(), 0.0)
        } else {
            camera_look
        };

        // Vanilla `Minecraft.tick`: pick and keybinds run before the entity
        // tick, so interaction packets precede the movement packet.
        let hand_stacks = game
            .player
            .inventory
            .hand_stacks(input.selected_slot())
            .map(Option::<&_>::cloned);
        let hand_stacks = hand_stacks.each_ref().map(Option::as_ref);
        let held_stack = hand_stacks[0];
        let creative = crate::player::is_creative(game.player.game_mode);
        let held_item = held_stack
            .as_ref()
            .map(|data| crate::player::inventory::item_resource_name(data.kind));
        game.interaction.update_target(
            &game.block_registry,
            game.player.eye_pos(),
            game.player.look_dir,
            &game.chunk_store,
            &game.entity_store,
            &game.player.attributes,
            creative,
            held_item.as_deref(),
        );
        let legacy_mining = crate::tool::legacy_mining(
            held_stack,
            game.player
                .inventory
                .armor_slots()
                .iter()
                .filter_map(|stack| stack.as_present()),
            game.block_registry.protocol(),
        );
        let hands = hand_stacks.map(|stack| crate::player::interaction::HandItem {
            stack,
            place_block: stack.and_then(|s| {
                game.block_registry
                    .placeable_block_for_item(&crate::player::inventory::item_resource_name(s.kind))
            }),
            worn: worn_for(
                stack,
                &game.player.inventory,
                input.selected_slot(),
                creative,
                &game.registries,
            ),
        });
        let player_aabb = game.player.bounding_box();
        let mut effects = crate::player::interaction::BreakEffects {
            particles: &mut game.particle_store,
            registry: game.block_registry.render_view(),
            biome_climate: &game.biome_climate,
        };
        let dirty = game.interaction.tick_actions(
            &game.block_registry,
            input,
            &game.chunk_store,
            &connection.packet_tx,
            &self.audio,
            game.player.position.into(),
            player_aabb,
            game.player.eye_pos().into(),
            game.player.look_dir,
            game.player.on_ground,
            creative,
            game.player.food,
            input.selected_slot(),
            hands,
            &game.player.attributes,
            &game.player.effects,
            game.player.eyes_in_water,
            legacy_mining,
            &mut effects,
        );
        game.interaction.tick_using_item(
            hand_stacks,
            &connection.packet_tx,
            &self.audio,
            game.player.position.into(),
            game.player.eye_pos().into(),
            game.player.look_dir,
            &mut effects,
        );

        if game.chunk_load_bench.is_some() {
            game.player.velocity = crate::entity::components::Velocity::new(0.0, 0.0, 0.0);
        }
        // The vehicle ticks before its passenger.
        Self::tick_steered_boat(game, connection);
        let vehicle = game.riding_vehicle_id.map(|_| movement::Vehicle {
            jumpable: game.riding_jumpable_vehicle(),
        });
        let extra_boxes = game.collidable_entity_boxes();
        movement::tick(
            &mut game.player,
            input,
            crate::physics::collision::CollisionWorld::with_block_entities(
                &game.chunk_store,
                game.block_registry.block_state_table(),
                &game.block_entity_anim,
            )
            .with_extra_boxes(&extra_boxes),
            game.block_registry.tags(),
            game.interaction.use_speed_multiplier(),
            game.interaction.slow_due_to_using_item(),
            vehicle,
        );
        let dx = game.player.position.x - game.player.prev_position.x;
        let dz = game.player.position.z - game.player.prev_position.z;
        crate::entity::update_walk_animation(
            dx,
            0.0,
            dz,
            &mut game.player_walk_pos,
            &mut game.player_walk_speed,
            &mut game.player_prev_walk_speed,
        );
        game.player.tick_bob(dx, dz, false);
        game.player
            .tick_body_rotation(dx, dz, game.interaction.arm_swinging());
        game.player.drop_spam.tick();

        // The rest of `LocalPlayer.rideTick`: seated, turned with a boat, and
        // its keys left for the boat's next tick (`AbstractBoat.setInput`).
        // TODO: `handsBusy`, which the steering keys set.
        game.position_local_rider();
        if game.rider_yaw_turn != 0.0 {
            renderer.set_camera_look_dir(game.player.look_dir);
        }
        if let Some(boat) = game
            .steered_boat()
            .and_then(|id| game.entity_store.living.get_mut(&id))
        {
            boat.boat_sim.input = crate::entity::boat::BoatInput {
                left: input.action_held(Action::StrafeLeft),
                right: input.action_held(Action::StrafeRight),
                up: input.action_held(Action::Forward),
                down: input.action_held(Action::Backward),
            };
        }

        game.interaction
            .tick_player_state(input.is_cursor_captured(), held_stack);

        Self::send_abilities_packet(connection, game);
        Self::send_input_packet(input, connection, game);
        if game.riding_vehicle_id.is_some() {
            self.send_passenger_packets(connection, game);
        } else {
            self.send_sprint_command(connection, game);
            self.send_position_packet(connection, game);
        }
        if !dirty.is_empty() {
            let min_y = game.chunk_store.min_y();
            let n = game.chunk_store.section_count();
            let mut sections: Vec<(crate::position::ChunkPos, i32)> = Vec::new();
            for b in dirty {
                // Light lands in this frame's update_light, matching vanilla's
                // prediction timing (setBlockState queues; the per-frame
                // ClientLevel.update drains).
                game.light_engine.block_dirty(b.x, b.y, b.z);
                dirty_sections_for_block(&mut sections, b.x, b.y, b.z, min_y, n);
            }
            // One span per column so each column builds its mesh snapshot
            // once (the sections of an edit are contiguous per column).
            let mut spans: Vec<(crate::position::ChunkPos, i32, i32)> = Vec::new();
            for (col, si) in sections {
                match spans.iter_mut().find(|(c, ..)| *c == col) {
                    Some((_, lo, hi)) => {
                        *lo = (*lo).min(si);
                        *hi = (*hi).max(si);
                    }
                    None => spans.push((col, si, si)),
                }
            }
            for (col, lo, hi) in spans {
                game.mesh_sections_edit_now(renderer, col, lo..hi + 1);
            }
        }

        // Menus consume their own clicks later in the frame, so only clear
        // them when the simulation saw the live input.
        if input_live {
            self.input.clear_just_pressed_actions();
        }
    }

    // Vanilla onUpdateAbilities: report a locally toggled `flying` to the
    // server, which gates it on mayfly and corrects us via the clientbound
    // abilities packet if rejected.
    fn send_abilities_packet(connection: &ConnectionHandle, game: &mut GameState) {
        if game.player.abilities_dirty {
            connection
                .packet_tx
                .send(ServerboundGamePacket::PlayerAbilities(
                azalea_protocol::packets::game::s_player_abilities::ServerboundPlayerAbilities {
                    is_flying: game.player.flying,
                },
            ));
            game.player.abilities_dirty = false;
        }
    }

    fn send_input_packet(input: &InputState, connection: &ConnectionHandle, game: &mut GameState) {
        let sender = &connection.packet_tx;

        let analog_move = input.analog_movement_axes().unwrap_or(glam::Vec2::ZERO);

        let current =
            player_input_state(input, analog_move, input.performing_action(Action::Sprint));

        if current != game.last_sent_input {
            sender.send(ServerboundGamePacket::PlayerInput(
                serverbound_player_input(&current),
            ));
            game.last_sent_input = current;
        }
    }

    /// `ServerboundPlayerCommandPacket(player, action)`.
    fn send_player_command(
        &self,
        connection: &ConnectionHandle,
        game: &GameState,
        action: azalea_protocol::packets::game::s_player_command::Action,
    ) {
        connection
            .packet_tx
            .send(ServerboundGamePacket::PlayerCommand(
                azalea_protocol::packets::game::s_player_command::ServerboundPlayerCommand {
                    id: azalea_core::entity_id::MinecraftEntityId(game.player.entity_id),
                    action,
                    data: 0,
                },
            ));
    }

    pub fn send_sprint_command(&self, connection: &ConnectionHandle, game: &mut GameState) {
        let sprinting = game.player.sprinting;
        if sprinting != game.was_sprinting {
            let action = if sprinting {
                azalea_protocol::packets::game::s_player_command::Action::StartSprinting
            } else {
                azalea_protocol::packets::game::s_player_command::Action::StopSprinting
            };
            self.send_player_command(connection, game, action);
            game.was_sprinting = sprinting;
        }
    }

    /// Vanilla InBedChatScreen: leaving bed sends PlayerCommand STOP_SLEEPING.
    pub fn send_stop_sleeping(&self, connection: &ConnectionHandle, game: &GameState) {
        self.send_player_command(
            connection,
            game,
            azalea_protocol::packets::game::s_player_command::Action::StopSleeping,
        );
    }

    /// Vanilla `AbstractBoat.tick` for the boat the player steers, which
    /// sends `PaddleBoat` from its tick.
    fn tick_steered_boat(game: &mut GameState, connection: &ConnectionHandle) {
        let Some(id) = game.steered_boat() else {
            return;
        };
        let colliders = game.boat_colliders(id);
        let Some(boat) = game.entity_store.living.get_mut(&id) else {
            return;
        };
        let [left, right] = crate::entity::boat::tick_controlled(
            boat,
            &colliders,
            &game.chunk_store,
            game.block_registry.block_state_table(),
            game.block_registry.tags(),
        );
        connection
            .packet_tx
            .send_raw(pomme_protocol::wire::encode_paddle_boat(left, right));
    }

    /// `LocalPlayer.sendChanges` for a passenger: its look, then the vehicle
    /// it moves and its sprint.
    fn send_passenger_packets(&self, connection: &ConnectionHandle, game: &mut GameState) {
        use azalea_protocol::common::movements::MoveFlags;
        use azalea_protocol::packets::game::*;
        connection
            .packet_tx
            .send(ServerboundGamePacket::MovePlayerRot(
                ServerboundMovePlayerRot {
                    look_direction: game.player.look_dir.into(),
                    flags: MoveFlags {
                        on_ground: game.player.on_ground,
                        horizontal_collision: game.player.horizontal_collision,
                    },
                },
            ));
        if let Some(boat) = game
            .steered_boat()
            .and_then(|id| game.entity_store.living.get(&id))
        {
            connection.packet_tx.send_raw(move_vehicle_packet(boat));
            self.send_sprint_command(connection, game);
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

/// Where a dead local player goes: the death screen, or straight to a respawn
/// when the server disabled the screen (`showDeathScreen`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DeathRoute {
    ShowDeathScreen,
    Respawn,
}

pub(crate) fn death_route(show_death_screen: bool) -> DeathRoute {
    if show_death_screen {
        DeathRoute::ShowDeathScreen
    } else {
        DeathRoute::Respawn
    }
}

/// Vanilla `ChatListener.evaluateTrustLevel` and `ChatTrustLevel.createTag`.
/// Only the integrated server's own player skips evaluation.
pub(crate) fn accepted_player_chat_tag(
    local_sender: bool,
    signed: bool,
    body: &crate::net::chat_security::SignedChatBody,
    now_ms: u64,
    only_secure: bool,
    bindings: &crate::app::input::KeyBindings,
) -> Option<crate::ui::chat::ChatMessageTag> {
    if local_sender {
        return None;
    }
    let expired = now_ms > body.timestamp_ms.max(0) as u64 + 7 * 60 * 1000;
    if !signed || expired {
        return Some(crate::ui::chat::ChatMessageTag::NotSecure);
    }
    let decorated = if only_secure {
        &body.decorated_signed_content
    } else {
        &body.decorated_content
    };
    let modified = body.modified_style
        || !decorated
            .plain_text_with_bindings(bindings)
            .contains(&body.content);
    modified.then(|| crate::ui::chat::ChatMessageTag::Modified {
        original: body.content.clone(),
    })
}

fn sync_respawn_attributes(player: &mut LocalPlayer, keep_attribute_modifiers: bool) {
    if keep_attribute_modifiers {
        // `assignAllValues` dirties every instance.
        sync_player_attribute_mirrors(player, |_| true);
    } else {
        // Vanilla constructs a fresh player attribute map, then copies only
        // old base values into it; only a base that differs from the
        // supplier default dirties its instance.
        player.attributes.clear_modifiers();
        let dirty: Vec<AttributeKind> = AttributeKind::ALL
            .into_iter()
            .filter(|&kind| {
                player
                    .attributes
                    .instance(kind)
                    .map(AttributeInstance::base_value)
                    != kind.player_base_value()
            })
            .collect();
        sync_player_attribute_mirrors(player, |kind| dirty.contains(&kind));
    }
}

/// Refresh the fields current consumers read from an attribute map. `dirty`
/// gates vanilla `LivingEntity.onAttributeUpdated`'s clamps, which only run
/// for instances that changed.
fn sync_living_attribute_mirrors(
    attributes: &AttributeMap,
    health: &mut f32,
    max_health: &mut f32,
    camera_distance: &mut f32,
    dirty: impl Fn(AttributeKind) -> bool,
) {
    if let Some(value) = attributes.value(AttributeKind::MaxHealth) {
        *max_health = value as f32;
        if dirty(AttributeKind::MaxHealth) {
            *health = health.min(*max_health);
        }
    }
    if let Some(value) = attributes.value(AttributeKind::CameraDistance) {
        *camera_distance = value as f32;
    }
    // TODO: SCALE refreshes dimensions in vanilla once Pomme consumes it.
}

fn sync_player_attribute_mirrors(player: &mut LocalPlayer, dirty: impl Fn(AttributeKind) -> bool) {
    let attributes = &player.attributes;
    sync_living_attribute_mirrors(
        attributes,
        &mut player.health,
        &mut player.max_health,
        &mut player.camera_distance,
        &dirty,
    );
    if let Some(value) = attributes.value(AttributeKind::Armor) {
        player.armor = value.floor() as u32;
    }
    if dirty(AttributeKind::MaxAbsorption)
        && let Some(value) = attributes.value(AttributeKind::MaxAbsorption)
    {
        player.absorption = player.absorption.min(value as f32);
    }
}

/// LOD level for a column: full detail within `detail` chunks (the Chunk
/// Detail setting), half resolution to twice that, quarter beyond. Off keeps
/// every column at full detail.
pub(crate) fn chunk_lod(
    pos: crate::position::ChunkPos,
    player: crate::position::ChunkPos,
    detail: u32,
) -> u32 {
    if detail >= crate::ui::menu::CHUNK_DETAIL_OFF {
        return 0;
    }
    let dx = (pos.x - player.x).unsigned_abs();
    let dz = (pos.z - player.z).unsigned_abs();
    let dist = dx.max(dz);
    if dist <= detail {
        0
    } else if dist <= detail * 2 {
        1
    } else {
        2
    }
}

/// The [`Worn`](crate::player::interaction::Worn) for `stack`, if it's a
/// swappable equippable.
fn worn_for<'a>(
    stack: Option<&azalea_inventory::ItemStackData>,
    inventory: &'a crate::player::inventory::Inventory,
    selected: u8,
    creative: bool,
    registries: &azalea_core::registry_holder::RegistryHolder,
) -> crate::player::interaction::Worn<'a> {
    use azalea_core::registry_holder::components::PreventArmorChange;
    use azalea_inventory::components::{Enchantments, Equippable};
    use azalea_registry::DataRegistry;

    let Some(equippable) = stack
        .and_then(|s| s.get_component::<Equippable>())
        .filter(|e| e.swappable)
    else {
        return Default::default();
    };
    let worn = inventory.equipped(equippable.slot, selected);
    let locked = !creative
        && worn
            .and_then(|w| w.get_component::<Enchantments>())
            .is_some_and(|e| {
                e.levels.keys().any(|enchantment| {
                    registries
                        .enchantment
                        .map
                        .get_index(enchantment.protocol_id() as usize)
                        .is_some_and(|(_, data)| data.get::<PreventArmorChange>().is_some())
                })
            });
    crate::player::interaction::Worn {
        stack: worn,
        locked,
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

#[cfg(test)]
mod tests {
    use super::{
        CHUNK_PACKET_BUDGET, CursorOp, DeathRoute, HeadProfile, MENU_REPEAT_DELAY,
        MENU_REPEAT_INTERVAL, RepeatStepper, accepted_player_chat_tag, cursor_step, death_route,
        player_input_state, resolve_corrected_look, resolve_head_profile,
        resolve_player_position_correction, serverbound_player_input,
        sync_living_attribute_mirrors, sync_player_attribute_mirrors, sync_respawn_attributes,
    };
    use crate::app::input::{InputState, gamepad_movement_axes};
    use crate::attribute::{
        AttributeKind, AttributeMap, AttributeModifier, AttributeModifierOperation,
        AttributeSnapshot,
    };
    use crate::entity::components::{LookDirection, Position, Velocity};
    use crate::net::chat_security::SignedChatBody;
    use crate::player::tab_list::{PlayerInfoEntry, TabList};
    use crate::player::{LocalPlayer, valid_player_name};
    use crate::ui::chat::ChatMessageTag;

    fn tab_list_with(uuid: uuid::Uuid, name: &str, textures: Option<&str>) -> TabList {
        let mut tab_list = TabList::new();
        tab_list.apply_update(
            &pomme_protocol::player_info::Actions {
                add_player: true,
                ..Default::default()
            },
            &[PlayerInfoEntry {
                uuid,
                name: name.to_owned(),
                textures: textures.map(str::to_owned),
                game_mode: 0,
                listed: true,
                latency: 0,
                display_name: None,
                list_order: 0,
                chat_session: None,
            }],
        );
        tab_list
    }

    #[test]
    fn packets_wait_only_on_a_frame_that_owes_no_tick() {
        use crate::app::TICK_RATE;
        let spent = CHUNK_PACKET_BUDGET;
        assert!(!super::defer_packets(spent / 2, 0.0));
        assert!(super::defer_packets(spent, 0.9 * TICK_RATE));
        assert!(!super::defer_packets(spent, TICK_RATE));
        assert!(!super::defer_packets(spent * 10, 3.0 * TICK_RATE));
    }

    #[test]
    fn player_names_follow_the_codec_rules() {
        assert!(valid_player_name("Dinnerbone"));
        assert!(valid_player_name("a_1234567890_bcd"));
        // 17 characters, a space, a control character and a non-ASCII one.
        assert!(!valid_player_name("a_1234567890_bcde"));
        assert!(!valid_player_name("two words"));
        assert!(!valid_player_name("tab	here"));
        assert!(!valid_player_name("héllo"));
    }

    #[test]
    fn profiles_resolve_like_resolvable_profile() {
        let uuid = uuid::Uuid::from_u128(1);
        let tab_list = tab_list_with(uuid, "Alex", Some("packed"));

        // Dynamic: one of name/id and no properties, so the tab list answers
        // first — by name, ignoring case.
        let by_name = HeadProfile {
            id: None,
            name: Some("alex".to_owned()),
            textures: None,
            patch_texture: None,
        };
        assert!(!by_name.is_static());
        let resolved = resolve_head_profile(by_name, &tab_list);
        assert_eq!(resolved.id, Some(uuid));
        assert_eq!(resolved.textures.as_deref(), Some("packed"));

        let by_id = HeadProfile {
            id: Some(uuid),
            name: None,
            textures: None,
            patch_texture: None,
        };
        assert_eq!(
            resolve_head_profile(by_id, &tab_list).textures.as_deref(),
            Some("packed")
        );

        // A name nobody in the list has keeps its own identity for the
        // Mojang lookup.
        let stranger = HeadProfile {
            id: None,
            name: Some("Notch".to_owned()),
            textures: None,
            patch_texture: None,
        };
        assert_eq!(resolve_head_profile(stranger.clone(), &tab_list), stranger);

        // Static: both, neither, or properties present, and never resolved.
        let both = HeadProfile {
            id: Some(uuid),
            name: Some("Alex".to_owned()),
            textures: None,
            patch_texture: None,
        };
        assert!(both.is_static());
        assert_eq!(resolve_head_profile(both.clone(), &tab_list), both);
        assert!(
            HeadProfile {
                id: None,
                name: None,
                textures: None,
                patch_texture: None,
            }
            .is_static()
        );
        assert!(
            HeadProfile {
                id: None,
                name: Some("Alex".to_owned()),
                textures: Some("packed".to_owned()),
                patch_texture: None,
            }
            .is_static()
        );
    }

    /// Total steps from holding `dir` for `seconds` at a fixed frame time.
    fn held_steps(dir: i32, seconds: f32, dt: f32) -> i32 {
        let mut stepper = RepeatStepper::default();
        let mut total = 0;
        let mut elapsed = 0.0;
        while elapsed < seconds {
            total += stepper.step(dir, dt);
            elapsed += dt;
        }
        total
    }

    #[test]
    fn dpad_press_steps_once_then_waits_for_the_repeat_delay() {
        let dt = 1.0 / 60.0;
        // Only the press itself until the delay is up, then repeats.
        assert_eq!(held_steps(1, MENU_REPEAT_DELAY - dt, dt), 1);
        assert!(held_steps(1, MENU_REPEAT_DELAY + MENU_REPEAT_INTERVAL * 2.0, dt) > 1);
    }

    /// The regression this guards: reading the DPad as level state stepped
    /// once per rendered frame, so a second of holding gave 60 steps at 60fps
    /// and 300 at 300fps. Sampling a fixed repeat at different frame times
    /// still lands a step either side of a boundary, hence the tolerance.
    #[test]
    fn dpad_repeat_rate_does_not_depend_on_the_framerate() {
        // One press plus a second minus the delay, repeating at the interval.
        let expected = 1 + ((1.0 - MENU_REPEAT_DELAY) / MENU_REPEAT_INTERVAL) as i32;
        for dt in [1.0 / 30.0, 1.0 / 60.0, 1.0 / 144.0, 1.0 / 300.0] {
            let steps = held_steps(1, 1.0, dt);
            assert!(
                (steps - expected).abs() <= 1,
                "{steps} steps in a second at dt {dt}, expected about {expected}"
            );
        }
    }

    #[test]
    fn releasing_re_arms_the_delay_and_a_flip_steps_immediately() {
        let dt = 1.0 / 60.0;
        let mut stepper = RepeatStepper::default();
        assert_eq!(stepper.step(1, dt), 1);
        assert_eq!(stepper.step(0, dt), 0);
        assert_eq!(stepper.step(1, dt), 1);
        // Right to Left with no neutral frame between.
        assert_eq!(stepper.step(-1, dt), -1);
        assert_eq!(stepper.step(-1, dt), 0);
    }

    #[test]
    fn controller_packet_directions_match_physical_stick_direction() {
        let input = InputState::released();

        let right_state =
            player_input_state(&input, gamepad_movement_axes(glam::vec2(1.0, 0.0)), false);
        let right = serverbound_player_input(&right_state);
        assert!(right.right);
        assert!(!right.left);

        let left_state =
            player_input_state(&input, gamepad_movement_axes(glam::vec2(-1.0, 0.0)), false);
        let left = serverbound_player_input(&left_state);
        assert!(left.left);
        assert!(!left.right);

        let forward_right_state =
            player_input_state(&input, gamepad_movement_axes(glam::vec2(0.8, 0.8)), false);
        let forward_right = serverbound_player_input(&forward_right_state);
        assert!(forward_right.forward);
        assert!(forward_right.right);
        assert!(!forward_right.backward);
        assert!(!forward_right.left);
    }

    #[test]
    fn cursor_never_grabs_while_unfocused() {
        assert_eq!(
            cursor_step(false, false, true, false),
            (CursorOp::Keep, false, false)
        );
        assert_eq!(
            cursor_step(true, true, true, false),
            (CursorOp::Keep, true, true)
        );
    }

    #[test]
    fn cursor_calls_are_idempotent() {
        assert_eq!(
            cursor_step(false, false, false, true),
            (CursorOp::Keep, false, false)
        );
        assert_eq!(
            cursor_step(true, false, true, true),
            (CursorOp::Keep, true, false)
        );
    }

    #[test]
    fn cursor_centers_once_on_grabbed_to_released() {
        let (op, grabbed, stale) = cursor_step(true, false, false, true);
        assert_eq!(op, CursorOp::Release { center: true });
        assert_eq!(
            cursor_step(grabbed, stale, false, true),
            (CursorOp::Keep, false, false)
        );
    }

    #[test]
    fn stale_cursor_reissues_without_warping_a_free_cursor() {
        // Refocus while gameplay holds the grab: the WM may have dropped the
        // lock, so it is re-locked.
        assert_eq!(
            cursor_step(true, true, true, true),
            (CursorOp::Grab, true, false)
        );
        // Refocus on a screen: the release is re-issued without a warp.
        assert_eq!(
            cursor_step(false, true, false, true),
            (CursorOp::Release { center: false }, false, false)
        );
    }

    #[test]
    fn death_route_follows_show_death_screen() {
        assert_eq!(death_route(true), DeathRoute::ShowDeathScreen);
        assert_eq!(death_route(false), DeathRoute::Respawn);
    }

    fn chat_body() -> SignedChatBody {
        SignedChatBody {
            content: "hello".to_owned(),
            timestamp_ms: 1_000,
            salt: 0,
            last_seen: Vec::new(),
            message_index: 0,
            decorated_content: Box::new(crate::chat_component::Component::text("hello")),
            decorated_signed_content: Box::new(crate::chat_component::Component::text("hello")),
            modified_style: true,
            fully_filtered: false,
        }
    }

    #[test]
    fn integrated_server_own_chat_is_always_secure() {
        let body = chat_body();
        let bindings = crate::app::input::KeyBindings::default();
        let expired_now = 1_000 + 8 * 60 * 1000;
        assert_eq!(
            accepted_player_chat_tag(true, false, &body, expired_now, false, &bindings),
            None
        );
        assert_eq!(
            accepted_player_chat_tag(true, true, &body, expired_now, false, &bindings),
            None
        );
        assert_eq!(
            accepted_player_chat_tag(true, true, &body, 1_001, true, &bindings),
            None
        );

        assert_eq!(
            accepted_player_chat_tag(false, false, &body, 1_001, false, &bindings),
            Some(ChatMessageTag::NotSecure)
        );
        assert_eq!(
            accepted_player_chat_tag(false, true, &body, expired_now, false, &bindings),
            Some(ChatMessageTag::NotSecure)
        );
        assert_eq!(
            accepted_player_chat_tag(false, true, &body, 1_001, true, &bindings),
            Some(ChatMessageTag::Modified {
                original: "hello".to_owned(),
            })
        );
    }

    #[test]
    fn correction_pitch_clamps_before_entity_setter_modulo() {
        let current = LookDirection::new(30.0, 80.0);

        let absolute = resolve_corrected_look(current, 720.0, 360.0, false, false);
        assert_eq!(absolute.y_rot_deg(), 720.0);
        assert_eq!(absolute.x_rot_deg(), 90.0);

        let relative = resolve_corrected_look(current, 400.0, 50.0, true, true);
        assert_eq!(relative.y_rot_deg(), 430.0);
        assert_eq!(relative.x_rot_deg(), 90.0);
    }

    #[test]
    fn mounted_player_position_correction_is_acknowledged_without_local_application() {
        use pomme_protocol::entity::{PositionMoveRotation, Relative};

        let mut player = LocalPlayer::new();
        player.position = Position::new(1.0, 2.0, 3.0);
        player.prev_position = Position::new(0.5, 1.5, 2.5);
        player.velocity = Velocity::new(0.1, 0.2, 0.3);
        player.look_dir = LookDirection::new(10.0, 20.0);
        player.prev_look_dir = LookDirection::new(5.0, 15.0);

        let change = PositionMoveRotation {
            position: glam::DVec3::new(100.0, 200.0, 300.0),
            delta: glam::DVec3::new(1.0, 2.0, 3.0),
            y_rot: 90.0,
            x_rot: 45.0,
        };
        let relative = Relative::default();

        let unmounted = resolve_player_position_correction(&player, &change, &relative, false)
            .expect("unmounted correction applies");
        assert_eq!(unmounted.position, Position::new(100.0, 200.0, 300.0));
        assert_eq!(unmounted.velocity, Velocity::new(1.0, 2.0, 3.0));
        assert_eq!(unmounted.look_dir, LookDirection::new(90.0, 45.0));

        assert!(
            resolve_player_position_correction(&player, &change, &relative, true).is_none(),
            "vanilla mounted clients acknowledge the teleport but keep current local PosRot"
        );
    }

    #[test]
    fn player_input_packet_uses_physical_sprint_key_state() {
        let input = InputState::released();
        let pressed = serverbound_player_input(&player_input_state(&input, glam::Vec2::ZERO, true));
        let released =
            serverbound_player_input(&player_input_state(&input, glam::Vec2::ZERO, false));
        assert!(pressed.sprint);
        assert!(!released.sprint);
    }

    #[test]
    fn max_health_updates_clamp_current_health() {
        let mut attributes = AttributeMap::player();
        let (mut health, mut max_health, mut camera_distance) = (18.0, 20.0, 4.0);
        let mut set_max_health = |base, dirty: bool| {
            assert!(attributes.apply_snapshot(&AttributeSnapshot {
                attribute: AttributeKind::MaxHealth,
                base,
                modifiers: vec![],
            }));
            sync_living_attribute_mirrors(
                &attributes,
                &mut health,
                &mut max_health,
                &mut camera_distance,
                |_| dirty,
            );
            (health, max_health)
        };
        assert_eq!(set_max_health(10.0, false), (18.0, 10.0));
        assert_eq!(set_max_health(10.0, true), (10.0, 10.0));
        assert_eq!(set_max_health(30.0, true), (10.0, 30.0));
    }

    #[test]
    fn respawn_sync_applies_attributes_after_player_reset() {
        let mut player = LocalPlayer::new();
        assert!(player.attributes.apply_snapshot(&AttributeSnapshot {
            attribute: AttributeKind::Armor,
            base: 6.0,
            modifiers: vec![AttributeModifier {
                id: "minecraft:test_armor".into(),
                amount: 2.0,
                operation: AttributeModifierOperation::Value,
            }],
        }));
        assert!(player.attributes.apply_snapshot(&AttributeSnapshot {
            attribute: AttributeKind::MaxHealth,
            base: 10.0,
            modifiers: vec![AttributeModifier {
                id: "minecraft:test_health".into(),
                amount: 4.0,
                operation: AttributeModifierOperation::Value,
            }],
        }));

        player.reset_for_respawn(false);
        sync_respawn_attributes(&mut player, true);
        assert_eq!(player.armor, 8);
        assert_eq!(player.max_health, 14.0);
        assert_eq!(player.health, 14.0);

        player.reset_for_respawn(false);
        sync_respawn_attributes(&mut player, false);
        assert_eq!(player.armor, 6);
        assert_eq!(player.max_health, 10.0);
        assert_eq!(player.health, 10.0);
    }

    #[test]
    fn max_absorption_update_clamps_absorption() {
        let mut player = LocalPlayer::new();
        player.absorption = 8.0;
        assert!(player.attributes.apply_snapshot(&AttributeSnapshot {
            attribute: AttributeKind::MaxAbsorption,
            base: 4.0,
            modifiers: vec![],
        }));
        sync_player_attribute_mirrors(&mut player, |_| false);
        assert_eq!(player.absorption, 8.0);
        sync_player_attribute_mirrors(&mut player, |kind| kind == AttributeKind::MaxAbsorption);
        assert_eq!(player.absorption, 4.0);
    }

    #[test]
    fn chunk_detail_off_keeps_every_column_at_full_detail() {
        use crate::position::ChunkPos;
        let player = ChunkPos::new(0, 0);
        let far = ChunkPos::new(100, -100);
        assert_eq!(super::chunk_lod(far, player, 8), 2);
        assert_eq!(
            super::chunk_lod(far, player, crate::ui::menu::CHUNK_DETAIL_OFF),
            0
        );
    }
}
