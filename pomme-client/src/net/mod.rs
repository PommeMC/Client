#[cfg(test)]
mod azalea_compat;
pub mod commands;
pub mod connection;
pub mod handler;
pub mod resolve;
pub mod sender;
pub mod translate;

use std::sync::Arc;

use azalea_block::BlockState;
use azalea_core::position::{BlockPos, ChunkPos};
use azalea_inventory::ItemStack;
use azalea_registry::builtin::{BlockEntityKind, EntityKind};
use glam::DVec3;
use simdnbt::owned::NbtCompound;

use crate::entity::components::Position;
use crate::entity::villager::{VillagerKind, VillagerProfession};

/// A packet's per-column light payload (chunk load or standalone update):
/// present sections listed in `*_updates`, selected by `*_y_mask`, with
/// `empty_*_y_mask` marking explicitly-zero sections.
pub struct PacketLightData {
    pub sky_updates: Arc<Box<[Box<[u8]>]>>,
    pub block_updates: Arc<Box<[Box<[u8]>]>>,
    pub sky_y_mask: azalea_core::bitset::BitSet,
    pub block_y_mask: azalea_core::bitset::BitSet,
    pub empty_sky_y_mask: azalea_core::bitset::BitSet,
    pub empty_block_y_mask: azalea_core::bitset::BitSet,
}

impl From<&azalea_protocol::packets::game::c_light_update::ClientboundLightUpdatePacketData>
    for PacketLightData
{
    fn from(
        data: &azalea_protocol::packets::game::c_light_update::ClientboundLightUpdatePacketData,
    ) -> Self {
        Self {
            sky_updates: data.sky_updates.clone(),
            block_updates: data.block_updates.clone(),
            sky_y_mask: data.sky_y_mask.clone(),
            block_y_mask: data.block_y_mask.clone(),
            empty_sky_y_mask: data.empty_sky_y_mask.clone(),
            empty_block_y_mask: data.empty_block_y_mask.clone(),
        }
    }
}

pub enum NetworkEvent {
    Connected,
    Registries(Arc<azalea_core::registry_holder::RegistryHolder>),
    BiomeColors {
        colors: std::collections::HashMap<u32, crate::renderer::chunk::mesher::BiomeClimate>,
    },
    DimensionInfo {
        height: u32,
        min_y: i32,
        has_skylight: bool,
    },
    ChunkLoaded {
        pos: ChunkPos,
        /// Parsed on the net task so bursts don't spike the main thread.
        chunk: Box<azalea_world::chunk::Chunk>,
        light: PacketLightData,
        /// Sky-source heightmap, scanned on the net task for the same reason.
        sky_sources: crate::world::light::ChunkSkyLightSources,
    },
    /// Standalone server light correction (`ClientboundLightUpdate`).
    LightUpdate {
        pos: ChunkPos,
        light: PacketLightData,
    },
    ChunkUnloaded {
        pos: ChunkPos,
    },
    ChunkCacheCenter {
        x: i32,
        z: i32,
    },
    PlayerPosition {
        change: azalea_protocol::common::movements::PositionMoveRotation,
        relative: azalea_protocol::common::movements::RelativeMovements,
    },
    PlayerHealth {
        health: f32,
        food: u32,
        saturation: f32,
    },
    PlayerExperience {
        progress: f32,
        level: i32,
    },
    Waypoint {
        operation: azalea_protocol::packets::game::c_waypoint::WaypointOperation,
        waypoint: azalea_protocol::packets::game::c_waypoint::TrackedWaypoint,
    },
    EntityArmorUpdate {
        entity_id: i32,
        armor: u32,
    },
    ContainerContent {
        container_id: i32,
        items: Vec<ItemStack>,
        carried: ItemStack,
        state_id: u32,
    },
    ContainerSlot {
        container_id: i32,
        index: u16,
        item: ItemStack,
        state_id: u32,
    },
    /// A menu data value (furnace lit/cook progress, etc.).
    ContainerData {
        container_id: i32,
        id: u16,
        value: u16,
    },
    OpenScreen {
        container_id: i32,
        menu_type: azalea_registry::builtin::MenuKind,
        title: String,
    },
    ContainerClosed,
    CursorItem {
        item: ItemStack,
    },
    ChatMessage {
        spans: Vec<crate::ui::text::TextSpan>,
    },
    CommandTree {
        tree: Arc<crate::net::commands::CommandTree>,
    },
    CommandSuggestions {
        id: u32,
        /// Offset into the command string (as sent, including the leading `/`)
        /// where the completed range begins.
        start: usize,
        options: Vec<String>,
    },
    BlockUpdate {
        pos: BlockPos,
        state: BlockState,
    },
    BlockChangedAck {
        seq: u32,
    },
    SectionBlocksUpdate {
        updates: Vec<(BlockPos, BlockState)>,
    },
    BlockEntitySync {
        chunk_pos: ChunkPos,
        entries: Vec<(BlockPos, BlockEntityKind, NbtCompound)>,
    },
    BlockEntityUpdate {
        pos: BlockPos,
        kind: BlockEntityKind,
        nbt: Option<NbtCompound>,
    },
    BlockEvent {
        pos: BlockPos,
        action_id: u8,
        action_parameter: u8,
    },
    PlaySound {
        sound: crate::audio::SoundRef,
        category: u8,
        pos: Position,
        volume: f32,
        pitch: f32,
        seed: u64,
    },
    PlayEntitySound {
        sound: crate::audio::SoundRef,
        category: u8,
        entity_id: i32,
        volume: f32,
        pitch: f32,
        seed: u64,
    },
    TimeUpdate {
        game_time: u64,
        day_time: Option<u64>,
    },
    WeatherUpdate {
        event: azalea_protocol::packets::game::c_game_event::EventType,
        param: f32,
    },
    GameModeChanged {
        game_mode: u8,
    },
    PlayerAbilitiesChanged {
        flying: bool,
    },
    ServerViewDistance {
        distance: u32,
    },
    ServerSimulationDistance {
        distance: u32,
    },
    EntitySpawned {
        id: i32,
        uuid: uuid::Uuid,
        entity_type: EntityKind,
        position: Position,
        velocity: DVec3,
        y_rot_deg: f32,
        x_rot_deg: f32,
        head_y_rot_deg: f32,
    },
    EntityMoved {
        id: i32,
        dx: f64,
        dy: f64,
        dz: f64,
        on_ground: bool,
    },
    EntityMovedRotated {
        id: i32,
        dx: f64,
        dy: f64,
        dz: f64,
        y_rot_deg: f32,
        x_rot_deg: f32,
        on_ground: bool,
    },
    EntityMotion {
        id: i32,
        velocity: DVec3,
    },
    EntityTeleported {
        id: i32,
        position: Position,
        /// `TeleportEntity` applies the packet's velocity; `EntityPositionSync`
        /// doesn't (vanilla `setValuesFromPositionPacket` vs
        /// `handleEntityPositionSync`).
        velocity: Option<DVec3>,
        y_rot_deg: f32,
        x_rot_deg: f32,
        on_ground: bool,
    },
    LevelEvent {
        event_type: u32,
        pos: BlockPos,
        data: u32,
    },
    /// `ClientboundLevelParticles`. The handler drops unimplemented particle
    /// kinds and the `always_show` flag (it only matters below
    /// `ParticleStatus::All`, and pomme has no particles setting).
    LevelParticles {
        kind: crate::particle::ServerParticleKind,
        override_limiter: bool,
        pos: DVec3,
        x_dist: f32,
        y_dist: f32,
        z_dist: f32,
        max_speed: f32,
        count: u32,
    },
    EntitiesRemoved {
        ids: Vec<i32>,
    },
    EntityItemData {
        id: i32,
        item_name: String,
        item_id: u32,
        count: i32,
    },
    EntityHeadRotation {
        id: i32,
        head_y_rot_deg: f32,
    },
    EntityBabyFlag {
        id: i32,
        is_baby: bool,
    },
    EntityPose {
        id: i32,
        is_crouching: bool,
    },
    SheepWoolData {
        id: i32,
        color: u8,
        sheared: bool,
    },
    SheepEatStart {
        id: i32,
    },
    /// Entity event 9: the entity finished using its item (eating complete).
    FinishUseItem {
        id: i32,
    },
    CowVariant {
        id: i32,
        variant: u8,
    },
    VillagerData {
        id: i32,
        kind: VillagerKind,
        profession: VillagerProfession,
        level: u32,
    },
    VillagerUnhappy {
        id: i32,
        counter: i32,
    },
    EntityCustomName {
        id: i32,
        name: Option<String>,
    },
    EntityAggressive {
        id: i32,
        aggressive: bool,
    },
    EntitySwing {
        id: i32,
    },
    CreeperPowered {
        id: i32,
        powered: bool,
    },
    EntityDamaged {
        id: i32,
    },
    ItemPickedUp {
        item_id: i32,
        collector_id: i32,
        amount: i32,
    },
    PlayerLogin {
        entity_id: i32,
    },
    PlayerScore {
        entity_id: i32,
        score: i32,
    },
    PlayerDied {
        message: String,
    },
    ResourcePackPush {
        id: uuid::Uuid,
        url: String,
        hash: String,
        required: bool,
    },
    ResourcePackPop {
        id: Option<uuid::Uuid>,
    },
    Disconnected {
        reason: String,
    },
    PlayerInfoUpdate {
        actions: crate::player::tab_list::PlayerInfoActions,
        entries: Vec<crate::player::tab_list::PlayerInfoEntry>,
    },
    PlayerInfoRemove {
        uuids: Vec<uuid::Uuid>,
    },
    TabListHeaderFooter {
        header: String,
        footer: String,
    },
}

impl NetworkEvent {
    /// Variant tag for the benchmark's worst-event stat. Trivially cheap
    /// variants share a group tag; anything that touches the chunk store,
    /// disk, or a nontrivial subsystem is named individually.
    pub fn kind(&self) -> &'static str {
        match self {
            NetworkEvent::Connected => "connected",
            NetworkEvent::Registries(_) => "registries",
            NetworkEvent::BiomeColors { .. } => "biome_colors",
            NetworkEvent::DimensionInfo { .. } => "dimension_info",
            NetworkEvent::ChunkLoaded { .. } => "chunk_loaded",
            NetworkEvent::LightUpdate { .. } => "light_update",
            NetworkEvent::ChunkUnloaded { .. } => "chunk_unloaded",
            NetworkEvent::ChunkCacheCenter { .. } => "chunk_cache_center",
            NetworkEvent::PlayerPosition { .. } => "player_position",
            NetworkEvent::PlayerHealth { .. }
            | NetworkEvent::PlayerExperience { .. }
            | NetworkEvent::PlayerAbilitiesChanged { .. }
            | NetworkEvent::GameModeChanged { .. }
            | NetworkEvent::PlayerLogin { .. }
            | NetworkEvent::PlayerScore { .. }
            | NetworkEvent::PlayerDied { .. } => "player_state",
            NetworkEvent::Waypoint { .. } => "waypoint",
            NetworkEvent::ContainerContent { .. }
            | NetworkEvent::ContainerSlot { .. }
            | NetworkEvent::ContainerData { .. }
            | NetworkEvent::OpenScreen { .. }
            | NetworkEvent::ContainerClosed
            | NetworkEvent::CursorItem { .. }
            | NetworkEvent::EntityArmorUpdate { .. } => "container",
            NetworkEvent::ChatMessage { .. } => "chat_message",
            NetworkEvent::CommandTree { .. } => "command_tree",
            NetworkEvent::CommandSuggestions { .. } => "command_suggestions",
            NetworkEvent::BlockUpdate { .. } => "block_update",
            NetworkEvent::BlockChangedAck { .. } => "block_changed_ack",
            NetworkEvent::SectionBlocksUpdate { .. } => "section_blocks_update",
            NetworkEvent::BlockEntitySync { .. } => "block_entity_sync",
            NetworkEvent::BlockEntityUpdate { .. } => "block_entity_update",
            NetworkEvent::BlockEvent { .. } => "block_event",
            NetworkEvent::PlaySound { .. } | NetworkEvent::PlayEntitySound { .. } => "play_sound",
            NetworkEvent::TimeUpdate { .. } => "time_update",
            NetworkEvent::WeatherUpdate { .. } => "weather_update",
            NetworkEvent::ServerViewDistance { .. }
            | NetworkEvent::ServerSimulationDistance { .. } => "server_distance",
            NetworkEvent::EntitySpawned { .. } => "entity_spawned",
            NetworkEvent::EntitiesRemoved { .. } => "entities_removed",
            NetworkEvent::LevelEvent { .. } => "level_event",
            NetworkEvent::LevelParticles { .. } => "level_particles",
            NetworkEvent::EntityMoved { .. }
            | NetworkEvent::EntityMovedRotated { .. }
            | NetworkEvent::EntityMotion { .. }
            | NetworkEvent::EntityTeleported { .. }
            | NetworkEvent::EntityHeadRotation { .. }
            | NetworkEvent::EntityItemData { .. }
            | NetworkEvent::EntityBabyFlag { .. }
            | NetworkEvent::EntityPose { .. }
            | NetworkEvent::SheepWoolData { .. }
            | NetworkEvent::SheepEatStart { .. }
            | NetworkEvent::FinishUseItem { .. }
            | NetworkEvent::CowVariant { .. }
            | NetworkEvent::VillagerData { .. }
            | NetworkEvent::VillagerUnhappy { .. }
            | NetworkEvent::EntityCustomName { .. }
            | NetworkEvent::EntityAggressive { .. }
            | NetworkEvent::EntitySwing { .. }
            | NetworkEvent::CreeperPowered { .. }
            | NetworkEvent::EntityDamaged { .. }
            | NetworkEvent::ItemPickedUp { .. } => "entity_state",
            NetworkEvent::ResourcePackPush { .. } | NetworkEvent::ResourcePackPop { .. } => {
                "resource_pack"
            }
            NetworkEvent::Disconnected { .. } => "disconnected",
            NetworkEvent::PlayerInfoUpdate { .. } => "player_info_update",
            NetworkEvent::PlayerInfoRemove { .. } => "player_info_remove",
            NetworkEvent::TabListHeaderFooter { .. } => "tab_list",
        }
    }
}
