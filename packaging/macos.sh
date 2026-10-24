#!/usr/bin/env bash
# Wraps a release bundle (the files of pomme-client-macos-arm64.zip) in
# Pomme.app inside a dmg. The bundle's files all go to Contents/MacOS, where
# the binary finds its libraries and the updater swaps them.
#
# usage: packaging/macos.sh <bundle-dir> <version> <out.dmg>
set -euo pipefail

bundle=$1 version=$2 out=$3
here=$(cd "$(dirname "$0")" && pwd)
stage=$(mktemp -d)
trap 'rm -rf "$stage"' EXIT

app="$stage/Pomme.app/Contents"
mkdir -p "$app/MacOS" "$app/Resources" "$(dirname "$out")"
cp "$bundle"/* "$app/MacOS/"
cp "$here/../pomme-client/assets/icon.icns" "$app/Resources/"
sed "s/@VERSION@/$version/g" "$here/macos/Info.plist" > "$app/Info.plist"
ln -s /Applications "$stage/Applications"

hdiutil create -volname Pomme -srcfolder "$stage" -format UDZO -ov "$out"
