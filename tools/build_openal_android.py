#!/usr/bin/env python3
"""Build OpenAL Soft for Android arm64 into the APK's native libraries.

LWJGL ships no Android build of the OpenAL Soft that desktop uses, so this
compiles the same release from source with the NDK. The source tag and its
archive checksum are pinned in pomme-client/third-party/openal-natives.txt.

Needs ANDROID_HOME (with the SDK's CMake) and ANDROID_NDK_HOME.

Usage:
  build_openal_android.py <out-dir>
"""

import argparse
import os
import shutil
import subprocess
import sys
import tarfile
from pathlib import Path

from fetch_openal import REPO_ROOT, Unreachable, download_verified, read_manifest

CMAKE_VERSION = "3.22.1"
ABI = "arm64-v8a"
# The APK's minSdk.
PLATFORM = "android-29"
WORK_DIR = REPO_ROOT / "target" / "openal-android"
LIBRARY = "libopenal.so"


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


def build(source, tag):
    cmake_bin = env_path("ANDROID_HOME") / "cmake" / CMAKE_VERSION / "bin"
    toolchain = env_path("ANDROID_NDK_HOME") / "build" / "cmake" / "android.toolchain.cmake"
    build_dir = WORK_DIR / f"build-{tag}-{ABI}"
    subprocess.run(
        [
            cmake_bin / "cmake",
            "-S", source,
            "-B", build_dir,
            "-G", "Ninja",
            f"-DCMAKE_MAKE_PROGRAM={cmake_bin / 'ninja'}",
            f"-DCMAKE_TOOLCHAIN_FILE={toolchain}",
            f"-DANDROID_ABI={ABI}",
            f"-DANDROID_PLATFORM={PLATFORM}",
            # One self-contained library rather than a libc++_shared.so beside it.
            "-DANDROID_STL=c++_static",
            "-DCMAKE_BUILD_TYPE=Release",
            # Only a git checkout sets this; an archive build's would be empty.
            f"-DLIB_VERSION={tag}",
            "-DALSOFT_UTILS=OFF",
            "-DALSOFT_EXAMPLES=OFF",
            "-DALSOFT_TESTS=OFF",
        ],
        check=True,
    )
    subprocess.run([cmake_bin / "cmake", "--build", build_dir], check=True)
    return build_dir / LIBRARY


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("out_dir", metavar="out-dir")
    args = parser.parse_args()

    _, _, android_source = read_manifest()
    if android_source is None:
        raise SystemExit("openal-natives.txt pins no android-source")
    tag, expected_sha256 = android_source

    out = Path(args.out_dir) / LIBRARY
    # Kept out of the jniLibs directory, which Gradle packs into the APK.
    stamp = WORK_DIR / "staged"
    staged = f"{tag} {out.resolve()}"
    if out.is_file() and stamp.is_file() and stamp.read_text() == staged:
        print(f"{out} is already OpenAL Soft {tag}")
        return

    library = build(fetch_source(tag, expected_sha256), tag)
    out.parent.mkdir(parents=True, exist_ok=True)
    shutil.copyfile(library, out)
    stamp.write_text(staged)
    print(f"built {out} (OpenAL Soft {tag})")


if __name__ == "__main__":
    sys.exit(main())
