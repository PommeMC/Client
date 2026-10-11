use crate::ui::components::debug::entries::fps::FpsDebugEntry;
use crate::ui::components::debug::entries::frame_timings::FrameTimingsDebugEntry;
use crate::ui::components::debug::entries::game_version::GameVersionDebugEntry;
use crate::ui::components::debug::entries::memory::{DetailedMemoryDebugEntry, MemoryDebugEntry};
use crate::ui::components::debug::entries::noop::NoopDebugEntry;
use crate::ui::components::debug::entries::position::{
    PositionDebugEntry, SectionPositionDebugEntry,
};
use crate::ui::components::debug::entries::system_specs::SystemSpecsDebugEntry;
use crate::ui::components::debug::entries::tps::TpsDebugEntry;
use crate::ui::components::debug::entry::DebugEntry;

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

    // Custom
    FrameTimings,
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
        // Custom
        Self::FrameTimings,
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
            // Custom
            Self::FrameTimings => "frame_timings",
        }
    }

    pub fn create_entry(self) -> Box<dyn DebugEntry> {
        use DebugEntryId as Id;
        match self {
            Id::GameVersion => Box::new(GameVersionDebugEntry::new()),
            Id::Fps => Box::new(FpsDebugEntry),
            Id::Tps => Box::new(TpsDebugEntry),
            Id::Memory => Box::new(MemoryDebugEntry),
            Id::DetailedMemory => Box::new(DetailedMemoryDebugEntry),
            Id::SystemSpecs => Box::new(SystemSpecsDebugEntry),
            Id::PlayerPosition => Box::new(PositionDebugEntry),
            Id::PlayerSectionPosition => Box::new(SectionPositionDebugEntry),
            Id::FrameTimings => Box::new(FrameTimingsDebugEntry),

            Id::EntityHitboxes
            | Id::ChunkBorders
            | Id::ThreeDimensionalCrosshair
            | Id::ChunkSectionPaths
            | Id::ChunkSectionOctree
            | Id::VisualizeWaterLevels
            | Id::VisualizeHeightmap
            | Id::VisualizeCollisionBoxes
            | Id::VisualizeEntitySupportingBlocks
            | Id::VisualizeBlockLightLevels
            | Id::VisualizeSkyLightLevels
            | Id::VisualizeSolidFaces
            | Id::VisualizeChunksOnServer
            | Id::VisualizeSkyLightSections
            | Id::ChunkSectionVisibility => Box::new(NoopDebugEntry::new()),

            // TODO(debug-overlay): Move each out of this arm
            // when it gets its own file.
            Id::LookingAtBlockState
            | Id::LookingAtBlockTags
            | Id::LookingAtFluidState
            | Id::LookingAtFluidTags
            | Id::LookingAtEntity
            | Id::LookingAtEntityTags
            | Id::ChunkRenderStats
            | Id::ChunkSectionStatus
            | Id::ChunkGenerationStats
            | Id::ChunkLoadStatus
            | Id::EntityRenderStats
            | Id::ParticleRenderStats
            | Id::ChunkSourceStats
            | Id::PlayerSpeed
            | Id::LightLevels
            | Id::LightmapTexture
            | Id::Heightmap
            | Id::Biome
            | Id::LocalDifficulty
            | Id::DayCount
            | Id::EntitySpawnCounts
            | Id::SoundMood
            | Id::SoundCache
            | Id::PostEffects
            | Id::GpuUtilization
            | Id::SimplePerformanceImpactors => Box::new(NoopDebugEntry::new()),
        }
    }
}
