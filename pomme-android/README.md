# Android

An early port: the client builds as an arm64 APK, signs in with a Microsoft
account, downloads the game's assets on first launch (about 500 MB) and plays
with touch controls.

## Setup (macOS)

```sh
brew install --cask android-commandlinetools
brew install openjdk@17 gradle
rustup target add aarch64-linux-android
cargo install cargo-ndk

export JAVA_HOME=/opt/homebrew/opt/openjdk@17/libexec/openjdk.jdk/Contents/Home
export ANDROID_HOME=/opt/homebrew/share/android-commandlinetools
sdkmanager --install "platform-tools" "platforms;android-35" "build-tools;35.0.0" \
    "ndk;27.2.12479018" "cmake;3.22.1" "emulator" \
    "system-images;android-35;google_apis;arm64-v8a"
avdmanager create avd -n pomme -k "system-images;android-35;google_apis;arm64-v8a" -d pixel_7
```

Gradle needs JDK 17; the Android Gradle plugin doesn't run on newer ones.
`cmake` is for `aws-lc-rs`, which rustls builds from C, and for OpenAL Soft,
which the first `android-build` compiles from source.

## Running

```sh
emulator -avd pomme -gpu host &           # or a phone with USB debugging on
just ingame-server                         # prints the port; offline mode
just android-install                       # builds the APK and installs it
just android-push-assets                   # optional: copy a desktop install's assets
                                           # instead of downloading them
just android-config --version 26.2 --username Tester \
    --quick-access-multiplayer 10.0.2.2:<port>
just android-run
just android-log                           # logcat, filtered to pomme
```

`10.0.2.2` is the Mac as seen from the emulator. `launch.txt` (what
`android-config` writes) holds the same flags the desktop client takes. It
and the data folder (laid out as on desktop: `minecraft/`, `profiles/` and so
on) live in the app's internal storage, reached with
`adb shell run-as com.pommemc.client`.

## Signing in

With no account flags in `launch.txt` (drop `--username`), the app opens on
a sign-in screen: it shows a code to enter at microsoft.com/link, on the
phone (Open Link opens it in a Custom Tab) or any other device. The account
is saved in the app's storage and restored on the next launch. With
`--username`, as above, it plays offline as that name instead.

## Controls

In the world: drag on the left to move (the stick appears where you press),
drag elsewhere to look, tap to use or place, hold still to break. Buttons:
jump (bottom right), chat, inventory and pause (top right), and tapping a
hotbar slot selects it. On any screen, a touch is the mouse pointer. The
keyboard opens while a text field has focus, and Back works as Escape.

## Limitations

- No singleplayer: the embedded server is off on Android.
- The keyboard can cover the field being typed in: NativeActivity doesn't
  resize the window for it.
- The emulator's Vulkan is translated to the Mac's GPU, so rendering bugs
  seen there may not be real, and its performance says nothing about phones.
  Untested on real devices.
