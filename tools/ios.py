#!/usr/bin/env python3
"""Build, install and run the iOS app; see pomme-ios/README.md.

Usage:
  ios.py device <team>                build, sign and install on the connected
                                      iPhone or iPad, then run it with its log
  ios.py sim                          the same on the booted simulator
  ios.py config [--sim] <args...>     write the launch arguments (launch.txt)
  ios.py push-assets [--sim] [--from <minecraft dir>] [--version <v>]
                                      copy a desktop install's assets in
  ios.py archive <team>               Release archive and App Store .ipa
  ios.py upload <team>                upload that archive to App Store Connect

<team> is your Apple Developer team id. `device` and `config`/`push-assets`
without --sim use the first connected device.
"""

import argparse
import json
import plistlib
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

from fetch_openal import REPO_ROOT

BUNDLE = "com.pommemc.client"
PROJECT = REPO_ROOT / "pomme-ios" / "Pomme.xcodeproj"
BUILD_DIR = REPO_ROOT / "target" / "ios-build"
ARCHIVE = BUILD_DIR / "Pomme.xcarchive"
DESKTOP_MINECRAFT = Path.home() / "Library" / "Application Support" / "Pomme" / "minecraft"
# Inside the app's data container.
LAUNCH_FILE = "Documents/launch.txt"
MINECRAFT_DIR = "Library/Application Support/Pomme/minecraft"


def run(*args, **kwargs):
    return subprocess.run([str(a) for a in args], check=True, **kwargs)


def generate_project():
    run("xcodegen", "generate", "--quiet", cwd=PROJECT.parent)


def xcodebuild(sdk, *args):
    destination = "generic/platform=iOS Simulator" if sdk == "iphonesimulator" else "generic/platform=iOS"
    run(
        "xcodebuild", "-quiet",
        "-project", PROJECT,
        "-scheme", "Pomme",
        "-sdk", sdk,
        "-derivedDataPath", BUILD_DIR,
        "-destination", destination,
        *args,
    )


def device_id():
    """The first paired iPhone or iPad; devicectl connects to it when used."""
    with tempfile.TemporaryDirectory() as tmp:
        listing = Path(tmp) / "devices.json"
        run("xcrun", "devicectl", "list", "devices", "--json-output", listing, stdout=subprocess.DEVNULL)
        devices = json.loads(listing.read_text())["result"]["devices"]
    for device in devices:
        if (
            device["hardwareProperties"].get("platform") == "iOS"
            and device["connectionProperties"].get("pairingState") == "paired"
        ):
            return device["identifier"]
    raise SystemExit("no iPhone or iPad paired: plug one in, unlock it and trust this Mac")


def copy_into_app(sim, source, destination):
    """Copies `source` to `destination` inside the app's data container."""
    if sim:
        container = Path(
            run("xcrun", "simctl", "get_app_container", "booted", BUNDLE, "data", capture_output=True, text=True)
            .stdout.strip()
        )
        target = container / destination
        target.parent.mkdir(parents=True, exist_ok=True)
        if Path(source).is_dir():
            shutil.copytree(source, target, dirs_exist_ok=True)
        else:
            shutil.copyfile(source, target)
    else:
        run(
            "xcrun", "devicectl", "device", "copy", "to",
            "--device", device_id(),
            "--domain-type", "appDataContainer",
            "--domain-identifier", BUNDLE,
            "--source", source,
            "--destination", destination,
        )


def device(team):
    generate_project()
    xcodebuild("iphoneos", "-allowProvisioningUpdates", f"DEVELOPMENT_TEAM={team}", "build")
    target = device_id()
    run("xcrun", "devicectl", "device", "install", "app", "--device", target,
        BUILD_DIR / "Build" / "Products" / "Debug-iphoneos" / "Pomme.app")
    run("xcrun", "devicectl", "device", "process", "launch", "--device", target,
        "--terminate-existing", "--console", BUNDLE)


def sim():
    generate_project()
    xcodebuild("iphonesimulator", "build")
    run("xcrun", "simctl", "install", "booted",
        BUILD_DIR / "Build" / "Products" / "Debug-iphonesimulator" / "Pomme.app")
    run("xcrun", "simctl", "launch", "--console-pty", "--terminate-running-process", "booted", BUNDLE)


def config(is_sim, words):
    with tempfile.TemporaryDirectory() as tmp:
        launch = Path(tmp) / "launch.txt"
        launch.write_text(" ".join(words) + "\n")
        copy_into_app(is_sim, launch, LAUNCH_FILE)


def push_assets(is_sim, source, version):
    copy_into_app(is_sim, source / "assets", f"{MINECRAFT_DIR}/assets")
    copy_into_app(is_sim, source / "versions" / version, f"{MINECRAFT_DIR}/versions/{version}")


def archive(team):
    generate_project()
    xcodebuild("iphoneos", "-configuration", "Release", "-allowProvisioningUpdates",
               f"DEVELOPMENT_TEAM={team}", "archive", "-archivePath", ARCHIVE)
    export(team, "export")
    print(f"exported {BUILD_DIR / 'export' / 'Pomme.ipa'}")


def export(team, destination):
    """`exportArchive` for the App Store, to a local .ipa or straight to App Store Connect."""
    with tempfile.TemporaryDirectory() as tmp:
        options = Path(tmp) / "ExportOptions.plist"
        options.write_bytes(plistlib.dumps({
            "method": "app-store-connect",
            "signingStyle": "automatic",
            "teamID": team,
            "destination": destination,
        }))
        run("xcodebuild", "-quiet", "-exportArchive", "-archivePath", ARCHIVE,
            "-exportOptionsPlist", options, "-exportPath", BUILD_DIR / "export",
            "-allowProvisioningUpdates")


def main():
    # The launch arguments are the client's, so argparse never sees them.
    if sys.argv[1:2] == ["config"]:
        words = sys.argv[2:]
        is_sim = words[:1] == ["--sim"]
        config(is_sim, words[1:] if is_sim else words)
        return

    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    commands = parser.add_subparsers(dest="command", required=True)
    for name in ("device", "archive", "upload"):
        commands.add_parser(name).add_argument("team")
    commands.add_parser("sim")
    assets_parser = commands.add_parser("push-assets")
    assets_parser.add_argument("--sim", action="store_true")
    assets_parser.add_argument("--from", dest="source", type=Path, default=DESKTOP_MINECRAFT)
    assets_parser.add_argument("--version", default="26.2")
    args = parser.parse_args()

    if args.command == "device":
        device(args.team)
    elif args.command == "sim":
        sim()
    elif args.command == "push-assets":
        push_assets(args.sim, args.source, args.version)
    elif args.command == "archive":
        archive(args.team)
    elif args.command == "upload":
        export(args.team, "upload")


if __name__ == "__main__":
    sys.exit(main())
