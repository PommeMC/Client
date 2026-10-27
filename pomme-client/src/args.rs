use clap::Parser;

use crate::dirs::DataDirs;

#[derive(Parser, Debug)]
#[command(name = "pomme", about = "Minecraft client")]
pub struct LaunchArgs {
    #[arg(long)]
    pub version: Option<String>,

    /// Initial window width in logical pixels (windowed mode).
    #[arg(long, default_value_t = 854, value_parser = clap::value_parser!(u32).range(1..))]
    pub width: u32,

    /// Initial window height in logical pixels (windowed mode).
    #[arg(long, default_value_t = 480, value_parser = clap::value_parser!(u32).range(1..))]
    pub height: u32,

    #[arg(long)]
    pub username: Option<String>,

    #[arg(long)]
    pub uuid: Option<String>,

    #[arg(long)]
    pub access_token: Option<String>,

    /// Passed by the old launcher, which keeps its own copies of the client
    /// up to date; it turns off the client's own update.
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

    /// Joins the quick-access world, or else the fixed-seed bench world, runs
    /// the chunk-load benchmark at this render distance, writes
    /// `chunk_load.json` and exits.
    #[arg(
        long,
        value_name = "RD",
        value_parser = clap::value_parser!(u32)
            .range(crate::benchmark::CHUNK_LOAD_MIN_RD as i64 + 1
                ..=crate::world::chunk::MAX_VIEW_DISTANCE as i64)
    )]
    pub chunk_bench: Option<u32>,

    /// Runs `--chunk-bench` with no window or GPU: chunks are meshed but not
    /// uploaded or drawn.
    #[arg(long, requires = "chunk_bench")]
    pub headless: bool,

    /// Runs the headless CPU benchmarks (optionally only those whose id
    /// contains the filter) and exits.
    #[cfg(feature = "cpu-bench")]
    #[arg(long, value_name = "FILTER")]
    pub cpu_bench: Option<Option<String>>,

    /// Prints an order-independent hash of the mesher's output for each bench
    /// scene, and exits.
    #[cfg(feature = "cpu-bench")]
    #[arg(long)]
    pub mesh_hash: bool,

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

    /// Records every frame the server sends, before translation, into this
    /// file (`pomme_protocol::capture`).
    #[cfg(feature = "test-harness")]
    #[arg(long, value_name = "FILE")]
    pub capture_frames: Option<std::path::PathBuf>,
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

    pub fn quick_play(&self) -> Option<crate::app::QuickPlay> {
        self.quick_access_multiplayer
            .clone()
            .map(crate::app::QuickPlay::Multiplayer)
            .or(self
                .quick_access_singleplayer
                .clone()
                .map(crate::app::QuickPlay::Singleplayer))
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
    use clap::Parser;

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

    #[test]
    fn window_size_defaults_each_side_to_vanilla() {
        for (argv, size) in [
            (&[][..], (854, 480)),
            (&["--width", "1280", "--height", "720"], (1280, 720)),
            (&["--width", "1280"], (1280, 480)),
            (&["--height", "720"], (854, 720)),
        ] {
            let args = LaunchArgs::try_parse_from(["pomme"].iter().chain(argv)).unwrap();
            assert_eq!((args.width, args.height), size, "{argv:?}");
        }
    }

    #[test]
    fn chunk_bench_takes_a_render_distance_above_the_reset() {
        let words = |text| LaunchArgs::from_words(text).map(|a| a.chunk_bench);
        assert_eq!(words("--chunk-bench 16").unwrap(), Some(16));
        assert_eq!(
            words("--chunk-bench 8 --quick-access-multiplayer localhost").unwrap(),
            Some(8)
        );
        // The reset distance itself would time an empty load.
        assert!(words("--chunk-bench 2 --quick-access-singleplayer World").is_err());
        assert!(words("--chunk-bench 129 --quick-access-singleplayer World").is_err());
        assert!(words("--headless --quick-access-singleplayer World").is_err());
    }
}
