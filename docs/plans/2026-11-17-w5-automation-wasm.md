# W5 — Automation: TCP JSON control, MCP, CLI parity, WASM web

> **For Hermes:** W5a = automation crate (TCP control + MCP) dispatched;
> W5b = CLI/app wiring (parent, after W5a); W5c = WASM web build (next wake).
> Model: photocraft control-protocol.md (tmp/research/photocraft/) — loopback
> only, token file 0600, first-frame auth, registry-driven methods.

**Goal:** every engine command drivable via TCP JSON control channel + MCP;
headless CLI MCP server; same Rust compiles to WASM web build.

## W5a LOCKED CONTRACT (crates/automation, L6)

```rust
// TCP control server — loopback ONLY (127.0.0.1), line-delimited JSON frames:
//   req: {"id": <n>, "method": "<name>", "params": {...}}
//   res: {"id": <n>, "ok": true, "result": {...}} | {"id": <n>, "ok": false, "error": "<msg>"}
// FIRST frame on every connection MUST be {"method":"auth","params":{"token":"<hex>"}};
// wrong/missing token → {"ok":false,"error":"authentication required"} + close.
// Methods (after auth): commands.list, command.run {id, params} (engine dispatch),
//   session.info, doc.inspect, ui.screenshot (render composite → PNG base64 {png,width,height}).
// Unknown method → error frame, connection stays. Bounded frame size (1 MB), idle timeout.
pub fn generate_token() -> String;              // 256-bit random hex (getrandom or rand — allowlist)
pub fn write_token_file(path: &Path, token: &str) -> Result<(), AutomationError>; // create 0600 unix
pub fn read_token_file(path: &Path) -> Result<String, AutomationError>;
pub struct ControlHandle; // shutdown()
pub fn spawn_control(session: Arc<Mutex<Session>>, port: u16, token: String)
    -> Result<ControlHandle, AutomationError>; // thread + std::net::TcpListener, per-conn thread
// MCP (rmcp 3.10, stdio transport): tools generated from the engine registry —
//   command_list, command_run {id, params}, session_info, doc_inspect, doc_export {path,format,quality}
pub fn serve_mcp_stdio(session: Arc<Mutex<Session>>) -> Result<(), AutomationError>; // blocks
```

Session is `Arc<Mutex<snagcraft_engine::Session>>` (engine Session holds channels —
lock per request, never hold across await). rmcp API: VERIFY against vendored
source (~/.cargo/registry/src/*/rmcp-3.10.*/) — #[tool]/#[tool_router] macros or
ServerHandler impl; license MIT (allowlist ✓, confirm in Cargo.toml).

## W5b (parent, after W5a)
- apps/snagcraft: --control <port> --control-token-file <path> flags → spawn_control
  (token created/reused 0600); connection info to stderr; shutdown on exit.
- snagcraft-cli: `mcp [--headless]` (stdio MCP over a fresh headless Session),
  `control --port --token-file --method --params` (one-shot client), existing
  capture/convert/commands/run unchanged.
- Gates: control server integration test (spawn on ephemeral port, auth, command.run
  doc.new + edit.add + doc.export, ui.screenshot PNG decodes, wrong token rejected);
  MCP smoke (initialize + tools/list over stdio pipes).

## W5c (next wake): WASM web
- xtask wasm gate: L0–L5 cargo check --target wasm32-unknown-unknown.
- engine features: native (capture/recording/io-fs/platform) vs web — cfg-gate the
  native-only deps; web Session uses in-memory surfaces + browser file APIs.
- apps/snagcraft-web: trunk + wasm-bindgen + eframe web runner; same ui-egui shell.
- PhotoCraft model: crates L0–L5 must compile wasm; native backends behind features.

## Appendix — measured reality
(append rows as assumptions break)