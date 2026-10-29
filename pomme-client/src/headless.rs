//! `--chunk-bench --headless`: the chunk-load benchmark with no window or GPU.
//! Chunks are received, lit and meshed as in game; the meshes are counted
//! instead of uploaded.

use std::collections::HashMap;
use std::path::Path;
use std::rc::Rc;
use std::time::{Duration, Instant};

use azalea_protocol::packets::game::{ServerboundGamePacket, s_resource_pack};

use crate::app::core::AppCore;
use crate::app::phases::in_game::{GameState, section_bits};
use crate::app::{QuickPlay, TICK_RATE};
use crate::args::LaunchArgs;
use crate::benchmark::{BenchMode, CHUNK_LOAD_MIN_RD, ChunkLoadBench, ChunkLoadStep};
use crate::dirs::DataDirs;
use crate::net::NetworkEvent;
use crate::net::connection::{ConnectArgs, ConnectionHandle, Transport, spawn_connection};
use crate::position::ChunkPos;
use crate::renderer::RenderTimings;
use crate::renderer::chunk::mesh_source::CpuMeshAssets;
use crate::resources::ResourceManager;
use crate::resources::packs::ResourceConfiguration;
use crate::singleplayer::World;
use crate::ui::chat::ChatOptions;
use crate::ui::menu::{SkinOptions, saved_chunk_detail};
use crate::ui::world_list::WorldList;
use crate::user::UserData;
use crate::world::block::block_registry::BlockDefinitions;

/// For a server with no `--username`, the stored sign-in, which an online
/// server needs. Otherwise, or if it can't be restored, offline: the
/// integrated server doesn't check accounts.
pub fn user(
    args: &LaunchArgs,
    play: &QuickPlay,
    game_dir: &Path,
    rt: &tokio::runtime::Runtime,
) -> UserData {
    let stored = (args.username.is_none() && matches!(play, QuickPlay::Multiplayer(_)))
        .then(|| rt.block_on(crate::account::restore(&*crate::account::store(game_dir))))
        .flatten();
    match stored {
        Some(account) => UserData::from_account(&account),
        None => UserData::from_args(
            args.username.clone(),
            args.uuid.clone(),
            args.access_token.clone(),
        ),
    }
}

/// Runs the benchmark at `rd` in `play`'s world; returns the exit code.
pub fn run(
    rd: u32,
    play: QuickPlay,
    user: UserData,
    version: &str,
    dirs: &DataDirs,
    rt: &tokio::runtime::Runtime,
) -> i32 {
    let mut configuration =
        ResourceConfiguration::new(dirs.game_dir.join("resourcepacks/.server_cache"));
    configuration.set_installation(Some(dirs.installation_location(version)));
    let resources = match ResourceManager::from_configuration(&configuration) {
        Ok(resources) => resources,
        Err(error) => {
            tracing::error!(%error, jar = %dirs.client_jar.display(), "Failed to open headless resources");
            return 1;
        }
    };
    let (transport, mut world) = match open(play, dirs, rd) {
        Ok(opened) => opened,
        Err(reason) => {
            tracing::error!("{reason}");
            return 1;
        }
    };
    let assets = CpuMeshAssets::load(&resources);
    let chat_options = ChatOptions::default();
    let skin_options = SkinOptions::default();
    let connection = spawn_connection(
        rt,
        ConnectArgs {
            transport,
            username: user.username,
            uuid: user.uuid,
            access_token: user.access_token,
            view_distance: rd as u8,
            chat_options,
            skin_options,
        },
    );
    let mut game = GameState::new(
        &assets,
        rd,
        world.is_some(),
        chat_options,
        skin_options,
        Rc::new(BlockDefinitions::new()),
        crate::version::selected_protocol(),
    );

    let code = drive(
        rd,
        chat_options,
        &assets,
        &connection,
        &mut game,
        world.as_mut(),
        dirs,
    );
    drop(connection);
    if let Some(mut world) = world {
        world.begin_close();
        let mut last_poll_error = None;
        while !world.is_closed() {
            if let Err(error) = world.poll()
                && last_poll_error.as_ref() != Some(&error)
            {
                tracing::warn!(%error, "Failed to poll world while waiting for shutdown");
                last_poll_error = Some(error);
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    code
}

fn open(play: QuickPlay, dirs: &DataDirs, rd: u32) -> Result<(Transport, Option<World>), String> {
    match play {
        QuickPlay::Multiplayer(server) => Ok((
            Transport::Remote {
                server,
                protocol: None,
            },
            None,
        )),
        QuickPlay::Singleplayer(folder) => {
            let saves_dir = crate::ui::world_list::saves_dir(&dirs.game_dir);
            let worlds = WorldList::scan(&saves_dir);
            let summary = worlds
                .get(&folder)
                .ok_or_else(|| format!("No world in {}", saves_dir.join(&folder).display()))?;
            let (world, end) =
                crate::singleplayer::open(summary, &saves_dir.join(&folder), rd as u8)?;
            Ok((
                Transport::Memory {
                    end,
                    protocol: crate::singleplayer::PROTOCOL,
                },
                Some(world),
            ))
        }
    }
}

/// The game loop's chunk path, in `update_game`'s order: network, meshes,
/// ticks, light, then the benchmark step.
fn drive(
    rd: u32,
    chat_options: ChatOptions,
    assets: &CpuMeshAssets,
    connection: &ConnectionHandle,
    game: &mut GameState,
    mut world: Option<&mut World>,
    dirs: &DataDirs,
) -> i32 {
    let chunk_detail = saved_chunk_detail(&dirs.game_dir);
    let skin_options = game.last_skin_options;
    // Columns with at least one non-empty section, as the GPU buffers count
    // them.
    let mut drawn: HashMap<ChunkPos, u32> = HashMap::new();
    let mut ticking = false;
    let mut tick_accumulator = 0.0;
    let mut last_pass = Instant::now();

    loop {
        if let Some(world) = world.as_deref_mut()
            && let Err(reason) = world.poll()
        {
            tracing::error!("{reason}");
            return 1;
        }

        let t_net = Instant::now();
        while let Ok(event) = connection.event_rx.try_recv() {
            match event {
                NetworkEvent::ProtocolSelected { protocol } => game.select_protocol(protocol),
                NetworkEvent::Connected => {
                    tracing::info!("Connected to server");
                    ticking = true;
                }
                NetworkEvent::LevelChunksLoadStart => {
                    if let Some(tracker) = &mut game.level_load {
                        tracker.loading_packets_received();
                    }
                }
                NetworkEvent::Registries(registries) => game.registries = registries,
                NetworkEvent::BiomeColors { colors } => game.set_biome_climate(colors),
                NetworkEvent::DimensionInfo(dim) => {
                    game.enter_dimension(dim, rd, assets);
                    drawn.clear();
                }
                NetworkEvent::PlayerLogin { entity_id, .. } => {
                    game.player.entity_id = entity_id;
                    game.start_level_load();
                }
                NetworkEvent::ChunkLoaded {
                    pos,
                    data,
                    heightmaps,
                    light,
                } => game.load_chunk(pos, &data, &heightmaps, &light),
                NetworkEvent::LightUpdate { pos, light } => game.apply_light_update(pos, &light),
                NetworkEvent::ChunkUnloaded { pos } => {
                    game.unload_chunk(pos);
                    drawn.remove(&pos);
                }
                NetworkEvent::ChunkCacheCenter { x, z } => {
                    game.chunk_store.set_center(ChunkPos::new(x, z));
                }
                NetworkEvent::Ping { id } => AppCore::send_pong(connection, id),
                NetworkEvent::PlayerPosition {
                    id,
                    change,
                    relative,
                } => {
                    game.apply_player_position(connection, id, &change, &relative);
                }
                NetworkEvent::ServerViewDistance { distance } => {
                    game.set_server_view_distance(distance);
                }
                NetworkEvent::ResourcePackPush { id, .. } => {
                    connection
                        .packet_tx
                        .send(ServerboundGamePacket::ResourcePack(
                            s_resource_pack::ServerboundResourcePack {
                                id,
                                action: s_resource_pack::Action::Declined,
                            },
                        ));
                }
                NetworkEvent::Disconnected { reason } => {
                    tracing::error!("Disconnected: {reason}");
                    return 1;
                }
                _ => {}
            }
        }
        let ms = |t: Instant| t.elapsed().as_secs_f32() * 1000.0;
        game.last_update_phases.net_decode_ms = ms(t_net);
        // Meshing doesn't wait on visibility, which only orders the drawing.
        let t_rescan = Instant::now();
        game.rescan_mesh_jobs(chunk_detail);
        game.last_update_phases.rescan_ms = ms(t_rescan);

        let batch = game.drain_meshes();
        for mesh in &batch {
            let mask = drawn.entry(mesh.pos).or_default();
            *mask &= !section_bits(mesh.replaced.clone());
            *mask |= section_bits(mesh.sections.iter().map(|s| s.section_index));
            if *mask == 0 {
                drawn.remove(&mesh.pos);
            }
        }
        let pass_secs = last_pass.elapsed().as_secs_f32();
        last_pass = Instant::now();
        if ticking {
            tick_accumulator += pass_secs;
            while tick_accumulator >= TICK_RATE {
                AppCore::tick_level_load(*game.player.eye_pos(), connection, game);
                AppCore::send_client_tick_end(connection);
                tick_accumulator -= TICK_RATE;
            }
        }
        game.update_light();

        if game.chunk_load_bench.is_none() && game.client_loaded {
            game.chunk_load_bench = Some(ChunkLoadBench::new(
                rd,
                game.server_render_distance,
                chunk_detail,
                BenchMode::Headless,
                *game.player.position,
            ));
            game.sync_client_information(connection, CHUNK_LOAD_MIN_RD, chat_options, skin_options);
        }
        // Taken for the step, as in `update_game`, so a new distance can be
        // sent; `drain_meshes` records the mesh timings into it meanwhile.
        if let Some(mut bench) = game.chunk_load_bench.take() {
            match bench.update(
                drawn.len() as u32,
                pass_secs * 1000.0,
                &RenderTimings::default(),
                game.last_update_phases,
            ) {
                ChunkLoadStep::Wait => {}
                ChunkLoadStep::Load(rd) => {
                    game.sync_client_information(connection, rd, chat_options, skin_options);
                }
                ChunkLoadStep::Done(result) => {
                    result.save(&dirs.game_dir);
                    return 0;
                }
            }
            game.chunk_load_bench = Some(bench);
        }
        std::thread::sleep(Duration::from_millis(1));
    }
}
