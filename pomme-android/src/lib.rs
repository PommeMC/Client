//! Android entry point. `NativeActivity` loads this library and calls
//! `android_main`, which reads the launch arguments from the app's files
//! directory and runs the client there.
#![cfg(target_os = "android")]

use pomme_client::{AndroidApp, LaunchArgs, Platform};

/// The launch arguments, as on a command line (`just android-config`).
const LAUNCH_FILE: &str = "launch.txt";

#[unsafe(no_mangle)]
fn android_main(app: AndroidApp) {
    pomme_client::install_panic_hook();
    // Internal storage, which the app owns; `adb push` into the external
    // directory leaves files the app can't write to.
    let files = app
        .internal_data_path()
        .expect("the app has no internal files directory");
    let text = std::fs::read_to_string(files.join(LAUNCH_FILE)).unwrap_or_default();
    let args = LaunchArgs::from_words(&text).unwrap_or_else(|e| {
        tracing::error!("{LAUNCH_FILE}: {e}");
        LaunchArgs::from_words("").expect("no arguments always parse")
    });
    pomme_client::run(
        args,
        Platform {
            data_root: Some(files.to_path_buf()),
            android_app: app,
        },
    );
}
