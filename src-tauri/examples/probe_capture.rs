//! PROBE GATE: confirm ashpd Screenshot portal returns a real PNG on this
//! GNOME/Wayland session. Run with the user's session bus:
//!   XDG_RUNTIME_DIR=/run/user/1000 DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/1000/bus \
//!     cargo run --example probe_capture
use ashpd::desktop::screenshot::Screenshot;
use base64::Engine;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let resp = Screenshot::request().interactive(false).send().await?;
    let shot = resp.response()?;
    // ashpd 0.14 models URIs as an opaque string: no `to_file_path`.
    let uri = shot.uri().as_str();
    let path = std::path::PathBuf::from(
        uri.strip_prefix("file://")
            .ok_or_else(|| anyhow::anyhow!("not a file uri: {uri}"))?,
    );
    let bytes = std::fs::read(&path)?;
    let img = image::load_from_memory(&bytes)?;
    let (w, h) = (img.width(), img.height());
    let _b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
    let out = std::env::temp_dir().join("snagcraft-probe.png");
    std::fs::write(&out, &bytes)?;
    println!(
        "PROBE_OK {w}x{h} bytes={} saved={}",
        bytes.len(),
        out.display()
    );
    Ok(())
}
