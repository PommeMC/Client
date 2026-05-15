use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use azalea_core::position::ChunkPos;
use azalea_protocol::packets::game::{ServerboundClientInformation, ServerboundGamePacket};

use crate::app::core::{AppCore, PlayerInputState};
use crate::app::phases::Gfx;
use crate::app::{DEFAULT_RENDER_DISTANCE, TICK_RATE};
use crate::benchmark::{Benchmark, BenchmarkResult};
use crate::entity::{EntityStore, ItemEntityStore};
use crate::net::connection::ConnectionHandle;
use crate::player::LocalPlayer;
use crate::player::interaction::InteractionState;
use crate::player::tab_list::TabList;
use crate::renderer::chunk::mesher::{BiomeClimate, MeshDispatcher};
use crate::renderer::pipelines::entity_renderer::EntityRenderInfo;
use crate::renderer::pipelines::menu_overlay::MenuElement;
use crate::renderer::{Renderer, SkyState};
use crate::resource_pack::ResourcePackManager;
use crate::ui::chat::ChatState;
use crate::ui::death::{self, DeathAction};
use crate::ui::pause::{self, PauseAction};
use crate::ui::{common, hud};
use crate::world::chunk::ChunkStore;

pub struct GameState {
    pub chunk_store: ChunkStore,
    pub entity_store: EntityStore,
    pub position_set: bool,
    pub player_loaded_sent: bool,
    pub player: LocalPlayer,
    pub prev_player_pos: glam::Vec3,
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
    pub chat: ChatState,
    pub tab_list: TabList,
    pub interaction: InteractionState,
    pub sky_state: crate::renderer::SkyState,
    pub show_debug: bool,
    pub show_chunk_borders: bool,
    pub last_sent_input: PlayerInputState,
    pub last_sent_pos: glam::Vec3,
    pub last_sent_yaw: f32,
    pub last_sent_pitch: f32,
    pub last_sent_on_ground: bool,
    pub last_sent_horizontal_collision: bool,
    pub was_sprinting: bool,
    pub position_send_counter: u32,
    pub options_from_game: bool,
    pub last_render_distance: u32,
    pub server_render_distance: u32,
    pub server_simulation_distance: u32,
    pub item_entity_store: ItemEntityStore,
    pub benchmark: Option<Benchmark>,
    pub benchmark_result: Option<BenchmarkResult>,
    pub last_player_chunk: ChunkPos,
    pub meshed_lod: HashMap<ChunkPos, u32>,
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
            player: LocalPlayer::new(),
            prev_player_pos: glam::Vec3::ZERO,
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
            chat: ChatState::new(),
            tab_list: TabList::new(),
            interaction: InteractionState::new(),
            sky_state: SkyState::default_day(),
            show_debug: false,
            show_chunk_borders: false,
            last_sent_input: PlayerInputState::default(),
            last_sent_pos: glam::Vec3::ZERO,
            last_sent_yaw: 0.0,
            last_sent_pitch: 0.0,
            last_sent_on_ground: false,
            last_sent_horizontal_collision: false,
            was_sprinting: false,
            position_send_counter: 0,
            benchmark: None,
            benchmark_result: None,
            last_player_chunk: ChunkPos::new(0, 0),
            meshed_lod: HashMap::new(),
        }
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
    let disconnect_reason =
        core.drain_network_events(connection, None, &mut gfx.renderer, &gfx.window, game);
    if let Some(reason) = disconnect_reason {
        return GameUpdateResult::Disconnected { reason };
    }

    for mesh in game.mesh_dispatcher.drain_results() {
        gfx.renderer.upload_chunk_mesh(&mesh);
    }

    // Sky time ticks unconditionally so it keeps flowing in menus;
    // server SetTime packets reconcile drift.
    core.time_tick_accumulator = (core.time_tick_accumulator + dt).min(1.0);
    while core.time_tick_accumulator >= TICK_RATE {
        game.sky_state.day_time = game.sky_state.day_time.wrapping_add(1);
        game.sky_state.game_time = game.sky_state.game_time.wrapping_add(1);
        core.time_tick_accumulator -= TICK_RATE;
    }

    if !game.paused && !game.inventory_open && !game.chat.is_open() {
        gfx.renderer.update_camera(&mut core.input);

        core.tick_accumulator += dt;
        while core.tick_accumulator >= TICK_RATE {
            core.tick_physics(&mut gfx.renderer, connection, game);
            game.item_entity_store.tick(
                |bx, by, bz| !game.chunk_store.get_block_state(bx, by, bz).is_air(),
                |bx, by, bz| block_friction(game.chunk_store.get_block_state(bx, by, bz)),
            );
            core.tick_accumulator -= TICK_RATE;
        }
    }

    let alpha = core.tick_accumulator / TICK_RATE;
    let interp_pos = game.prev_player_pos.lerp(game.player.position, alpha);
    let eye_pos = interp_pos + glam::Vec3::new(0.0, 1.62, 0.0);
    let eye_pos_f64 = glam::DVec3::new(eye_pos.x as f64, eye_pos.y as f64, eye_pos.z as f64);

    if !game.paused && !game.inventory_open && !game.chat.is_open() {
        let yaw = gfx.renderer.camera_yaw();
        let pitch = gfx.renderer.camera_pitch();

        game.interaction
            .update_target(eye_pos, yaw, pitch, &game.chunk_store);
    }

    let typed = core.input.drain_typed_chars();
    let backspace = core.input.backspace_pressed();
    let enter = core.input.enter_pressed();
    if let Some(msg) = game.chat.handle_key_input(&typed, backspace, enter) {
        core.send_chat_message(connection, msg);
        core.apply_cursor_grab(&gfx.window, Some(game));
    }

    let mut close_inventory = false;
    let mut pause_action = PauseAction::None;
    let mut death_action = DeathAction::None;

    let yaw = gfx.renderer.camera_yaw();
    let pitch = gfx.renderer.camera_pitch();
    gfx.renderer.sync_camera_to_player(eye_pos_f64, yaw, pitch);
    gfx.renderer
        .update_third_person_distance(eye_pos, &game.chunk_store);

    let sw = gfx.renderer.screen_width() as f32;
    let sh = gfx.renderer.screen_height() as f32;
    let gs = hud::gui_scale(sw, sh, core.menu.gui_scale_setting);

    let mut elements: Vec<MenuElement> = Vec::new();
    let hide_cursor = !game.paused
        && !game.dead
        && !game.inventory_open
        && !game.chat.is_open()
        && core.input.is_cursor_captured();

    let debug = if game.show_debug {
        Some(hud::DebugInfo {
            fps: gfx.fps_counter.display_fps(),
            position: game.player.position,
            yaw: game.player.yaw,
            pitch: game.player.pitch,
            target_block: game.interaction.target.map(|t| {
                let state =
                    game.chunk_store
                        .get_block_state(t.block_pos.x, t.block_pos.y, t.block_pos.z);
                let block: Box<dyn azalea_block::BlockTrait> = state.into();
                (t.block_pos, t.face, block.id().to_string())
            }),
            chunk_count: gfx.renderer.loaded_chunk_count(),
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

    if core.input.tab_held()
        && !game.paused
        && !game.inventory_open
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
        core.input.clear_click_events();
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
        core.input.clear_click_events();
    } else if game.paused {
        let cursor = core.input.cursor_pos();
        let clicked = core.input.left_just_pressed();
        pause_action = pause::build_pause_menu(&mut elements, sw, sh, cursor, clicked, gs);
        core.input.clear_click_events();
    }

    if game.inventory_open {
        let cursor = core.input.cursor_pos();
        let clicked = core.input.left_just_pressed();
        close_inventory = crate::ui::inventory::build_inventory(
            &mut elements,
            sw,
            sh,
            cursor,
            clicked,
            &game.player.inventory,
            gs,
        );
        core.input.clear_click_events();
    }

    game.chat.build(&mut elements, sh, gs, &|t, s| {
        gfx.renderer.menu_text_width(t, s)
    });

    let swing_progress = game
        .interaction
        .get_swing_progress(core.tick_accumulator / TICK_RATE);
    let destroy_info = game.interaction.destroy_stage();

    let alpha = core.tick_accumulator / TICK_RATE;
    let mut entity_renders: Vec<EntityRenderInfo> = game
        .entity_store
        .living
        .values()
        .map(|e| {
            let pos = e.prev_position.lerp(e.position, alpha as f64);
            let body_yaw = e.prev_body_yaw + (e.body_yaw - e.prev_body_yaw) * alpha;
            let head_yaw = e.prev_head_yaw + (e.head_yaw - e.prev_head_yaw) * alpha;
            EntityRenderInfo {
                x: pos.x,
                y: pos.y,
                z: pos.z,
                yaw: body_yaw,
                pitch: e.prev_pitch + (e.pitch - e.prev_pitch) * alpha,
                head_yaw,
                is_baby: e.is_baby,
                walk_anim_pos: {
                    let scale = if e.is_baby { 3.0 } else { 1.0 };
                    (e.walk_anim_pos - e.walk_anim_speed * (1.0 - alpha)) * scale
                },
                walk_anim_speed: (e.prev_walk_anim_speed
                    + (e.walk_anim_speed - e.prev_walk_anim_speed) * alpha)
                    .min(1.0),
                entity_kind: e.entity_type,
            }
        })
        .collect();

    if !gfx.renderer.is_first_person() {
        let cam_yaw_deg = -gfx.renderer.camera_yaw().to_degrees();
        entity_renders.push(EntityRenderInfo {
            x: interp_pos.x as f64,
            y: interp_pos.y as f64,
            z: interp_pos.z as f64,
            yaw: cam_yaw_deg,
            pitch: gfx.renderer.camera_pitch().to_degrees(),
            head_yaw: cam_yaw_deg,
            is_baby: false,
            walk_anim_pos: game.player_walk_pos - game.player_walk_speed * (1.0 - alpha),
            walk_anim_speed: (game.player_prev_walk_speed
                + (game.player_walk_speed - game.player_prev_walk_speed) * alpha)
                .min(1.0),
            entity_kind: azalea_registry::builtin::EntityKind::Player,
        });
    }

    let sky_partial_tick = (core.time_tick_accumulator / TICK_RATE).clamp(0.0, 1.0);
    let sky = crate::renderer::SkyState {
        day_time: game.sky_state.day_time,
        game_time: game.sky_state.game_time,
        rain_level: game.sky_state.rain_level,
        partial_tick: sky_partial_tick,
    };
    if game.show_chunk_borders {
        gfx.renderer.update_chunk_borders(
            game.chunk_store.min_y(),
            game.chunk_store.min_y() + game.chunk_store.height() as i32,
        );
    }

    let cam_pos = glam::DVec3::new(
        game.player.position.x as f64,
        game.player.position.y as f64,
        game.player.position.z as f64,
    );
    let partial_tick = core.tick_accumulator / TICK_RATE;
    let item_renders = build_item_render_infos(
        &game.item_entity_store,
        &game.chunk_store,
        cam_pos,
        partial_tick,
    );

    if let Err(e) = gfx.renderer.render_world(
        &gfx.window,
        hide_cursor,
        elements,
        swing_progress,
        destroy_info,
        game.show_chunk_borders,
        sky,
        &entity_renders,
        &item_renders,
    ) {
        tracing::error!("Render error: {e}");
    }

    if close_inventory {
        game.inventory_open = false;
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

fn get_entity_light(chunk_store: &ChunkStore, pos: glam::DVec3) -> f32 {
    use crate::renderer::chunk::mesher::LIGHT_TABLE;
    let bx = pos.x.floor() as i32;
    let by = pos.y.floor() as i32;
    let bz = pos.z.floor() as i32;
    let level = chunk_store
        .get_sky_light(bx, by, bz)
        .max(chunk_store.get_block_light(bx, by, bz));
    LIGHT_TABLE[level as usize]
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
