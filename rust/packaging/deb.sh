#!/bin/bash
# Build a SnagCraft .deb from a release binary using dpkg-deb (no cargo-deb).
# Usage: deb.sh <path-to-snagcraft-binary> <output-dir>
# Env:   VERSION (default: version from apps/snagcraft/Cargo.toml)
# The package is architecture amd64 and carries the .desktop entry, the
# hicolor icon, copyright and changelog alongside usr/bin/snagcraft.
set -eu
umask 022

BIN="${1:?usage: deb.sh <binary> <outdir>}"
OUT="${2:?usage: deb.sh <binary> <outdir>}"
HERE="$(cd "$(dirname "$0")" && pwd)"
ARCH="amd64"

[ -f "$BIN" ] || { echo "deb.sh: no such binary: $BIN" >&2; exit 1; }
command -v dpkg-deb >/dev/null 2>&1 || { echo "deb.sh: dpkg-deb not found" >&2; exit 1; }

VERSION="${VERSION:-}"
if [ -z "$VERSION" ]; then
    VERSION="$(awk -F'"' '/^version = /{print $2; exit}' "$HERE/../apps/snagcraft/Cargo.toml")"
fi
[ -n "$VERSION" ] || { echo "deb.sh: could not determine version" >&2; exit 1; }

mkdir -p "$OUT"
OUT="$(cd "$OUT" && pwd)"
STAGE="$(mktemp -d)"
# mktemp makes 0700; a package root must be world-readable/traversable.
chmod 755 "$STAGE"
trap 'rm -rf "$STAGE"' EXIT

# --- payload ---------------------------------------------------------------
install -Dm755 "$BIN" "$STAGE/usr/bin/snagcraft"
install -Dm644 "$HERE/snagcraft.desktop" \
    "$STAGE/usr/share/applications/snagcraft.desktop"
install -Dm644 "$HERE/snagcraft.png" \
    "$STAGE/usr/share/icons/hicolor/256x256/apps/snagcraft.png"

mkdir -p "$STAGE/usr/share/doc/snagcraft"
cat > "$STAGE/usr/share/doc/snagcraft/copyright" <<'EOF'
Format: https://www.debian.org/doc/packaging-manuals/copyright-format/1.0/
Upstream-Name: snagcraft
Source: https://github.com/rostads/snagcraft

Files: *
Copyright: 2026 rostad <web@rostad.cc>
License: GPL-3.0-or-later
 SnagCraft is free software: you can redistribute it and/or modify it under
 the terms of the GNU General Public License as published by the Free Software
 Foundation, either version 3 of the License, or (at your option) any later
 version.
 .
 SnagCraft is distributed in the hope that it will be useful, but WITHOUT ANY
 WARRANTY; without even the implied warranty of MERCHANTABILITY or FITNESS FOR
 A PARTICULAR PURPOSE. See the GNU General Public License for more details.
 .
 On Debian systems the full text of the GPL-3 license is available in
 /usr/share/common-licenses/GPL-3.
Comment:
 Dual licensing: this build is offered under GPL-3.0-or-later. A separate
 commercial licence is available from the copyright holder for use cases where
 the GPL is not suitable; contact web@rostad.cc. The commercial option does not
 change the terms of this package.
EOF

{
    printf 'snagcraft (%s) unstable; urgency=medium\n\n' "$VERSION"
    printf '  * Release build of the native Rust edition (snagcraft %s).\n' "$VERSION"
    printf '\n -- rostad <web@rostad.cc>  %s\n' "$(LC_ALL=C date -R)"
} > "$STAGE/usr/share/doc/snagcraft/changelog"

# --- control ---------------------------------------------------------------
INSTALLED_SIZE="$(du -sk --exclude=DEBIAN "$STAGE" | cut -f1)"
mkdir -p "$STAGE/DEBIAN"
cat > "$STAGE/DEBIAN/control" <<EOF
Package: snagcraft
Version: $VERSION
Section: graphics
Priority: optional
Architecture: $ARCH
Maintainer: rostad <web@rostad.cc>
Installed-Size: $INSTALLED_SIZE
Depends: libwayland-client0, libxkbcommon0, libgl1, libegl1, libfontconfig1, libfreetype6, libdbus-1-3, libpipewire-0.3-0
Recommends: ffmpeg, tesseract-ocr
Description: capture/annotate/record native Rust edition
 SnagCraft captures screenshots and screen recordings, annotates them and
 exports the result. This package is the native Rust rebuild: a sans-I/O
 engine driving an egui front end, a CLI, a TCP control channel and an MCP
 server from one command registry.
EOF
chmod 644 "$STAGE/DEBIAN/control"

DEB="$OUT/snagcraft_${VERSION}_${ARCH}.deb"
dpkg-deb --build --root-owner-group "$STAGE" "$DEB"
echo "deb built: $DEB"