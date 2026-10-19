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

/// The launch arguments' file on a phone, in the app's own storage.
pub const LAUNCH_FILE: &str = "launch.txt";

impl LaunchArgs {
    /// Arguments written out as on a command line, whitespace-separated, for
    /// phones, which start without an argv.
    pub fn from_words(text: &str) -> Result<Self, clap::Error> {
        Self::try_parse_from(std::iter::once("pomme").chain(text.split_whitespace()))
    }

    /// [`Self::from_words`] read from `path` (a [`LAUNCH_FILE`]), which the
    /// phone dev recipes write. A missing file is no arguments; a bad one is
    /// logged and ignored.
    pub fn from_file(path: &std::path::Path) -> Self {
        let text = std::fs::read_to_string(path).unwrap_or_default();
        Self::from_words(&text).unwrap_or_else(|e| {
            tracing::error!("{}: {e}", path.display());
            Self::from_words("").expect("no arguments always parse")
        })
    }

    pub fn data_dirs(&self, version: &str) -> DataDirs {
        DataDirs::resolve(
            version,
            self.assets_dir.as_deref(),
            self.versions_dir.as_deref(),
            self.game_dir.as_deref(),
        )
    }

    /// No asset or version folder was passed. A launcher that passes them owns
    /// them, and an older one still uses the old layout.
    pub fn owns_data_folder(&self) -> bool {
        self.assets_dir.is_none() && self.versions_dir.is_none()
    }
}

#[cfg(test)]
mod tests {
    use super::LaunchArgs;

    #[test]
    fn launch_words_parse_like_argv() {
        let args = LaunchArgs::from_words(
            "--username Tester\n--quick-access-multiplayer 10.0.2.2:25565\n",
        )
        .unwrap();
        assert_eq!(args.username.as_deref(), Some("Tester"));
        assert_eq!(
            args.quick_access_multiplayer.as_deref(),
            Some("10.0.2.2:25565")
        );
        assert!(LaunchArgs::from_words("").unwrap().username.is_none());
        assert!(LaunchArgs::from_words("--no-such-flag").is_err());
    }
}
