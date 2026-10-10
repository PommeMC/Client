use std::sync::OnceLock;

use pyronyx::vk::PresentModeKHR;
use sysinfo::{CpuRefreshKind, System};
use winit::window::Window;

use crate::app::core::AppCore;
use crate::app::phases::Gfx;
use crate::app::phases::in_game::GameState;
use crate::memory::MemoryStats;
use crate::net::connection::ConnectionHandle;
use crate::renderer::{GpuType, Renderer};
use crate::singleplayer::World;
use crate::ui::components::debug::displayer::DebugScreenDisplayer;
use crate::ui::components::debug::groups::DebugGroup;
use crate::ui::menu::MAX_FRAMERATE_UNLIMITED;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DebugEntryId {
    GameVersion,
    Fps,
    Tps,
    Memory,
    DetailedMemory,
    SystemSpecs,
    LookingAtBlockState,
    LookingAtBlockTags,
    LookingAtFluidState,
    LookingAtFluidTags,
    LookingAtEntity,
    LookingAtEntityTags,
    ChunkRenderStats,
    ChunkSectionStatus,
    ChunkGenerationStats,
    ChunkLoadStatus,
    EntityRenderStats,
    ParticleRenderStats,
    ChunkSourceStats,
    PlayerPosition,
    PlayerSectionPosition,
    PlayerSpeed,
    LightLevels,
    LightmapTexture,
    Heightmap,
    Biome,
    LocalDifficulty,
    DayCount,
    EntitySpawnCounts,
    SoundMood,
    SoundCache,
    PostEffects,
    EntityHitboxes,
    ChunkBorders,
    #[serde(rename = "3d_crosshair")]
    ThreeDimensionalCrosshair,
    ChunkSectionPaths,
    GpuUtilization,
    SimplePerformanceImpactors,
    ChunkSectionOctree,
    VisualizeWaterLevels,
    VisualizeHeightmap,
    VisualizeCollisionBoxes,
    VisualizeEntitySupportingBlocks,
    VisualizeBlockLightLevels,
    VisualizeSkyLightLevels,
    VisualizeSolidFaces,
    VisualizeChunksOnServer,
    VisualizeSkyLightSections,
    ChunkSectionVisibility,
}

impl DebugEntryId {
    pub const ALL: &'static [Self] = &[
        Self::GameVersion,
        Self::Fps,
        Self::Tps,
        Self::Memory,
        Self::DetailedMemory,
        Self::SystemSpecs,
        Self::LookingAtBlockState,
        Self::LookingAtBlockTags,
        Self::LookingAtFluidState,
        Self::LookingAtFluidTags,
        Self::LookingAtEntity,
        Self::LookingAtEntityTags,
        Self::ChunkRenderStats,
        Self::ChunkSectionStatus,
        Self::ChunkGenerationStats,
        Self::ChunkLoadStatus,
        Self::EntityRenderStats,
        Self::ParticleRenderStats,
        Self::ChunkSourceStats,
        Self::PlayerPosition,
        Self::PlayerSectionPosition,
        Self::PlayerSpeed,
        Self::LightLevels,
        Self::LightmapTexture,
        Self::Heightmap,
        Self::Biome,
        Self::LocalDifficulty,
        Self::DayCount,
        Self::EntitySpawnCounts,
        Self::SoundMood,
        Self::SoundCache,
        Self::PostEffects,
        Self::EntityHitboxes,
        Self::ChunkBorders,
        Self::ThreeDimensionalCrosshair,
        Self::ChunkSectionPaths,
        Self::GpuUtilization,
        Self::SimplePerformanceImpactors,
        Self::ChunkSectionOctree,
        Self::VisualizeWaterLevels,
        Self::VisualizeHeightmap,
        Self::VisualizeCollisionBoxes,
        Self::VisualizeEntitySupportingBlocks,
        Self::VisualizeBlockLightLevels,
        Self::VisualizeSkyLightLevels,
        Self::VisualizeSolidFaces,
        Self::VisualizeChunksOnServer,
        Self::VisualizeSkyLightSections,
        Self::ChunkSectionVisibility,
    ];

    pub const fn name(self) -> &'static str {
        match self {
            Self::GameVersion => "game_version",
            Self::Fps => "fps",
            Self::Tps => "tps",
            Self::Memory => "memory",
            Self::DetailedMemory => "detailed_memory",
            Self::SystemSpecs => "system_specs",
            Self::LookingAtBlockState => "looking_at_block_state",
            Self::LookingAtBlockTags => "looking_at_block_tags",
            Self::LookingAtFluidState => "looking_at_fluid_state",
            Self::LookingAtFluidTags => "looking_at_fluid_tags",
            Self::LookingAtEntity => "looking_at_entity",
            Self::LookingAtEntityTags => "looking_at_entity_tags",
            Self::ChunkRenderStats => "chunk_render_stats",
            Self::ChunkSectionStatus => "chunk_section_status",
            Self::ChunkGenerationStats => "chunk_generation_stats",
            Self::ChunkLoadStatus => "chunk_load_status",
            Self::EntityRenderStats => "entity_render_stats",
            Self::ParticleRenderStats => "particle_render_stats",
            Self::ChunkSourceStats => "chunk_source_stats",
            Self::PlayerPosition => "player_position",
            Self::PlayerSectionPosition => "player_section_position",
            Self::PlayerSpeed => "player_speed",
            Self::LightLevels => "light_levels",
            Self::LightmapTexture => "lightmap_texture",
            Self::Heightmap => "heightmap",
            Self::Biome => "biome",
            Self::LocalDifficulty => "local_difficulty",
            Self::DayCount => "day_count",
            Self::EntitySpawnCounts => "entity_spawn_counts",
            Self::SoundMood => "sound_mood",
            Self::SoundCache => "sound_cache",
            Self::PostEffects => "post_effects",
            Self::EntityHitboxes => "entity_hitboxes",
            Self::ChunkBorders => "chunk_borders",
            Self::ThreeDimensionalCrosshair => "3d_crosshair",
            Self::ChunkSectionPaths => "chunk_section_paths",
            Self::GpuUtilization => "gpu_utilization",
            Self::SimplePerformanceImpactors => "simple_performance_impactors",
            Self::ChunkSectionOctree => "chunk_section_octree",
            Self::VisualizeWaterLevels => "visualize_water_levels",
            Self::VisualizeHeightmap => "visualize_heightmap",
            Self::VisualizeCollisionBoxes => "visualize_collision_boxes",
            Self::VisualizeEntitySupportingBlocks => "visualize_entity_supporting_blocks",
            Self::VisualizeBlockLightLevels => "visualize_block_light_levels",
            Self::VisualizeSkyLightLevels => "visualize_sky_light_levels",
            Self::VisualizeSolidFaces => "visualize_solid_faces",
            Self::VisualizeChunksOnServer => "visualize_chunks_on_server",
            Self::VisualizeSkyLightSections => "visualize_sky_light_sections",
            Self::ChunkSectionVisibility => "chunk_section_visibility",
        }
    }

    pub fn display(self, displayer: &mut DebugScreenDisplayer, info: &DebugInfo) {
        match self {
            Self::GameVersion => {
                displayer.add_priority_line(format!(
                    "Minecraft {} ({}/{})",
                    info.game_version_name, info.game_version_string, info.client_brand,
                ));
            }
            Self::Fps => {
                let framerate_limit = match info.framerate_limit {
                    MAX_FRAMERATE_UNLIMITED => "inf",
                    lim => &lim.to_string(),
                };

                let present_mode = match info.present_mode {
                    PresentModeKHR::Immediate => " (immediate)",
                    PresentModeKHR::Mailbox => " (mailbox)",
                    PresentModeKHR::Fifo => " (fifo)",
                    PresentModeKHR::FifoRelaxed => " (fifo relaxed)",
                    _ => "",
                };

                let refresh_rate: String = info
                    .refresh_rate_millihertz
                    .map(|mhz| {
                        if mhz % 1000 == 0 {
                            (mhz / 1000).to_string()
                        } else {
                            format!("{:.2}", mhz as f32 / 1000.0)
                        }
                    })
                    .unwrap_or_else(|| "0".to_string());

                displayer.add_priority_line(format!(
                    "{} fps T: {}{} @{}Hz",
                    info.fps, framerate_limit, present_mode, refresh_rate
                ));
            }
            Self::Tps => {
                let Some(tps) = &info.tps else {
                    return;
                };

                displayer.add_fact_to_group(DebugGroup::Misc, "Server", |fact| {
                    let mut run_status = if tps.is_stepping_forward {
                        "frozen - stepping"
                    } else if tps.is_frozen {
                        "frozen"
                    } else {
                        ""
                    };

                    if let Some(server) = &tps.integrated {
                        if server.is_sprinting {
                            run_status = "sprinting";
                        }

                        let tps_target = if server.is_sprinting {
                            "-".to_string()
                        } else {
                            format!("{:.1}", tps.target_mspt)
                        };

                        fact.value("Integrated")
                            .text(" @ ")
                            .value(format!("{:.1}", server.smoothed_tick_ms))
                            .text("/")
                            .value(tps_target)
                            .text(" ms");
                    } else {
                        fact.text("\"").value(tps.server_brand).text("\"");
                    }

                    if !run_status.is_empty() {
                        fact.text(" (").value(run_status).text(")");
                    }
                });

                displayer.add_fact_to_group(DebugGroup::Misc, "Packets", |fact| {
                    fact.value(format!("{:.0}", tps.avg_sent_packets))
                        .text(" tx, ")
                        .value(format!("{:.0}", tps.avg_received_packets))
                        .text(" rx");
                });
            }
            Self::Memory => {
                let m = info.memory;
                let mib = |b: u64| b / 1024 / 1024;
                let percent = (m.live_bytes * 100).checked_div(m.max).unwrap_or(0);

                displayer.add_fact_to_group(DebugGroup::Memory, "Used", |f| {
                    f.value(format!("{percent:>2}"))
                        .text("% ")
                        .value(format!("{:03}", mib(m.live_bytes)))
                        .text("/")
                        .value(format!("{:03}", mib(m.max)))
                        .text("MiB");
                });
            }
            Self::DetailedMemory => {
                let m = info.memory;
                let mib = |b: u64| b / 1024 / 1024;

                displayer.add_fact_to_group(DebugGroup::Memory, "Allocs", |f| {
                    f.value(m.live_allocs);
                });
                displayer.add_fact_to_group(DebugGroup::Memory, "Alloc rate", |f| {
                    f.value(format!("{:03}", mib(m.alloc_bytes_per_sec)))
                        .text("MiB/s");
                });
                displayer.add_fact_to_group(DebugGroup::Memory, "Dealloc rate", |f| {
                    f.value(format!("{:03}", mib(m.free_bytes_per_sec)))
                        .text("MiB/s");
                });
            }
            Self::SystemSpecs => {
                let s = &info.system;

                displayer.add_fact_to_group(DebugGroup::SystemSpecs, "Rust", |f| {
                    f.value(env!("RUSTC_VERSION"));
                });
                displayer.add_fact_to_group(DebugGroup::SystemSpecs, "CPU", |f| {
                    f.value(SystemSpecsInfo::cpu_info());
                });
                displayer.add_fact_to_group(DebugGroup::SystemSpecs, "Display", |f| {
                    f.value(s.display_width)
                        .text("x")
                        .value(s.display_height)
                        .text(" (")
                        .value(s.vendor_name)
                        .text(")");
                });
                displayer.add_fact_to_group(DebugGroup::SystemSpecs, "Window", |f| {
                    f.value(s.window_width)
                        .text("x")
                        .value(s.window_height)
                        .text(" (")
                        .value(format!("{:.2}", s.pixel_density))
                        .text("x pixel density)");
                });

                let type_name = match s.gpu_type {
                    GpuType::Other => "",
                    GpuType::Integrated => " (iGPU)",
                    GpuType::Discrete => " (dGPU)",
                    GpuType::Virtual => " (vGPU)",
                    GpuType::Cpu => " (software)",
                };
                displayer.add_to_group(
                    DebugGroup::SystemSpecs,
                    format!("{}{type_name}", s.gpu_name),
                );
                if let Some(driver_info) = s.driver_info.as_ref() {
                    displayer.add_to_group(
                        DebugGroup::SystemSpecs,
                        format!("{} {}", s.vulkan_version, first_line(driver_info)),
                    );
                }
            }

            // TODO(debug-overlay)
            Self::PlayerPosition => {
                let Some(p) = &info.position else { return };

                let (bx, by, bz) = (p.x.floor() as i32, p.y.floor() as i32, p.z.floor() as i32);
                let (cx, cy, cz) = (bx >> 4, by >> 4, bz >> 4);

                let (direction, face) = match ((p.y_rot / 90.0 + 0.5).floor() as i32) & 3 {
                    0 => ("south", "Towards positive Z"),
                    1 => ("west", "Towards negative X"),
                    2 => ("north", "Towards negative Z"),
                    _ => ("east", "Towards positive X"),
                };

                displayer.add_fact_to_group(DebugGroup::Position, "XYZ", |f| {
                    f.value(format!("{:.3}", p.x))
                        .text(" / ")
                        .value(format!("{:.5}", p.y))
                        .text(" / ")
                        .value(format!("{:.3}", p.z));
                });
                displayer.add_fact_to_group(DebugGroup::Position, "Block", |f| {
                    f.value(bx).text(" ").value(by).text(" ").value(bz);
                });
                displayer.add_fact_to_group(DebugGroup::Position, "Chunk", |f| {
                    f.value(cx)
                        .text(" ")
                        .value(cy)
                        .text(" ")
                        .value(cz)
                        .text(" [")
                        .value(cx & 31)
                        .text(" ")
                        .value(cz & 31)
                        .text(" in ")
                        .value(format!("r.{}.{}.mca", cx >> 5, cz >> 5))
                        .text("]");
                });
                displayer.add_fact_to_group(DebugGroup::Position, "Facing", |f| {
                    f.value(direction)
                        .text(" (")
                        .value(face)
                        .text(") (")
                        .value(format!("{:.1}", wrap_degrees(p.y_rot)))
                        .text(" / ")
                        .value(format!("{:.1}", wrap_degrees(p.x_rot)))
                        .text(")");
                });
                displayer.add_fact_to_group(DebugGroup::Position, "Dimension", |f| {
                    f.value(p.dimension);
                });
                if p.force_loaded_chunks > 0 {
                    displayer.add_fact_to_group(DebugGroup::Position, "Forced Chunks", |f| {
                        f.value(p.force_loaded_chunks);
                    });
                }
            }
            Self::PlayerSectionPosition => {
                let Some(p) = &info.position else { return };

                let (bx, by, bz) = (p.x.floor() as i32, p.y.floor() as i32, p.z.floor() as i32);

                displayer.add_fact_to_group(DebugGroup::Position, "Section-Relative", |f| {
                    f.value(format!("{:02} {:02} {:02}", bx & 15, by & 15, bz & 15));
                });
            }

            // TODO(debug-overlay)
            _ => {}
        }
    }
}

pub struct DebugInfo<'a> {
    pub game_version_name: String,
    pub game_version_string: String,
    pub client_brand: &'a str,

    pub fps: u32,
    pub framerate_limit: u32,
    pub present_mode: PresentModeKHR,
    pub refresh_rate_millihertz: Option<u32>,

    pub tps: Option<TpsDebugInfo<'a>>,

    pub memory: MemoryStats,

    pub system: SystemSpecsInfo<'a>,

    pub position: Option<PositionDebugInfo<'a>>,
}

impl<'a> DebugInfo<'a> {
    pub fn new(
        core: &'_ AppCore,
        gfx: &'_ Gfx,
        tps: Option<TpsDebugInfo<'a>>,
        system: SystemSpecsInfo<'a>,
        game: Option<&'a GameState>,
    ) -> Self {
        DebugInfo {
            game_version_name: core.version.to_owned(),
            game_version_string: core.version.to_owned(),
            client_brand: "vanilla",

            fps: gfx.fps_counter.display_fps(),
            framerate_limit: core.menu.max_framerate,
            present_mode: gfx.renderer.present_mode(),
            refresh_rate_millihertz: gfx
                .window
                .current_monitor()
                .and_then(|monitor| monitor.refresh_rate_millihertz()),

            tps,

            memory: MemoryStats::sample(),

            system,

            position: game.map(|game| PositionDebugInfo::new(game, gfx)),
        }
    }
}

pub struct TpsDebugInfo<'a> {
    pub is_stepping_forward: bool,
    pub is_frozen: bool,
    pub target_mspt: f32,
    /// `Some` when in singleplayer
    pub integrated: Option<IntegratedServerInfo>,
    pub server_brand: &'a str,
    pub avg_sent_packets: f32,
    pub avg_received_packets: f32,
}

pub struct IntegratedServerInfo {
    pub is_sprinting: bool,
    pub smoothed_tick_ms: f32,
}

impl<'a> TpsDebugInfo<'a> {
    pub fn new(connection: &'_ ConnectionHandle, world: Option<&'_ mut World>) -> Self {
        TpsDebugInfo {
            is_stepping_forward: false, // TODO(debug-overlay)
            is_frozen: false,           // TODO(debug-overlay)
            target_mspt: 50.0,          // TODO(debug-overlay)
            integrated: world.map(|_| IntegratedServerInfo {
                is_sprinting: false,   // TODO(debug-overlay)
                smoothed_tick_ms: 0.0, // TODO(debug-overlay)
            }),
            server_brand: "vanilla", // TODO(debug-overlay)
            avg_sent_packets: connection.packet_stats.avg_sent(),
            avg_received_packets: connection.packet_stats.avg_received(),
        }
    }
}

pub struct SystemSpecsInfo<'a> {
    pub display_width: u32,
    pub display_height: u32,
    pub vendor_name: &'a str,
    pub window_width: u32,
    pub window_height: u32,
    pub pixel_density: f64,
    pub gpu_name: &'a str,
    pub gpu_type: GpuType,
    pub vulkan_version: &'a str,
    pub driver_info: Option<&'a str>,
}

impl<'a> SystemSpecsInfo<'a> {
    pub fn new(window: &'_ Window, renderer: &'a Renderer) -> Self {
        let display_size = window.inner_size();
        let pixel_density = window.scale_factor();
        let window_size = window.inner_size().to_logical(pixel_density);

        Self {
            display_width: display_size.width,
            display_height: display_size.height,
            vendor_name: Self::vendor_name(renderer.gpu_vendor_id()),
            window_width: window_size.width,
            window_height: window_size.height,
            pixel_density,
            gpu_name: renderer.gpu_name(),
            gpu_type: renderer.gpu_type(),
            vulkan_version: renderer.vulkan_version(),
            driver_info: renderer.driver_info(),
        }
    }

    pub fn cpu_info() -> &'static str {
        static CPU: OnceLock<String> = OnceLock::new();
        CPU.get_or_init(|| {
            let mut sys = System::new();
            sys.refresh_cpu_specifics(CpuRefreshKind::nothing());
            match sys.cpus().first() {
                Some(cpu) => {
                    let name = cpu.brand().split_whitespace().collect::<Vec<_>>().join(" ");
                    format!("{}x {name}", sys.cpus().len())
                }
                None => "<unknown>".to_owned(),
            }
        })
    }

    pub fn vendor_name(id: u32) -> &'static str {
        match id {
            0x10DE => "NVIDIA",
            0x1002 => "AMD",
            0x8086 => "INTEL",
            0x106B => "APPLE",
            0x13B5 => "ARM",
            0x5143 => "QUALCOMM",
            0x1010 => "IMAGINATION",
            _ => "Unknown",
        }
    }
}

pub struct PositionDebugInfo<'a> {
    pub x: f64,
    pub y: f64,
    pub z: f64,
    pub y_rot: f32,
    pub x_rot: f32,
    pub dimension: &'a str,
    pub force_loaded_chunks: usize,
}

impl<'a> PositionDebugInfo<'a> {
    pub fn new(game: &'a GameState, gfx: &'_ Gfx) -> Self {
        Self {
            x: game.player.position.x,
            y: game.player.position.y,
            z: game.player.position.z,
            y_rot: gfx.renderer.camera_look_dir().y_rot_deg(),
            x_rot: gfx.renderer.camera_look_dir().x_rot_deg(),
            dimension: &game.dimension,
            force_loaded_chunks: 0, // TODO(debug-overlay)
        }
    }
}

fn first_line(s: &str) -> &str {
    s.lines().next().unwrap_or(s)
}

/// Vanilla `Mth.wrapDegrees`.
fn wrap_degrees(value: f32) -> f32 {
    let mut f = value % 360.0;
    if f >= 180.0 {
        f -= 360.0;
    }
    if f < -180.0 {
        f += 360.0;
    }
    f
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DebugEntryStatus {
    Never,
    InOverlay,
    AlwaysOn,
}
