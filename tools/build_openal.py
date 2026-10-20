#!/usr/bin/env python3
"""Build OpenAL Soft for Android or iOS.

LWJGL ships no mobile build of the OpenAL Soft that desktop uses, so this
compiles the same release from source. The source tag and its archive checksum
are pinned in pomme-client/third-party/openal-natives.txt.

- android: arm64 `libopenal.so` for the APK. Needs ANDROID_HOME (with the
  SDK's CMake) and ANDROID_NDK_HOME.
- ios, ios-simulator: arm64 `soft_oal.framework` for the app bundle, which iOS
  needs for a library loaded at runtime. Needs Xcode and CMake.

Usage:
  build_openal.py <platform> <out-dir>
"""

import argparse
import os
import plistlib
import shutil
import subprocess
import sys
import tarfile
from pathlib import Path

from fetch_openal import REPO_ROOT, Unreachable, download_verified, read_manifest

WORK_DIR = REPO_ROOT / "target" / "openal-soft"
ANDROID_CMAKE_VERSION = "3.22.1"
# The APK's minSdk.
ANDROID_PLATFORM = "android-29"
# The app's MinimumOSVersion.
IOS_DEPLOYMENT_TARGET = "16.0"
COMMON_FLAGS = [
    "-DCMAKE_BUILD_TYPE=Release",
    "-DALSOFT_UTILS=OFF",
    "-DALSOFT_EXAMPLES=OFF",
    "-DALSOFT_TESTS=OFF",
]


def env_path(name):
    value = os.environ.get(name)
    if not value:
        raise SystemExit(f"{name} is not set (see pomme-android/README.md)")
    return Path(value)


def fetch_source(tag, expected_sha256):
    """The unpacked source tree, downloading and verifying it once."""
    source = WORK_DIR / f"openal-soft-{tag}"
    if source.is_dir():
        return source
    url = f"https://github.com/kcat/openal-soft/archive/refs/tags/{tag}.tar.gz"
    try:
        archive = download_verified(url, expected_sha256, WORK_DIR / f"openal-soft-{tag}.tar.gz")
    except Unreachable as e:
        raise SystemExit(str(e)) from e
    # Unpacked aside and moved into place whole, so an interrupted run leaves
    # nothing at `source` to be mistaken for a finished tree.
    staging = WORK_DIR / f"openal-soft-{tag}.part"
    shutil.rmtree(staging, ignore_errors=True)
    with tarfile.open(archive) as tar:
        # The data filter (Python 3.12+, and backports) refuses paths that
        # escape the target directory.
        if hasattr(tarfile, "data_filter"):
            tar.extractall(staging, filter="data")
        else:
            tar.extractall(staging)
    (staging / source.name).rename(source)
    shutil.rmtree(staging)
    return source


def android_build():
    """CMake and its flags."""
    cmake_bin = env_path("ANDROID_HOME") / "cmake" / ANDROID_CMAKE_VERSION / "bin"
    toolchain = env_path("ANDROID_NDK_HOME") / "build" / "cmake" / "android.toolchain.cmake"
    flags = [
        "-G", "Ninja",
        f"-DCMAKE_MAKE_PROGRAM={cmake_bin / 'ninja'}",
        f"-DCMAKE_TOOLCHAIN_FILE={toolchain}",
        "-DANDROID_ABI=arm64-v8a",
        f"-DANDROID_PLATFORM={ANDROID_PLATFORM}",
        # One self-contained library rather than a libc++_shared.so beside it.
        "-DANDROID_STL=c++_static",
    ]
    return cmake_bin / "cmake", flags


def ios_build(sdk):
    flags = [
        "-DCMAKE_SYSTEM_NAME=iOS",
        f"-DCMAKE_OSX_SYSROOT={sdk}",
        "-DCMAKE_OSX_ARCHITECTURES=arm64",
        f"-DCMAKE_OSX_DEPLOYMENT_TARGET={IOS_DEPLOYMENT_TARGET}",
        "-DALSOFT_OSX_FRAMEWORK=ON",
    ]
    return "cmake", flags


def build(source, tag, platform):
    cmake, flags = (
        android_build() if platform == "android" else ios_build(IOS_SDKS[platform])
    )
    build_dir = WORK_DIR / f"build-{tag}-{platform}"
    subprocess.run(
        [
            cmake,
            "-S", source,
            "-B", build_dir,
            *flags,
            *COMMON_FLAGS,
            # Only a git checkout sets this; an archive build's would be empty.
            f"-DLIB_VERSION={tag}",
        ],
        check=True,
    )
    subprocess.run([cmake, "--build", build_dir, "--parallel"], check=True)
    built = build_dir / PRODUCTS[platform]
    if platform in IOS_SDKS:
        # App Store validation wants every embedded framework to say which
        # iOS it needs.
        info_path = built / "Info.plist"
        info = plistlib.loads(info_path.read_bytes())
        info["MinimumOSVersion"] = IOS_DEPLOYMENT_TARGET
        info_path.write_bytes(plistlib.dumps(info))
    return built


IOS_SDKS = {"ios": "iphoneos", "ios-simulator": "iphonesimulator"}
PRODUCTS = {
    "android": "libopenal.so",
    "ios": "soft_oal.framework",
    "ios-simulator": "soft_oal.framework",
}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("platform", choices=PRODUCTS)
    parser.add_argument("out_dir", metavar="out-dir")
    args = parser.parse_args()

    _, _, source = read_manifest()
    if source is None:
        raise SystemExit("openal-natives.txt pins no source")
    tag, expected_sha256 = source

    out = Path(args.out_dir) / PRODUCTS[args.platform]
    # Kept out of the output directory, which gets packed into the app.
    stamp = WORK_DIR / f"staged-{args.platform}"
    staged = f"{tag} {out.resolve()}"
    if out.exists() and stamp.is_file() and stamp.read_text() == staged:
        print(f"{out} is already OpenAL Soft {tag}")
        return

    built = build(fetch_source(tag, expected_sha256), tag, args.platform)
    out.parent.mkdir(parents=True, exist_ok=True)
    if built.is_dir():
        shutil.rmtree(out, ignore_errors=True)
        shutil.copytree(built, out, symlinks=True)
    else:
        shutil.copyfile(built, out)
    stamp.write_text(staged)
    print(f"built {out} (OpenAL Soft {tag})")


if __name__ == "__main__":
    sys.exit(main())
