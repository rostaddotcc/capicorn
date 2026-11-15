#!/usr/bin/env bash
# Build a "thin" AppImage from a normal Tauri AppImage.
#
# A default Tauri AppImage bundles the whole WebKitGTK/GTK/GStreamer stack
# (~240 MB uncompressed, ~82 MB compressed). This repacks the AppImage WITHOUT
# the bundled lib dir, so the app resolves those libraries from the system
# (which every desktop with webkit2gtk-4.1 already has). Result: ~4 MB.
#
# Trade-off: the thin AppImage needs webkit2gtk-4.1, GTK 3, GLib, GStreamer and
# ICU on the host. The full AppImage stays self-contained.
#
# usage: thin-appimage.sh <full.AppImage> [out-dir] [out-name]
set -euo pipefail

SRC="${1:?usage: thin-appimage.sh <full.AppImage> [out-dir] [out-name]}"
OUTDIR="${2:-$(dirname "$SRC")}"
SRC_ABS="$(readlink -f "$SRC")"
# Resolve OUTDIR to an absolute path BEFORE we cd into the temp work dir, or a
# relative out-dir would land inside $WORK and be deleted with it.
mkdir -p "$OUTDIR"
OUTDIR="$(readlink -f "$OUTDIR")"
NAME="${3:-$(basename "$SRC" .AppImage)-thin.AppImage}"

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

cd "$WORK"
"$SRC_ABS" --appimage-extract >/dev/null

# The point of the exercise: drop the bundled library stack. The binary's rpath
# is $ORIGIN/../lib, which then falls through to the system loader paths.
rm -rf squashfs-root/usr/lib

PLUGIN="$(ls "$HOME"/.cache/tauri/linuxdeploy-plugin-appimage*.AppImage 2>/dev/null | head -1 || true)"
if [ -z "$PLUGIN" ]; then
  echo "thin-appimage: linuxdeploy-plugin-appimage not found in ~/.cache/tauri (run a tauri build first)" >&2
  exit 1
fi

export APPIMAGE_EXTRACT_AND_RUN=1
export ARCH=x86_64
"$PLUGIN" --appdir="$WORK/squashfs-root" >/dev/null

# The plugin names its output after the AppDir; take whatever .AppImage appeared.
BUILT="$(ls -t "$WORK"/*.AppImage 2>/dev/null | head -1 || true)"
if [ -z "$BUILT" ]; then
  echo "thin-appimage: repack produced no AppImage" >&2
  exit 1
fi

mkdir -p "$OUTDIR"
cp "$BUILT" "$OUTDIR/$NAME"
echo "thin-appimage: $OUTDIR/$NAME ($(du -h "$OUTDIR/$NAME" | cut -f1))"
