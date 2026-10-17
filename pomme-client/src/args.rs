use clap::Parser;

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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_size_defaults() {
        let args = LaunchArgs::try_parse_from(["pomme"]).unwrap();
        assert_eq!((args.width, args.height), (854, 480));
    }

    #[test]
    fn window_size_overrides() {
        let args =
            LaunchArgs::try_parse_from(["pomme", "--width", "1280", "--height", "720"]).unwrap();
        assert_eq!((args.width, args.height), (1280, 720));

        let args = LaunchArgs::try_parse_from(["pomme", "--width", "1280"]).unwrap();
        assert_eq!((args.width, args.height), (1280, 480));

        let args = LaunchArgs::try_parse_from(["pomme", "--height", "720"]).unwrap();
        assert_eq!((args.width, args.height), (854, 720));
    }

    #[test]
    fn window_size_rejects_invalid_dimensions() {
        for flag in ["--width", "--height"] {
            for value in ["0", "-1", "abc", "4294967296"] {
                assert!(LaunchArgs::try_parse_from(["pomme", flag, value]).is_err());
            }
            assert!(LaunchArgs::try_parse_from(["pomme", flag]).is_err());
        }
    }
}
