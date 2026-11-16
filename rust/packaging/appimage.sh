#!/bin/sh
# Build a SnagCraft AppImage from a release binary.
# Usage: appimage.sh <path-to-snagcraft-binary> <output-dir>
# Needs linuxdeploy + linuxdeploy-plugin-appimage on PATH (or LINUXDEPLOY env).
set -eu
BIN="${1:?usage: appimage.sh <binary> <outdir>}"
OUT="${2:?usage: appimage.sh <binary> <outdir>}"
HERE="$(cd "$(dirname "$0")" && pwd)"
LINUXDEPLOY="${LINUXDEPLOY:-linuxdeploy}"

APPDIR="$OUT/AppDir"
rm -rf "$APPDIR"
mkdir -p "$APPDIR/usr/bin" "$APPDIR/usr/share/applications" \
         "$APPDIR/usr/share/icons/hicolor/256x256/apps"
cp "$BIN" "$APPDIR/usr/bin/snagcraft"
cp "$HERE/snagcraft.desktop" "$APPDIR/usr/share/applications/snagcraft.desktop"
cp "$HERE/snagcraft.png" "$APPDIR/usr/share/icons/hicolor/256x256/apps/snagcraft.png"
cp "$HERE/snagcraft.png" "$APPDIR/snagcraft.png"

APPIMAGE_EXTRACT_AND_RUN=1 NO_STRIP=1 "$LINUXDEPLOY" --appdir "$APPDIR" --output appimage
mv ./*.AppImage "$OUT/" 2>/dev/null || true
echo "AppImage built in $OUT"