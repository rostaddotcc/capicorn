# SnagCraft as a system application

SnagCraft is a Tauri (Rust + WebView) app. It cannot literally *be* a GNOME
Shell extension or a KDE plasmoid — those run inside the compositor process
(GJS / QML), a different runtime. The classic pattern instead is a **thin
companion** in the panel that talks to the real app. That is what ships here,
plus the OS plumbing that makes SnagCraft behave like a native system utility.

## What the app itself does (v1.3.0+)

- **Single instance.** Launching SnagCraft again never opens a second window —
  it focuses the running one (`tauri-plugin-single-instance`).
- **CLI capture triggers.** `snagcraft --capture` (full screen) and
  `snagcraft --region` ask the running instance to capture; if it is not
  running, it starts and captures. This is the hook every companion below uses.
- **File associations.** The .deb/.msi/NSIS installers register SnagCraft as an
  editor for PNG/JPEG/WebP/GIF/BMP/AVIF/TIFF — right-click an image →
  *Open with SnagCraft* loads it straight into the editor.
- **`--help` / `--version`** answer on the console without opening a window.
- **Start at login** (Settings → General) and **minimize to tray** were already
  in place; together with single instance they give the classic
  always-available-capture utility feel on both Linux and Windows.

## Put `snagcraft` on PATH

The companions call the `snagcraft` command:

- **.deb install:** already on PATH.
- **AppImage:** symlink it once —
  `ln -s /path/to/SnagCraft_*.AppImage ~/.local/bin/snagcraft`
  (make sure `~/.local/bin` is on PATH; log out/in if not).
- **Windows:** the NSIS/MSI installer puts the install dir on PATH; the
  companions below are Linux-only, but `snagcraft --capture` works from any
  shortcut or script there too.

## GNOME Shell extension (panel button)

`gnome-shell-extension/snagcraft@rostad.cc/` — GNOME 45+.

```sh
mkdir -p ~/.local/share/gnome-shell/extensions
cp -r gnome-shell-extension/snagcraft@rostad.cc ~/.local/share/gnome-shell/extensions/
# Wayland: log out and back in. X11: Alt+F2, r, Enter.
gnome-extensions enable snagcraft@rostad.cc
```

Gestures: **left click** = capture screen, **middle click** = capture region,
**right click** = open the editor.

## KDE Plasma 6 plasmoid (panel widget)

`kde-plasmoid/` — install with:

```sh
kpackagetool6 --type Plasma/Applet --install kde-plasmoid
```

(or copy the folder to `~/.local/share/plasma/plasmoids/cc.rostad.snagcraft/`).
Then *Add Widgets…* → search **SnagCraft Capture** and drag it to the panel.
Click the icon for a full capture; expand it for Region / Open editor.

## Desktop entry (AppImage desktop integration)

`snagcraft.desktop` gives the AppImage a proper launcher entry with
right-click actions (*Capture screen*, *Capture region*) and image MIME types:

```sh
# point Exec/Icon at your AppImage, then:
desktop-file-install --dir=$HOME/.local/share/applications snagcraft.desktop
update-desktop-database ~/.local/share/applications
```

The .deb already ships an equivalent entry (with MIME associations) — this
file is for AppImage users.

## Windows

There is no shell-extension concept on Windows; the equivalents are all in the
box: tray icon, start-at-login, single instance, file associations and the
`--capture` CLI (bind it to a shortcut or a PowerToys Run command). The
NSIS `.exe` and WiX `.msi` are built by CI on every release.

## Status / testing

- App-side plumbing (single instance, CLI, file open): unit-tested + built on
  Linux; Windows path is CI-built, needs a live Windows check.
- GNOME extension / plasmoid: reference implementations, not yet validated on
  a live GNOME 49 / Plasma 6 session — install and report back.
