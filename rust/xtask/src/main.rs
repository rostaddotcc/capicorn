//! cargo xtask — build tasks for the SnagCraft Rust workspace.
//! `layers` enforces the L0–L6 dependency rules; `ci` runs the full local gate.

mod layers;

use std::process::Command;

fn main() {
    let task = std::env::args().nth(1).unwrap_or_else(|| "help".into());
    let result = match task.as_str() {
        "layers" => layers::check(),
        "ci" => ci(),
        _ => {
            eprintln!("usage: cargo xtask <layers|ci>");
            std::process::exit(2);
        }
    };
    if let Err(e) = result {
        eprintln!("xtask {task} failed: {e}");
        std::process::exit(1);
    }
}

fn run(cmd: &mut Command) -> Result<(), String> {
    let status = cmd.status().map_err(|e| format!("spawn: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("{cmd:?} exited with {status}"))
    }
}

fn ci() -> Result<(), String> {
    run(Command::new("cargo").args(["fmt", "--all", "--check"]))?;
    run(Command::new("cargo").args([
        "clippy",
        "--workspace",
        "--all-targets",
        "--",
        "-D",
        "warnings",
    ]))?;
    run(Command::new("cargo").args(["test", "--workspace"]))?;
    layers::check()?;
    println!("ci: all gates green");
    Ok(())
}
