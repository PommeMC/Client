// Per-thread-heap allocator (see Cargo.toml): keeps the chunk-mesh worker
// pool's cross-thread Vec churn from serializing on the system heap's global
// lock and stalling the main thread.
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

mod account;
mod app;
mod args;
mod assets;
mod attribute;
mod audio;
mod benchmark;
mod chat_component;
#[cfg(feature = "cpu-bench")]
mod cpu_bench;
mod dirs;
mod discord;
mod entity;
#[cfg(feature = "test-harness")]
mod harness;
mod lang;
mod logging;
mod mob_effect;
mod net;
mod particle;
mod physics;
mod player;
mod position;
mod recipe;
mod renderer;
mod resource_pack;
mod singleplayer;
#[cfg(test)]
mod test_util;
mod tool;
mod ui;
mod update;
mod user;
mod util;
mod version;
mod world;

use std::path::PathBuf;
use std::sync::Arc;

use pomme_protocol::ProtocolVersion;
use pomme_protocol::version::{NATIVE, VERSIONS};
#[cfg(target_os = "android")]
pub use winit::platform::android::activity::AndroidApp;

use crate::app::App;
pub use crate::args::{LAUNCH_FILE, LaunchArgs};
#[cfg(target_os = "android")]
pub use crate::logging::install_panic_hook;
use crate::user::UserData;

/// macOS has no system Vulkan: releases ship MoltenVK beside the binary,
/// and the loader only finds it through its manifest.
#[cfg(target_os = "macos")]
fn use_bundled_moltenvk() {
    if std::env::var_os("VK_ICD_FILENAMES").is_some()
        || std::env::var_os("VK_DRIVER_FILES").is_some()
    {
        return;
    }
    let Some(icd) = std::env::current_exe()
        .ok()
        .and_then(|exe| Some(exe.parent()?.join("MoltenVK_icd.json")))
        .filter(|icd| icd.exists())
    else {
        return;
    };
    // SAFETY: the first thing `run` does, before it starts any thread.
    unsafe { std::env::set_var("VK_ICD_FILENAMES", icd) };
}

/// What differs per platform at startup.
pub struct Platform {
    /// Replaces the per-user data directory; Android passes its app storage.
    pub data_root: Option<PathBuf>,
    #[cfg(target_os = "android")]
    pub android_app: AndroidApp,
}

pub fn run(args: LaunchArgs, platform: Platform) {
    #[cfg(target_os = "macos")]
    use_bundled_moltenvk();
    if let Some(root) = platform.data_root {
        pomme_dirs::set_root(root);
    }

    // Ahead of logging, which writes into the folders this moves.
    if args.owns_data_folder()
        && let Err(e) = pomme_dirs::migrate()
    {
        eprintln!("Couldn't move the old .pomme folder: {e}");
    }
    #[cfg(target_os = "android")]
    if let Ok(mut app) = util::ANDROID_APP.write() {
        *app = Some(platform.android_app.clone());
    }

    // Ahead of the launch-token guard, which bench builds have no launcher for.
    #[cfg(feature = "cpu-bench")]
    if let Some(filter) = &args.cpu_bench {
        let version = args.version.as_deref().unwrap_or(NATIVE.name);
        let data_dirs = args.data_dirs(version);
        std::process::exit(cpu_bench::run(version, &data_dirs, filter.as_deref()));
    }

    update::clean_up();
    let check_for_update = update::enabled(&args);

    // Bare launches take the newest joinable version, skipping any staged one.
    let default_version = VERSIONS
        .iter()
        .find(|v| net::translate::joinable(v.protocol))
        .unwrap_or(&NATIVE);
    let version = args.version.as_deref().unwrap_or(default_version.name);

    match ProtocolVersion::from_name(version) {
        Some(v) => version::set_selected_protocol(v.protocol),
        None => {
            eprintln!(
                "{version} is not currently supported. Supported versions: {}",
                VERSIONS
                    .iter()
                    .map(|v| v.name)
                    .collect::<Vec<_>>()
                    .join(", ")
            );
            #[cfg(not(debug_assertions))]
            std::process::exit(1);
        }
    }

    let data_dirs = args.data_dirs(version);

    let log_dir = data_dirs.game_dir.join("logs");
    std::fs::create_dir_all(&log_dir).unwrap();
    let _guard = logging::init(&log_dir);
    #[cfg(target_os = "ios")]
    for dir in [
        &data_dirs.assets_dir,
        &data_dirs.versions_dir,
        &data_dirs.cache_dir,
    ] {
        util::exclude_from_backup(dir);
    }

    // Block-state tables must be loaded before any world/render code runs.
    world::block::init(version);

    data_dirs.ensure_game_dir().ok();
    tracing::info!("Installation directory: {}", data_dirs.game_dir.display());

    // A single connection needs only a few async workers; the default runtime
    // spawns one per core and floods them decoding the chunk-load burst, starving
    // the render/mesh threads. Cap it so those cores stay free.
    let rt = Arc::new(
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(4)
            .enable_all()
            .build()
            .expect("Failed to create tokio runtime"),
    );
    {
        let _runtime = rt.enter();
        crate::net::chat_security::ProfileKeyServices::prefetch();
    }

    let (user, require_account) = UserData::for_launch(
        args.username,
        args.uuid,
        args.access_token,
        cfg!(not(debug_assertions)),
        cfg!(mobile),
    );

    // A harness script stays off the user's Discord profile, and phones have
    // no Discord client to talk to.
    #[cfg(feature = "test-harness")]
    let discord = args.harness_script.is_none();
    #[cfg(not(feature = "test-harness"))]
    let discord = cfg!(not(mobile));
    let presence = discord
        .then(|| {
            crate::discord::DiscordPresence::start(version)
                .inspect_err(|e| tracing::warn!("Discord rich presence unavailable: {e}"))
                .ok()
        })
        .flatten();

    let mut app = App::new(
        version.to_owned(),
        data_dirs,
        rt,
        presence,
        user,
        args.quick_access_multiplayer
            .map(app::QuickPlay::Multiplayer)
            .or(args
                .quick_access_singleplayer
                .map(app::QuickPlay::Singleplayer)),
        winit::dpi::LogicalSize::new(args.width, args.height),
    );
    if check_for_update {
        app.check_for_update();
    }
    if require_account {
        app.require_account();
    }
    #[cfg(feature = "test-harness")]
    {
        let harness = crate::harness::Harness::from_args(
            args.harness_port,
            args.harness_script,
            args.harness_out,
        )
        .unwrap_or_else(|e| {
            tracing::error!("{e}");
            std::process::exit(2);
        });
        app = app.with_harness(harness);
    }
    #[cfg(target_os = "android")]
    let event_loop = {
        use winit::platform::android::EventLoopBuilderExtAndroid;
        winit::event_loop::EventLoop::builder()
            .with_android_app(platform.android_app)
            .build()
    };
    #[cfg(not(target_os = "android"))]
    let event_loop = winit::event_loop::EventLoop::new();
    if let Err(e) = event_loop
        .map_err(Into::into)
        .and_then(|event_loop| app.run(event_loop))
    {
        tracing::error!("Fatal: {e}");
        std::process::exit(1);
    }
    #[cfg(feature = "test-harness")]
    if let Some(code) = app.harness_exit_code() {
        std::process::exit(code);
    }
}
