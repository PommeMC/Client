#!/usr/bin/env python3
"""Stage Minecraft's OpenAL Soft native library into one or more directories.

The client loads OpenAL at runtime with `libloading`, trying the directory
holding the executable before any system path, so dropping the library into
`target/debug` is all a development build needs. Release bundles use the same
script so CI and dev builds stay on one pinned version.

Usage:
  fetch_openal.py <out-dir> [<out-dir> ...] [--classifier <lwjgl-classifier>]

The classifier defaults to the host platform. Pinned version, jar members and
checksums live in pomme-client/third-party/openal-natives.txt.
"""

import argparse
import hashlib
import platform
import sys
import urllib.request
import zipfile
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
MANIFEST = REPO_ROOT / "pomme-client" / "third-party" / "openal-natives.txt"
CACHE_DIR = REPO_ROOT / "target" / "openal-natives"
BASE_URL = "https://libraries.minecraft.net/org/lwjgl/lwjgl-openal"


class Unreachable(Exception):
    """The library could not be downloaded. Not fatal: audio degrades to off."""


class Native:
    def __init__(self, member, name, sha256):
        self.member = member
        self.name = name
        self.sha256 = sha256


def read_manifest():
    """Returns (version, {classifier: Native})."""
    version = None
    natives = {}
    for lineno, raw in enumerate(MANIFEST.read_text(encoding="utf-8").splitlines(), 1):
        line = raw.split("#", 1)[0].strip()
        if not line:
            continue
        fields = line.split()
        if fields[0] == "version" and len(fields) == 2:
            version = fields[1]
        elif len(fields) == 4:
            natives[fields[0]] = Native(fields[1], fields[2], fields[3])
        else:
            raise SystemExit(f"{MANIFEST}:{lineno}: cannot parse {raw!r}")
    if version is None:
        raise SystemExit(f"{MANIFEST}: no version line")
    return version, natives


def host_classifier():
    """Returns the LWJGL classifier for this host, or None if there is none.

    TODO: only the three classifiers the release job builds for are pinned in
    the manifest. A host outside that set (Intel mac, arm64 Linux) resolves to
    a classifier with no entry and degrades to no audio.
    """
    system = platform.system()
    machine = platform.machine().lower()
    arm = machine in ("arm64", "aarch64")
    if system == "Windows":
        return "natives-windows-arm64" if arm else "natives-windows"
    if system == "Linux":
        return "natives-linux-arm64" if arm else "natives-linux"
    if system == "Darwin":
        return "natives-macos-arm64" if arm else "natives-macos"
    return None


def fetch_jar(version, classifier, expected_sha256):
    """Downloads the jar unless a verified copy is already cached."""
    cached = CACHE_DIR / f"lwjgl-openal-{version}-{classifier}.jar"
    if cached.is_file() and sha256(cached.read_bytes()) == expected_sha256:
        return cached

    url = f"{BASE_URL}/{version}/lwjgl-openal-{version}-{classifier}.jar"
    print(f"downloading {url}")
    try:
        with urllib.request.urlopen(url) as response:
            payload = response.read()
    except OSError as e:
        # An offline build should still compile and run, just without sound.
        raise Unreachable(f"{url}: {e}") from e
    actual = sha256(payload)
    if actual != expected_sha256:
        raise SystemExit(f"{url}: sha256 {actual} does not match the pinned {expected_sha256}")
    CACHE_DIR.mkdir(parents=True, exist_ok=True)
    cached.write_bytes(payload)
    return cached


def sha256(payload):
    return hashlib.sha256(payload).hexdigest()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("out_dirs", nargs="+", metavar="out-dir")
    parser.add_argument("--classifier", default=None)
    parser.add_argument(
        "--require",
        action="store_true",
        help="fail instead of warning when the library cannot be downloaded "
        "(release packaging, where a silent miss would ship a mute build)",
    )
    args = parser.parse_args()

    version, natives = read_manifest()
    classifier = args.classifier or host_classifier()
    native = natives.get(classifier) if classifier else None
    if native is None:
        known = ", ".join(sorted(natives))
        described = classifier or f"{platform.system()}/{platform.machine()}"
        message = f"{MANIFEST} pins no native for {described} (has {known})"
        # Release packaging names its classifier explicitly and must not ship a
        # mute build; a development host outside the pinned set just goes quiet.
        if args.require:
            raise SystemExit(message)
        print(f"warning: {message}; audio will be disabled", file=sys.stderr)
        return

    targets = [Path(d) for d in args.out_dirs]
    missing = [d for d in targets if not (d / native.name).is_file()]
    if not missing:
        print(f"{native.name} is already staged in every requested directory")
        return

    try:
        jar = fetch_jar(version, classifier, native.sha256)
    except Unreachable as e:
        if args.require:
            raise SystemExit(str(e)) from e
        print(f"warning: {e}; audio will be disabled", file=sys.stderr)
        return
    with zipfile.ZipFile(jar) as archive:
        payload = archive.read(native.member)
    for directory in missing:
        directory.mkdir(parents=True, exist_ok=True)
        out = directory / native.name
        out.write_bytes(payload)
        print(f"staged {out} (OpenAL Soft via LWJGL {version})")


if __name__ == "__main__":
    sys.exit(main())
