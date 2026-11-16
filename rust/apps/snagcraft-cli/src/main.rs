#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unimplemented,
    clippy::todo,
    clippy::unreachable
)]
//! SnagCraft headless CLI — every engine command is drivable from here.
//! stdout carries command results (JSON); logs go to stderr via tracing.

use std::path::PathBuf;

use clap::{Parser, Subcommand};
use snagcraft_engine::Session;

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
