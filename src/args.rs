use clap::Parser;

#[derive(Parser, Debug)]
#[command(name = "pomc", about = "Minecraft client")]
pub struct LaunchArgs {
    #[arg(long)]
    pub username: Option<String>,

    #[arg(long)]
    pub uuid: Option<String>,

    #[arg(long)]
    pub access_token: Option<String>,

    #[arg(long)]
    pub server: Option<String>,

    #[arg(long)]
    pub assets_dir: Option<String>,

    #[arg(long)]
    pub game_dir: Option<String>,

    #[arg(long)]
    pub version: Option<String>,

    #[arg(long)]
    pub launch_token: Option<String>,

    #[arg(long)]
    pub dev: bool,
}
