use clap::{Parser, ValueEnum};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, ValueEnum)]
pub enum ChunkRendererMode {
    #[default]
    Auto,
    Indirect,
    Task,
}

#[derive(Parser, Debug)]
#[command(name = "pomme", about = "Minecraft client")]
pub struct LaunchArgs {
    #[arg(long)]
    pub version: Option<String>,

    #[arg(long)]
    pub username: Option<String>,

    #[arg(long)]
    pub uuid: Option<String>,

    #[arg(long)]
    pub access_token: Option<String>,

    #[arg(long)]
    pub launch_token: Option<String>,

    #[arg(long)]
    pub assets_dir: Option<String>,

    #[arg(long)]
    pub versions_dir: Option<String>,

    #[arg(long)]
    pub game_dir: Option<String>,

    #[arg(long)]
    pub quick_access_multiplayer: Option<String>,

    /// Terrain renderer. `auto` prefers task shaders and falls back to indirect
    /// draws.
    #[arg(long, value_enum, default_value_t)]
    pub chunk_renderer: ChunkRendererMode,

    /// Add named regions to Vulkan command buffers for graphics debuggers.
    #[arg(long)]
    pub debug_labels: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunk_renderer_modes_parse() {
        assert_eq!(
            LaunchArgs::parse_from(["pomme"]).chunk_renderer,
            ChunkRendererMode::Auto
        );
        assert_eq!(
            LaunchArgs::parse_from(["pomme", "--chunk-renderer", "indirect"]).chunk_renderer,
            ChunkRendererMode::Indirect
        );
        assert_eq!(
            LaunchArgs::parse_from(["pomme", "--chunk-renderer", "task"]).chunk_renderer,
            ChunkRendererMode::Task
        );
    }

    #[test]
    fn debug_labels_are_opt_in() {
        assert!(!LaunchArgs::parse_from(["pomme"]).debug_labels);
        assert!(LaunchArgs::parse_from(["pomme", "--debug-labels"]).debug_labels);
    }
}
