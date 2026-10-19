//! Android entry point. `NativeActivity` loads this library and calls
//! `android_main`, which reads the launch arguments from the app's files
//! directory and runs the client there.
#![cfg(target_os = "android")]

use pomme_client::{AndroidApp, LAUNCH_FILE, LaunchArgs, Platform};

#[unsafe(no_mangle)]
fn android_main(app: AndroidApp) {
    pomme_client::install_panic_hook();
    // Internal storage, which the app owns; `adb push` into the external
    // directory leaves files the app can't write to.
    let files = app
        .internal_data_path()
        .expect("the app has no internal files directory");
    pomme_client::run(
        LaunchArgs::from_file(&files.join(LAUNCH_FILE)),
        Platform {
            data_root: Some(files.to_path_buf()),
            android_app: app,
        },
    );
}
