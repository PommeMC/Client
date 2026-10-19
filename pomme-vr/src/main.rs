//! Pomme in a PC VR headset: the desktop client with an OpenXR runtime
//! (SteamVR, Meta's, Monado, the Meta XR Simulator) rendering the world.

mod backend;

use clap::Parser;

fn main() {
    let stereo = match backend::OpenXrBackend::new() {
        Ok(backend) => backend,
        Err(e) => {
            eprintln!("Couldn't start VR: {e}");
            std::process::exit(1);
        }
    };
    pomme_client::run(
        pomme_client::LaunchArgs::parse(),
        pomme_client::Platform {
            data_root: None,
            stereo: Some(Box::new(stereo)),
        },
    );
}
