use pyronyx::vk::PresentModeKHR;

use crate::app::core::AppCore;
use crate::app::phases::Gfx;
use crate::net::connection::ConnectionHandle;
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
}

impl<'a> DebugInfo<'a> {
    pub fn new(core: &'_ AppCore, gfx: &'_ Gfx, tps: Option<TpsDebugInfo<'a>>) -> Self {
        DebugInfo {
            game_version_name: core.version.to_owned(),
            game_version_string: core.version.to_owned(),
            client_brand: "vanilla",

            fps: gfx.fps_counter.display_fps(),
            framerate_limit: core.menu.max_framerate,
            present_mode: pyronyx::vk::PresentModeKHR::Immediate, // TODO(debug-overlay)
            refresh_rate_millihertz: gfx
                .window
                .current_monitor()
                .and_then(|monitor| monitor.refresh_rate_millihertz()),

            tps,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DebugEntryStatus {
    Never,
    InOverlay,
    AlwaysOn,
}
