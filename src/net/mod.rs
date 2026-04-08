pub mod connection;
pub mod handler;
pub mod sender;

use std::sync::Arc;

use azalea_block::BlockState;
use azalea_core::heightmap_kind::HeightmapKind;
use azalea_core::position::{BlockPos, ChunkPos};
use azalea_inventory::ItemStack;
use azalea_registry::builtin::EntityKind;

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
        x: f64,
        y: f64,
        z: f64,
        yaw: f32,
        pitch: f32,
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
        text: String,
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
    TimeUpdate {
        game_time: u64,
        day_time: Option<u64>,
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
        x: f64,
        y: f64,
        z: f64,
        yaw: f32,
        pitch: f32,
        head_yaw: f32,
        velocity: [f64; 3],
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
        yaw: f32,
        pitch: f32,
    },
    EntityTeleported {
        id: i32,
        x: f64,
        y: f64,
        z: f64,
        yaw: f32,
        pitch: f32,
    },
    EntitiesRemoved {
        ids: Vec<i32>,
    },
    EntityItemData {
        id: i32,
        item_name: String,
        count: i32,
    },
    EntityHeadRotation {
        id: i32,
        head_yaw: f32,
    },
    EntityBabyFlag {
        id: i32,
        is_baby: bool,
    },
    ItemPickedUp {
        item_id: i32,
        collector_id: i32,
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
