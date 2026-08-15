use clap::{Parser, ValueEnum};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, ValueEnum)]
pub enum ChunkRendererMode {
    #[default]
    Auto,
    Legacy,
    Mesh,
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

    /// Terrain rendering backend. `mesh` fails at startup when unsupported.
    #[arg(long, value_enum, default_value_t)]
    pub chunk_renderer: ChunkRendererMode,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunk_renderer_defaults_to_auto_and_accepts_forced_modes() {
        assert_eq!(
            LaunchArgs::parse_from(["pomme"]).chunk_renderer,
            ChunkRendererMode::Auto
        );
        assert_eq!(
            LaunchArgs::parse_from(["pomme", "--chunk-renderer", "legacy"]).chunk_renderer,
            ChunkRendererMode::Legacy,
        );
        assert_eq!(
            LaunchArgs::parse_from(["pomme", "--chunk-renderer", "mesh"]).chunk_renderer,
            ChunkRendererMode::Mesh,
        );
    }
}
