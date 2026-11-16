//! Generic image upload: POST a PNG to a user-configured endpoint and hand back
//! whatever link the server replies with.
//!
//! Deliberately provider-agnostic. SnagCraft ships no credentials and hardcodes no
//! service: the user points it at their own endpoint (a small HTTP handler, an
//! S3 presign proxy, a self-hosted image host …) and copies the returned link.
//!
//! Transport is `curl`, which is present on every Linux desktop and ships with
//! Windows 10+, so this stays dependency-free (no TLS stack to bundle).

/// Upload `data_url` to `endpoint` as `multipart/form-data` (`file` field) and
/// return the link.
///
/// The response body is used as the link; a JSON body is probed for the usual
/// keys (`url`, `link`, `href`) first.
#[tauri::command]
pub async fn upload_image(data_url: String, endpoint: String) -> Result<String, String> {
    let endpoint = endpoint.trim().to_string();
    if !(endpoint.starts_with("http://") || endpoint.starts_with("https://")) {
        return Err("the upload endpoint must start with http:// or https://".to_string());
    }

    let png = crate::capture::decode_png_data_url(&data_url).map_err(|e| e.to_string())?;

    // A real file is needed because curl streams multipart bodies from disk.
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let tmp = std::env::temp_dir().join(format!("snagcraft-upload-{stamp}.png"));
    std::fs::write(&tmp, &png).map_err(|e| format!("cannot stage the upload: {e}"))?;

    let form = format!("file=@{};type=image/png", tmp.display());
    let output = std::process::Command::new("curl")
        .args([
            "-sS",
            "--max-time",
            "60",
            "-X",
            "POST",
            "-F",
            &form,
            &endpoint,
        ])
        .output();
    let _ = std::fs::remove_file(&tmp);

    let output = output.map_err(|e| format!("could not run curl (is it installed?): {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let message = stderr.trim();
        return Err(if message.is_empty() {
            format!("the upload failed with {}", output.status)
        } else {
            format!("the upload failed: {message}")
        });
    }

    let body = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if body.is_empty() {
        return Err("the endpoint returned an empty response".to_string());
    }

    // Accept the common JSON shapes, otherwise treat the body as the link.
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(&body) {
        for key in ["url", "link", "href", "file", "location"] {
            if let Some(link) = value.get(key).and_then(|v| v.as_str()) {
                return Ok(link.to_string());
            }
        }
        if let Some(link) = value
            .get("data")
            .and_then(|v| v.get("url"))
            .and_then(|v| v.as_str())
        {
            return Ok(link.to_string());
        }
    }

    Ok(body)
}

/// Open a URL or file path with the desktop's default handler.
///
/// `open_path` already exists for files; this is the URL twin used by the
/// "copy link" affordance so a fresh upload can be opened straight away.
#[tauri::command]
pub fn open_url(app: tauri::AppHandle, url: String) -> Result<(), String> {
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return Err("only http(s) links can be opened".to_string());
    }
    let _ = app; // keep the signature uniform if we ever need the window handle
    let status = if cfg!(target_os = "windows") {
        std::process::Command::new("cmd")
            .args(["/C", "start", "", &url])
            .status()
    } else {
        std::process::Command::new("xdg-open").arg(&url).status()
    };
    status
        .map_err(|e| format!("could not open the link: {e}"))
        .and_then(|s| {
            if s.success() {
                Ok(())
            } else {
                Err("the opener exited with an error".to_string())
            }
        })
}
