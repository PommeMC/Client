pub mod commands;
pub mod connection;
pub mod handler;
pub mod resolve;
pub mod sender;

use std::sync::Arc;

use azalea_block::BlockState;
use azalea_core::heightmap_kind::HeightmapKind;
use azalea_core::position::{BlockPos, ChunkPos};
use azalea_inventory::ItemStack;
use azalea_registry::builtin::{BlockEntityKind, EntityKind};
use simdnbt::owned::NbtCompound;

use crate::entity::components::Position;

pub enum NetworkEvent {
    Connected,
    BiomeColors {
        colors: std::collections::HashMap<u32, crate::renderer::chunk::mesher::BiomeClimate>,
    },
    DimensionInfo {
        height: u32,
        min_y: i32,
    },
    ChunkLoaded {
        pos: ChunkPos,
        data: Arc<Box<[u8]>>,
        heightmaps: Vec<(HeightmapKind, Box<[u64]>)>,
        sky_light: Arc<Box<[Box<[u8]>]>>,
        block_light: Arc<Box<[Box<[u8]>]>>,
        sky_y_mask: azalea_core::bitset::BitSet,
        block_y_mask: azalea_core::bitset::BitSet,
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
    EntityArmorUpdate {
        entity_id: i32,
        armor: u32,
    },
    InventoryContent {
        items: Vec<ItemStack>,
    },
    InventorySlot {
        index: u16,
        item: ItemStack,
    },
    ChatMessage {
        spans: Vec<crate::ui::text::TextSpan>,
    },
    CommandTree {
        tree: Arc<crate::net::commands::CommandTree>,
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
    ServerViewDistance {
        distance: u32,
    },
    ServerSimulationDistance {
        distance: u32,
    },
    EntitySpawned {
        id: i32,
        entity_type: EntityKind,
        position: Position,
        y_rot_deg: f32,
        x_rot_deg: f32,
        head_y_rot_deg: f32,
    },
    EntityMoved {
        id: i32,
        dx: f64,
        dy: f64,
        dz: f64,
    },
    EntityMovedRotated {
        id: i32,
        dx: f64,
        dy: f64,
        dz: f64,
        y_rot_deg: f32,
        x_rot_deg: f32,
    },
    EntityTeleported {
        id: i32,
        position: Position,
        y_rot_deg: f32,
        x_rot_deg: f32,
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
    CowVariant {
        id: i32,
        variant: u8,
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
