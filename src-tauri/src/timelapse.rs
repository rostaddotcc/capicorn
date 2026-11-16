//! Interval capture → timelapse.
//!
//! The shell writes one PNG per interval tick (`SnagCraft-interval-*.png` in the
//! save folder), and stops by calling `make_timelapse` to turn that run into an
//! MP4. Nothing here knows about the capture loop itself — it only reads the
//! files the loop already wrote, so a failed render never costs the shots.
//!
//! | command           | arguments                    | returns              |
//! |-------------------|------------------------------|----------------------|
//! | `make_timelapse`  | `dir`, `pattern`, `fps`, `out` | output path (`string`) |
//!
//! `pattern` is a plain file-name glob (`SnagCraft-interval-*.png`) resolved
//! inside `dir`; it may not contain path separators. ffmpeg is invoked with
//! `-pattern_type glob`, so the frames play in lexicographic order — which is
//! exactly the order the zero-padded shot numbers give.
//!
//! ffmpeg is a hard requirement (same as screen recording). A missing binary is
//! reported as a plain, actionable message rather than a spawn error.

use std::path::{Path, PathBuf};
use std::process::Command;

/// Encoding filter shared with the recorder: even dimensions for yuv420p.
const EVEN_PIXEL_FILTER: &str = "pad=ceil(iw/2)*2:ceil(ih/2)*2";

/// Clamp a requested rate to something ffmpeg will happily encode.
fn sanitize_fps(fps: u32) -> u32 {
    fps.clamp(1, 120)
}

/// One readable line for the failure modes a timelapse actually hits.
fn explain_stderr(stderr: &str, glob: &str) -> String {
    let detail = stderr.trim();
    let lower = detail.to_ascii_lowercase();
    if lower.contains("no such file")
        || lower.contains("could not open")
        || lower.contains("invalid data found")
        || lower.contains("matched no files")
        || lower.contains("no files matched")
    {
        return format!(
            "No shots matched {glob} — capture a few intervals before building a timelapse."
        );
    }
    if lower.contains("invalid argument") || lower.contains("error initializing") {
        return format!("ffmpeg could not encode the timelapse: {detail}");
    }
    if detail.is_empty() {
        "ffmpeg failed to build the timelapse (no details given).".to_string()
    } else {
        format!("ffmpeg failed to build the timelapse: {detail}")
    }
}

/// Blocking half: run ffmpeg over the glob and confirm a file actually landed.
fn render(glob: &str, fps: u32, out: &Path) -> Result<String, String> {
    let rate = fps.to_string();
    let output = Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-y"])
        .arg("-framerate")
        .arg(&rate)
        // No shell is involved, so the glob reaches ffmpeg verbatim — quoting it
        // here would make glob(3) look for a file whose name has quotes in it.
        .args(["-pattern_type", "glob", "-i", glob])
        .args(["-vf", EVEN_PIXEL_FILTER])
        .args(["-pix_fmt", "yuv420p", "-movflags", "+faststart"])
        .arg(out)
        .output()
        .map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => "ffmpeg is required to build a timelapse but was not \
                 found on PATH. Install it (e.g. `sudo apt install ffmpeg` or \
                 `sudo dnf install ffmpeg`) and try again."
                .to_string(),
            _ => format!("could not run ffmpeg: {e}"),
        })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(explain_stderr(&stderr, glob));
    }

    // A zero-exit with no bytes on disk is still a failed timelapse.
    let size = std::fs::metadata(out).map(|meta| meta.len()).unwrap_or(0);
    if size == 0 {
        return Err(format!(
            "ffmpeg finished but wrote no timelapse at {} — check that the shots are readable.",
            out.display()
        ));
    }

    Ok(out.to_string_lossy().into_owned())
}

/// Build an MP4 from the PNGs an interval run left in `dir`.
///
/// `pattern` is a file-name glob anchored inside `dir` (no separators allowed),
/// `fps` is the playback rate — 10 makes a 30-second interval read as motion —
/// and `out` is the absolute path for the video, created if needed.
#[tauri::command]
pub async fn make_timelapse(
    dir: String,
    pattern: String,
    fps: u32,
    out: String,
) -> Result<String, String> {
    let dir = dir.trim();
    let pattern = pattern.trim();
    let out = out.trim();

    if dir.is_empty() {
        return Err("no folder given for the timelapse".to_string());
    }
    if pattern.is_empty() {
        return Err("no file pattern given for the timelapse".to_string());
    }
    if out.is_empty() {
        return Err("no output path given for the timelapse".to_string());
    }
    if pattern.contains('/') || pattern.contains('\\') {
        return Err(format!(
            "the timelapse pattern must be a file name inside the folder, not a path: {pattern}"
        ));
    }

    let dir = PathBuf::from(dir);
    if !dir.is_dir() {
        return Err(format!(
            "the timelapse folder does not exist: {}",
            dir.display()
        ));
    }

    let out = PathBuf::from(out);
    if let Some(parent) = out.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
        }
    }

    let glob = dir.join(pattern).to_string_lossy().into_owned();
    let fps = sanitize_fps(fps);

    tauri::async_runtime::spawn_blocking(move || render(&glob, fps, &out))
        .await
        .map_err(|e| format!("the timelapse job failed: {e}"))?
}
