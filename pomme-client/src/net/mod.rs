mod advancement_wire;
#[cfg(test)]
mod azalea_compat;
mod block_wire;
pub(crate) mod capture;
pub(crate) mod chat;
pub(crate) mod chat_security;
pub mod chunk_batch;
pub(crate) mod chunk_wire;
pub mod commands;
pub mod conn;
pub mod connection;
mod container_wire;
mod dialog;
mod entity_data;
mod entity_wire;
mod environment_wire;
pub mod handler;
pub(crate) mod http;
mod hud_wire;
pub mod item_codec;
pub mod known_packs;
mod lifecycle_wire;
mod player_info_wire;
mod player_wire;
mod recipe_wire;
pub mod resolve;
mod scoreboard_wire;
pub mod sender;
mod session;
mod sound_wire;
pub mod stream;
pub mod translate;
#[cfg(test)]
mod wire_corpus;

use std::sync::Arc;

use azalea_block::BlockState;
use azalea_inventory::ItemStack;
use azalea_inventory::components::EquipmentSlot;
use azalea_registry::builtin::{BlockEntityKind, EntityKind};
use glam::DVec3;
use simdnbt::owned::NbtCompound;

use crate::entity::MetaValue;
use crate::entity::components::Position;
use crate::entity::villager::{VillagerKind, VillagerProfession};
use crate::position::{BlockPos, ChunkPos};
use crate::world::chunk::HeightmapKind;

/// A packet's per-column light payload (chunk load or standalone update):
/// present sections listed in `*_updates`, selected by `*_y_mask`, with
/// `empty_*_y_mask` marking explicitly-zero sections.
pub struct PacketLightData {
    pub sky_updates: Arc<Box<[Box<[u8]>]>>,
    pub block_updates: Arc<Box<[Box<[u8]>]>>,
    pub sky_y_mask: crate::world::light::BitSet,
    pub block_y_mask: crate::world::light::BitSet,
    pub empty_sky_y_mask: crate::world::light::BitSet,
    pub empty_block_y_mask: crate::world::light::BitSet,
}

/// Decodes a native game packet with `read`, which returns `None` for the
/// packets it doesn't own; a frame with bytes left over is malformed.
fn read_raw_packet<P>(
    packet_id: u32,
    payload: &[u8],
    read: fn(
        &str,
        &mut pomme_protocol::codec::Reader,
    ) -> Option<pomme_protocol::codec::DecodeResult<P>>,
) -> Option<pomme_protocol::codec::DecodeResult<P>> {
    use pomme_protocol::{Direction, PacketTable, Phase};

    let name = PacketTable::native().name_of(Phase::Game, Direction::Clientbound, packet_id)?;
    let mut r = pomme_protocol::codec::Reader::new(payload);
    Some(read(name, &mut r)?.and_then(|packet| {
        r.ensure_eof()?;
        Ok(packet)
    }))
}

/// A `GameType` id as the client stores it; `GameType.byId` maps an unknown id
/// to SURVIVAL.
fn game_type(id: u32) -> u8 {
    if id <= 3 { id as u8 } else { 0 }
}

/// An optional field converted with `convert`, which failing drops the
/// packet.
fn present<T, U>(value: Option<T>, convert: impl FnOnce(T) -> Option<U>) -> Option<Option<U>> {
    match value {
        Some(value) => convert(value).map(Some),
        None => Some(None),
    }
}

/// `ClientboundGameEventPacket`'s weather changes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Weather {
    StartRaining,
    StopRaining,
    RainLevel(f32),
    ThunderLevel(f32),
}

/// A dimension type's fields the client uses, from login and respawn.
pub struct DimensionInfo {
    pub height: u32,
    pub min_y: i32,
    pub has_skylight: bool,
    pub cardinal_light: crate::world::block::model::CardinalLightType,
    /// Dimension-type `ambient_light`, the lightmap's floor.
    pub ambient_light: f32,
    /// The dimension's `timelines` include the overworld day, which dims
    /// its sky light at night.
    pub overworld_day: bool,
    /// The `world_clock` driving this dimension's day time.
    pub default_clock: Option<String>,
    /// The lightmap's attribute layers.
    pub light_environment: crate::world::environment::LightEnvironment,
}

pub enum NetworkEvent {
    /// Ordered before login/configuration events on both transports.
    ProtocolSelected {
        protocol: i32,
    },
    Connected,
    Registries(Arc<azalea_core::registry_holder::RegistryHolder>),
    /// The `minecraft:dialog` registry with its tags, sent with `Registries`
    /// and again whenever a tag update replaces the dialog tags.
    DialogRegistry(Arc<crate::ui::server_dialog::DialogRegistry>),
    ItemTags(crate::recipe::ItemTags),
    BlockTags {
        tags: Vec<(String, Vec<i32>)>,
    },
    BiomeColors {
        colors: std::collections::HashMap<u32, crate::renderer::chunk::mesher::BiomeClimate>,
    },
    /// `LEVEL_CHUNKS_LOAD_START`: the server has started sending the level.
    LevelChunksLoadStart,
    DimensionInfo(DimensionInfo),
    ChunkLoaded {
        pos: ChunkPos,
        data: Arc<Box<[u8]>>,
        heightmaps: Vec<(HeightmapKind, Box<[u64]>)>,
        light: PacketLightData,
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
    Ping {
        id: u32,
    },
    PlayerPosition {
        /// Teleport id to acknowledge.
        id: u32,
        change: pomme_protocol::entity::PositionMoveRotation,
        relative: pomme_protocol::entity::Relative,
    },
    PlayerRotation {
        y_rot: f32,
        relative_y: bool,
        x_rot: f32,
        relative_x: bool,
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
    UpdateMobEffect {
        entity_id: i32,
        effect: crate::mob_effect::MobEffectInstance,
    },
    RemoveMobEffect {
        entity_id: i32,
        effect_id: u32,
    },
    ClearMobEffects,
    SetPassengers {
        vehicle: i32,
        passengers: Vec<i32>,
    },
    EntityEquipment {
        entity_id: i32,
        slots: Vec<(EquipmentSlot, ItemStack)>,
    },
    Waypoint {
        operation: azalea_protocol::packets::game::c_waypoint::WaypointOperation,
        waypoint: azalea_protocol::packets::game::c_waypoint::TrackedWaypoint,
    },
    EntityAttributesUpdate {
        entity_id: i32,
        snapshots: Vec<crate::attribute::AttributeSnapshot>,
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
    /// `SetPlayerInventory`, indexed by vanilla `Inventory` slot.
    PlayerInventorySlot {
        index: u32,
        item: ItemStack,
    },
    HeldSlot {
        slot: u8,
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
        /// Preserves keybind references for resolution using the app's current
        /// bindings.
        component: Box<crate::chat_component::Component>,
        /// Same bound chat type decorated from the signed body with unsigned
        /// content removed, for Vanilla's `onlyShowSecureChat` path.
        secure_component: Option<Box<crate::chat_component::Component>>,
        missing_profile_component: Option<Box<crate::chat_component::Component>>,
        signature: Option<[u8; 256]>,
        sender_uuid: Option<uuid::Uuid>,
        signed_body: Option<crate::net::chat_security::SignedChatBody>,
        source: crate::ui::chat::ChatMessageSource,
        tag: Option<crate::ui::chat::ChatMessageTag>,
    },
    DeleteChatMessage {
        signature: [u8; 256],
    },
    ActionBar {
        component: crate::chat_component::Component,
    },
    ServerLinks {
        links: Vec<crate::ui::server_dialog::ServerLink>,
    },
    ShowDialog {
        dialog: crate::ui::server_dialog::DialogReference,
    },
    ClearDialog,
    BossBarUpdate {
        id: uuid::Uuid,
        op: crate::ui::boss_bar::BossBarOp,
    },
    AdvancementsUpdate(Box<crate::ui::toast::AdvancementsUpdate>),
    RecipeBookAdd {
        entries: Vec<crate::recipe::RecipeBookAddEntry>,
        replace: bool,
    },
    RecipeBookRemove {
        ids: Vec<crate::recipe::RecipeDisplayId>,
    },
    RecipeBookSettings(crate::recipe::RecipeBookSettings),
    RecipeData(crate::recipe::RecipeData),
    GhostRecipe(Box<crate::recipe::GhostRecipe>),
    RecipeToastAdd {
        entries: Vec<crate::ui::toast::RecipeToastEntry>,
    },
    TitleText {
        spans: Vec<crate::ui::text::TextSpan>,
    },
    SubtitleText {
        spans: Vec<crate::ui::text::TextSpan>,
    },
    TitlesAnimation {
        fade_in: i32,
        stay: i32,
        fade_out: i32,
    },
    ClearTitles {
        reset_times: bool,
    },
    ScoreboardObjective {
        name: String,
        display: Option<Vec<crate::ui::text::TextSpan>>,
        number_format: Option<crate::ui::hud::ScoreNumberFormat>,
    },
    ScoreboardDisplay {
        name: Option<String>,
    },
    ScoreboardScore {
        owner: String,
        objective: String,
        score: i32,
        display: Option<Vec<crate::ui::text::TextSpan>>,
        number_format: Option<crate::ui::hud::ScoreNumberFormat>,
    },
    ScoreboardReset {
        owner: String,
        objective: Option<String>,
    },
    ScoreboardTeam {
        name: String,
        display_name: Vec<crate::ui::text::TextSpan>,
        prefix: Vec<crate::ui::text::TextSpan>,
        suffix: Vec<crate::ui::text::TextSpan>,
        color: [f32; 4],
        fill_color: Option<u32>,
        collision_rule: crate::ui::hud::CollisionRule,
        see_friendly_invisibles: bool,
        members: Option<Vec<String>>,
    },
    ScoreboardTeamMembers {
        name: String,
        members: Vec<String>,
        join: bool,
    },
    ScoreboardTeamRemoved {
        name: String,
    },
    CommandTree {
        tree: Arc<crate::net::commands::CommandTree>,
    },
    CommandSuggestions {
        id: u32,
        /// Offset into the command string (as sent, including the leading `/`)
        /// where the completed range begins.
        start: usize,
        options: Vec<crate::ui::chat::ChatSuggestion>,
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
        nbt: NbtCompound,
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
    StopSound {
        sound_id: Option<String>,
        category: Option<u8>,
    },
    TimeUpdate {
        game_time: u64,
        /// Every world clock by name, with its total ticks.
        clocks: Vec<(String, u64)>,
    },
    WeatherUpdate(Weather),
    GameModeChanged {
        game_mode: u8,
        /// `Some` = authoritative previous mode from login/respawn (which may
        /// itself be absent); `None` = derive from the mode being replaced
        /// (the GameEvent packet carries no previous mode).
        previous: Option<Option<u8>>,
    },
    DimensionName {
        name: String,
    },
    PlayerAbilitiesChanged {
        flying: bool,
        can_fly: bool,
        flying_speed: f32,
        walking_speed: f32,
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
    EntityRotated {
        id: i32,
        y_rot_deg: f32,
        x_rot_deg: f32,
        on_ground: bool,
    },
    EntityMotion {
        id: i32,
        velocity: DVec3,
    },
    PlayerKnockback {
        delta: DVec3,
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
        damage: i32,
        count: i32,
        stack: azalea_inventory::ItemStackData,
    },
    EntityHeadRotation {
        id: i32,
        head_y_rot_deg: f32,
    },
    /// A raw scalar entity-data value; `EntityStore::apply_entity_data`
    /// resolves its meaning per (kind, index) like vanilla's
    /// `onSyncedDataUpdated`.
    EntityData {
        id: i32,
        index: u8,
        value: MetaValue,
    },
    EntityPose {
        id: i32,
        is_crouching: bool,
    },
    /// Vanilla `handleSetCamera`: view the world from entity `id`, which is
    /// the local player's own id when spectating ends.
    SetCamera {
        id: i32,
    },
    /// LivingEntity metadata index 14 (SLEEPING_POS): Some while in a bed.
    /// Vanilla `isSleeping()` is `getSleepingPos().isPresent()`.
    EntitySleepingPos {
        id: i32,
        pos: Option<BlockPos>,
    },
    /// `ClientboundAnimate` action 2: vanilla `handleAnimate` calls
    /// `stopSleepInBed(false, false)`, forcing the sleep counter to 100.
    EntityWakeUp {
        id: i32,
    },
    SheepEatStart {
        id: i32,
    },
    /// Entity event 9: the entity finished using its item (eating complete).
    FinishUseItem {
        id: i32,
    },
    /// Registry/wire variant slot; meaning is per-kind. `kind` is the mob
    /// the emitting arm resolved for, guarding overloaded metadata indices.
    EntityVariant {
        id: i32,
        kind: EntityKind,
        variant: u32,
    },
    WolfShaking {
        id: i32,
        shaking: bool,
    },
    RabbitJump {
        id: i32,
    },
    SquidTentacleReset {
        id: i32,
    },
    /// Entity event 4: iron golem punch.
    GolemPunch {
        id: i32,
    },
    /// Entity events 11 / 34: iron golem flower offer start / stop.
    GolemOfferFlower {
        id: i32,
        offering: bool,
    },
    VillagerData {
        id: i32,
        kind: VillagerKind,
        profession: VillagerProfession,
        level: u32,
    },
    EntityCustomName {
        id: i32,
        name: Option<String>,
    },
    EntitySwing {
        id: i32,
    },
    EntityDamaged {
        id: i32,
    },
    EntityDied {
        id: i32,
    },
    HurtAnimation {
        id: i32,
        yaw: f32,
    },
    ItemPickedUp {
        item_id: i32,
        collector_id: i32,
        amount: i32,
    },
    PlayerLogin {
        entity_id: i32,
        hardcore: bool,
        show_death_screen: bool,
        online_mode: bool,
    },
    SecureChatEnforced {
        enforced: bool,
    },
    PlayerScore {
        entity_id: i32,
        score: i32,
    },
    PlayerAbsorption {
        entity_id: i32,
        absorption: f32,
    },
    /// Vanilla `handleRespawn`: a fresh `LocalPlayer` is built, restoring old
    /// entity data / attribute modifiers only per the packet's keep flags.
    PlayerRespawned {
        keep_entity_data: bool,
        keep_attribute_modifiers: bool,
    },
    PlayerDied {
        player_id: i32,
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
    /// The server re-entered the configuration phase (a proxy transfer);
    /// world-scoped state resets like vanilla's `clearClientLevel`.
    Reconfiguring,
    Disconnected {
        reason: String,
    },
    PlayerInfoUpdate {
        actions: pomme_protocol::player_info::Actions,
        entries: Vec<crate::player::tab_list::PlayerInfoEntry>,
    },
    PlayerInfoRemove {
        uuids: Vec<uuid::Uuid>,
    },
    TabListHeaderFooter {
        header: Vec<crate::ui::text::TextSpan>,
        footer: Vec<crate::ui::text::TextSpan>,
    },
}

/// The client information pomme reports. The configuration and game phase
/// packets wrap the same struct under different field names, and 1.20.1 sends
/// it in the game phase only, so all three sites share this.
/// Vanilla `Options.buildPlayerInformation`.
pub fn client_information(
    view_distance: u8,
    chat_options: crate::ui::chat::ChatOptions,
    skin: crate::ui::menu::SkinOptions,
) -> azalea_protocol::common::client_information::ClientInformation {
    use azalea_entity::HumanoidArm;
    use azalea_protocol::common::client_information::*;
    let chat_visibility = match chat_options.visibility {
        crate::ui::chat::ChatVisibilitySetting::Full => ChatVisibility::Full,
        crate::ui::chat::ChatVisibilitySetting::System => ChatVisibility::System,
        crate::ui::chat::ChatVisibilitySetting::Hidden => ChatVisibility::Hidden,
    };
    ClientInformation {
        language: "en_us".into(),
        view_distance,
        chat_visibility,
        chat_colors: chat_options.colors,
        model_customization: ModelCustomization {
            cape: skin.cape,
            jacket: skin.jacket,
            left_sleeve: skin.left_sleeve,
            right_sleeve: skin.right_sleeve,
            left_pants: skin.left_pants,
            right_pants: skin.right_pants,
            hat: skin.hat,
        },
        main_hand: if skin.main_hand_right {
            HumanoidArm::Right
        } else {
            HumanoidArm::Left
        },
        text_filtering_enabled: false,
        allows_listing: true,
        particle_status: ParticleStatus::All,
    }
}

/// The `minecraft:brand` payload pomme announces itself with. Sent from the
/// configuration phase, or the game phase on versions without one.
pub fn brand_payload() -> Vec<u8> {
    let mut out = Vec::new();
    pomme_protocol::wire::write_utf(&mut out, "pomme");
    out
}
