use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::Instant;

use azalea_core::position::{BlockPos, ChunkPos, ChunkSectionPos};
use azalea_protocol::packets::game::{
    ServerboundClientInformation, ServerboundCommandSuggestion, ServerboundGamePacket,
};
use azalea_registry::builtin::{BlockEntityKind, EntityKind};
use glam::FloatExt as _;

use crate::app::core::{AppCore, PlayerInputState};
use crate::app::phases::Gfx;
use crate::app::{DEFAULT_RENDER_DISTANCE, TICK_RATE, input};
use crate::audio::{CATEGORY_PLAYERS, SoundRef};
use crate::benchmark::{
    Benchmark, BenchmarkResult, ChunkLoadBench, ChunkLoadResult, ChunkLoadStep, UploadHandle,
    UploadStatus, upload_result,
};
use crate::entity::components::{LookDirection, Position};
use crate::entity::{EntityStore, ItemEntityStore, lerp_angle};
use crate::net::connection::ConnectionHandle;
use crate::player::LocalPlayer;
use crate::player::interaction::{HitResult, InteractionState};
use crate::player::menu_click::ContainerKind;
use crate::player::tab_list::TabList;
use crate::renderer::chunk::dispatcher::ChunkMeshing;
use crate::renderer::chunk::mesher::BiomeClimate;
use crate::renderer::pipelines::block_entity;
use crate::renderer::pipelines::entity_renderer::{
    EntityRenderInfo, MAX_OVERLAYS, WHITE_TINT, jeb_sheep_tint, wool_color_tint,
};
use crate::renderer::pipelines::menu_overlay::MenuElement;
use crate::renderer::{Renderer, SkyState};
use crate::resource_pack::ResourcePackManager;
use crate::ui::chat::ChatState;
use crate::ui::death::{self, DeathAction};
use crate::ui::pause::{self, PauseAction, PauseScreen};
use crate::ui::{common, hud};
use crate::world::block_entity_anim::BlockEntityAnimStore;
use crate::world::chunk::ChunkStore;

/// Which screen a server-opened container renders as.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ContainerScreen {
    CraftingTable,
    Furnace(crate::ui::furnace::FurnaceVariant),
    Chest { rows: u8 },
    ShulkerBox,
    Anvil,
    Enchantment,
}

impl ContainerScreen {
    /// The click-prediction menu kind backing this screen.
    pub fn click_kind(self) -> ContainerKind {
        match self {
            Self::CraftingTable => ContainerKind::CraftingTable,
            Self::Furnace(_) => ContainerKind::Furnace,
            Self::Chest { rows } => ContainerKind::Chest { rows },
            Self::ShulkerBox => ContainerKind::ShulkerBox,
            Self::Anvil => ContainerKind::Anvil,
            Self::Enchantment => ContainerKind::Enchantment,
        }
    }
}

/// A server-opened container screen.
pub struct OpenContainer {
    pub id: i32,
    pub title: String,
    pub screen: ContainerScreen,
    /// Menu slots in container indices; slots from `inv_start()` on are backed
    /// by the player inventory.
    pub slots: Vec<azalea_inventory::ItemStack>,
    /// The menu's data values (`ClientboundContainerSetData`), e.g. furnace
    /// lit/cook progress or the anvil repair cost. Vanilla data slots are
    /// shorts; the enchanting table uses all 10 (costs, seed, clues) with -1
    /// sentinels, so values are kept sign-extended.
    pub data: [i16; 10],
    /// The anvil rename field's state; Some only for the anvil screen.
    pub anvil: Option<crate::ui::anvil::AnvilState>,
    /// The book animation's state; Some only for the enchantment screen.
    pub enchant: Option<crate::ui::enchantment::EnchantState>,
    /// This menu's latest server state id, echoed in container clicks.
    pub state_id: u32,
}

impl OpenContainer {
    /// First container slot backed by the player inventory; container slot `i`
    /// maps to player inventory slot `i - inv_start() + 9` from here on.
    fn inv_start(&self) -> usize {
        self.screen.click_kind().inv_start()
    }
}

pub struct GameState {
    pub chunk_store: ChunkStore,
    /// Client-side light engine (vanilla `LevelLightEngine`); recreated with
    /// the chunk store on dimension changes, drained once per tick.
    pub light_engine: crate::world::light::LevelLightEngine,
    pub entity_store: EntityStore,
    pub position_set: bool,
    pub player_loaded_sent: bool,
    pub player: LocalPlayer,
    /// Bubble index the pop sound last played for, so each pop fires once.
    pub last_bubble_pop_sound_played: i32,
    pub biome_climate: Arc<HashMap<u32, BiomeClimate>>,
    pub player_walk_pos: f32,
    pub player_walk_speed: f32,
    pub player_prev_walk_speed: f32,
    pub meshing: ChunkMeshing,
    pub paused: bool,
    pub dead: bool,
    pub death_message: String,
    pub death_instant: Instant,
    pub death_confirm: bool,
    pub death_confirm_instant: Instant,
    pub respawn_sent: bool,
    pub inventory_open: bool,
    pub creative_inventory_open: bool,
    pub creative_state: crate::ui::creative_inventory::CreativeState,
    /// The inventory menu's (container 0) latest server state id, echoed in
    /// container clicks; an open container keeps its own.
    pub inventory_state_id: u32,
    /// Carried (cursor) stack for container screens, driven by the server.
    pub cursor_item: azalea_inventory::ItemStack,
    /// The server-opened container screen (crafting table), if any.
    pub open_container: Option<OpenContainer>,
    /// Which container menu was open last frame (0 = survival inventory), to
    /// detect the close transition and send a container-close packet.
    pub container_was_open: Option<i32>,
    /// Active survival click-drag (button + slots covered), if any.
    pub inv_drag: Option<(azalea_inventory::operations::QuickCraftKind, Vec<u16>)>,
    /// Last survival left click (slot, time) for double-click detection.
    pub inv_last_click: Option<(u16, Instant)>,
    /// Server registries, for hashing predicted container clicks.
    pub registries: Arc<azalea_core::registry_holder::RegistryHolder>,
    pub chat: ChatState,
    pub command_tree: Option<Arc<crate::net::commands::CommandTree>>,
    pub tab_list: TabList,
    /// Locator bar waypoints tracked by the server.
    pub waypoints: crate::world::waypoints::WaypointMap,
    /// Network events pulled off the (bounded, drop-on-full) channel but not
    /// yet applied: the channel must always be emptied so the net thread's
    /// `try_send` never drops, while applying is time-budgeted per frame.
    pub pending_events: VecDeque<crate::net::NetworkEvent>,
    /// Client tick counter (vanilla `player.tickCount`).
    pub tick_count: u64,
    /// Tick of the last XP progress change; the XP bar outprioritizes the
    /// locator bar for 100 ticks after it (vanilla
    /// `experienceDisplayStartTick`; `i64::MIN` = untouched since (re)spawn,
    /// so the first change after joining never takes priority).
    pub xp_display_start_tick: i64,
    pub interaction: InteractionState,
    pub sky_state: crate::renderer::SkyState,
    pub show_debug: bool,
    pub show_chunk_borders: bool,
    pub advanced_item_tooltips: bool,
    pub last_sent_input: PlayerInputState,
    pub last_sent_pos: Position,
    pub last_sent_look_dir: LookDirection,
    pub last_sent_on_ground: bool,
    pub last_sent_horizontal_collision: bool,
    pub was_sprinting: bool,
    pub position_send_counter: u32,
    pub options_from_game: bool,
    pub last_render_distance: u32,
    pub server_render_distance: u32,
    pub server_simulation_distance: u32,
    pub item_entity_store: ItemEntityStore,
    pub particle_store: crate::particle::ParticleStore,
    pub block_entity_anim: BlockEntityAnimStore,
    pub benchmark: Option<Benchmark>,
    pub benchmark_result: Option<BenchmarkResult>,
    /// In-flight/finished upload of the FPS result, while its overlay is shown.
    pub benchmark_upload: Option<UploadHandle>,
    /// Which pause screen is showing (main / benchmark submenu / chunk loader).
    pub pause_screen: PauseScreen,
    pub chunk_load_bench: Option<ChunkLoadBench>,
    pub chunk_load_result: Option<ChunkLoadResult>,
    /// Set by Esc while a chunk-load benchmark runs; consumed next frame to
    /// cancel it.
    pub chunk_load_abort: bool,
    /// In-flight/finished upload of the chunk-load result, while its overlay is
    /// shown.
    pub chunk_load_upload: Option<UploadHandle>,
    /// Last frame's `update_game` CPU phase timings, for the chunk-load
    /// benchmark's worst-frame breakdown.
    pub last_update_phases: crate::benchmark::UpdatePhases,
}

impl GameState {
    pub fn new(renderer: &Renderer, resource_packs: &ResourcePackManager) -> Self {
        let chunk_store = ChunkStore::new(DEFAULT_RENDER_DISTANCE);
        let biome_climate = Arc::new(HashMap::new());
        let meshing = ChunkMeshing::new(
            renderer,
            Arc::clone(&chunk_store.shared),
            Arc::clone(&biome_climate),
            Some(resource_packs),
        );
        Self {
            light_engine: crate::world::light::LevelLightEngine::new(
                chunk_store.height(),
                chunk_store.min_y(),
                true,
            ),
            chunk_store,
            entity_store: EntityStore::new(),
            position_set: false,
            player_loaded_sent: false,
            options_from_game: false,
            last_render_distance: DEFAULT_RENDER_DISTANCE,
            server_render_distance: 0,
            server_simulation_distance: 0,
            item_entity_store: ItemEntityStore::new(),
            particle_store: {
                let (grass, foliage, dry_foliage) = meshing.colormaps();
                crate::particle::ParticleStore::new(
                    renderer.atlas_uv_map().clone(),
                    grass,
                    foliage,
                    dry_foliage,
                )
            },
            block_entity_anim: BlockEntityAnimStore::default(),
            player: LocalPlayer::new(),
            last_bubble_pop_sound_played: 0,
            biome_climate,
            player_walk_pos: 0.0,
            player_walk_speed: 0.0,
            player_prev_walk_speed: 0.0,
            meshing,
            paused: false,
            dead: false,
            death_message: String::new(),
            death_instant: Instant::now(),
            death_confirm: false,
            death_confirm_instant: Instant::now(),
            respawn_sent: false,
            inventory_open: false,
            creative_inventory_open: false,
            creative_state: crate::ui::creative_inventory::CreativeState::new(),
            inventory_state_id: 0,
            cursor_item: azalea_inventory::ItemStack::Empty,
            open_container: None,
            container_was_open: None,
            inv_drag: None,
            inv_last_click: None,
            registries: Arc::new(azalea_core::registry_holder::RegistryHolder::default()),
            chat: ChatState::new(),
            command_tree: None,
            tab_list: TabList::new(),
            waypoints: crate::world::waypoints::WaypointMap::default(),
            pending_events: VecDeque::new(),
            tick_count: 0,
            xp_display_start_tick: i64::MIN,
            interaction: InteractionState::new(),
            sky_state: SkyState::default_day(),
            show_debug: false,
            show_chunk_borders: false,
            advanced_item_tooltips: false,
            last_sent_input: PlayerInputState::default(),
            last_sent_pos: Position::default(),
            last_sent_look_dir: LookDirection::default(),
            last_sent_on_ground: false,
            last_sent_horizontal_collision: false,
            was_sprinting: false,
            position_send_counter: 0,
            benchmark: None,
            benchmark_result: None,
            benchmark_upload: None,
            pause_screen: PauseScreen::Main,
            chunk_load_bench: None,
            chunk_load_result: None,
            chunk_load_abort: false,
            chunk_load_upload: None,
            last_update_phases: crate::benchmark::UpdatePhases::default(),
        }
    }

    pub fn gui_open(&self) -> bool {
        self.inventory_open || self.creative_inventory_open || self.open_container.is_some()
    }

    /// The container menu the player currently has open (0 = survival
    /// inventory), if any.
    pub fn open_menu_id(&self) -> Option<i32> {
        if let Some(c) = &self.open_container {
            Some(c.id)
        } else if self.inventory_open {
            Some(0)
        } else {
            None
        }
    }

    /// The currently open menu's slots: the open container's, else the player
    /// inventory's.
    pub fn menu_slots(&self) -> &[azalea_inventory::ItemStack] {
        match &self.open_container {
            Some(c) => &c.slots,
            None => self.player.inventory.slots(),
        }
    }

    /// Set a slot of the currently open menu. Container slots backing the
    /// player inventory mirror into it, so the hotbar and a reopened
    /// inventory stay in sync.
    pub fn set_menu_slot(&mut self, index: usize, item: azalea_inventory::ItemStack) {
        match &mut self.open_container {
            Some(c) => {
                let Some(s) = c.slots.get_mut(index) else {
                    return;
                };
                *s = item.clone();
                let inv_start = c.inv_start();
                if index >= inv_start {
                    self.player.inventory.set_slot(index - inv_start + 9, item);
                }
            }
            None => self.player.inventory.set_slot(index, item),
        }
    }

    /// Re-mirror the inventory-backed slots into the open container after a
    /// direct player-inventory update.
    pub fn sync_container_from_inventory(&mut self) {
        let Some(c) = &mut self.open_container else {
            return;
        };
        let inv_start = c.inv_start();
        for (i, slot) in c.slots.iter_mut().enumerate().skip(inv_start) {
            *slot = self.player.inventory.slot(i - inv_start + 9).clone();
        }
    }

    /// Record the open container's latest server state id.
    pub fn set_container_state_id(&mut self, state_id: u32) {
        if let Some(c) = &mut self.open_container {
            c.state_id = state_id;
        }
    }

    pub fn close_creative_inventory(&mut self) {
        self.creative_inventory_open = false;
        self.creative_state.reset_interaction();
    }

    /// Close whichever container menu is open. Clears the carried stack
    /// (vanilla switches to the inventory menu, whose carried stack is empty;
    /// the server returns the items via inventory sync) and any in-flight
    /// gesture so a stale drag can't commit on reopen.
    pub fn close_menu(&mut self) {
        self.inventory_open = false;
        self.open_container = None;
        self.cursor_item = azalea_inventory::ItemStack::Empty;
        self.inv_drag = None;
        self.inv_last_click = None;
    }

    /// A focused text field (anvil rename, creative search) is capturing
    /// keyboard input: letter/digit keys must type instead of acting as
    /// hotkeys. The anvil field is editable only while its input slot is
    /// filled, matching vanilla.
    pub fn wants_text_input(&self) -> bool {
        if self.creative_inventory_open {
            return self.creative_state.tab.captures_typing();
        }
        matches!(
            &self.open_container,
            Some(c) if c.screen == ContainerScreen::Anvil
                && c.slots.first().is_some_and(|s| s.is_present())
        )
    }

    /// No menu (pause, inventory, chat) is capturing input.
    pub fn input_live(&self) -> bool {
        !self.paused
            && !self.gui_open()
            && !self.chat.is_open()
            && self.benchmark_result.is_none()
            && self.chunk_load_result.is_none()
    }

    /// F3-family debug toggles; these fire even while a menu is open,
    /// matching vanilla KeyboardHandler. Returns true if handled.
    pub fn handle_debug_key(&mut self, code: winit::keyboard::KeyCode, f3_held: bool) -> bool {
        use winit::keyboard::KeyCode;
        match code {
            KeyCode::F3 => {
                self.show_debug = !self.show_debug;
            }
            KeyCode::KeyG if f3_held => {
                self.show_chunk_borders = !self.show_chunk_borders;
            }
            _ => return false,
        }
        true
    }

    pub fn sync_render_distance(&mut self, connection: &ConnectionHandle, render_distance: u32) {
        self.last_render_distance = render_distance;
        tracing::info!("Render distance changed to {render_distance}");
        use azalea_entity::HumanoidArm;
        use azalea_protocol::common::client_information::*;
        connection
            .packet_tx
            .send(ServerboundGamePacket::ClientInformation(
                ServerboundClientInformation {
                    client_information: ClientInformation {
                        language: "en_us".into(),
                        view_distance: render_distance as u8,
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

    /// Mark a section dirty by advancing its content generation, returning the
    /// new value. Any in-flight mesh built from an older generation is
    /// dropped on arrival, so a deferred section always remeshes with the
    /// latest blocks.
    pub fn bump_content_gen(&mut self, pos: ChunkSectionPos) -> u64 {
        self.meshing.bump_content_gen(pos)
    }

    /// The chunk column the player stands in.
    pub fn player_chunk(&self) -> ChunkPos {
        ChunkPos::new(
            (self.player.position.x as i32).div_euclid(16),
            (self.player.position.z as i32).div_euclid(16),
        )
    }

    /// Runs one light update (vanilla `ClientLevel.update`, called per frame
    /// from `Minecraft.runTick`: drain queued light tasks, then
    /// `runLightUpdates`) and turns the resulting dirty scope into remesh
    /// work: columns whose chunk-load light applied go through the
    /// content-gen path like chunk loads (the rescan enqueues them
    /// nearest-first), individual lit sections remesh on the priority lane.
    pub fn update_light(&mut self) {
        let mut dirty = crate::world::light::LightDirty::default();
        self.light_engine
            .poll_and_run(&mut self.chunk_store, &mut dirty);
        if dirty.columns.is_empty() && dirty.sections.is_empty() {
            return;
        }
        let min_section_y = self.chunk_store.min_section_y();
        let section_count = self.chunk_store.section_count();
        let mut bumped: Vec<ChunkPos> = Vec::new();
        for &(x, z) in &dirty.columns {
            for p in crate::world::chunk::mesh_neighborhood(ChunkPos::new(x, z)) {
                if self.chunk_store.shared.has_chunk(p) && !bumped.contains(&p) {
                    bumped.push(p);
                }
            }
        }
        for &pos in &bumped {
            for si in 0..section_count {
                self.bump_content_gen(ChunkSectionPos::new(pos.x, min_section_y + si, pos.z));
            }
        }
        let player_chunk = self.player_chunk();
        for key in &dirty.sections {
            let si = key.y - min_section_y;
            let col = ChunkPos::new(key.x, key.z);
            // Padding/out-of-range sections have no mesh; columns already
            // bumped above remesh wholesale anyway.
            if si < 0 || si >= section_count || bumped.contains(&col) {
                continue;
            }
            if !self.chunk_store.shared.has_chunk(col) {
                continue;
            }
            self.enqueue_section_edit(
                ChunkSectionPos::new(key.x, key.y, key.z),
                crate::app::core::chunk_lod(col, player_chunk),
            );
        }
    }

    /// Mesh a single edited section now on the priority lane, ungated by
    /// visibility. Bumps that section's generation so the result is dropped
    /// only if the same section is edited again before it lands.
    pub fn enqueue_section_edit(&mut self, pos: ChunkSectionPos, lod: u32) {
        self.meshing.enqueue_section_edit(pos, lod);
    }

    pub fn mesh_edit_now(&mut self, pos: ChunkSectionPos, lod: u32) {
        self.meshing.mesh_edit_now(pos, lod);
    }

    /// Enqueue every loaded column's not-yet-meshed sections (re-meshing the
    /// whole column on a lod/content change). Like vanilla, every section in
    /// render distance meshes regardless of visibility — occlusion gates only
    /// drawing — and the queue orders the backlog nearest-first. Runs every
    /// frame to drain it.
    pub fn rescan_mesh_jobs(
        &mut self,
        player_chunk: ChunkPos,
        frustum: &[[f32; 4]; 6],
        eye: glam::DVec3,
    ) {
        self.meshing
            .rescan_mesh_jobs(self.chunk_store.loaded_set(), player_chunk, frustum, eye);
    }
}

pub enum GameUpdateResult {
    None,
    ManualDisconnect,
    Disconnected { reason: String },
}

enum ResultKind {
    Fps,
    ChunkLoad,
}

/// Carry out the button/dismiss action a benchmark result overlay reported,
/// targeting the matching benchmark's result/upload fields.
fn apply_result_action(
    action: common::ResultAction,
    kind: ResultKind,
    status: Option<UploadStatus>,
    json: String,
    core: &AppCore,
    gfx: &Gfx,
    game: &mut GameState,
) {
    match action {
        common::ResultAction::StartUpload => {
            let handle = Some(upload_result(&core.tokio_rt, json));
            match kind {
                ResultKind::Fps => game.benchmark_upload = handle,
                ResultKind::ChunkLoad => game.chunk_load_upload = handle,
            }
        }
        common::ResultAction::Recopy => {
            if let Some(UploadStatus::Done { url, .. }) = status {
                common::set_clipboard(&url);
            }
        }
        common::ResultAction::Dismiss => {
            match kind {
                ResultKind::Fps => {
                    game.benchmark_result = None;
                    game.benchmark_upload = None;
                }
                ResultKind::ChunkLoad => {
                    game.chunk_load_result = None;
                    game.chunk_load_upload = None;
                }
            }
            core.apply_cursor_grab(&gfx.window, Some(game));
        }
        common::ResultAction::None => {}
    }
}

/// Set the active render distance (the persisted menu value) and push it to the
/// server — used by the chunk-load benchmark as it ramps the distance up and
/// down.
fn apply_render_distance(
    core: &mut AppCore,
    game: &mut GameState,
    connection: &ConnectionHandle,
    rd: u32,
) {
    core.menu.render_distance = rd;
    game.sync_render_distance(connection, rd);
}

/// Predict each container click locally (instant UI + drag preview), then send
/// the predicted diff as `HashedStack`es so the server suppresses corrections
/// when the prediction is right (vanilla lockstep).
fn send_container_clicks(
    game: &mut GameState,
    connection: &ConnectionHandle,
    ops: Vec<azalea_inventory::operations::ClickOperation>,
) {
    use azalea_inventory::ItemStack;
    use azalea_inventory::operations::{
        ClickOperation, QuickCraftClick, QuickCraftKind, QuickCraftStatus,
    };
    use azalea_protocol::packets::game::s_container_click::{
        HashedStack, ServerboundContainerClick,
    };

    use crate::player::menu_click;

    let (container_id, kind, state_id) = match &game.open_container {
        Some(c) => (c.id, c.screen.click_kind(), c.state_id),
        None => (0, ContainerKind::Player, game.inventory_state_id),
    };

    let mut drag_kind = QuickCraftKind::Left;
    let mut drag_slots: Vec<u16> = Vec::new();
    for op in &ops {
        let (changed, carried): (Vec<(u16, ItemStack)>, ItemStack) = match op {
            ClickOperation::QuickCraft(QuickCraftClick {
                kind: qc_kind,
                status,
            }) => match status {
                QuickCraftStatus::Start => {
                    drag_kind = qc_kind.clone();
                    drag_slots.clear();
                    (Vec::new(), game.cursor_item.clone())
                }
                QuickCraftStatus::Add { slot } => {
                    drag_slots.push(*slot);
                    (Vec::new(), game.cursor_item.clone())
                }
                QuickCraftStatus::End => {
                    let (changed, remainder) = menu_click::drag_distribution(
                        kind,
                        game.menu_slots(),
                        &game.cursor_item,
                        &drag_kind,
                        &drag_slots,
                    );
                    for (s, item) in &changed {
                        game.set_menu_slot(*s as usize, item.clone());
                    }
                    game.cursor_item = remainder.clone();
                    (changed, remainder)
                }
            },
            other => {
                let mut cursor = std::mem::take(&mut game.cursor_item);
                let changed = menu_click::apply_click(kind, game.menu_slots(), &mut cursor, other);
                game.cursor_item = cursor;
                for (s, item) in &changed {
                    game.set_menu_slot(*s as usize, item.clone());
                }
                (changed, game.cursor_item.clone())
            }
        };
        let mut click = ServerboundContainerClick {
            container_id,
            state_id,
            slot_num: op.slot_num().map(|s| s as i16).unwrap_or(-999),
            button_num: op.button_num(),
            click_type: op.click_type(),
            changed_slots: Default::default(),
            carried_item: HashedStack::from_item_stack(&carried, &game.registries),
        };
        for (s, item) in &changed {
            click
                .changed_slots
                .insert(*s, HashedStack::from_item_stack(item, &game.registries));
        }
        connection
            .packet_tx
            .send(ServerboundGamePacket::ContainerClick(click));
    }
}

pub fn update_game(
    core: &mut AppCore,
    dt: f32,
    raw_dt: f32,
    gfx: &mut Gfx,
    connection: &ConnectionHandle,
    game: &mut GameState,
) -> GameUpdateResult {
    // Snapshot last frame's phase timings before this frame overwrites them: they
    // align with `raw_dt`, which measures the previous frame's full duration.
    let frame_start = std::time::Instant::now();
    let prev_phases = game.last_update_phases;

    // Position the audio listener at the player's head and push current
    // volumes before draining sound packets this frame.
    let listener_pos = game.player.eye_pos();
    core.audio
        .set_listener(listener_pos, game.player.look_dir.y_rot_deg());
    core.audio.set_volumes(core.menu.category_volumes());
    gfx.renderer.set_vsync(core.menu.vsync);

    let disconnect_reason =
        core.drain_network_events(connection, None, &mut gfx.renderer, &gfx.window, game);
    if let Some(reason) = disconnect_reason {
        return GameUpdateResult::Disconnected { reason };
    }

    // Sky time ticks unconditionally so it keeps flowing in menus;
    // server SetTime packets reconcile drift.
    core.time_tick_accumulator = (core.time_tick_accumulator + dt).min(1.0);
    while core.time_tick_accumulator >= TICK_RATE {
        game.sky_state.day_time = game.sky_state.day_time.wrapping_add(1);
        game.sky_state.game_time = game.sky_state.game_time.wrapping_add(1);
        core.time_tick_accumulator -= TICK_RATE;
    }

    if game.input_live() && game.chunk_load_bench.is_none() {
        gfx.renderer.update_camera(&mut core.input, dt);
    }

    // Menus never pause the simulation; tick_physics substitutes neutral input.
    core.tick_accumulator += dt;
    while core.tick_accumulator >= TICK_RATE {
        game.tick_count = game.tick_count.wrapping_add(1);
        core.tick_physics(&mut gfx.renderer, connection, game);
        game.item_entity_store.tick(&game.chunk_store);
        game.particle_store.tick(&game.chunk_store);
        game.block_entity_anim.tick();
        if let Some(c) = &mut game.open_container
            && let Some(state) = &mut c.enchant
        {
            state.tick(&c.slots, &c.data);
            // Vanilla `EnchantmentScreen.containerTick` keeps the XP bar
            // prioritized while the screen is open.
            game.xp_display_start_tick = game.tick_count as i64;
        }
        core.tick_accumulator -= TICK_RATE;
    }

    // Once per frame after the frame's ticks, where vanilla `Minecraft.runTick`
    // calls `level.update()`.
    let t_light = std::time::Instant::now();
    game.update_light();
    game.last_update_phases.light_ms = t_light.elapsed().as_secs_f32() * 1000.0;

    let partial_tick = core.tick_accumulator / TICK_RATE;

    let typed = core.input.drain_typed_chars();
    let backspace = core.input.backspace_pressed();
    let enter = core.input.enter_pressed();
    let tab = core.input.tab_pressed();
    let shift = core.input.shift_held();
    if let Some(msg) = game.chat.handle_key_input(
        &typed,
        backspace,
        enter,
        tab,
        shift,
        game.command_tree.as_deref(),
    ) {
        core.send_chat_message(connection, msg);
        core.apply_cursor_grab(&gfx.window, Some(game));
    }
    if let Some((id, command)) = game.chat.take_suggestion_request() {
        connection
            .packet_tx
            .send(ServerboundGamePacket::CommandSuggestion(
                ServerboundCommandSuggestion { id, command },
            ));
    }

    core.input.text_capture = game.wants_text_input();

    let mut close_inventory = false;
    let mut pause_action = PauseAction::None;
    let mut death_action = DeathAction::None;

    gfx.renderer.sync_camera_pos(
        game.player
            .prev_eye_pos()
            .lerp(game.player.eye_pos(), partial_tick as f64),
    );

    // Bound the drain: a load burst can ready thousands of results at once,
    // and moving them all in one frame spikes it. Leftovers stay in the
    // channel for the next frame; the upload's staging budget throttles
    // downstream anyway.
    const DRAIN_CAP: usize = 1024;
    let t_drain = std::time::Instant::now();
    for mesh in game.meshing.drain_results().take(DRAIN_CAP) {
        // Stale meshes count too: worker time spent is worker time spent.
        if let Some(bench) = &mut game.chunk_load_bench {
            bench.record_mesh(mesh.queue_ms, mesh.mesh_ms);
        }
        if game.meshing.is_stale(&mesh) {
            continue;
        }
        gfx.renderer.mesh_queue.push_back(mesh);
    }
    game.last_update_phases.mesh_drain_ms = t_drain.elapsed().as_secs_f32() * 1000.0;

    let t_upload = std::time::Instant::now();
    gfx.renderer.stage_mesh_batch();
    game.last_update_phases.upload_ms = t_upload.elapsed().as_secs_f32() * 1000.0;
    game.last_update_phases.upload_reclaim_ms = gfx.renderer.last_upload_reclaim_ms();

    // Per-frame FOV interpolation; set before the frustum/view-projection reads.
    gfx.renderer.set_render_partial_tick(partial_tick);

    // Plain lerp (vanilla getInterpolatedWalkDistance); the forward-extrapolating
    // camera variant judders across tick boundaries when per-tick speed varies.
    let bob_walk = game
        .player
        .prev_walk_dist
        .lerp(game.player.walk_dist, partial_tick);
    let bob_amount = game.player.prev_bob.lerp(game.player.bob, partial_tick);
    gfx.renderer
        .set_view_bob(bob_walk, bob_amount, core.menu.view_bobbing);
    gfx.renderer.update_third_person_distance(
        game.player
            .prev_eye_pos()
            .lerp(game.player.eye_pos(), partial_tick as f64),
        &game.chunk_store,
    );

    // Esc cancels a running benchmark: restore the render distance it changed.
    if std::mem::take(&mut game.chunk_load_abort)
        && let Some(bench) = game.chunk_load_bench.take()
    {
        apply_render_distance(core, game, connection, bench.original_rd());
    }

    // Watch the chunk-load benchmark from straight above, framed to its load
    // radius.
    match &game.chunk_load_bench {
        Some(bench) => {
            let radius = bench.effective_rd().max(1) as f32 * 16.0;
            gfx.renderer.set_top_down_radius(radius);
        }
        None => gfx.renderer.clear_top_down(),
    }

    let sw = gfx.renderer.screen_width() as f32;
    let sh = gfx.renderer.screen_height() as f32;
    let gs = hud::gui_scale(sw, sh, core.menu.gui_scale_setting);

    let mut elements: Vec<MenuElement> = Vec::new();

    let debug = if game.show_debug {
        Some(hud::DebugInfo {
            fps: gfx.fps_counter.display_fps(),
            position: *game.player.position,
            y_rot_deg: gfx.renderer.camera_look_dir().y_rot_deg(),
            x_rot_deg: gfx.renderer.camera_look_dir().x_rot_deg(),
            target_block: game.interaction.target.and_then(|t| {
                let HitResult::Block(t) = t else {
                    return None;
                };
                let state =
                    game.chunk_store
                        .get_block_state(t.block_pos.x, t.block_pos.y, t.block_pos.z);
                let props = crate::world::block::block_properties(state)
                    .entries()
                    .map(|(k, v)| format!("{k}: {v}"))
                    .collect();
                Some((
                    t.block_pos,
                    t.face,
                    crate::world::block::block_id(state).to_string(),
                    props,
                ))
            }),
            chunk_count: gfx.renderer.loaded_chunk_count(),
            sections_drawn: gfx.renderer.sections_drawn(),
            occlusion_on: true,
            sections_total: game.chunk_store.section_count() as u32
                * game.chunk_store.loaded_set().len() as u32,
            gpu_name: gfx.renderer.gpu_name(),
            vulkan_version: gfx.renderer.vulkan_version(),
            screen_w: gfx.renderer.screen_width(),
            screen_h: gfx.renderer.screen_height(),
            timings: Some(hud::FrameTimings {
                frame_ms: gfx.renderer.last_timings().frame_ms(),
                cull_ms: gfx.renderer.last_timings().cull_ms(),
                gui_bake_ms: gfx.renderer.last_timings().gui_bake_ms(),
                terrain_ms: gfx.renderer.last_timings().terrain_ms(),
                entities_ms: gfx.renderer.last_timings().entities_ms(),
                translucent_ms: gfx.renderer.last_timings().translucent_ms(),
                ui_ms: gfx.renderer.last_timings().ui_ms(),
                hiz_ms: gfx.renderer.last_timings().hiz_ms(),
                visibility_ms: gfx.renderer.last_timings().visibility_ms(),
            }),
        })
    } else {
        None
    };

    // The chunk-load benchmark renders a clean top-down view: only terrain, no HUD,
    // entities/player, held item, clouds, or weather — and skipping them also keeps
    // the measured frame times honest.
    let benchmark_running = game.chunk_load_bench.is_some();
    if !benchmark_running {
        let is_survival = crate::player::is_survival(game.player.game_mode);
        let air_bubbles = hud::air_bubbles(game.player.air_supply, game.player.eyes_in_water)
            .filter(|_| is_survival);
        // TODO: gate the pop sound on HUD visibility if a hide-HUD toggle (F1) is
        // added.
        if let Some(bubbles) = &air_bubbles {
            if !game.player.eyes_in_water {
                game.last_bubble_pop_sound_played = 0;
            } else if bubbles.is_popping && game.last_bubble_pop_sound_played != bubbles.popping_pos
            {
                let volume = 0.5 + 0.1 * (bubbles.empty - 3 + 1).max(0) as f32;
                let pitch = 1.0 + 0.1 * (bubbles.empty - 5 + 1).max(0) as f32;
                core.audio.play_world_sound(
                    &SoundRef::Event("ui.hud.bubble_pop".into()),
                    CATEGORY_PLAYERS,
                    game.player.position,
                    volume,
                    pitch,
                    fastrand::u64(..),
                );
                game.last_bubble_pop_sound_played = bubbles.popping_pos;
            }
        }
        // Contextual bar choice (vanilla Hud.nextContextualInfoState): the
        // locator bar takes the XP bar's slot while waypoints are tracked,
        // except for 100 ticks after an XP change.
        let show_locator = game.waypoints.has_waypoints()
            && !(is_survival && game.xp_display_start_tick + 100 > game.tick_count as i64);
        let locator_dots = if show_locator {
            let (yaw_deg, pitch_deg) = gfx.renderer.camera_effective_look_deg();
            let cam = crate::world::waypoints::WaypointCamera {
                position: gfx.renderer.camera_render_position(),
                yaw_deg,
                pitch_deg,
                view_rot_proj: gfx.renderer.locator_projection(),
                fov_y_deg: gfx.renderer.camera_fov_degrees(),
            };
            let store = &game.entity_store;
            let entity_eye_pos = |uuid: &uuid::Uuid| {
                store.player_by_uuid(uuid).map(|e| {
                    let feet = e.prev_position.lerp(e.position, partial_tick as f64);
                    // TODO: swimming/gliding eye height needs entity pose data.
                    let eye_height = if e.is_crouching {
                        crate::player::CROUCH_EYE_HEIGHT
                    } else {
                        crate::player::STANDING_EYE_HEIGHT
                    };
                    let block_pos = glam::IVec3::new(
                        e.position.x.floor() as i32,
                        e.position.y.floor() as i32,
                        e.position.z.floor() as i32,
                    );
                    (block_pos, *feet + glam::DVec3::new(0.0, eye_height, 0.0))
                })
            };
            game.waypoints.extract_dots(
                &cam,
                *game.player.position,
                core.user.uuid,
                &entity_eye_pos,
            )
        } else {
            Vec::new()
        };
        let bar = if show_locator {
            hud::ContextualBarKind::Locator {
                dots: &locator_dots,
                arrow_frame_1: game.tick_count % 14 >= 10,
            }
        } else if is_survival {
            hud::ContextualBarKind::Experience
        } else {
            hud::ContextualBarKind::Empty
        };
        hud::build_hud(
            &mut elements,
            sw,
            sh,
            core.input.selected_slot(),
            game.player.health,
            game.player.food,
            game.player.armor,
            air_bubbles,
            game.player.eyes_in_water,
            game.tick_count,
            game.player.experience_level,
            game.player.experience_progress,
            bar,
            game.player.game_mode,
            game.player.inventory.hotbar_slots(),
            gfx.renderer.is_first_person(),
            debug.as_ref(),
            core.menu.gui_scale_setting,
            &|t, s| gfx.renderer.menu_text_width(t, s),
        );
    }

    if core.input.performing_action(input::Action::ViewPlayerList)
        && !game.paused
        && !game.gui_open()
        && !game.chat.is_open()
        && !game.dead
    {
        let r = &gfx.renderer;
        crate::ui::player_tab::build_player_tab_overlay(
            &mut elements,
            sw,
            &game.tab_list,
            gs,
            &|t, s| r.menu_text_width(t, s),
        );
    }

    if let Some(ref mut bench) = game.benchmark {
        let entity_count = game.entity_store.living.len() as u32;
        let done = bench.record_frame(
            raw_dt * 1000.0,
            gfx.renderer.last_timings(),
            gfx.renderer.loaded_chunk_count(),
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
            let bench = game.benchmark.take().unwrap();
            game.benchmark_result = Some(bench.finish(&core.data_dirs.game_dir));
            game.benchmark_upload = None;
            core.apply_cursor_grab(&gfx.window, Some(game));
        }
    }

    if let Some(ref result) = game.benchmark_result {
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
            format!("Min: {:.0} / Max: {:.0}", result.min_fps, result.max_fps),
            format!(
                "Frame: {:.2}ms / P1: {:.2}ms / P99: {:.2}ms",
                result.avg_frame_ms, result.p1_frame_ms, result.p99_frame_ms
            ),
            format!(
                "Cull: {:.2}ms / GuiBake: {:.2}ms / Terrain: {:.2}ms",
                result.avg_cull_ms, result.avg_gui_bake_ms, result.avg_terrain_ms
            ),
            format!(
                "Entities: {:.2}ms / Translucent: {:.2}ms / UI: {:.2}ms",
                result.avg_entities_ms, result.avg_translucent_ms, result.avg_ui_ms
            ),
            format!(
                "{} spikes (>{:.0}ms) - Saved to benchmark.json",
                result.spike_count, 8.0
            ),
        ];
        let json = serde_json::to_string_pretty(result).unwrap_or_default();
        let status = game
            .benchmark_upload
            .as_ref()
            .map(|h| h.lock().unwrap().clone());
        let action = common::push_results_overlay(
            &mut elements,
            sw,
            sh,
            gs,
            sh / 2.0 - 90.0,
            "Benchmark Complete",
            &lines,
            status.as_ref(),
            core.input.cursor_pos(),
            core.input.left_just_pressed(),
            core.input.escape_pressed(),
        );
        apply_result_action(action, ResultKind::Fps, status, json, core, gfx, game);
    }

    if let Some(mut bench) = game.chunk_load_bench.take() {
        let count = gfx.renderer.loaded_chunk_count();
        let client_cached = game.chunk_store.loaded_set().len() as u32;
        match bench.update(
            count,
            client_cached,
            raw_dt * 1000.0,
            gfx.renderer.last_timings(),
            prev_phases,
        ) {
            ChunkLoadStep::Wait => {
                game.chunk_load_bench = Some(bench);
            }
            ChunkLoadStep::StartTiming => {
                gfx.renderer.clear_chunk_meshes();
                game.meshing.clear();
                game.chunk_load_bench = Some(bench);
            }
            ChunkLoadStep::Done(result) => {
                apply_render_distance(core, game, connection, bench.original_rd());
                tracing::info!(
                    "Chunk load RD {} (effective {}): {} chunks in {:.2}s ({:.0} chunks/s), \
                     first chunk {:.2}s, frame avg {:.1}ms / worst {:.1}ms",
                    result.target_rd,
                    result.effective_rd,
                    result.chunk_count,
                    result.load_secs,
                    result.chunks_per_sec,
                    result.time_to_first_secs,
                    result.avg_frame_ms,
                    result.worst_frame_ms,
                );
                result.save(&core.data_dirs.game_dir);
                game.chunk_load_result = Some(*result);
                game.chunk_load_upload = None;
                core.apply_cursor_grab(&gfx.window, Some(game));
            }
        }
    }

    if let Some(ref bench) = game.chunk_load_bench {
        let progress = format!("Run {}/{}", bench.current_run(), bench.total_runs());
        let mut info_lines = Vec::new();
        info_lines.push(format!("=== CHUNK LOAD BENCHMARK ({}) ===", progress));
        if bench.resetting() {
            let expected = (bench.target_rd() * 2 + 1).pow(2);
            info_lines.push(format!(
                "Phase: Waiting for server chunks (Target RD: {})",
                bench.target_rd()
            ));
            info_lines.push(format!(
                "Chunks (Client Cache): {} / ~{}",
                bench.client_cached(),
                expected
            ));
            info_lines.push(format!(
                "Elapsed Wait Time: {:.2}s",
                bench.reset_elapsed_secs()
            ));
        } else {
            let expected = bench.client_cached();
            info_lines.push(format!(
                "Phase: Timing meshing/upload (Target RD: {})",
                bench.target_rd()
            ));
            info_lines.push(format!(
                "Chunks (GPU Mesh): {} / {}",
                bench.loaded(),
                expected
            ));
            info_lines.push(format!("Chunks (Client Cache): {}", bench.client_cached()));
            info_lines.push(format!(
                "Pending Mesh Jobs: {}",
                game.meshing.pending_jobs()
            ));
            info_lines.push(format!(
                "Elapsed Load Time: {:.2}s",
                bench.load_elapsed_secs()
            ));
        }
        info_lines.push(String::new());
        info_lines.push(format!(
            "FPS: {:.1} (avg: {:.1} ms, worst: {:.1} ms)",
            1.0 / raw_dt.max(0.0001),
            bench.avg_frame_ms(),
            bench.worst_frame_ms()
        ));
        info_lines.push(format!("GPU: {}", gfx.renderer.gpu_name()));
        info_lines.push(format!(
            "System Threads: {}",
            std::thread::available_parallelism()
                .map(|n| n.get())
                .unwrap_or(1)
        ));
        let padding = 10.0 * gs;
        let line_height = 14.0 * gs;
        let box_w = 320.0 * gs;
        let box_h = (info_lines.len() as f32 * 14.0 + 8.0) * gs;
        let box_x = 10.0 * gs;
        let box_y = 10.0 * gs;
        elements.push(MenuElement::Rect {
            x: box_x,
            y: box_y,
            w: box_w,
            h: box_h,
            corner_radius: 4.0 * gs,
            color: [0.0, 0.0, 0.0, 0.75],
        });
        for (idx, line) in info_lines.into_iter().enumerate() {
            elements.push(MenuElement::Text {
                x: box_x + padding,
                y: box_y + padding + (idx as f32 * line_height),
                text: line,
                scale: 6.0 * gs,
                color: [1.0, 1.0, 1.0, 1.0],
                centered: false,
            });
        }
    }

    if let Some(ref result) = game.chunk_load_result {
        let rd_line = if result.effective_rd != result.target_rd {
            format!(
                "Render Distance: {} (server-capped to {})",
                result.target_rd, result.effective_rd
            )
        } else if result.achieved_rd < result.target_rd {
            format!(
                "Render Distance: {} (server loaded ~{})",
                result.target_rd, result.achieved_rd
            )
        } else {
            format!("Render Distance: {}", result.target_rd)
        };
        let mut lines = vec![
            rd_line,
            format!(
                "Loaded {} chunks in {:.2}s (avg of {} runs)",
                result.chunk_count, result.load_secs, result.runs
            ),
            format!(
                "{:.0} chunks/sec - first chunk in {:.2}s",
                result.chunks_per_sec, result.time_to_first_secs
            ),
            format!(
                "Frame while loading: avg {:.1}ms / worst {:.1}ms",
                result.avg_frame_ms, result.worst_frame_ms
            ),
            format!("GPU: {} / Vulkan {}", result.gpu, result.vulkan),
            format!(
                "{} {} / {} threads / v{} / {}x{}",
                result.os,
                result.arch,
                result.cpu_threads,
                result.version,
                result.resolution[0],
                result.resolution[1],
            ),
            "Saved to chunk_load.json".to_string(),
        ];
        if crate::benchmark::is_debug_build() {
            lines.push("Debug build - frame times are not representative".to_string());
        }
        let json = serde_json::to_string_pretty(result).unwrap_or_default();
        let status = game
            .chunk_load_upload
            .as_ref()
            .map(|h| h.lock().unwrap().clone());
        let action = common::push_results_overlay(
            &mut elements,
            sw,
            sh,
            gs,
            sh / 2.0 - 100.0,
            "Chunk Load Complete",
            &lines,
            status.as_ref(),
            core.input.cursor_pos(),
            core.input.left_just_pressed(),
            core.input.escape_pressed(),
        );
        apply_result_action(action, ResultKind::ChunkLoad, status, json, core, gfx, game);
    }

    if game.options_from_game {
        let menu_input = core.build_menu_input();
        let r = &gfx.renderer;
        let result = core
            .menu
            .build(sw, sh, &menu_input, |t, s| r.menu_text_width(t, s));
        elements.extend(result.elements);
        core.input.clear_just_pressed_actions();
    } else if game.dead {
        let cursor = core.input.cursor_pos();
        let clicked = core.input.left_just_pressed() && !game.respawn_sent;
        death_action = if game.death_confirm {
            death::build_death_confirm(
                &mut elements,
                sw,
                sh,
                cursor,
                clicked,
                gs,
                game.death_confirm_instant.elapsed().as_secs_f32() >= 1.0,
            )
        } else {
            let buttons_enabled =
                !game.respawn_sent && game.death_instant.elapsed().as_secs_f32() >= 1.0;
            let r = &gfx.renderer;
            death::build_death_screen(
                &mut elements,
                sw,
                sh,
                cursor,
                clicked,
                gs,
                &game.death_message,
                game.player.score,
                buttons_enabled,
                &|t, s| r.menu_text_width(t, s),
            )
        };
        core.input.clear_just_pressed_actions();
    } else if game.paused {
        let cursor = core.input.cursor_pos();
        let clicked = core.input.left_just_pressed();
        pause_action = pause::build_pause_menu(
            &mut elements,
            sw,
            sh,
            cursor,
            clicked,
            gs,
            game.pause_screen,
            game.server_render_distance,
        );
        core.input.clear_just_pressed_actions();
    }

    let mut player_preview = None;
    let mut book_preview = None;
    if game.inventory_open || game.open_container.is_some() {
        let input = crate::ui::container::ContainerInput {
            left_pressed: core.input.left_just_pressed(),
            right_pressed: core.input.right_just_pressed(),
            middle_pressed: core.input.middle_just_pressed(),
            left_held: core.input.left_held(),
            right_held: core.input.right_held(),
            shift: core.input.shift_held(),
        };
        // The anvil rename field consumes this frame's typing; a changed
        // accepted name goes to the server (vanilla `onNameChanged`).
        if let Some(c) = &mut game.open_container
            && let Some(state) = &mut c.anvil
            && let Some(name) = crate::ui::anvil::update_rename(state, &c.slots, &typed, backspace)
        {
            use azalea_protocol::packets::game::s_rename_item::ServerboundRenameItem;
            connection
                .packet_tx
                .send(ServerboundGamePacket::RenameItem(ServerboundRenameItem {
                    name,
                }));
        }
        let (clicked_outside, ops) = if let Some(container) = &game.open_container {
            let result = match container.screen {
                ContainerScreen::CraftingTable => crate::ui::crafting_table::build_crafting_table(
                    &mut elements,
                    sw,
                    sh,
                    core.input.cursor_pos(),
                    &input,
                    &container.slots,
                    &container.title,
                    &game.cursor_item,
                    &mut game.inv_drag,
                    &mut game.inv_last_click,
                    gs,
                ),
                ContainerScreen::Furnace(variant) => crate::ui::furnace::build_furnace(
                    &mut elements,
                    sw,
                    sh,
                    core.input.cursor_pos(),
                    &input,
                    variant,
                    &container.slots,
                    &container.data,
                    &container.title,
                    &game.cursor_item,
                    &mut game.inv_drag,
                    &mut game.inv_last_click,
                    gs,
                    &|t, s| gfx.renderer.menu_text_width(t, s),
                ),
                ContainerScreen::Chest { rows } => crate::ui::chest::build_chest(
                    &mut elements,
                    sw,
                    sh,
                    core.input.cursor_pos(),
                    &input,
                    rows,
                    &container.slots,
                    &container.title,
                    &game.cursor_item,
                    &mut game.inv_drag,
                    &mut game.inv_last_click,
                    gs,
                ),
                ContainerScreen::ShulkerBox => crate::ui::chest::build_shulker_box(
                    &mut elements,
                    sw,
                    sh,
                    core.input.cursor_pos(),
                    &input,
                    &container.slots,
                    &container.title,
                    &game.cursor_item,
                    &mut game.inv_drag,
                    &mut game.inv_last_click,
                    gs,
                ),
                ContainerScreen::Anvil => crate::ui::anvil::build_anvil(
                    &mut elements,
                    sw,
                    sh,
                    core.input.cursor_pos(),
                    &input,
                    &container.slots,
                    &container.data,
                    &container.title,
                    container.anvil.as_ref().expect("anvil screen has state"),
                    game.player.experience_level,
                    crate::player::is_creative(game.player.game_mode),
                    &game.cursor_item,
                    &mut game.inv_drag,
                    &mut game.inv_last_click,
                    gs,
                    &|t, s| gfx.renderer.menu_text_width(t, s),
                ),
                ContainerScreen::Enchantment => {
                    let result = crate::ui::enchantment::build_enchantment(
                        &mut elements,
                        sw,
                        sh,
                        core.input.cursor_pos(),
                        &input,
                        &container.slots,
                        &container.data,
                        &container.title,
                        container
                            .enchant
                            .as_ref()
                            .expect("enchantment screen has state"),
                        partial_tick,
                        &game.registries,
                        game.player.experience_level,
                        crate::player::is_creative(game.player.game_mode),
                        &game.cursor_item,
                        &mut game.inv_drag,
                        &mut game.inv_last_click,
                        gs,
                        &|t, s| gfx.renderer.menu_text_width(t, s),
                        &|t, s| gfx.renderer.menu_text_width_sga(t, s),
                    );
                    book_preview = Some(result.book);
                    result.container
                }
            };
            if let Some(button_id) = result.button {
                use azalea_protocol::packets::game::s_container_button_click::ServerboundContainerButtonClick;
                connection
                    .packet_tx
                    .send(ServerboundGamePacket::ContainerButtonClick(
                        ServerboundContainerButtonClick {
                            container_id: container.id,
                            button_id,
                        },
                    ));
            }
            (result.clicked_outside, result.ops)
        } else {
            let result = crate::ui::inventory::build_inventory(
                &mut elements,
                sw,
                sh,
                core.input.cursor_pos(),
                &input,
                &game.player.inventory,
                &game.cursor_item,
                &mut game.inv_drag,
                &mut game.inv_last_click,
                gs,
            );
            player_preview = Some(result.player_preview);
            (result.clicked_outside, result.ops)
        };
        close_inventory = clicked_outside;
        send_container_clicks(game, connection, ops);
        core.input.clear_just_pressed_actions();
    }

    if game.creative_inventory_open {
        let cursor = core.input.cursor_pos();
        let clicked = core.input.left_just_pressed();
        let middle_clicked = core.input.middle_just_pressed();
        let right_clicked = core.input.right_just_pressed();
        let scroll_delta = core.input.consume_menu_scroll();
        // `typed`/`backspace` come from the frame's single drain up top; a
        // second drain here would always read empty.
        let action = crate::ui::creative_inventory::build_creative_inventory(
            &mut elements,
            &mut game.creative_state,
            sw,
            sh,
            cursor,
            clicked,
            middle_clicked,
            right_clicked,
            scroll_delta,
            &typed,
            backspace,
            &game.player.inventory,
            gs,
            game.advanced_item_tooltips,
            core.input.left_held(),
            core.input.right_held(),
            &|t, s| gfx.renderer.menu_text_width(t, s),
        );
        use azalea_protocol::packets::game::s_set_creative_mode_slot::ServerboundSetCreativeModeSlot;
        let mut set_creative_slot = |slot_num: u16, item: azalea_inventory::ItemStack| {
            if crate::player::is_creative(game.player.game_mode) {
                connection
                    .packet_tx
                    .send(ServerboundGamePacket::SetCreativeModeSlot(
                        ServerboundSetCreativeModeSlot {
                            slot_num,
                            item_stack: item.clone(),
                        },
                    ));
                // Optimistic local update; the server echoes via ContainerSetSlot.
                game.player.inventory.set_slot(slot_num as usize, item);
            }
        };
        match action {
            crate::ui::creative_inventory::CreativeAction::Close => {
                close_inventory = true;
            }
            crate::ui::creative_inventory::CreativeAction::SetSlot(slot_num, item) => {
                set_creative_slot(slot_num, item);
            }
            crate::ui::creative_inventory::CreativeAction::SetSlots(items) => {
                for (slot_num, item) in items {
                    set_creative_slot(slot_num, item);
                }
            }
            crate::ui::creative_inventory::CreativeAction::None => {}
        }
        core.input.clear_just_pressed_actions();
    }

    game.chat.build(&mut elements, sw, sh, gs, &|t, s| {
        gfx.renderer.menu_text_width(t, s)
    });

    // Chat consumes keys, not clicks; nothing else clears them while only chat
    // is open, so drop them here to keep stray clicks out of the live sim.
    if game.chat.is_open() {
        core.input.clear_just_pressed_actions();
    }

    let swing_progress = game.interaction.get_swing_progress(partial_tick);
    let use_anim = game.interaction.use_animation(partial_tick);
    let destroy_info = game.interaction.destroy_stage().map(|(pos, stage)| {
        let state = game.chunk_store.get_block_state(pos.x, pos.y, pos.z);
        (pos, stage, state)
    });

    let mut entity_renders: Vec<EntityRenderInfo> = if benchmark_running {
        Vec::new()
    } else {
        game.entity_store
            .living
            .iter()
            .map(|(&entity_id, e)| {
                let interp_pos = e.prev_position.lerp(e.position, partial_tick as f64);
                let extras = entity_extras(entity_id, e, partial_tick);
                EntityRenderInfo {
                    position: interp_pos,
                    head_y_rot_deg: lerp_angle(
                        e.prev_head_y_rot_deg,
                        e.head_y_rot_deg,
                        partial_tick,
                    ),
                    head_x_rot_deg: e
                        .prev_look_dir
                        .x_rot_deg()
                        .lerp(e.look_dir.x_rot_deg(), partial_tick),
                    body_y_rot_deg: lerp_angle(
                        e.prev_body_y_rot_deg,
                        e.body_y_rot_deg,
                        partial_tick,
                    ),
                    is_baby: e.is_baby,
                    is_crouching: e.is_crouching,
                    walk_anim_pos: {
                        let scale = if e.is_baby { 3.0 } else { 1.0 };
                        (e.walk_anim_pos - e.walk_anim_speed * (1.0 - partial_tick)) * scale
                    },
                    walk_anim_speed: (e.prev_walk_anim_speed
                        + (e.walk_anim_speed - e.prev_walk_anim_speed) * partial_tick)
                        .min(1.0),
                    entity_kind: e.entity_type,
                    player_uuid: e.player_uuid,
                    variant_index: extras.variant_index,
                    overlay_tints: extras.overlay_tints,
                    overlay_variants: extras.overlay_variants,
                    is_unhappy: e.unhappy_counter > 0,
                    head_y_offset: extras.head_y_offset,
                    head_x_rot_deg_override: extras.head_x_rot_deg_override,
                    has_red_overlay: e.hurt_time > 0,
                    aggressive: e.aggressive,
                    age_in_ticks: e.age_in_ticks as f32 + partial_tick,
                    attack_time: e.swing_progress(partial_tick),
                    skip_cull: false,
                }
            })
            .collect()
    };

    if !benchmark_running && !gfx.renderer.is_first_person() {
        let interp_pos = game
            .player
            .prev_position
            .lerp(game.player.position, partial_tick as f64);
        let interp_y_rot_deg = lerp_angle(
            game.player.prev_look_dir.y_rot_deg(),
            game.player.look_dir.y_rot_deg(),
            partial_tick,
        );
        entity_renders.push(EntityRenderInfo {
            position: interp_pos,
            head_y_rot_deg: interp_y_rot_deg,
            head_x_rot_deg: gfx.renderer.camera_look_dir().x_rot_deg(),
            body_y_rot_deg: interp_y_rot_deg, // TODO: proper body rotation affected by collisions
            is_baby: false,
            is_crouching: game.player.crouching,
            walk_anim_pos: game.player_walk_pos - game.player_walk_speed * (1.0 - partial_tick),
            walk_anim_speed: (game.player_prev_walk_speed
                + (game.player_walk_speed - game.player_prev_walk_speed) * partial_tick)
                .min(1.0),
            entity_kind: EntityKind::Player,
            player_uuid: Some(core.user.uuid),
            variant_index: 0,
            overlay_tints: [None; MAX_OVERLAYS],
            overlay_variants: [0; MAX_OVERLAYS],
            is_unhappy: false,
            head_y_offset: 0.0,
            head_x_rot_deg_override: None,
            has_red_overlay: false,
            aggressive: false,
            age_in_ticks: 0.0,
            attack_time: 0.0,
            skip_cull: true,
        });
    }

    let sky_partial_tick = (core.time_tick_accumulator / TICK_RATE).clamp(0.0, 1.0);
    let sky = crate::renderer::SkyState {
        day_time: game.sky_state.day_time,
        game_time: game.sky_state.game_time,
        rain_level: game.sky_state.rain_level,
        thunder_level: game.sky_state.thunder_level,
        partial_tick: sky_partial_tick,
    };

    if game.show_chunk_borders {
        gfx.renderer.update_chunk_borders(
            game.chunk_store.min_y(),
            game.chunk_store.min_y() + game.chunk_store.height() as i32,
        );
    }

    let item_renders = if benchmark_running {
        Vec::new()
    } else {
        build_item_render_infos(
            &game.item_entity_store,
            &game.chunk_store,
            *gfx.renderer.camera_pivot_position(),
            gfx.renderer.camera_anchor(),
            partial_tick,
        )
    };

    let block_entity_renders: Vec<crate::renderer::BlockEntityRenderInfo> = if benchmark_running {
        Vec::new()
    } else {
        game.chunk_store
            .block_entities
            .iter()
            .filter_map(|(pos, be)| {
                let state = game.chunk_store.get_block_state(pos.x, pos.y, pos.z);
                let id = crate::world::block::block_id(state);
                // A predicted break leaves a stale entry until the server
                // confirms; don't render entries whose block is gone.
                if !crate::world::block_entity::is_block_entity_block(id) {
                    return None;
                }
                let props = crate::world::block::block_properties(state);
                let variant = block_entity::variant_for_block(be.kind, id, props);
                let yaw = block_entity::yaw_for_block(be.kind, props);
                let openness_at = |p: &BlockPos| {
                    game.block_entity_anim
                        .container(p)
                        .map(|a| a.openness(partial_tick))
                        .unwrap_or(0.0)
                };
                let mut lid_open = openness_at(pos);
                // A double chest's lids follow the max openness of both halves
                // (vanilla opennessCombiner); the open block event only arrives
                // at the interacted half's position.
                if matches!(
                    be.kind,
                    BlockEntityKind::Chest | BlockEntityKind::TrappedChest
                ) && let Some((dx, dz)) = block_entity::chest_partner_offset(
                    props.get("facing").unwrap_or("north"),
                    props.get("type").unwrap_or("single"),
                ) {
                    let partner = BlockPos::new(pos.x + dx, pos.y, pos.z + dz);
                    lid_open = lid_open.max(openness_at(&partner));
                }
                Some(crate::renderer::BlockEntityRenderInfo {
                    pos: *pos,
                    kind: be.kind,
                    yaw,
                    variant,
                    lid_open,
                })
            })
            .collect()
    };

    let weather_columns = if benchmark_running {
        Vec::new()
    } else {
        build_weather_columns(
            &game.chunk_store,
            &game.biome_climate,
            gfx.renderer.camera_render_position(),
            sky.rain(),
        )
    };

    let particle_quads = if benchmark_running {
        Vec::new()
    } else {
        game.particle_store
            .extract(partial_tick, gfx.renderer.camera_anchor())
    };

    let effective_rd = if game.server_render_distance > 0 {
        core.menu.render_distance.min(game.server_render_distance)
    } else {
        core.menu.render_distance
    };

    let held_item = if benchmark_running {
        None
    } else {
        match game.player.inventory.hotbar_slots()[core.input.selected_slot() as usize] {
            azalea_inventory::ItemStack::Present(ref data) => {
                let name = crate::player::inventory::item_resource_name(data.kind);
                (name != "air").then(|| {
                    let light =
                        get_entity_light(&game.chunk_store, gfx.renderer.camera_pivot_position());
                    (name, light)
                })
            }
            _ => None,
        }
    };

    // Recompute after this frame's state changes (a finished benchmark releases
    // the cursor mid-frame), so the renderer doesn't re-hide it from a stale value.
    let hide_cursor = game.input_live() && !game.dead && core.input.is_cursor_captured();
    let t_render = std::time::Instant::now();
    if let Err(e) = gfx.renderer.render_world(
        &gfx.window,
        hide_cursor,
        elements,
        swing_progress,
        use_anim,
        held_item,
        destroy_info,
        game.show_chunk_borders,
        sky,
        &entity_renders,
        &item_renders,
        &block_entity_renders,
        &particle_quads,
        &weather_columns,
        if benchmark_running {
            crate::renderer::CloudMode::Off
        } else {
            core.menu.cloud_mode
        },
        effective_rd,
        player_preview,
        book_preview,
        game.player.eyes_in_water,
        game.chunk_store.shared.min_y(),
        game.chunk_store.shared.height(),
        core.menu.frustum_padding,
    ) {
        tracing::error!("Render error: {e}");
    }
    game.last_update_phases.render_cpu_ms = t_render.elapsed().as_secs_f32() * 1000.0;
    game.last_update_phases.meta_rebuild_ms = gfx.renderer.meta_rebuild_ms();
    // Whole-frame wall time (incl. render), read next frame to align with `raw_dt`.
    game.last_update_phases.update_ms = frame_start.elapsed().as_secs_f32() * 1000.0;

    if close_inventory {
        game.close_menu();
        game.close_creative_inventory();
        core.apply_cursor_grab(&gfx.window, Some(game));
    }

    // Tell the server when a container menu closes so it returns/drops the
    // cursor stack (and a crafting grid's contents).
    let open_menu = game.open_menu_id();
    if let Some(prev) = game.container_was_open
        && open_menu != Some(prev)
    {
        use azalea_protocol::packets::game::s_container_close::ServerboundContainerClose;
        connection
            .packet_tx
            .send(ServerboundGamePacket::ContainerClose(
                ServerboundContainerClose { container_id: prev },
            ));
    }
    game.container_was_open = open_menu;

    match death_action {
        DeathAction::Respawn => {
            game.death_confirm = false;
            core.send_respawn(connection, game);
        }
        DeathAction::TitleScreen => {
            return GameUpdateResult::ManualDisconnect;
        }
        DeathAction::ShowConfirm => {
            game.death_confirm = true;
            game.death_confirm_instant = Instant::now();
        }
        DeathAction::None => {}
    }

    match pause_action {
        PauseAction::Resume => {
            game.paused = false;
            core.apply_cursor_grab(&gfx.window, Some(game));
        }
        PauseAction::Options => {
            core.menu.open_options();
            game.options_from_game = true;
            core.apply_cursor_grab(&gfx.window, Some(game));
        }
        PauseAction::Disconnect => {
            return GameUpdateResult::ManualDisconnect;
        }
        PauseAction::OpenBenchmark => {
            game.pause_screen = PauseScreen::Benchmark;
        }
        PauseAction::OpenChunkLoader => {
            game.pause_screen = PauseScreen::ChunkLoader;
        }
        PauseAction::Back => {
            game.pause_screen = match game.pause_screen {
                PauseScreen::ChunkLoader => PauseScreen::Benchmark,
                _ => PauseScreen::Main,
            };
        }
        PauseAction::StartFpsBenchmark => {
            game.benchmark = Some(Benchmark::new(
                gfx.renderer.gpu_name(),
                gfx.renderer.screen_width(),
                gfx.renderer.screen_height(),
                core.menu.render_distance,
            ));
            game.benchmark_result = None;
            game.pause_screen = PauseScreen::Main;
            game.paused = false;
            core.apply_cursor_grab(&gfx.window, Some(game));
        }
        PauseAction::StartChunkLoad(rd) => {
            game.chunk_load_bench = Some(ChunkLoadBench::new(
                rd,
                core.menu.render_distance,
                game.server_render_distance,
                gfx.renderer.gpu_name(),
                gfx.renderer.vulkan_version(),
                gfx.renderer.screen_width(),
                gfx.renderer.screen_height(),
                [
                    game.player.position.x,
                    game.player.position.y,
                    game.player.position.z,
                ],
            ));
            game.chunk_load_result = None;
            game.pause_screen = PauseScreen::Main;
            game.paused = false;
            // Initialize the server request to the target render distance.
            apply_render_distance(core, game, connection, core.menu.render_distance);
            core.apply_cursor_grab(&gfx.window, Some(game));
        }
        PauseAction::ReportBugs => {
            let _ = open::that("https://github.com/PommeMC/Client/issues");
        }
        PauseAction::None => {}
    }

    if game.options_from_game {
        if core.menu.render_distance != game.last_render_distance {
            game.sync_render_distance(connection, core.menu.render_distance);
        }
        if !core.menu.is_options_screen() {
            game.options_from_game = false;
            game.paused = true;
            core.apply_cursor_grab(&gfx.window, Some(game));
        }
    }

    GameUpdateResult::None
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

fn get_entity_light(chunk_store: &ChunkStore, pos: Position) -> f32 {
    crate::renderer::chunk::mesher::world_brightness(
        chunk_store,
        pos.x.floor() as i32,
        pos.y.floor() as i32,
        pos.z.floor() as i32,
    )
}

/// Builds the rain/snow columns in a square around the camera (vanilla
/// WeatherEffectRenderer.extractRenderState). Returns empty when it is not
/// raining or when no precipitation biomes are nearby.
fn build_weather_columns(
    chunk_store: &ChunkStore,
    biome_climate: &HashMap<u32, BiomeClimate>,
    cam: glam::DVec3,
    rain: f32,
) -> Vec<crate::renderer::WeatherColumn> {
    use crate::renderer::WeatherColumn;
    use crate::renderer::pipelines::weather::{Precip, WEATHER_RADIUS, precipitation_for};

    if rain <= 0.0 {
        return Vec::new();
    }

    let cam_x = cam.x.floor() as i32;
    let cam_y = cam.y.floor() as i32;
    let cam_z = cam.z.floor() as i32;
    let mut columns = Vec::new();

    for dz in -WEATHER_RADIUS..=WEATHER_RADIUS {
        for dx in -WEATHER_RADIUS..=WEATHER_RADIUS {
            let wx = cam_x + dx;
            let wz = cam_z + dz;
            let terrain = chunk_store.motion_blocking_height(wx, wz);
            let y0 = (cam_y - WEATHER_RADIUS).max(terrain);
            let y1 = (cam_y + WEATHER_RADIUS).max(terrain);
            if y1 - y0 == 0 {
                continue;
            }
            let climate = biome_climate
                .get(&chunk_store.biome_id(wx, cam_y, wz))
                .copied()
                .unwrap_or_default();
            let precip = precipitation_for(&climate, cam_y);
            if precip == Precip::None {
                continue;
            }
            let light_y = cam_y.max(terrain);
            let light = get_entity_light(
                chunk_store,
                Position::new(wx as f64, light_y as f64, wz as f64),
            );
            columns.push(WeatherColumn {
                x: wx,
                z: wz,
                bottom_y: y0 as f32,
                top_y: y1 as f32,
                precip,
                light,
            });
        }
    }
    columns
}

/// Emits the hovering, spinning, multi-copy cluster for one dropped item,
/// shared by resting items and the pickup fly-animation. Mirrors
/// `ItemEntityRenderer.submit` + `submitMultipleFromCount`: hover from the
/// post-scale model bounds, 3D-vs-flat copy layout on the model depth, scatter
/// RNG seeded by item id.
#[allow(clippy::too_many_arguments)]
fn emit_item_copies(
    infos: &mut Vec<crate::renderer::pipelines::item_entity::ItemRenderInfo>,
    item_name: &str,
    item_id: u32,
    count: i32,
    anchor_rel_pos: glam::Vec3,
    age_f: f32,
    bob_offset: f32,
    is_block_model: bool,
    min_y: f32,
    z_size: f32,
    light: f32,
) {
    use crate::renderer::pipelines::item_entity::ItemRenderInfo;
    use crate::util::JavaRandom;

    let bob = (age_f / 10.0 + bob_offset).sin() * 0.1 + 0.1;
    let spin = age_f / 20.0 + bob_offset;
    let copies = stack_render_count(count);

    // GROUND display scale: blocks 0.25, flat items 0.5.
    let scale = if is_block_model { 0.25 } else { 0.5 };
    let min_y_r = min_y * scale;
    let z_size_r = z_size * scale;

    // hover = bob + (-modelBoundingBox.minY) + 0.0625
    let hover_y = bob - min_y_r + 0.0625;

    let base = glam::Mat4::from_translation(anchor_rel_pos + glam::Vec3::new(0.0, hover_y, 0.0))
        * glam::Mat4::from_rotation_y(spin);
    let scale_mat = glam::Mat4::from_scale(glam::Vec3::splat(scale));

    let mut push = |copy_offset: glam::Mat4| {
        infos.push(ItemRenderInfo {
            item_name: item_name.to_string(),
            model_matrix: base * copy_offset * scale_mat,
            light,
        });
    };

    // getSeedForItemStack seeds from item id (+ damage, not extracted yet).
    let mut rng = JavaRandom::new(item_id as i64);
    let mut jitter = |spread: f32| (rng.next_float() * 2.0 - 1.0) * spread;

    if z_size_r > 0.0625 {
        push(glam::Mat4::IDENTITY);
        for _ in 1..copies {
            let off = glam::Vec3::new(jitter(0.15), jitter(0.15), jitter(0.15));
            push(glam::Mat4::from_translation(off));
        }
    } else {
        let z_step = z_size_r * 1.5;
        let z_start = -(z_step * (copies - 1) as f32 / 2.0);
        push(glam::Mat4::from_translation(glam::Vec3::new(
            0.0, 0.0, z_start,
        )));
        for i in 1..copies {
            let z = z_start + z_step * i as f32;
            let off = glam::Vec3::new(jitter(0.15 * 0.5), jitter(0.15 * 0.5), z);
            push(glam::Mat4::from_translation(off));
        }
    }
}

fn build_item_render_infos(
    entity_store: &crate::entity::ItemEntityStore,
    chunk_store: &ChunkStore,
    camera_pos: glam::DVec3,
    anchor: glam::DVec3,
    partial_tick: f32,
) -> Vec<crate::renderer::pipelines::item_entity::ItemRenderInfo> {
    let mut infos = Vec::new();

    for item in entity_store.visible_items(camera_pos, 64.0) {
        let age_f = item.age as f32 + partial_tick;
        let lerped = item.prev_position.lerp(item.position, partial_tick as f64);
        let light = get_entity_light(chunk_store, lerped);
        emit_item_copies(
            &mut infos,
            &item.item_name,
            item.item_id,
            item.count,
            (*lerped - anchor).as_vec3(),
            age_f,
            item.bob_offset,
            item.is_block_model,
            item.min_y,
            item.z_size,
            light,
        );
    }

    // Pickup fly-animation: the cluster at the lerped position, age frozen at
    // pickup.
    for pickup in entity_store.active_pickups(partial_tick) {
        let age_f = pickup.age as f32 + partial_tick;
        let light = get_entity_light(chunk_store, pickup.position);
        emit_item_copies(
            &mut infos,
            &pickup.item_name,
            pickup.item_id,
            pickup.count,
            (*pickup.position - anchor).as_vec3(),
            age_f,
            pickup.bob_offset,
            pickup.is_block_model,
            pickup.min_y,
            pickup.z_size,
            light,
        );
    }

    infos
}

struct EntityExtras {
    variant_index: u32,
    overlay_tints: [Option<[f32; 4]>; MAX_OVERLAYS],
    overlay_variants: [u32; MAX_OVERLAYS],
    head_y_offset: f32,
    head_x_rot_deg_override: Option<f32>,
}

const EMPTY_EXTRAS: EntityExtras = EntityExtras {
    variant_index: 0,
    overlay_tints: [None; MAX_OVERLAYS],
    overlay_variants: [0; MAX_OVERLAYS],
    head_y_offset: 0.0,
    head_x_rot_deg_override: None,
};

/// Only the first overlay slot visible, untinted.
const SLOT0_TINTS: [Option<[f32; 4]>; MAX_OVERLAYS] = {
    let mut tints = [None; MAX_OVERLAYS];
    tints[0] = Some(WHITE_TINT);
    tints
};

fn entity_extras(entity_id: i32, e: &crate::entity::LivingEntity, alpha: f32) -> EntityExtras {
    match e.entity_type {
        EntityKind::Cow => EntityExtras {
            variant_index: e.cow_variant as u32,
            ..EMPTY_EXTRAS
        },
        EntityKind::Sheep => sheep_extras(entity_id, e, alpha),
        EntityKind::Villager => villager_extras(e),
        // Spider eyes overlay is always visible (slot 0).
        EntityKind::Spider => EntityExtras {
            overlay_tints: SLOT0_TINTS,
            ..EMPTY_EXTRAS
        },
        // Charged-creeper aura overlay (slot 0) only when powered.
        EntityKind::Creeper if e.powered => EntityExtras {
            overlay_tints: SLOT0_TINTS,
            ..EMPTY_EXTRAS
        },
        _ => EMPTY_EXTRAS,
    }
}

fn sheep_extras(entity_id: i32, e: &crate::entity::LivingEntity, alpha: f32) -> EntityExtras {
    let is_jeb = e.custom_name.as_deref() == Some("jeb_");
    let tint = if is_jeb {
        jeb_sheep_tint(entity_id, e.age_in_ticks)
    } else if let Some(c) = e.wool_color {
        wool_color_tint(c)
    } else {
        WHITE_TINT
    };
    let mut overlay_tints = [None; MAX_OVERLAYS];
    if !e.is_sheared {
        if e.is_baby {
            overlay_tints[0] = Some(tint);
        } else {
            let undercoat_visible = is_jeb || e.wool_color.is_some_and(|c| c != 0);
            overlay_tints[0] = if undercoat_visible { Some(tint) } else { None };
            overlay_tints[1] = Some(tint);
        }
    }

    let (pos_scale, angle_scale) = sheep_eat_scales(e.eat_anim_tick, e.prev_eat_anim_tick, alpha);
    let age_scale = if e.is_baby { 0.5 } else { 1.0 };
    let head_y_offset = pos_scale * 9.0 * age_scale;
    let head_x_rot_deg_override = if e.eat_anim_tick > 0 || e.prev_eat_anim_tick > 0 {
        Some(angle_scale)
    } else {
        None
    };
    EntityExtras {
        overlay_tints,
        head_y_offset,
        head_x_rot_deg_override,
        ..EMPTY_EXTRAS
    }
}

/// Whether the type texture's built-in hat is fully or partially covered by
/// the profession texture's own hat, per the `villager` sections of the
/// `.png.mcmeta` files under `textures/entity/villager/` (hardcoded — no
/// resource-pack support). 0 = none, 1 = partial, 2 = full.
const VILLAGER_TYPE_HAT: [u8; 7] = [2, 0, 0, 0, 2, 0, 0]; // desert, snow = full
const VILLAGER_PROFESSION_HAT: [u8; 15] = [
    0, // none
    0, // armorer
    1, // butcher (partial)
    0, // cartographer
    0, // cleric
    2, // farmer
    2, // fisherman
    2, // fletcher
    0, // leatherworker
    2, // librarian
    0, // mason
    0, // nitwit
    2, // shepherd
    0, // toolsmith
    0, // weaponsmith
];

/// Overlay slots: 0 = biome type (full model), 1 = biome type (no-hat model),
/// 2 = profession, 3 = profession level. Mirrors vanilla
/// `VillagerProfessionLayer.submit`.
fn villager_extras(e: &crate::entity::LivingEntity) -> EntityExtras {
    use crate::entity::villager::VillagerProfession;

    let kind = e.villager_kind as usize;
    let profession = e.villager_profession as usize;

    let type_hat = VILLAGER_TYPE_HAT[kind];
    let prof_hat = VILLAGER_PROFESSION_HAT[profession];
    let type_hat_visible = prof_hat == 0 || (prof_hat == 1 && type_hat != 2);

    let mut overlay_tints = [None; MAX_OVERLAYS];
    overlay_tints[if type_hat_visible { 0 } else { 1 }] = Some(WHITE_TINT);
    // Profession and level layers are adult-only; nitwits have no level badge.
    if !e.is_baby && e.villager_profession != VillagerProfession::None {
        overlay_tints[2] = Some(WHITE_TINT);
        if e.villager_profession != VillagerProfession::Nitwit {
            overlay_tints[3] = Some(WHITE_TINT);
        }
    }

    EntityExtras {
        overlay_tints,
        overlay_variants: [
            kind as u32,
            kind as u32,
            (profession as u32).saturating_sub(1),
            e.villager_level.clamp(1, 5) - 1,
        ],
        ..EMPTY_EXTRAS
    }
}

fn sheep_eat_scales(eat_tick: u8, prev_eat_tick: u8, alpha: f32) -> (f32, f32) {
    use std::f32::consts::PI;
    // Mirrors vanilla Sheep.java:127-149. Linear-blend previous and current tick
    // first so the head dip is smooth between server ticks.
    let interp = prev_eat_tick as f32 + (eat_tick as f32 - prev_eat_tick as f32) * alpha;
    let pos_scale = if interp <= 0.0 {
        0.0
    } else if (4.0..=36.0).contains(&interp) {
        1.0
    } else if interp < 4.0 {
        interp / 4.0
    } else {
        -(interp - 40.0) / 4.0
    };
    let angle_scale = if (4.0..36.0).contains(&interp) {
        let s = (interp - 4.0) / 32.0;
        PI / 5.0 + (PI * 7.0 / 100.0) * (s * 28.7).sin()
    } else if interp > 0.0 {
        PI / 5.0
    } else {
        0.0
    };
    (pos_scale, angle_scale)
}
