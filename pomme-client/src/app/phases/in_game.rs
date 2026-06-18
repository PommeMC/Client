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
use crate::renderer::pipelines::entity_renderer::{
    EntityRenderInfo, WHITE_TINT, jeb_sheep_tint, wool_color_tint,
};
use crate::renderer::pipelines::menu_overlay::MenuElement;
use crate::renderer::{HizReadback, Renderer, SkyState};
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
    pub last_player_chunk: ChunkPos,
    /// Monotonic content generation per column, bumped on every edit (and chunk
    /// load). This is the dirty marker: a column needs (re)meshing whenever its
    /// `content_gen` outruns what was last enqueued, regardless of visibility,
    /// so an edit to a deferred/hidden column can never be lost.
    pub content_gen: HashMap<ChunkPos, u64>,
    /// What `(lod, content_gen)` was most recently enqueued for each column,
    /// used to dedup the per-frame visibility re-scan (don't re-push an
    /// in-flight job).
    pub enqueued: HashMap<ChunkPos, MeshKey>,
    /// Cached per-column meshing tier (0 visible … 2 hidden) and whether it is
    /// trustworthy. Recomputed only when the camera moves a chunk / rotates;
    /// shared with the mesh queue so `poll` and the re-scan agree.
    pub vis_tiers: HashMap<ChunkPos, u8>,
    pub vis_valid: bool,
    /// Throttle keys for the visibility recompute (player chunk + rotation
    /// bucket).
    pub last_vis_rot: (i32, i32),
}

/// A column's desired mesh identity: which LOD and which content generation.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct MeshKey {
    pub lod: u32,
    pub content_gen: u64,
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
            last_player_chunk: ChunkPos::new(0, 0),
            content_gen: HashMap::new(),
            enqueued: HashMap::new(),
            vis_tiers: HashMap::new(),
            vis_valid: false,
            last_vis_rot: (i32::MIN, i32::MIN),
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

    /// Mesh an edited column now on the priority lane, ungated by visibility —
    /// the player just changed those blocks. Bumps the content generation so
    /// any older in-flight mesh of the column is dropped when it arrives.
    pub fn enqueue_edit(&mut self, pos: ChunkPos, lod: u32) {
        let content_gen = self.bump_content_gen(pos);
        self.enqueued.insert(pos, MeshKey { lod, content_gen });
        self.mesh_dispatcher
            .enqueue(&self.chunk_store, pos, lod, true, content_gen);
    }

    /// Recompute the per-column meshing tier (visible/margin/hidden) from the
    /// camera frustum, throttled to camera-chunk / rotation / load changes
    /// (mirrors vanilla's frustum-update gating). Pushes the result to the mesh
    /// queue so `poll` orders bulk work tier-then-distance.
    pub fn update_visibility(
        &mut self,
        renderer: &Renderer,
        player_chunk: ChunkPos,
        loads_happened: bool,
    ) {
        // Before the camera is placed the frustum is meaningless, so trust
        // nothing and let the queue mesh everything nearest-first.
        if !self.position_set {
            if self.vis_valid {
                self.vis_valid = false;
                self.vis_tiers.clear();
                self.mesh_dispatcher.set_visibility(HashMap::new(), false);
            }
            return;
        }

        let look = renderer.camera_look_dir();
        let rot_bucket = (
            (look.y_rot_deg() / 2.0).floor() as i32,
            (look.x_rot_deg() / 2.0).floor() as i32,
        );
        let moved = player_chunk != self.last_player_chunk;
        let rotated = rot_bucket != self.last_vis_rot;
        if self.vis_valid && !moved && !rotated && !loads_happened {
            return;
        }
        self.last_player_chunk = player_chunk;
        self.last_vis_rot = rot_bucket;

        let planes = renderer.frustum_planes();
        let planes_wide = renderer.frustum_planes_dilated(VIS_MARGIN_RADIANS);
        let eye = renderer.camera_render_position().as_vec3();
        let min_y = self.chunk_store.min_y() as f32;
        let max_y = min_y + self.chunk_store.height() as f32;
        // Hi-Z occluder depth used to demote in-frustum columns fully behind
        // terrain. `None` => frustum-only (occlusion off, or no readback yet).
        let occlusion = renderer
            .hiz_occlusion_enabled()
            .then(|| renderer.hiz_readback())
            .flatten();

        let mut tiers = HashMap::new();
        for pos in self.chunk_store.loaded_positions() {
            let near = column_is_near(pos, eye);
            let mut tier = if near {
                0
            } else {
                column_frustum_tier(pos, eye, &planes, &planes_wide, min_y, max_y)
            };
            if !near
                && tier == 0
                && let Some(rb) = occlusion
                && column_occluded(pos, &self.chunk_store, min_y, rb)
            {
                tier = 2;
            }
            tiers.insert(pos, tier);
        }
        self.vis_tiers = tiers.clone();
        self.vis_valid = true;
        self.mesh_dispatcher.set_visibility(tiers, true);
    }

    /// Enqueue every loaded column whose desired `(lod, content_gen)` differs
    /// from what was last enqueued. Visible (tier 0) and about-to-be-seen
    /// (tier 1) columns go immediately; hidden (tier 2) ones are rate-limited
    /// and only backfilled while the GPU pool has spare capacity
    /// (`allow_backfill`), so a render distance larger than the pool meshes
    /// the visible set rather than thrashing a full pool. Skipped hidden
    /// columns retry once buckets free up. Runs every frame to drain the
    /// backlog.
    pub fn rescan_mesh_jobs(&mut self, player_chunk: ChunkPos, allow_backfill: bool) {
        let mut bg_budget = MAX_BACKGROUND_ENQUEUE_PER_FRAME;
        for pos in self.chunk_store.loaded_positions() {
            let lod = crate::app::core::chunk_lod(pos, player_chunk);
            let content_gen = self.content_gen.get(&pos).copied().unwrap_or(0);
            let desired = MeshKey { lod, content_gen };
            if self.enqueued.get(&pos) == Some(&desired) {
                continue;
            }
            let tier = if self.vis_valid {
                self.vis_tiers.get(&pos).copied().unwrap_or(0)
            } else {
                0
            };
            if tier >= 2 {
                if !allow_backfill || bg_budget == 0 {
                    continue;
                }
                bg_budget -= 1;
            }
            self.enqueued.insert(pos, desired);
            self.mesh_dispatcher
                .enqueue(&self.chunk_store, pos, lod, false, content_gen);
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
/// Max hidden (tier 2) columns enqueued per frame, so backfill completes the
/// world without stealing worker throughput from visible work.
const MAX_BACKGROUND_ENQUEUE_PER_FRAME: u32 = 8;

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

/// Margin (blocks) tested above a column's surface so trees / small builds
/// above the motion-blocking height still count as visible geometry.
/// Over-demoting is safe (the column is still backfilled), so this only needs
/// to cover the common case, not every floating structure.
const OCCLUSION_TEST_MARGIN: f32 = 32.0;

/// Whether a column's occupied span is fully behind the recorded Hi-Z occluder
/// depth (with at least one section on-screen to judge). Only sections up to
/// the surface (+margin) are tested — the empty air above always projects onto
/// distant/sky depth and would otherwise keep every column "visible". Unknown
/// surface => not occluded. Conservative: any visible or near-plane-crossing
/// section keeps the column. Mirrors `cull.comp`'s `occluded` on the CPU.
fn column_occluded(pos: ChunkPos, chunk_store: &ChunkStore, min_y: f32, rb: &HizReadback) -> bool {
    let surface = chunk_store.motion_blocking_height(pos.x * 16 + 8, pos.z * 16 + 8);
    if surface <= chunk_store.min_y() {
        return false; // no heightmap yet: don't risk demoting
    }
    let top = surface as f32 + OCCLUSION_TEST_MARGIN;

    let bx = pos.x as f32 * 16.0;
    let bz = pos.z as f32 * 16.0;
    let mut any_testable = false;
    let mut y = min_y;
    while y < top {
        let sect_top = (y + 16.0).min(top);
        let mn = [bx, y, bz];
        let mx = [bx + 16.0, sect_top, bz + 16.0];
        match section_occlusion(&mn, &mx, rb) {
            SectionOcc::Visible => return false,
            SectionOcc::Occluded => any_testable = true,
            SectionOcc::Offscreen => {}
        }
        y += 16.0;
    }
    any_testable
}

enum SectionOcc {
    Visible,
    Occluded,
    Offscreen,
}

/// Project a section's box with the Hi-Z frame's matrix and compare its nearest
/// depth against the coarse occluder depth in its screen footprint.
fn section_occlusion(mn: &[f32; 3], mx: &[f32; 3], rb: &HizReadback) -> SectionOcc {
    let mut rect_min = glam::Vec2::splat(f32::INFINITY);
    let mut rect_max = glam::Vec2::splat(f32::NEG_INFINITY);
    let mut near_depth = f32::INFINITY;
    for i in 0..8 {
        let corner = glam::Vec3::new(
            if i & 1 != 0 { mx[0] } else { mn[0] },
            if i & 2 != 0 { mx[1] } else { mn[1] },
            if i & 4 != 0 { mx[2] } else { mn[2] },
        ) - rb.eye;
        let clip = rb.view_proj * corner.extend(1.0);
        if clip.w <= 0.0 {
            return SectionOcc::Visible; // crosses the near plane: don't trust it
        }
        let ndc = clip.truncate() / clip.w;
        let uv = ndc.truncate() * 0.5 + 0.5;
        rect_min = rect_min.min(uv);
        rect_max = rect_max.max(uv);
        near_depth = near_depth.min(ndc.z);
    }

    if rect_max.x < 0.0 || rect_min.x > 1.0 || rect_max.y < 0.0 || rect_min.y > 1.0 {
        return SectionOcc::Offscreen;
    }
    let lo = rect_min.clamp(glam::Vec2::ZERO, glam::Vec2::ONE);
    let hi = rect_max.clamp(glam::Vec2::ZERO, glam::Vec2::ONE);
    let occ = sample_hiz(rb, lo.x, lo.y)
        .max(sample_hiz(rb, hi.x, lo.y))
        .max(sample_hiz(rb, lo.x, hi.y))
        .max(sample_hiz(rb, hi.x, hi.y));
    if near_depth - 0.0005 > occ {
        SectionOcc::Occluded
    } else {
        SectionOcc::Visible
    }
}

fn sample_hiz(rb: &HizReadback, u: f32, v: f32) -> f32 {
    let x = ((u * rb.width as f32) as i32).clamp(0, rb.width as i32 - 1) as usize;
    let y = ((v * rb.height as f32) as i32).clamp(0, rb.height as i32 - 1) as usize;
    rb.depth[y * rb.width as usize + x]
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
        // Drop a mesh built from an out-of-date snapshot: the column was edited
        // after this job was enqueued, and a newer job already holds the truth.
        let cur_gen = game.content_gen.get(&mesh.pos).copied().unwrap_or(0);
        if mesh.content_gen < cur_gen {
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
        gfx.renderer.upload_chunk_mesh(&mesh);
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
        game.item_entity_store.tick(
            |bx, by, bz| !game.chunk_store.get_block_state(bx, by, bz).is_air(),
            |bx, by, bz| block_friction(game.chunk_store.get_block_state(bx, by, bz)),
        );
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
            occlusion_on: gfx.renderer.hiz_occlusion_enabled(),
            mesh_gate: game.vis_valid.then(|| {
                let mut t = [0u32; 3];
                for &tier in game.vis_tiers.values() {
                    t[(tier as usize).min(2)] += 1;
                }
                (t[0], t[1], t[2])
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
            game.paused = false;
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
