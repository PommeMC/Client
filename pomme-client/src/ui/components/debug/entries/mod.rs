pub mod fps;
pub mod frame_timings;
pub mod game_version;
pub mod memory;
pub mod noop;
pub mod position;
pub mod system_specs;
pub mod tps;

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

macro_rules! define_debug_entry_ids {
    ($($variant:ident => $name:literal),* $(,)?) => {
        #[derive(
            Debug, Clone, Copy, PartialEq, Eq, Hash,
            serde::Serialize, serde::Deserialize
        )]
        pub enum DebugEntryId {
            $(
                #[serde(rename = $name)]
                $variant,
            )*
        }

        impl DebugEntryId {
            pub const ALL: &'static [Self] = &[
                $(Self::$variant,)*
            ];

            pub const fn name(self) -> &'static str {
                match self {
                    $(Self::$variant => $name,)*
                }
            }
        }
    };
}

define_debug_entry_ids! {
    GameVersion => "game_version",
    Fps => "fps",
    Tps => "tps",
    Memory => "memory",
    DetailedMemory => "detailed_memory",
    SystemSpecs => "system_specs",
    LookingAtBlockState => "looking_at_block_state",
    LookingAtBlockTags => "looking_at_block_tags",
    LookingAtFluidState => "looking_at_fluid_state",
    LookingAtFluidTags => "looking_at_fluid_tags",
    LookingAtEntity => "looking_at_entity",
    LookingAtEntityTags => "looking_at_entity_tags",
    ChunkRenderStats => "chunk_render_stats",
    ChunkSectionStatus => "chunk_section_status",
    ChunkGenerationStats => "chunk_generation_stats",
    ChunkLoadStatus => "chunk_load_status",
    EntityRenderStats => "entity_render_stats",
    ParticleRenderStats => "particle_render_stats",
    ChunkSourceStats => "chunk_source_stats",
    PlayerPosition => "player_position",
    PlayerSectionPosition => "player_section_position",
    PlayerSpeed => "player_speed",
    LightLevels => "light_levels",
    LightmapTexture => "lightmap_texture",
    Heightmap => "heightmap",
    Biome => "biome",
    LocalDifficulty => "local_difficulty",
    DayCount => "day_count",
    EntitySpawnCounts => "entity_spawn_counts",
    SoundMood => "sound_mood",
    SoundCache => "sound_cache",
    PostEffects => "post_effects",
    EntityHitboxes => "entity_hitboxes",
    ChunkBorders => "chunk_borders",
    ThreeDimensionalCrosshair => "3d_crosshair",
    ChunkSectionPaths => "chunk_section_paths",
    GpuUtilization => "gpu_utilization",
    SimplePerformanceImpactors => "simple_performance_impactors",
    ChunkSectionOctree => "chunk_section_octree",
    VisualizeWaterLevels => "visualize_water_levels",
    VisualizeHeightmap => "visualize_heightmap",
    VisualizeCollisionBoxes => "visualize_collision_boxes",
    VisualizeEntitySupportingBlocks => "visualize_entity_supporting_blocks",
    VisualizeBlockLightLevels => "visualize_block_light_levels",
    VisualizeSkyLightLevels => "visualize_sky_light_levels",
    VisualizeSolidFaces => "visualize_solid_faces",
    VisualizeChunksOnServer => "visualize_chunks_on_server",
    VisualizeSkyLightSections => "visualize_sky_light_sections",
    ChunkSectionVisibility => "chunk_section_visibility",

    // Custom
    FrameTimings => "pomme:frame_timings",
}

impl DebugEntryId {
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

            // TODO(debug-overlay): Move each out of this arm when it gets its own file.
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
