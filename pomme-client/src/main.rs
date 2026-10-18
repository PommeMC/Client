use clap::Parser;

fn main() {
    pomme_client::run(
        pomme_client::LaunchArgs::parse(),
        pomme_client::Platform { data_root: None },
    );
}
