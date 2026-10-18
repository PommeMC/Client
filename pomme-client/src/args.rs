use clap::Parser;

use crate::dirs::DataDirs;

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

    #[arg(long, conflicts_with = "quick_access_singleplayer")]
    pub quick_access_multiplayer: Option<String>,

    /// Opens the singleplayer world in this save folder (vanilla
    /// `--quickPlaySingleplayer`).
    #[arg(long)]
    pub quick_access_singleplayer: Option<String>,

    /// Runs the headless CPU benchmarks (optionally only those whose id
    /// contains the filter) and exits.
    #[cfg(feature = "cpu-bench")]
    #[arg(long, value_name = "FILTER")]
    pub cpu_bench: Option<Option<String>>,

    /// Serves the test harness on this localhost port.
    #[cfg(feature = "test-harness")]
    #[arg(long, value_name = "PORT")]
    pub harness_port: Option<u16>,

    /// Runs a test-harness script, then exits with its result.
    #[cfg(feature = "test-harness")]
    #[arg(long, value_name = "FILE")]
    pub harness_script: Option<std::path::PathBuf>,

    /// Where a harness script writes its report and screenshots.
    #[cfg(feature = "test-harness")]
    #[arg(long, value_name = "DIR")]
    pub harness_out: Option<std::path::PathBuf>,
}

impl LaunchArgs {
    pub fn data_dirs(&self, version: &str) -> DataDirs {
        DataDirs::resolve(
            version,
            self.assets_dir.as_deref(),
            self.versions_dir.as_deref(),
            self.game_dir.as_deref(),
        )
    }
}
