#!/usr/bin/env python3
"""Stage MoltenVK for an iOS build, as the Vulkan library pyronyx links.

iOS has no Vulkan loader, so the app links MoltenVK statically. pyronyx's
build script links `-lvulkan` from `$VULKAN_SDK/lib`, so this unpacks the
right slice of MoltenVK's static xcframework as `<out>/lib/libvulkan.a`, and
the build points VULKAN_SDK at `<out>`.

Usage:
  fetch_moltenvk.py <ios|ios-simulator>    (prints the directory to use)
"""

import argparse
import sys
import tarfile

from fetch_openal import REPO_ROOT, Unreachable, download_verified

VERSION = "v1.4.2"
# The release's MoltenVK-all.tar: the only one with the simulator slice.
ARCHIVE_SHA256 = "562a15a29bc358446a56a4091c5f7e08f604184187c1d34f712148b61ef17276"
URL = f"https://github.com/KhronosGroup/MoltenVK/releases/download/{VERSION}/MoltenVK-all.tar"
WORK_DIR = REPO_ROOT / "target" / "moltenvk"
SLICES = {"ios": "ios-arm64", "ios-simulator": "ios-arm64_x86_64-simulator"}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("platform", choices=SLICES)
    args = parser.parse_args()

    out = WORK_DIR / f"{VERSION}-{args.platform}"
    library = out / "lib" / "libvulkan.a"
    if not library.is_file():
        try:
            archive = download_verified(URL, ARCHIVE_SHA256, WORK_DIR / f"MoltenVK-all-{VERSION}.tar")
        except Unreachable as e:
            raise SystemExit(str(e)) from e
        member = f"MoltenVK/MoltenVK/static/MoltenVK.xcframework/{SLICES[args.platform]}/libMoltenVK.a"
        with tarfile.open(archive) as tar:
            source = tar.extractfile(member)
            if source is None:
                raise SystemExit(f"{archive} has no {member}")
            # Written aside and renamed, so a cut-off run leaves no half library.
            part = library.with_suffix(".part")
            part.parent.mkdir(parents=True, exist_ok=True)
            part.write_bytes(source.read())
            part.rename(library)
    print(out)


if __name__ == "__main__":
    sys.exit(main())
