#!/usr/bin/env bash
# Wraps a release bundle (the files of pomme-client-linux-x64-gnu.zip) in an
# AppImage. The bundle's files all go to usr/bin, where the binary finds its
# libraries; the updater replaces the whole AppImage, since its inside is a
# read-only mount.
#
# usage: packaging/linux.sh <bundle-dir> <version> <out.AppImage>
set -euo pipefail

bundle=$1 version=$2 out=$3
here=$(cd "$(dirname "$0")" && pwd)
stage=$(mktemp -d)
trap 'rm -rf "$stage"' EXIT

# Pinned, so a release never builds with an unreviewed tool. The static type2
# runtime needs no libfuse2 on the player's machine.
tool_url=https://github.com/AppImage/appimagetool/releases/download/1.9.1/appimagetool-x86_64.AppImage
tool_sha=ed4ce84f0d9caff66f50bcca6ff6f35aae54ce8135408b3fa33abfc3cb384eb0
runtime_url=https://github.com/AppImage/type2-runtime/releases/download/20251108/runtime-x86_64
runtime_sha=2fca8b443c92510f1483a883f60061ad09b46b978b2631c807cd873a47ec260d

fetch() {
  curl -fsSL -o "$2" "$1"
  echo "$3  $2" | sha256sum -c -
}
fetch "$tool_url" "$stage/appimagetool" "$tool_sha"
fetch "$runtime_url" "$stage/runtime" "$runtime_sha"
chmod +x "$stage/appimagetool"

appdir="$stage/Pomme.AppDir"
mkdir -p "$appdir/usr/bin" "$(dirname "$out")"
cp "$bundle"/* "$appdir/usr/bin/"
cp "$here/linux/pomme.desktop" "$appdir/"
cp "$here/../pomme-client/assets/icon.png" "$appdir/pomme.png"
ln -s usr/bin/pomme-client "$appdir/AppRun"

# CI runners have no FUSE to mount the tool's own AppImage.
APPIMAGE_EXTRACT_AND_RUN=1 VERSION=$version ARCH=x86_64 "$stage/appimagetool" \
  --runtime-file "$stage/runtime" "$appdir" "$out"
