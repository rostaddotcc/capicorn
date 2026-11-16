# SnagCraft Roadmap — FOSS Snagit alternative (Rust / Tauri 2)

Synthesis of deep research (snagit.md, competitors.md, technical.md, ui-inspiration.md).
Goal: a polished FOSS Snagit alternative in Rust, ArtCraft-style "small app, many features".
No POC — production quality, end-to-end verified, iterated.

## Architecture decision
ArtCraft's PhotoCraft is **egui + wgpu** (it bans Tauri/JS). SnagCraft stays **Tauri 2 + Rust**
(already shipping, cross-platform, working) but borrows ArtCraft's *structural* lessons:
engine-first modules, one command surface, factored platform/settings/quit modules, honest
Wayland docs, blur via fast multi-pass. We do NOT rewrite to egui (would discard working progress).

## Gap analysis (SnagCraft v0.3.0 vs Snagit / best competitors)
Have: full+region capture, editor (select/crop/arrow/text/box/blur-pixelate/bucket/step),
color/stroke/fonts, undo/redo, copy/save PNG, auto-save, save-copy, dark mode, settings(save dir),
dropdown, persistence.

Lacking (ranked):
- **P0 (user-mandated):** real blur (gaussian+pixelate+blackout), timer, tray+minimize-to-tray,
  floating widget, settings(general/formats/quality), save-before-close guard, global hotkeys.
- **P1 (credibility/ROI):** spotlight, highlighter, callout, better arrows/shapes, contextual
  properties panel + presets, recent-captures filmstrip, pin screenshot, filename wildcards,
  JPG/WebP/PDF export with quality (Snagit has NO image-quality slider — we can beat it).
- **P2 (big):** screen recording (MP4/GIF), scrolling capture, OCR (grab text), magnify, stamps,
  simplify, Smart-Move-like cutout.

## Key technical findings (from technical.md — load-bearing)
- **Global hotkeys:** tauri-plugin-global-shortcut is **X11-only → broken on KDE Wayland**.
  Working path = `org.freedesktop.portal.GlobalShortcuts` via **ashpd `global_shortcuts`**
  (NewShortcut::new().preferred_trigger(), create_session, bind_shortcuts, receive_activated).
  KDE implements it; GNOME≤46 does not. Ship a HotkeyBackend trait (portal|X11|none) + tell the
  user which is live; user-approval dialog; one bind/session; granted set may be smaller.
- **Tray:** works (libayatana-appindicator3) but **tray mouse events NOT emitted on Linux** →
  restore must be a MENU item, not click.
- **Floating widget:** decorations(false)+drag OK; **always_on_top & set_outer_position are
  unsupported on Wayland (tao)** → accept normal stacking or offer a KWin rule; document it.
- **Close-guard:** WindowEvent::CloseRequested + api.prevent_close() + in-app modal (no blocking
  async dialog in the GTK loop).
- **Save:** JpegEncoder::new_with_quality(u8) confirmed; image WebP is lossless-only → use `webp`
  crate for lossy quality (or PNG+JPEG v1).
- **Blur:** `stackblur-canvas` (canvasRGBA, 0 deps) over ctx.filter (WebKitGTK varies); blur at
  source pixels, feathered edges; keep pixelate + add blackout. Blur is THE differentiator
  (Flameshot fringe-pixelate / Unredacter awareness; offer radius/block/blackout + honest label).
- **Timer:** portal Screenshot has no delay → own countdown, hide windows + ~250ms before grab.

## Wave plan
- **Wave 5 (v0.4.0):** real blur (3 modes+radius) + spotlight + highlighter + callout (E);
  export formats/quality + tray + minimize-to-tray + close-guard + timer-delay + settings cmds (R);
  settings UI(general/formats/quality/filename) + timer countdown + close-guard modal + tray wiring (U).
- **Wave 6 (v0.5.0):** global hotkeys (portal+backend) + floating widget + contextual properties
  panel + presets + recent filmstrip + pin.
- **Wave 7 (v0.6.0):** screen recording (ScreenCast portal + PipeWire) + scrolling capture + OCR.
Each wave: parallel subagents (disjoint files, locked contracts) → integrate → build → verify → AppImage.

## Appendix — measured reality
(append claim → measured → change)