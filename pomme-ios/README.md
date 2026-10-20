# iOS

An early port: the client runs on iPhone and iPad (iOS 16+), signs in with a
Microsoft account, downloads the game's assets on first launch (about 500 MB)
and plays with the same touch controls as Android. All of it is Rust: the app's
executable is the `pomme-ios` binary, and the Xcode project only packages and
signs it.

## Setup

Xcode, then:

```sh
brew install xcodegen cmake
rustup target add aarch64-apple-ios aarch64-apple-ios-sim
```

`project.yml` is the Xcode project; `just ios-*` generates `Pomme.xcodeproj`
from it. The first build downloads MoltenVK (`tools/fetch_moltenvk.py`, the
Vulkan driver, linked in statically) and compiles OpenAL Soft
(`tools/build_openal.py`, embedded as `soft_oal.framework`).

Signing uses your Apple Developer team: its id is under Membership on
developer.apple.com, or in Xcode's Settings → Accounts. Xcode creates the
profile for `com.pommemc.client` the first time. The device needs Developer
Mode on (Settings → Privacy & Security).

## Running

```sh
just ingame-server                          # prints the port; offline mode
just ios-device <team>                      # builds, installs and runs it,
                                            # with the log in the terminal
just ios-push-assets                        # optional: copy a desktop install's
                                            # assets instead of downloading them
just ios-config --version 26.2 --username Tester \
    --quick-access-multiplayer <mac's ip>:<port>
```

`launch.txt` (what `ios-config` writes, in the app's Documents) holds the same
flags the desktop client takes. The data folder (laid out as on desktop:
`minecraft/`, `profiles/` and so on) is the app's `Library/Application
Support/Pomme`; `minecraft/` and the cache are kept out of iCloud backups.

## TestFlight

```sh
just ios-archive <team>                     # target/ios-build/export/Pomme.ipa
just ios-upload <team>                      # to App Store Connect
```

The upload needs an App Store Connect app for `com.pommemc.client`. Bump
`CURRENT_PROJECT_VERSION` in `project.yml` for each upload.

## Signing in

As on Android: with no account flags, the app opens on a sign-in screen that
shows a code for microsoft.com/link (Open Link opens Safari). The account goes
in the iOS Keychain and is restored on the next launch.

## Controls

The Android ones: drag on the left to move, drag elsewhere to look, tap to use
or place, hold still to break; buttons for jump, chat, inventory and pause.
There's no Back key, so the pause button is the way out of the world.

## Limitations

- The simulator builds and starts but can't render: its GPU has no
  `drawIndirectFirstInstance`, which the chunk renderer needs. Use a device.
- No singleplayer: the embedded server is off, as on Android.
- The HUD ignores the safe area, so the notch and rounded corners can cover
  its edges.
