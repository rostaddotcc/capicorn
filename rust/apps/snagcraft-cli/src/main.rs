#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unimplemented,
    clippy::todo,
    clippy::unreachable
)]
// Tests may unwrap: a failed assertion should be a loud test failure, not a
// `Result` the harness silently ignores.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]
//! SnagCraft headless CLI — every engine command is drivable from here.
//! stdout carries command results (JSON); logs go to stderr via tracing.
//!
//! W5b adds the two automation doors as subcommands: `mcp` serves the engine
//! over MCP on stdio, and `control` sends one authenticated request to a running
//! desktop app's control channel (auth frame first, no acknowledgement — the
//! first frame read back is the answer to the request).

use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use clap::{Parser, Subcommand};
use snagcraft_engine::Session;

/// How long a one-shot control request waits for connect / a reply.
const CONTROL_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Parser)]
#[command(
    name = "snagcraft-cli",
    version,
    about = "SnagCraft headless CLI (Rust rebuild)"
)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Capture the full screen and export it
    Capture {
        /// Output image path (extension picks the format: png/jpeg/webp)
        out: PathBuf,
        #[arg(long, default_value = "90")]
        quality: u8,
    },
    /// Convert an image between formats
    Convert {
        input: PathBuf,
        output: PathBuf,
        #[arg(long, default_value = "png")]
        format: String,
        #[arg(long, default_value = "90")]
        quality: u8,
    },
    /// List the engine command registry
    Commands,
    /// Run any engine command by id with JSON params
    Run {
        cmd: String,
        #[arg(long, default_value = "{}")]
        params: String,
        /// Open this image before running the command
        #[arg(long)]
        input: Option<PathBuf>,
        /// Export the document to this path after the command
        #[arg(long)]
        output: Option<PathBuf>,
    },
    /// Serve the engine over MCP on stdio (headless; blocks until stdin closes)
    Mcp,
    /// Send one request to a running app's control channel (see `--control`)
    Control {
        /// Port the app printed for `--control` (0 there means: read its stderr)
        #[arg(long)]
        port: u16,
        /// Token file the app wrote (default: beside settings.json)
        #[arg(long)]
        token_file: Option<PathBuf>,
        /// Control method: commands.list, command.run, session.info, doc.inspect,
        /// ui.screenshot
        #[arg(long)]
        method: String,
        /// JSON params object (e.g. '{"id":"doc.new","params":{"w":320,"h":200}}')
        #[arg(long, default_value = "{}")]
        params: String,
    },
}

/// `control.token` beside `settings.json`, matching what the desktop app writes.
fn default_token_file() -> PathBuf {
    snagcraft_io::settings::path().with_file_name("control.token")
}

/// One JSON frame per line, flushed immediately: the server reads line-delimited
/// frames and would otherwise block on a buffered write.
fn send_frame(writer: &mut impl Write, frame: &serde_json::Value) -> Result<(), String> {
    let mut line = serde_json::to_vec(frame).map_err(|e| format!("encode frame: {e}"))?;
    line.push(b'\n');
    writer.write_all(&line).map_err(|e| format!("write: {e}"))?;
    writer.flush().map_err(|e| format!("flush: {e}"))
}

/// One authenticated request against `127.0.0.1:port`.
///
/// The auth frame is *not* acknowledged on success (W5a contract), so this sends
/// auth + request and reads exactly one frame — the request's answer. A rejected
/// token still yields one frame, the `authentication required` error, which
/// surfaces here as `Err`.
fn control_request(
    port: u16,
    token: &str,
    method: &str,
    params: &serde_json::Value,
) -> Result<serde_json::Value, String> {
    let addr = std::net::SocketAddr::from((std::net::Ipv4Addr::LOCALHOST, port));
    let stream = std::net::TcpStream::connect_timeout(&addr, CONTROL_TIMEOUT)
        .map_err(|e| format!("connect {addr}: {e}"))?;
    stream
        .set_read_timeout(Some(CONTROL_TIMEOUT))
        .map_err(|e| format!("read timeout: {e}"))?;
    stream
        .set_nodelay(true)
        .map_err(|e| format!("nodelay: {e}"))?;
    let mut writer = stream
        .try_clone()
        .map_err(|e| format!("clone socket: {e}"))?;
    let mut reader = std::io::BufReader::new(stream);

    send_frame(
        &mut writer,
        &serde_json::json!({"id": 0, "method": "auth", "params": {"token": token}}),
    )?;
    send_frame(
        &mut writer,
        &serde_json::json!({"id": 1, "method": method, "params": params}),
    )?;

    let mut line = String::new();
    reader
        .read_line(&mut line)
        .map_err(|e| format!("read {addr}: {e}"))?;
    let line = line.trim();
    if line.is_empty() {
        return Err(format!("{addr} closed the connection without a response"));
    }
    let frame: serde_json::Value =
        serde_json::from_str(line).map_err(|e| format!("response was not a JSON frame: {e}"))?;
    if frame["ok"].as_bool() != Some(true) {
        let message = frame["error"].as_str().unwrap_or("control request failed");
        return Err(message.to_string());
    }
    Ok(frame.get("result").cloned().unwrap_or_default())
}

/// `control` subcommand: read the token, ask one question, print the result.
fn control_command(
    port: u16,
    token_file: Option<&Path>,
    method: &str,
    params: &str,
) -> Result<(), String> {
    let params: serde_json::Value =
        serde_json::from_str(params).map_err(|e| format!("params JSON: {e}"))?;
    let path = token_file.map_or_else(default_token_file, Path::to_path_buf);
    let token = snagcraft_automation::read_token_file(&path).map_err(|e| e.to_string())?;
    let result = control_request(port, &token, method, &params)?;
    println!(
        "{}",
        serde_json::to_string_pretty(&result).map_err(|e| e.to_string())?
    );
    Ok(())
}

fn format_from_path(path: &std::path::Path) -> String {
    match path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("png")
        .to_ascii_lowercase()
        .as_str()
    {
        "jpg" | "jpeg" => "jpeg".into(),
        "webp" => "webp".into(),
        _ => "png".into(),
    }
}

fn run() -> Result<(), String> {
    let cli = Cli::parse();
    // The client talks to a *running* app and needs no session of its own.
    if let Cmd::Control {
        port,
        token_file,
        method,
        params,
    } = &cli.cmd
    {
        return control_command(*port, token_file.as_deref(), method, params);
    }
    let mut session = Session::new();
    match cli.cmd {
        Cmd::Capture { out, quality } => {
            session
                .dispatch("capture.full", serde_json::json!({}))
                .map_err(|e| e.to_string())?;
            let fmt = format_from_path(&out);
            session.dispatch("doc.export", serde_json::json!({"path": out.to_string_lossy(), "format": fmt, "quality": quality})).map_err(|e| e.to_string())?;
            println!("{}", serde_json::json!({"captured": out.to_string_lossy()}));
        }
        Cmd::Convert {
            input,
            output,
            format,
            quality,
        } => {
            session
                .dispatch(
                    "doc.open",
                    serde_json::json!({"path": input.to_string_lossy()}),
                )
                .map_err(|e| e.to_string())?;
            session.dispatch("doc.export", serde_json::json!({"path": output.to_string_lossy(), "format": format, "quality": quality})).map_err(|e| e.to_string())?;
            println!(
                "{}",
                serde_json::json!({"converted": output.to_string_lossy()})
            );
        }
        Cmd::Commands => {
            let list = session
                .dispatch("commands.list", serde_json::json!({}))
                .map_err(|e| e.to_string())?;
            println!(
                "{}",
                serde_json::to_string_pretty(&list).map_err(|e| e.to_string())?
            );
        }
        Cmd::Mcp => {
            let session = Arc::new(Mutex::new(session));
            snagcraft_automation::serve_mcp_stdio(session).map_err(|e| e.to_string())?;
        }
        // Handled before the session exists (no engine state is needed).
        Cmd::Control { .. } => {}
        Cmd::Run {
            cmd,
            params,
            input,
            output,
        } => {
            if let Some(input) = input {
                session
                    .dispatch(
                        "doc.open",
                        serde_json::json!({"path": input.to_string_lossy()}),
                    )
                    .map_err(|e| e.to_string())?;
            }
            let params: serde_json::Value =
                serde_json::from_str(&params).map_err(|e| format!("params JSON: {e}"))?;
            let result = session.dispatch(&cmd, params).map_err(|e| e.to_string())?;
            if let Some(output) = output {
                let fmt = format_from_path(&output);
                session
                    .dispatch(
                        "doc.export",
                        serde_json::json!({"path": output.to_string_lossy(), "format": fmt}),
                    )
                    .map_err(|e| e.to_string())?;
            }
            println!(
                "{}",
                serde_json::to_string(&result).map_err(|e| e.to_string())?
            );
        }
    }
    Ok(())
}

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "warn".into()),
        )
        .with_writer(std::io::stderr)
        .init();
    if let Err(e) = run() {
        // CLI error payload on stdout, machine-readable.
        println!("{}", serde_json::json!({"error": e}));
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A real control server on an ephemeral port, plus its token.
    fn live() -> (snagcraft_automation::ControlHandle, String) {
        let session = Arc::new(Mutex::new(Session::new()));
        let token = snagcraft_automation::generate_token();
        let handle = snagcraft_automation::spawn_control(session, 0, token.clone())
            .expect("spawn control server");
        (handle, token)
    }

    #[test]
    fn auth_first_no_ack_one_frame_back() {
        let (handle, token) = live();
        let port = handle.local_addr().port();
        // If auth were acknowledged, this read would return the ack and the
        // result would be `{"authenticated": true}` instead of the registry.
        let result = control_request(port, &token, "commands.list", &serde_json::json!({}))
            .expect("commands.list");
        let list = result.as_array().expect("registry is an array");
        assert!(!list.is_empty(), "registry must not be empty");
        assert!(
            list.iter().any(|c| c["id"] == "doc.new"),
            "registry should expose doc.new: {list:?}"
        );
        handle.shutdown();
    }

    #[test]
    fn command_run_round_trip() {
        let (handle, token) = live();
        let port = handle.local_addr().port();
        let params = serde_json::json!({"id": "doc.new", "params": {"w": 64, "h": 48}});
        let result = control_request(port, &token, "command.run", &params).expect("command.run");
        assert_eq!(result["width"], 64);
        assert_eq!(result["height"], 48);
        handle.shutdown();
    }

    #[test]
    fn wrong_token_is_one_error_frame_not_a_hang() {
        let (handle, _token) = live();
        let port = handle.local_addr().port();
        let bogus = snagcraft_automation::generate_token();
        let err = control_request(port, &bogus, "session.info", &serde_json::json!({}))
            .expect_err("a wrong token must fail");
        assert!(err.contains("authentication"), "{err}");
        handle.shutdown();
    }

    #[test]
    fn shutdown_closes_the_listener() {
        let (handle, token) = live();
        let port = handle.local_addr().port();
        handle.shutdown();
        let err = control_request(port, &token, "session.info", &serde_json::json!({}))
            .expect_err("listener is gone");
        assert!(err.contains("connect"), "{err}");
    }
}
