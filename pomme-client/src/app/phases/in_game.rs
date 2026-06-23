use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use azalea_core::position::ChunkPos;
use azalea_protocol::packets::game::{ServerboundClientInformation, ServerboundGamePacket};
use azalea_registry::builtin::EntityKind;
use glam::FloatExt as _;

use crate::app::core::{AppCore, PlayerInputState};
use crate::app::phases::Gfx;
use crate::app::{DEFAULT_RENDER_DISTANCE, TICK_RATE, input};
use crate::benchmark::{Benchmark, BenchmarkResult};
use crate::entity::components::{LookDirection, Position};
use crate::entity::{EntityStore, ItemEntityStore, lerp_angle};
use crate::net::connection::ConnectionHandle;
use crate::player::LocalPlayer;
use crate::player::interaction::{HitResult, InteractionState};
use crate::player::tab_list::TabList;
use crate::renderer::chunk::mesher::{BiomeClimate, MeshDispatcher};
use crate::renderer::chunk::occlusion_graph::{self, VisibilitySet};
use crate::renderer::pipelines::entity_renderer::{
    EntityRenderInfo, WHITE_TINT, jeb_sheep_tint, wool_color_tint,
};
use crate::renderer::pipelines::menu_overlay::MenuElement;
use crate::renderer::{Renderer, SkyState};
use crate::resource_pack::ResourcePackManager;
use crate::ui::chat::ChatState;
use crate::ui::death::{self, DeathAction};
use crate::ui::pause::{self, PauseAction};
use crate::ui::{common, hud};
use crate::world::block_entity_anim::BlockEntityAnimStore;
use crate::world::chunk::ChunkStore;

pub struct GameState {
    pub chunk_store: ChunkStore,
    pub entity_store: EntityStore,
    pub position_set: bool,
    pub player_loaded_sent: bool,
    pub player: LocalPlayer,
    pub biome_climate: Arc<HashMap<u32, BiomeClimate>>,
    pub player_walk_pos: f32,
    pub player_walk_speed: f32,
    pub player_prev_walk_speed: f32,
    pub mesh_dispatcher: MeshDispatcher,
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
    pub chat: ChatState,
    pub command_tree: Option<Arc<crate::net::commands::CommandTree>>,
    pub tab_list: TabList,
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
    pub block_entity_anim: BlockEntityAnimStore,
    pub benchmark: Option<Benchmark>,
    pub benchmark_result: Option<BenchmarkResult>,
    /// Monotonic content generation per column, bumped on every edit (and chunk
    /// load). This is the dirty marker: a column needs (re)meshing whenever its
    /// `content_gen` outruns what was last enqueued, regardless of visibility,
    /// so an edit to a deferred/hidden column can never be lost.
    pub content_gen: HashMap<ChunkPos, u64>,
    /// What was most recently meshed for each column: the LOD, the column
    /// `content_gen`, and the bitmask of section indices already meshed. The
    /// re-scan meshes only sections newly made visible (or re-meshes all on a
    /// lod/content change), so hidden sections never mesh.
    pub meshed: HashMap<ChunkPos, MeshedCol>,
    /// Per-column bitmask of currently-visible section indices (bit `si` set =
    /// section is in-frustum and not occluded). Computed in
    /// `update_visibility`.
    pub vis_mask: HashMap<ChunkPos, u32>,
    /// Per-section generation for edits only (bulk uses the column
    /// `content_gen` above). Bumped per edited section so a result is
    /// dropped only when *that* section was edited again — editing one
    /// section never invalidates a sibling section's in-flight result.
    pub section_gen: HashMap<(ChunkPos, i32), u64>,
    pub next_section_gen: u64,
    /// Per-section cave-cull visibility (vanilla `VisibilitySet`), keyed like
    /// `section_gen`. Fed by mesh results; consumed by the occlusion walk.
    pub section_vis: HashMap<(ChunkPos, i32), VisibilitySet>,
    /// Highest upload epoch each `section_vis` entry was set from; mirrors the
    /// buffer's per-section geometry gate so a stale bulk can't re-stale an
    /// edited section's visibility.
    pub section_vis_epoch: HashMap<(ChunkPos, i32), u64>,
    /// Cached per-column frustum tier (0 in view, 1 margin, 2 behind),
    /// recomputed each time an occlusion walk completes. Only the F3
    /// overlay reads it now.
    pub vis_tiers: HashMap<ChunkPos, u8>,
    pub vis_valid: bool,
    /// Camera 8-block bucket that last triggered an occlusion walk — movement,
    /// not rotation, drives recomputes (vanilla's cadence).
    pub last_vis_cam: (i32, i32, i32),
    /// In-flight async occlusion walk; its result is applied a few frames
    /// later.
    pub vis_task: Option<crossbeam_channel::Receiver<HashMap<ChunkPos, u32>>>,
    /// Runtime toggle for graph-driven chunk occlusion culling (F3+O). When
    /// off, only frustum culling applies (full masks pushed to the
    /// renderer).
    pub chunk_occlusion_enabled: bool,
}

/// What a column was last meshed as: LOD, content generation, and the set of
/// section indices (bitmask) that have been meshed so far.
#[derive(Clone, Copy)]
pub struct MeshedCol {
    pub lod: u32,
    pub content_gen: u64,
    pub mask: u32,
}

impl GameState {
    pub fn new(renderer: &Renderer, resource_packs: &ResourcePackManager) -> Self {
        let biome_climate = Arc::new(HashMap::new());
        let mesh_dispatcher = renderer.create_mesh_dispatcher(biome_climate, Some(resource_packs));

        Self {
            chunk_store: ChunkStore::new(DEFAULT_RENDER_DISTANCE),
            entity_store: EntityStore::new(),
            position_set: false,
            player_loaded_sent: false,
            options_from_game: false,
            last_render_distance: DEFAULT_RENDER_DISTANCE,
            server_render_distance: 0,
            server_simulation_distance: 0,
            item_entity_store: ItemEntityStore::new(),
            block_entity_anim: BlockEntityAnimStore::default(),
            player: LocalPlayer::new(),
            biome_climate: Arc::new(HashMap::new()),
            player_walk_pos: 0.0,
            player_walk_speed: 0.0,
            player_prev_walk_speed: 0.0,
            mesh_dispatcher,
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
            chat: ChatState::new(),
            command_tree: None,
            tab_list: TabList::new(),
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
            content_gen: HashMap::new(),
            meshed: HashMap::new(),
            vis_mask: HashMap::new(),
            section_gen: HashMap::new(),
            next_section_gen: 0,
            section_vis: HashMap::new(),
            section_vis_epoch: HashMap::new(),
            vis_tiers: HashMap::new(),
            vis_valid: false,
            last_vis_cam: (i32::MIN, i32::MIN, i32::MIN),
            vis_task: None,
            chunk_occlusion_enabled: true,
        }
    }

    pub fn gui_open(&self) -> bool {
        self.inventory_open || self.creative_inventory_open
    }

    /// No menu (pause, inventory, chat) is capturing input.
    pub fn input_live(&self) -> bool {
        !self.paused && !self.gui_open() && !self.chat.is_open()
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

    /// Mark a column dirty by advancing its content generation, returning the
    /// new value. Any in-flight mesh built from an older generation is
    /// dropped on arrival, so a deferred column always remeshes with the
    /// latest blocks.
    pub fn bump_content_gen(&mut self, pos: ChunkPos) -> u64 {
        let g = self.content_gen.entry(pos).or_insert(0);
        *g += 1;
        *g
    }

    /// Mesh a single edited section now on the priority lane, ungated by
    /// visibility. Bumps that section's generation so the result is dropped
    /// only if the same section is edited again before it lands.
    pub fn enqueue_section_edit(&mut self, col: ChunkPos, si: i32, lod: u32) {
        self.next_section_gen += 1;
        let g = self.next_section_gen;
        self.section_gen.insert((col, si), g);
        self.mesh_dispatcher
            .enqueue(&self.chunk_store, col, lod, true, g, si..si + 1);
    }

    /// Drive the cave-cull occlusion walk: apply a finished async walk to the
    /// per-column draw masks, then schedule the next one on 8-block camera
    /// movement or chunk loads (one at a time, off the main thread — vanilla's
    /// async, movement-gated cadence). The walk is rotation-independent;
    /// frustum culling runs per-frame on the GPU.
    pub fn update_visibility(
        &mut self,
        renderer: &mut Renderer,
        player_chunk: ChunkPos,
        loads_happened: bool,
    ) {
        // Before the camera is placed the frustum is meaningless, so trust
        // nothing and let the queue mesh everything nearest-first.
        if !self.position_set {
            if self.vis_valid {
                self.vis_valid = false;
                self.vis_tiers.clear();
            }
            return;
        }

        // Apply a finished walk (its result lags a few frames, like vanilla's).
        let finished = self.vis_task.as_ref().and_then(|rx| rx.try_recv().ok());
        if let Some(bfs) = finished {
            self.vis_task = None;
            self.apply_visibility(renderer, &bfs);
        }

        // Schedule the next walk on 8-block movement, chunk loads, or an
        // invalidated result (`!vis_valid`, e.g. the F3+O toggle forcing a
        // recompute while stationary), one in flight.
        let eye = renderer.camera_render_position();
        let cam_bucket = (
            (eye.x / 8.0).floor() as i32,
            (eye.y / 8.0).floor() as i32,
            (eye.z / 8.0).floor() as i32,
        );
        if self.vis_task.is_none()
            && (!self.vis_valid || cam_bucket != self.last_vis_cam || loads_happened)
        {
            self.last_vis_cam = cam_bucket;
            let section_vis = self.section_vis.clone();
            let min_y = self.chunk_store.min_y();
            let n = self.chunk_store.section_count();
            let cam_si = ((eye.y - min_y as f64) / 16.0).floor() as i32;
            // Bound the walk by the actual loaded radius (a server can stream
            // terrain past the client render distance).
            let rd = self
                .chunk_store
                .loaded_positions()
                .iter()
                .map(|p| {
                    (p.x - player_chunk.x)
                        .abs()
                        .max((p.z - player_chunk.z).abs())
                })
                .max()
                .unwrap_or(0);
            let (tx, rx) = crossbeam_channel::bounded(1);
            std::thread::spawn(move || {
                let bfs = occlusion_graph::compute_visible_mask(
                    &section_vis,
                    player_chunk,
                    cam_si,
                    eye,
                    min_y,
                    n,
                    rd,
                );
                let _ = tx.send(bfs);
            });
            self.vis_task = Some(rx);
        }
    }

    /// Combine a finished walk with the current camera frustum into per-column
    /// draw masks (occluded sections omitted) and tiers, and push them to the
    /// GPU cull.
    fn apply_visibility(&mut self, renderer: &mut Renderer, bfs: &HashMap<ChunkPos, u32>) {
        let planes = renderer.frustum_planes();
        let planes_wide = renderer.frustum_planes_dilated(VIS_MARGIN_RADIANS);
        let eye_f = renderer.camera_render_position().as_vec3();
        let min_y = self.chunk_store.min_y() as f32;
        let max_y = min_y + self.chunk_store.height() as f32;
        let full = section_mask(self.chunk_store.section_count());

        let mut tiers = HashMap::new();
        let mut masks = HashMap::new();
        for pos in self.chunk_store.loaded_positions() {
            let near = column_is_near(pos, eye_f);
            let tier = if near {
                0
            } else {
                column_frustum_tier(pos, eye_f, &planes, &planes_wide, min_y, max_y)
            };
            // Near columns always draw fully; otherwise a column draws only the
            // sections the graph proved occlusion-visible (none => fully hidden).
            let mask = if near {
                full
            } else {
                bfs.get(&pos).copied().unwrap_or(0)
            };
            // A fully-occluded column (no visible section) drops to the hidden tier.
            let tier = if tier == 0 && mask == 0 { 2 } else { tier };
            tiers.insert(pos, tier);
            masks.insert(pos, mask);
        }
        self.vis_tiers = tiers;
        self.vis_mask = masks.clone();
        self.vis_valid = true;

        // With occlusion off, push full masks (frustum still applies on the GPU).
        if !self.chunk_occlusion_enabled {
            for m in masks.values_mut() {
                *m = full;
            }
        }
        renderer.set_chunk_visibility(masks);
    }

    /// Enqueue every loaded column's not-yet-meshed sections (re-meshing the
    /// whole column on a lod/content change). Like vanilla, every section in
    /// render distance meshes regardless of visibility — occlusion gates only
    /// drawing — and the queue orders the backlog nearest-first. Runs every
    /// frame to drain it.
    pub fn rescan_mesh_jobs(&mut self, player_chunk: ChunkPos) {
        let n = self.chunk_store.section_count();
        let full = section_mask(n);
        for pos in self.chunk_store.loaded_positions() {
            let lod = crate::app::core::chunk_lod(pos, player_chunk);
            let content_gen = self.content_gen.get(&pos).copied().unwrap_or(0);
            // Mesh the whole column once, then nothing until a lod/content change.
            // Occlusion gates drawing, not meshing, so off-screen and hidden
            // sections still mesh (the queue orders the backlog nearest-first).
            let to_mesh = match self.meshed.get(&pos) {
                Some(m) if m.lod == lod && m.content_gen == content_gen => full & !m.mask,
                _ => full,
            };
            if to_mesh != 0 {
                for (start, end) in contiguous_runs(to_mesh) {
                    self.mesh_dispatcher.enqueue(
                        &self.chunk_store,
                        pos,
                        lod,
                        false,
                        content_gen,
                        start..end,
                    );
                }
            }
            self.meshed.insert(
                pos,
                MeshedCol {
                    lod,
                    content_gen,
                    mask: full,
                },
            );
        }
    }
}

/// Always-mesh radius (vanilla `isNearby`, squared block distance in X/Z):
/// close columns are tier 0 regardless of frustum so the area around the player
/// is never deferred.
const NEARBY_DIST_SQ: f32 = 768.0;
/// Extra FOV (radians) for the tier-1 "about to be seen" margin frustum, so
/// small camera turns reveal already-meshed terrain instead of a meshing
/// curtain.
const VIS_MARGIN_RADIANS: f32 = 0.6;

/// Frustum tier for a column: 0 in view, 1 in the dilated margin, 2 behind the
/// camera. (Nearby columns are forced to 0 by the caller.)
fn column_frustum_tier(
    pos: ChunkPos,
    eye: glam::Vec3,
    planes: &[[f32; 4]; 6],
    planes_wide: &[[f32; 4]; 6],
    min_y: f32,
    max_y: f32,
) -> u8 {
    let bx = pos.x as f32 * 16.0;
    let bz = pos.z as f32 * 16.0;
    // Camera-relative full-height column box, matching how the GPU cull subtracts
    // the eye before its plane test (cull.comp).
    let mn = [bx - eye.x, min_y - eye.y, bz - eye.z];
    let mx = [bx + 16.0 - eye.x, max_y - eye.y, bz + 16.0 - eye.z];
    if aabb_in_frustum(&mn, &mx, planes) {
        0
    } else if aabb_in_frustum(&mn, &mx, planes_wide) {
        1
    } else {
        2
    }
}

/// Whether a column is within the always-mesh radius (never deferred/demoted).
fn column_is_near(pos: ChunkPos, eye: glam::Vec3) -> bool {
    let cx = pos.x as f32 * 16.0 + 8.0 - eye.x;
    let cz = pos.z as f32 * 16.0 + 8.0 - eye.z;
    cx * cx + cz * cz < NEARBY_DIST_SQ
}

/// Full mask for an `n`-section column (bits `0..n` set).
fn section_mask(n: i32) -> u32 {
    if n >= 32 { u32::MAX } else { (1u32 << n) - 1 }
}

/// Contiguous `(start, end)` index runs of set bits in `mask`, so a (usually
/// contiguous) visible set enqueues as a few range jobs — one gather per run.
fn contiguous_runs(mask: u32) -> Vec<(i32, i32)> {
    let mut runs = Vec::new();
    let mut i = 0i32;
    while i < 32 {
        if mask & (1u32 << i) != 0 {
            let start = i;
            while i < 32 && mask & (1u32 << i) != 0 {
                i += 1;
            }
            runs.push((start, i));
        } else {
            i += 1;
        }
    }
    runs
}

/// Conservative AABB-vs-frustum test (the dominant-corner max-dot used by
/// `cull.comp`): true unless the box is fully behind some plane.
fn aabb_in_frustum(mn: &[f32; 3], mx: &[f32; 3], planes: &[[f32; 4]; 6]) -> bool {
    for p in planes {
        let d = p[0] * if p[0] >= 0.0 { mx[0] } else { mn[0] }
            + p[1] * if p[1] >= 0.0 { mx[1] } else { mn[1] }
            + p[2] * if p[2] >= 0.0 { mx[2] } else { mn[2] }
            + p[3];
        if d < 0.0 {
            return false;
        }
    }
    true
}

pub enum GameUpdateResult {
    None,
    ManualDisconnect,
    Disconnected { reason: String },
}

pub fn update_game(
    core: &mut AppCore,
    dt: f32,
    gfx: &mut Gfx,
    connection: &ConnectionHandle,
    game: &mut GameState,
) -> GameUpdateResult {
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

    for mesh in game.mesh_dispatcher.drain_results() {
        // Drop a mesh built from an out-of-date snapshot. Edits (priority lane,
        // single section) are keyed per section so editing one section never
        // drops a sibling's in-flight result; bulk loads keep the column key.
        let stale = if mesh.timing.is_some() {
            mesh.replaced
                .clone()
                .any(|si| game.section_gen.get(&(mesh.pos, si)).copied() != Some(mesh.content_gen))
        } else {
            mesh.content_gen < game.content_gen.get(&mesh.pos).copied().unwrap_or(0)
        };
        if stale {
            continue;
        }
        if let Some(t) = &mesh.timing {
            let ms = |d: std::time::Duration| d.as_secs_f32() * 1000.0;
            tracing::info!(
                "edit remesh [{}, {}]: queue {:.1}ms + mesh {:.1}ms + drain {:.1}ms = {:.1}ms",
                mesh.pos.x,
                mesh.pos.z,
                ms(t.started_at - t.enqueued_at),
                ms(t.meshed_at - t.started_at),
                ms(t.meshed_at.elapsed()),
                ms(t.enqueued_at.elapsed()),
            );
        }
        let dropped = gfx.renderer.upload_chunk_mesh(&mesh);
        let pos = mesh.pos;
        // Sections dropped on pool exhaustion were retired from the buffer; clear
        // their meshed bit so the next rescan re-enqueues them.
        if !dropped.is_empty()
            && let Some(m) = game.meshed.get_mut(&pos)
        {
            for si in dropped {
                m.mask &= !(1u32 << si);
            }
        }
        for (si, vis) in mesh.visibility {
            let e = game.section_vis_epoch.entry((pos, si)).or_insert(0);
            if mesh.upload_epoch >= *e {
                *e = mesh.upload_epoch;
                game.section_vis.insert((pos, si), vis);
            }
        }
    }

    game.mesh_dispatcher
        .set_camera_position(*game.player.position);

    // Sky time ticks unconditionally so it keeps flowing in menus;
    // server SetTime packets reconcile drift.
    core.time_tick_accumulator = (core.time_tick_accumulator + dt).min(1.0);
    while core.time_tick_accumulator >= TICK_RATE {
        game.sky_state.day_time = game.sky_state.day_time.wrapping_add(1);
        game.sky_state.game_time = game.sky_state.game_time.wrapping_add(1);
        core.time_tick_accumulator -= TICK_RATE;
    }

    if game.input_live() {
        gfx.renderer.update_camera(&mut core.input, dt);
    }

    // Menus never pause the simulation; tick_physics substitutes neutral input.
    core.tick_accumulator += dt;
    while core.tick_accumulator >= TICK_RATE {
        core.tick_physics(&mut gfx.renderer, connection, game);
        game.item_entity_store.tick();
        game.block_entity_anim.tick();
        core.tick_accumulator -= TICK_RATE;
    }

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

    let mut close_inventory = false;
    let mut pause_action = PauseAction::None;
    let mut death_action = DeathAction::None;

    gfx.renderer.sync_camera_pos(
        game.player
            .prev_eye_pos()
            .lerp(game.player.eye_pos(), partial_tick as f64),
    );
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

    let sw = gfx.renderer.screen_width() as f32;
    let sh = gfx.renderer.screen_height() as f32;
    let gs = hud::gui_scale(sw, sh, core.menu.gui_scale_setting);

    let mut elements: Vec<MenuElement> = Vec::new();
    let hide_cursor = game.input_live() && !game.dead && core.input.is_cursor_captured();

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
                let block: Box<dyn azalea_block::BlockTrait> = state.into();
                Some((t.block_pos, t.face, block.id().to_string()))
            }),
            chunk_count: gfx.renderer.loaded_chunk_count(),
            sections_drawn: gfx.renderer.sections_drawn(),
            occlusion_on: game.chunk_occlusion_enabled,
            mesh_gate: game.vis_valid.then(|| {
                // Among in-frustum columns: sections we mesh vs sections skipped as
                // occluded (the per-section occlusion win). Middle slot unused.
                let n = game.chunk_store.section_count() as u32;
                let mut visible = 0u32;
                let mut hidden = 0u32;
                for (pos, &mask) in &game.vis_mask {
                    if game.vis_tiers.get(pos).copied().unwrap_or(0) == 0 {
                        let v = mask.count_ones();
                        visible += v;
                        hidden += n.saturating_sub(v);
                    }
                }
                (visible, 0, hidden)
            }),
            gpu_name: gfx.renderer.gpu_name(),
            vulkan_version: gfx.renderer.vulkan_version(),
            screen_w: gfx.renderer.screen_width(),
            screen_h: gfx.renderer.screen_height(),
            timings: Some(hud::FrameTimings {
                frame_ms: gfx.renderer.last_timings().frame_ms,
                fence_ms: gfx.renderer.last_timings().fence_ms,
                acquire_ms: gfx.renderer.last_timings().acquire_ms,
                cull_ms: gfx.renderer.last_timings().cull_ms,
                draw_ms: gfx.renderer.last_timings().draw_ms,
                present_ms: gfx.renderer.last_timings().present_ms,
            }),
        })
    } else {
        None
    };
    hud::build_hud(
        &mut elements,
        sw,
        sh,
        core.input.selected_slot(),
        game.player.health,
        game.player.food,
        game.player.armor,
        game.player.air_supply,
        game.player.eyes_in_water,
        game.player.experience_level,
        game.player.experience_progress,
        game.player.game_mode,
        game.player.inventory.hotbar_slots(),
        gfx.renderer.is_first_person(),
        debug.as_ref(),
        core.menu.gui_scale_setting,
    );

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
            dt * 1000.0,
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
        }
    }

    if let Some(ref result) = game.benchmark_result {
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
            format!("Min: {:.0} / Max: {:.0}", result.min_fps, result.max_fps),
            format!(
                "Frame: {:.2}ms / P1: {:.2}ms / P99: {:.2}ms",
                result.avg_frame_ms, result.p1_frame_ms, result.p99_frame_ms
            ),
            format!(
                "Fence: {:.2}ms / Cull: {:.2}ms / Draw: {:.2}ms",
                result.avg_fence_ms, result.avg_cull_ms, result.avg_draw_ms
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
        if core.input.escape_pressed() || core.input.left_just_pressed() {
            game.benchmark_result = None;
        }
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
        pause_action = pause::build_pause_menu(&mut elements, sw, sh, cursor, clicked, gs);
        core.input.clear_just_pressed_actions();
    }

    let mut player_preview = None;
    if game.inventory_open {
        let cursor = core.input.cursor_pos();
        let clicked = core.input.left_just_pressed();
        let result = crate::ui::inventory::build_inventory(
            &mut elements,
            sw,
            sh,
            cursor,
            clicked,
            &game.player.inventory,
            gs,
        );
        close_inventory = result.clicked_outside;
        player_preview = Some(result.player_preview);
        core.input.clear_just_pressed_actions();
    }

    if game.creative_inventory_open {
        let cursor = core.input.cursor_pos();
        let clicked = core.input.left_just_pressed();
        let scroll_delta = core.input.consume_menu_scroll();
        let typed = core.input.drain_typed_chars();
        let backspace = core.input.backspace_pressed();
        let selected_hotbar = core.input.selected_slot();
        let action = crate::ui::creative_inventory::build_creative_inventory(
            &mut elements,
            &mut game.creative_state,
            sw,
            sh,
            cursor,
            clicked,
            scroll_delta,
            &typed,
            backspace,
            &game.player.inventory,
            selected_hotbar,
            gs,
            game.advanced_item_tooltips,
            core.input.left_held(),
            &|t, s| gfx.renderer.menu_text_width(t, s),
        );
        match action {
            crate::ui::creative_inventory::CreativeAction::Close => {
                close_inventory = true;
            }
            crate::ui::creative_inventory::CreativeAction::Place(item, slot_num) => {
                use azalea_protocol::packets::game::s_set_creative_mode_slot::ServerboundSetCreativeModeSlot;
                if game.player.game_mode == 1 {
                    connection
                        .packet_tx
                        .send(ServerboundGamePacket::SetCreativeModeSlot(
                            ServerboundSetCreativeModeSlot {
                                slot_num,
                                item_stack: item,
                            },
                        ));
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
    let destroy_info = game.interaction.destroy_stage().map(|(pos, stage)| {
        let state = game.chunk_store.get_block_state(pos.x, pos.y, pos.z);
        (pos, stage, state)
    });

    let mut entity_renders: Vec<EntityRenderInfo> = game
        .entity_store
        .living
        .iter()
        .map(|(&entity_id, e)| {
            let interp_pos = e.prev_position.lerp(e.position, partial_tick as f64);
            let extras = entity_extras(entity_id, e, partial_tick);

            EntityRenderInfo {
                position: interp_pos,
                head_y_rot_deg: lerp_angle(e.prev_head_y_rot_deg, e.head_y_rot_deg, partial_tick),
                head_x_rot_deg: e
                    .prev_look_dir
                    .x_rot_deg()
                    .lerp(e.look_dir.x_rot_deg(), partial_tick),
                body_y_rot_deg: lerp_angle(e.prev_body_y_rot_deg, e.body_y_rot_deg, partial_tick),
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
                head_y_offset: extras.head_y_offset,
                head_x_rot_deg_override: extras.head_x_rot_deg_override,
                has_red_overlay: e.hurt_time > 0,
                aggressive: e.aggressive,
                age_in_ticks: e.age_in_ticks as f32 + partial_tick,
                attack_time: e.swing_progress(partial_tick),
                skip_cull: false,
            }
        })
        .collect();

    if !gfx.renderer.is_first_person() {
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
            overlay_tints: [None, None],
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

    let item_renders = build_item_render_infos(
        &game.item_entity_store,
        &game.chunk_store,
        *gfx.renderer.camera_pivot_position(),
        partial_tick,
    );

    let block_entity_renders: Vec<crate::renderer::BlockEntityRenderInfo> = game
        .chunk_store
        .block_entities
        .iter()
        .map(|(pos, be)| {
            let state = game.chunk_store.get_block_state(pos.x, pos.y, pos.z);
            let block: Box<dyn azalea_block::BlockTrait> = state.into();
            let props = block.property_map();
            let variant =
                crate::renderer::pipelines::block_entity::variant_for_block(be.kind, block.id());
            let yaw = crate::renderer::pipelines::block_entity::yaw_for_block(be.kind, &props);
            let lid_open = game
                .block_entity_anim
                .container(pos)
                .map(|a| a.openness)
                .unwrap_or(0.0);
            crate::renderer::BlockEntityRenderInfo {
                pos: *pos,
                kind: be.kind,
                yaw,
                variant,
                lid_open,
            }
        })
        .collect();

    let weather_columns = build_weather_columns(
        &game.chunk_store,
        &game.biome_climate,
        gfx.renderer.camera_render_position(),
        sky.rain(),
    );

    let effective_rd = if game.server_render_distance > 0 {
        core.menu.render_distance.min(game.server_render_distance)
    } else {
        core.menu.render_distance
    };
    let held_item = match game.player.inventory.hotbar_slots()[core.input.selected_slot() as usize]
    {
        azalea_inventory::ItemStack::Present(ref data) => {
            let name = crate::player::inventory::item_resource_name(data.kind);
            (name != "air").then(|| {
                let light =
                    get_entity_light(&game.chunk_store, gfx.renderer.camera_pivot_position());
                (name, light)
            })
        }
        _ => None,
    };
    if let Err(e) = gfx.renderer.render_world(
        &gfx.window,
        hide_cursor,
        elements,
        swing_progress,
        held_item,
        destroy_info,
        game.show_chunk_borders,
        sky,
        &entity_renders,
        &item_renders,
        &block_entity_renders,
        &weather_columns,
        core.menu.cloud_mode,
        effective_rd,
        player_preview,
    ) {
        tracing::error!("Render error: {e}");
    }

    if close_inventory {
        game.inventory_open = false;
        game.creative_inventory_open = false;
        core.apply_cursor_grab(&gfx.window, Some(game));
    }

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
        PauseAction::Benchmark => {
            game.benchmark = Some(Benchmark::new(
                gfx.renderer.gpu_name(),
                gfx.renderer.screen_width(),
                gfx.renderer.screen_height(),
                core.menu.render_distance,
            ));
            game.benchmark_result = None;
            game.paused = false;
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
    use crate::renderer::chunk::mesher::LIGHT_TABLE;
    let bx = pos.x.floor() as i32;
    let by = pos.y.floor() as i32;
    let bz = pos.z.floor() as i32;
    let level = chunk_store
        .get_sky_light(bx, by, bz)
        .max(chunk_store.get_block_light(bx, by, bz));
    LIGHT_TABLE[level as usize]
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
    world_pos: glam::Vec3,
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

    let base = glam::Mat4::from_translation(world_pos + glam::Vec3::new(0.0, hover_y, 0.0))
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
            lerped.as_vec3(),
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
            pickup.position.as_vec3(),
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
    overlay_tints: [Option<[f32; 4]>; 2],
    head_y_offset: f32,
    head_x_rot_deg_override: Option<f32>,
}

const EMPTY_EXTRAS: EntityExtras = EntityExtras {
    variant_index: 0,
    overlay_tints: [None, None],
    head_y_offset: 0.0,
    head_x_rot_deg_override: None,
};

fn entity_extras(entity_id: i32, e: &crate::entity::LivingEntity, alpha: f32) -> EntityExtras {
    match e.entity_type {
        EntityKind::Cow => EntityExtras {
            variant_index: e.cow_variant as u32,
            ..EMPTY_EXTRAS
        },
        EntityKind::Sheep => sheep_extras(entity_id, e, alpha),
        // Spider eyes overlay is always visible (slot 0).
        EntityKind::Spider => EntityExtras {
            overlay_tints: [Some(WHITE_TINT), None],
            ..EMPTY_EXTRAS
        },
        // Charged-creeper aura overlay (slot 0) only when powered.
        EntityKind::Creeper if e.powered => EntityExtras {
            overlay_tints: [Some(WHITE_TINT), None],
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

    let overlay_tints = if e.is_sheared {
        [None, None]
    } else if e.is_baby {
        [Some(tint), None]
    } else {
        let undercoat_visible = is_jeb || e.wool_color.is_some_and(|c| c != 0);
        [
            if undercoat_visible { Some(tint) } else { None },
            Some(tint),
        ]
    };

    let (pos_scale, angle_scale) = sheep_eat_scales(e.eat_anim_tick, e.prev_eat_anim_tick, alpha);
    let age_scale = if e.is_baby { 0.5 } else { 1.0 };
    let head_y_offset = pos_scale * 9.0 * age_scale;
    let head_x_rot_deg_override = if e.eat_anim_tick > 0 || e.prev_eat_anim_tick > 0 {
        Some(angle_scale)
    } else {
        None
    };

    EntityExtras {
        variant_index: 0,
        overlay_tints,
        head_y_offset,
        head_x_rot_deg_override,
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
