# W8 — Parity backlog: upload POST flow + Ctrl+. stop + countdown HUD

> **For Hermes:** post-W7 parity backlog (docs/gaps.md B4 rows 4–9). Upload to
> a USER-configured endpoint is the app's upload feature — NOT the "external
> post to storytold repos/Discord/releases" gate (that stays closed). Offline-
> verifiable via a local mock HTTP server. No rostad gate for these.

**Goal:** close B4#4 (Ctrl+. stop), B4#5 (shrunk countdown HUD), B4#6–9
(upload POST-to-endpoint flow: endpoint config, request, response, errors).

## LOCKED CONTRACTS

### Feature 1 — upload POST flow (io + engine + ui-egui) [B4#6–9]
Read ../src-tauri/src/upload.rs (live model: POST a PNG to a user-configured
endpoint, hand back the returned link, copy to clipboard).
- io/upload.rs: add `upload_image(endpoint: &str, png: &[u8]) -> Result<String, UploadError>`
  — POST via `curl -sS -X POST -F file=@- <endpoint>` (subprocess, bounded wait
  + kill, like recording/assemble; PNG bytes on stdin) OR std TcpStream HTTP if
  simpler. Validate endpoint http(s) only (reuse open_url scheme guard). Parse
  the response: accept a JSON {url|link|...} or a plain-text URL; return the
  link. Hostile-safe: endpoint scheme-guarded, response size capped, no shell
  injection (curl args as array, endpoint validated). Errors: bad scheme, curl
  fail/timeout, no link in response → UploadError variants.
- engine: `upload.send {endpoint?}` (native-gated) — endpoint defaults to
  settings.upload_url; renders current doc → PNG (render_watermarked) →
  io::upload_image → {link}. No doc → InvalidParams; no endpoint → InvalidParams.
  Register native (registry 42→43). Keep upload.open as-is.
- ui-egui: File → Upload… menu item (native): refuse without an image (status),
  require endpoint (settings.upload_url; if empty, status "Set an upload URL in
  Settings"), Upload action → upload.send → copy link to clipboard (engine
  clipboard.copy_text or ctx.copy_text) + status "Uploaded: <link>". Error →
  status. cfg-gated native (web: upload hidden, already).
- TEST: local mock HTTP server (std::net::TcpListener in a thread, read the
  POST request line + headers + body, assert it's a multipart POST with PNG
  magic, respond `{"url":"https://example.com/i/abc.png"}`); upload_image →
  returns the link. Engine upload.send against the mock (settings.upload_url =
  mock addr). Hostile: file:// endpoint rejected, no-link response → error.
  Skip-if-no-curl guard.

### Feature 2 — Ctrl+. stop hotkey (ui-egui) [B4#4]
- While recording, Ctrl+. (Period) → record.stop (same as HUD Stop). Add to the
  key handler (match existing Ctrl+ shortcuts pattern). Only active when a take
  is running. kittest: inject recording state + Ctrl+. → record.stop dispatched
  (assert via session state / a seam).

### Feature 3 — shrunk countdown HUD (ui-egui) [B4#5]
- Before a recording take starts, a compact countdown HUD (3‑2‑1) then the take
  begins. Match the live app's shrunk countdown variant (read ../src/ui/ for the
  live countdown if present; else a compact centered countdown pill). Settings
  or default countdown secs (reuse capture_delay if applicable). kittest snapshot
  w8_countdown_hud.png showing the compact countdown.

## Gates
cargo test --workspace (native, all green incl new; bump automation control
drift-guard 42→43 — PARENT owns automation, child reports the need), clippy
--workspace --all-targets -D warnings, fmt, xtask layers, web check
(--no-default-features). Mock-server evidence for upload. Tick parity B4#4–9
with evidence; update gaps.md. NO git by child; parent commits.

## Appendix — measured reality