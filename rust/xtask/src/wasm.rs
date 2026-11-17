//! `cargo xtask wasm` — the web-core gate.
//!
//! Every crate the browser build can link must compile for
//! `wasm32-unknown-unknown` with default features off. Capturing, recording,
//! platform and automation crates are deliberately absent: they are native
//! L6 and never enter a wasm build.
//!
//! The check runs one `cargo check` per crate so a failure names the crate
//! that broke. `--no-default-features` is passed to all of them; the pure
//! crates define no features, so the flag is a no-op there, while `io`/`engine`
//! only expose their web-safe surface without `native`.

use std::process::Command;

/// Crates that must build for wasm32-unknown-unknown, in layer order.
pub const WEB_CRATES: &[&str] = &[
    "snagcraft-doc",
    "snagcraft-geom",
    "snagcraft-raster",
    "snagcraft-algo",
    "snagcraft-compose",
    "snagcraft-format",
    "snagcraft-ops",
    "snagcraft-text",
    "snagcraft-io",
    "snagcraft-engine",
];

/// Check every web crate, reporting pass/fail per crate.
pub fn check() -> Result<(), String> {
    let mut failed: Vec<&str> = Vec::new();
    for krate in WEB_CRATES {
        let ok = check_crate(krate)?;
        if ok {
            println!("wasm {} ok", krate);
        } else {
            println!("wasm {} FAILED", krate);
            failed.push(krate);
        }
    }
    if failed.is_empty() {
        println!("wasm: all {} web crates compile", WEB_CRATES.len());
        Ok(())
    } else {
        Err(format!(
            "wasm: {} of {} crates failed: {}",
            failed.len(),
            WEB_CRATES.len(),
            failed.join(", ")
        ))
    }
}

/// Run the check for one crate; `Ok(false)` means it compiled with errors.
fn check_crate(krate: &str) -> Result<bool, String> {
    let status = Command::new("cargo")
        .args([
            "check",
            "--package",
            krate,
            "--target",
            "wasm32-unknown-unknown",
            "--no-default-features",
        ])
        .status()
        .map_err(|e| format!("spawn cargo check for {krate}: {e}"))?;
    Ok(status.success())
}
