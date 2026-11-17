# W9 — Parity backlog: save & recent UX

> **For Hermes:** post-W8 parity long-tail. Save settings (save_dir,
> filename_pattern) ALREADY exist in io::settings; this closes the missing
> behaviour: filename token expansion, Save As, default save dir, Recent strip.
> Offline-verifiable. No rostad gate.

**Goal:** close parity rows — filename pattern tokens, Save As (Ctrl+Shift+S),
default save dir (Pictures/SnagCraft) + Change/Reset, Recent strip (newest-first,
RECENT_LIMIT=12, folder scan, click-to-open, toggle chip + count + hide).

## LOCKED CONTRACTS

### Feature 1 — filename pattern expansion (io)
- io: `build_filename(pattern: &str, format: ImageFormat, now: chrono::DateTime<Local>) -> String`
  expanding {date}=YYYY-MM-DD, {time}=HH-MM-SS, {datetime}=YYYY-MM-DD_HH-MM-SS
  (filesystem-safe, no colons), append correct extension (.png/.jpg/.webp).
  Unknown tokens left literal; empty pattern → "SnagCraft-{date}-{time}".
  Hostile-safe (pattern length cap, no path separators injected via tokens).
  Tests: each token, extension per format, unknown token literal.
- engine cmd_doc_save: when no explicit path, use save_dir (or default) +
  build_filename(settings.filename_pattern, format). Return {saved: path}.

### Feature 2 — default save dir (io + engine + ui-egui settings)
- io: `default_save_dir() -> PathBuf` = OS Pictures dir / "SnagCraft" (use `dirs`
  crate IF already a dep, else std::env::var("HOME")/Pictures fallback; check
  workspace deps — do NOT add a new crate without parent OK; prefer dirs-next
  if present, else HOME/Pictures). create_dir_all on first save.
- settings save_dir None → default_save_dir(). Settings UI: Save location row
  with Change… (rfd folder picker, native) + Reset to default. (May already
  partly exist — complete it.)

### Feature 3 — Save As (ui-egui + engine)
- engine `doc.save_as {path, format?, quality?}` (native): render_watermarked →
  io::save to explicit path. ui-egui Ctrl+Shift+S + File→Save As… → rfd save
  dialog suggested build_filename(...) → doc.save_as → status "Saved: path".
  kittest: seam asserts dialog path dispatch (native rfd not headless-testable;
  test the engine command + a UI seam).

### Feature 4 — Recent strip (ui-egui + io) [the big one]
Read ../src/ui/app.ts:667-700 (RECENT_LIMIT=12, RecentShot) + filmstrip markup
:904/:924-932 + the folder-scan logic. 
- io: `scan_recent(dir: &Path, limit: usize) -> Vec<RecentEntry>` — list image
  files (png/jpg/webp/gif/bmp) newest-first by mtime, cap limit, return
  {path, name, mtime, thumb?}. Thumbnail: decode + downscale to ~64px (io::decode
  + raster resize) lazily. Recording thumbnails: a video file → first-frame
  thumb via ffmpeg IF feasible (stretch; else a film-icon placeholder).
- ui-egui: filmstrip footer (toggle chip with count badge, hide button, track).
  In-session recents (push on capture/save, cap 12, newest-first) + folder scan
  of save_dir on startup (merge, dedup by path). Click a thumb → doc.open path.
  Toggle chip shows/hides the strip; count badge = number. kittest: inject
  recents → strip renders thumbs + count; click → doc.open dispatched; toggle
  hides. Snapshot w9_recent_strip.png.
- Keep it native+web compatible where possible (web: folder scan absent, in-session
  recents only; cfg-gate the fs scan).

## Gates
cargo test --workspace (native green; bump automation drift-guard if registry
changes — PARENT owns automation, report need), clippy --workspace --all-targets
-D warnings, fmt, xtask layers, web check. Tick parity rows with evidence;
update gaps.md. NO git by child; parent commits. NO new workspace crate without
parent OK (report if dirs needed).

## Appendix — measured reality