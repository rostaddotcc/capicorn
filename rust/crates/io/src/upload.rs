//! Opening links in the desktop's default browser.
//!
//! Ported from the live app's `upload::open_url` (`src-tauri/src/upload.rs:88`),
//! with the scheme check hardened: **only** `http://` and `https://` ever reach
//! a launcher. `file://`, `javascript:`, `data:`, `mailto:`, `smb://` and every
//! other scheme are rejected before a process is spawned, so a link that arrived
//! from a server response (an upload endpoint's reply) cannot be turned into a
//! local file being opened or a script being run.
//!
//! The launcher is started detached and reaped on a background thread: the UI
//! must never block on a browser starting, and no zombie is left behind.

use std::io::{Read, Write};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::IoError;

/// Longest URL we will hand to a launcher (hostile input cap).
pub const MAX_URL_LEN: usize = 2048;

/// Validate a URL for [`open_url`], returning the trimmed form to launch.
///
/// Pure: no process is spawned, which is what makes the security rule testable
/// on a headless build host.
pub fn checked_url(url: &str) -> Result<&str, IoError> {
    let trimmed = url.trim();
    if trimmed.is_empty() {
        return Err(IoError::Io("no link given".to_string()));
    }
    if trimmed.len() > MAX_URL_LEN {
        return Err(IoError::Io(format!(
            "the link is longer than {MAX_URL_LEN} bytes"
        )));
    }
    if trimmed.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return Err(IoError::Io(
            "the link contains whitespace or control characters".to_string(),
        ));
    }

    // Exact, lowercase scheme: a browser would happily read `FILE://` too, and
    // case-folding here would only widen what the allowlist lets through.
    if !(trimmed.starts_with("http://") || trimmed.starts_with("https://")) {
        return Err(IoError::Io("only http(s) links can be opened".to_string()));
    }

    let Some((_, rest)) = trimmed.split_once("://") else {
        return Err(IoError::Io("only http(s) links can be opened".to_string()));
    };
    if rest.is_empty() || rest.starts_with('/') {
        return Err(IoError::Io(format!("the link has no host: {trimmed}")));
    }

    Ok(trimmed)
}

/// The platform opener for `url`: program plus its arguments.
///
/// Split out from [`open_url`] so the OS matrix is inspectable without ever
/// spawning anything.
fn opener_command(url: &str) -> (&'static str, Vec<&str>) {
    if cfg!(target_os = "windows") {
        // `start` is a cmd builtin; the empty argument is the window title, so
        // a URL containing `&` cannot be read as a second command.
        ("cmd", vec!["/C", "start", "", url])
    } else if cfg!(target_os = "macos") {
        ("open", vec![url])
    } else {
        ("xdg-open", vec![url])
    }
}

/// Open `url` in the desktop's default handler.
///
/// Rejects every non-http(s) scheme, then spawns the platform opener detached
/// (stdio nulled) and returns immediately — the caller does not wait for the
/// browser.
pub fn open_url(url: &str) -> Result<(), IoError> {
    let url = checked_url(url)?;
    let (program, args) = opener_command(url);

    let child = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|err| IoError::Io(format!("could not open the link with {program}: {err}")))?;

    // Reap on a helper thread: the browser (or the opener handing off to it)
    // outlives this call, and dropping a live `Child` would leave a zombie.
    std::thread::spawn(move || {
        let mut child = child;
        let _ = child.wait();
    });

    Ok(())
}

/// Cap on an endpoint's URL length (hostile input).
pub const MAX_ENDPOINT_LEN: usize = MAX_URL_LEN;
/// Cap on the response body we will read back from an endpoint (1 MiB).
pub const MAX_RESPONSE_BYTES: usize = 1024 * 1024;
/// Hard ceiling on one upload (curl also gets `--max-time` just under this).
pub const UPLOAD_WAIT: Duration = Duration::from_secs(30);

/// Upload failures. Every one is a value the UI can print; none needs a panic.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum UploadError {
    /// The endpoint is empty, not http(s), too long, or otherwise hostile.
    #[error("the upload endpoint must be an http(s) URL")]
    Endpoint,
    /// Nothing to send.
    #[error("nothing to upload")]
    NoImage,
    /// curl could not start, failed, or was killed.
    #[error("upload failed: {0}")]
    Curl(String),
    /// The child outlived [`UPLOAD_WAIT`] and was killed.
    #[error("the upload did not finish within {0}s")]
    Timeout(u64),
    /// The server sent more than [`MAX_RESPONSE_BYTES`].
    #[error("the endpoint's response is too large (cap {MAX_RESPONSE_BYTES} bytes)")]
    ResponseTooLarge,
    /// A successful response carried no usable link.
    #[error("the endpoint returned no link: {0}")]
    NoLink(String),
}

/// Validate an upload endpoint, returning its trimmed form.
///
/// Reuses [`checked_url`], so exactly the same http(s)-only scheme guard that
/// protects [`open_url`] also protects what curl is pointed at: `file://`,
/// `javascript:` and friends never reach a subprocess.
pub fn checked_endpoint(endpoint: &str) -> Result<&str, UploadError> {
    if endpoint.trim().len() > MAX_ENDPOINT_LEN {
        return Err(UploadError::Endpoint);
    }
    checked_url(endpoint).map_err(|_| UploadError::Endpoint)
}

/// curl arguments for one upload. An **array** — never a shell string — so a
/// hostile endpoint or header cannot become a second command.
///
/// `file=@-` streams the PNG from stdin, which avoids staging a temp file (and
/// the cleanup window that comes with it). `Expect:` is disabled so curl does
/// not wait for a 100-continue round trip.
fn curl_args(endpoint: &str) -> Vec<String> {
    vec![
        "-sS".to_string(),
        "--max-time".to_string(),
        "25".to_string(),
        "--connect-timeout".to_string(),
        "10".to_string(),
        "-H".to_string(),
        "Expect:".to_string(),
        "-X".to_string(),
        "POST".to_string(),
        "-F".to_string(),
        "file=@-;filename=snagcraft.png;type=image/png".to_string(),
        endpoint.to_string(),
    ]
}

/// Read at most [`MAX_RESPONSE_BYTES`], flagging anything longer.
fn read_capped(reader: impl Read) -> Result<Vec<u8>, ()> {
    let mut buf = Vec::new();
    let cap = u64::try_from(MAX_RESPONSE_BYTES + 1).map_err(|_| ())?;
    let mut limited = reader.take(cap);
    limited.read_to_end(&mut buf).map_err(|_| ())?;
    if buf.len() > MAX_RESPONSE_BYTES {
        return Err(());
    }
    Ok(buf)
}

/// Turn an endpoint response body into a link.
///
/// Accepts the common JSON shapes (`url`, `link`, `href`, `file`, `location`,
/// then `data.url`) or a plain-text body. Whatever comes back must itself be a
/// clean http(s) URL: a server must not be able to hand us `file:///etc/passwd`
/// that a later "open link" affordance would launch.
pub fn extract_link(body: &str) -> Result<String, UploadError> {
    let body = body.trim();
    if body.is_empty() {
        return Err(UploadError::NoLink("the response was empty".to_string()));
    }

    if let Ok(value) = serde_json::from_str::<Value>(body) {
        for key in ["url", "link", "href", "file", "location"] {
            if let Some(link) = value.get(key).and_then(Value::as_str) {
                return clean_link(link);
            }
        }
        if let Some(link) = value
            .get("data")
            .and_then(|data| data.get("url"))
            .and_then(Value::as_str)
        {
            return clean_link(link);
        }
        return Err(UploadError::NoLink(
            "the JSON response has no url/link field".to_string(),
        ));
    }
    if body.starts_with('{') || body.starts_with('[') {
        return Err(UploadError::NoLink(
            "the response is malformed JSON".to_string(),
        ));
    }
    clean_link(body)
}

fn clean_link(link: &str) -> Result<String, UploadError> {
    let link = link.trim();
    if link.is_empty() {
        return Err(UploadError::NoLink("the link was empty".to_string()));
    }
    checked_url(link)
        .map(str::to_string)
        .map_err(|_| UploadError::NoLink(format!("'{link}' is not an http(s) link")))
}

/// POST `png` to `endpoint` as `multipart/form-data` (`file` field) and return
/// the link the server replies with.
///
/// The child is bounded by [`UPLOAD_WAIT`] and killed on expiry (no orphan),
/// its response is capped at [`MAX_RESPONSE_BYTES`], and its stdin/stdout/stderr
/// are pumped on helper threads so a chatty server cannot deadlock us on a full
/// pipe. The endpoint is validated *before* curl is spawned.
pub fn upload_image(endpoint: &str, png: &[u8]) -> Result<String, UploadError> {
    if png.is_empty() {
        return Err(UploadError::NoImage);
    }
    let endpoint = checked_endpoint(endpoint)?;

    let mut child = Command::new("curl")
        .args(curl_args(endpoint))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|err| {
            UploadError::Curl(format!("could not run curl (is it installed?): {err}"))
        })?;

    let Some(mut stdin) = child.stdin.take() else {
        let _ = child.kill();
        let _ = child.wait();
        return Err(UploadError::Curl("curl's stdin is unavailable".to_string()));
    };
    let body = png.to_vec();
    let writer = std::thread::spawn(move || {
        // A curl that exited early closes the pipe: that error is expected and
        // the exit status / stderr carry the real reason.
        let _ = stdin.write_all(&body);
        let _ = stdin.flush();
    });

    let Some(stdout) = child.stdout.take() else {
        let _ = child.kill();
        let _ = child.wait();
        let _ = writer.join();
        return Err(UploadError::Curl(
            "curl's stdout is unavailable".to_string(),
        ));
    };
    let stdout_reader = std::thread::spawn(move || read_capped(stdout));
    let Some(stderr) = child.stderr.take() else {
        let _ = child.kill();
        let _ = child.wait();
        let _ = writer.join();
        let _ = stdout_reader.join();
        return Err(UploadError::Curl(
            "curl's stderr is unavailable".to_string(),
        ));
    };
    let stderr_reader = std::thread::spawn(move || read_capped(stderr));

    let deadline = Instant::now() + UPLOAD_WAIT;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(25));
            }
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = writer.join();
                let _ = stdout_reader.join();
                let _ = stderr_reader.join();
                return Err(UploadError::Timeout(UPLOAD_WAIT.as_secs()));
            }
            Err(err) => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = writer.join();
                let _ = stdout_reader.join();
                let _ = stderr_reader.join();
                return Err(UploadError::Curl(format!(
                    "cannot read curl's exit status: {err}"
                )));
            }
        }
    };

    let _ = writer.join();
    let response = stdout_reader.join().unwrap_or(Err(()));
    let error_output = stderr_reader.join().unwrap_or(Ok(Vec::new()));
    let stderr_text = error_output
        .map(|bytes| String::from_utf8_lossy(&bytes).trim().to_string())
        .unwrap_or_default();

    if !status.success() {
        return Err(UploadError::Curl(if stderr_text.is_empty() {
            format!("curl exited with {status}")
        } else {
            stderr_text
        }));
    }
    let body = response.map_err(|()| UploadError::ResponseTooLarge)?;
    extract_link(&String::from_utf8_lossy(&body))
}

/// PNG signature, so a mock server (and a human reading a capture) can tell the
/// upload really carried a PNG.
pub const PNG_MAGIC: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn http_and_https_are_accepted() {
        assert_eq!(
            checked_url("https://example.com/a.png").expect("https is allowed"),
            "https://example.com/a.png"
        );
        assert_eq!(
            checked_url("  http://localhost:8080/x?y=1#z  ").expect("http is allowed"),
            "http://localhost:8080/x?y=1#z"
        );
    }

    #[test]
    fn local_and_script_schemes_are_rejected() {
        for url in [
            "file:///etc/passwd",
            "javascript:alert(1)",
            "data:text/html,<script>alert(1)</script>",
            "mailto:someone@example.com",
            "smb://server/share",
            "ftp://example.com/a.png",
            "chrome://settings",
            "//example.com/a.png",
            "example.com",
            "HTTP://example.com",
            "hTtPs://example.com",
        ] {
            assert!(checked_url(url).is_err(), "{url} must be rejected");
        }
    }

    #[test]
    fn empty_host_and_hostile_whitespace_are_rejected() {
        assert!(checked_url("").is_err());
        assert!(checked_url("   ").is_err());
        assert!(checked_url("https://").is_err());
        assert!(checked_url("http:///path").is_err());
        assert!(checked_url("https://example.com/\nx").is_err());
        // A leading dash or embedded argument must never reach the opener.
        assert!(checked_url("https://example.com --headless").is_err());
        assert!(checked_url("https://example.com\t-x").is_err());
    }

    #[test]
    fn urls_are_bounded() {
        let long = format!("https://example.com/{}", "a".repeat(MAX_URL_LEN));
        assert!(checked_url(&long).is_err());
        let just_under = format!("https://example.com/{}", "a".repeat(MAX_URL_LEN - 20));
        assert!(checked_url(&just_under).is_ok());
    }

    #[test]
    fn errors_are_readable() {
        let err = checked_url("file:///tmp/a.png").expect_err("file scheme");
        assert_eq!(
            err.to_string(),
            "io error: only http(s) links can be opened"
        );
    }

    #[test]
    fn opener_matches_the_platform() {
        let (program, args) = opener_command("https://example.com");
        if cfg!(target_os = "windows") {
            assert_eq!(program, "cmd");
            assert_eq!(args, vec!["/C", "start", "", "https://example.com"]);
        } else if cfg!(target_os = "macos") {
            assert_eq!(program, "open");
            assert_eq!(args, vec!["https://example.com"]);
        } else {
            assert_eq!(program, "xdg-open");
            assert_eq!(args, vec!["https://example.com"]);
        }
    }

    // ---- upload POST flow ------------------------------------------------

    const MOCK_PNG: &[u8] = b"\x89PNG\r\n\x1a\n\x00\x00\x00\rIHDR-snagcraft-mock";

    fn have_curl() -> bool {
        Command::new("curl")
            .arg("--version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|status| status.success())
            .unwrap_or(false)
    }

    /// One-shot local HTTP server. Returns the endpoint plus a join handle with
    /// the raw request bytes it saw.
    ///
    /// The click-to-read body is bounded by an idle timeout rather than
    /// `Content-Length`, because curl streams a multipart body from stdin with
    /// chunked encoding (no length to trust).
    #[allow(clippy::unwrap_used, clippy::expect_used)]
    fn mock_server(response: &'static str) -> (String, std::thread::JoinHandle<Vec<u8>>) {
        use std::io::Write as _;
        use std::net::TcpListener;

        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind mock server");
        let addr = listener.local_addr().expect("mock server address");
        let handle = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept the upload");
            stream
                .set_read_timeout(Some(Duration::from_millis(1500)))
                .expect("read timeout");
            let mut request = Vec::new();
            let mut chunk = [0u8; 8192];
            loop {
                match stream.read(&mut chunk) {
                    Ok(0) => break,
                    Ok(n) => request.extend_from_slice(&chunk[..n]),
                    // curl is waiting for our reply: the body is complete.
                    Err(_) => break,
                }
            }
            let reply = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                response.len(),
                response
            );
            stream.write_all(reply.as_bytes()).expect("write reply");
            let _ = stream.flush();
            request
        });
        (format!("http://{addr}/upload"), handle)
    }

    fn contains(haystack: &[u8], needle: &[u8]) -> bool {
        haystack.windows(needle.len()).any(|w| w == needle)
    }

    #[test]
    fn upload_posts_multipart_png_and_returns_the_json_link() {
        if !have_curl() {
            eprintln!("skipping upload test: curl is not installed");
            return;
        }
        let (endpoint, server) = mock_server(r#"{"url":"https://example.com/i/abc.png"}"#);
        let link = upload_image(&endpoint, MOCK_PNG).expect("mock upload succeeds");
        assert_eq!(link, "https://example.com/i/abc.png");

        let request = server.join().expect("mock server thread");
        assert!(
            request.starts_with(b"POST /upload HTTP/1.1"),
            "not a POST: {}",
            String::from_utf8_lossy(&request[..request.len().min(160)])
        );
        let head = String::from_utf8_lossy(&request);
        assert!(
            head.contains("multipart/form-data; boundary="),
            "no multipart content type"
        );
        assert!(head.contains("name=\"file\""), "no file field");
        assert!(head.contains("filename=\"snagcraft.png\""));
        assert!(
            contains(&request, &PNG_MAGIC),
            "the multipart body does not carry a PNG"
        );
    }

    #[test]
    fn upload_accepts_a_plain_text_link() {
        if !have_curl() {
            eprintln!("skipping upload test: curl is not installed");
            return;
        }
        let (endpoint, server) = mock_server("https://example.com/plain.png");
        let link = upload_image(&endpoint, MOCK_PNG).expect("plain link accepted");
        assert_eq!(link, "https://example.com/plain.png");
        server.join().expect("mock server thread");
    }

    #[test]
    fn upload_without_a_link_in_the_response_is_an_error() {
        if !have_curl() {
            eprintln!("skipping upload test: curl is not installed");
            return;
        }
        let (endpoint, server) = mock_server(r#"{"ok":true,"id":7}"#);
        let err = upload_image(&endpoint, MOCK_PNG).expect_err("no link");
        assert!(matches!(err, UploadError::NoLink(_)), "{err:?}");
        server.join().expect("mock server thread");
    }

    #[test]
    fn upload_rejects_hostile_endpoints_before_spawning_curl() {
        for endpoint in [
            "",
            "   ",
            "file:///etc/passwd",
            "javascript:alert(1)",
            "ftp://example.com/upload",
            "//example.com/upload",
            "example.com/upload",
            "http:///nohost",
            "https://example.com/up load",
            "https://example.com --headless",
        ] {
            let err = upload_image(endpoint, MOCK_PNG).expect_err("must be rejected");
            assert_eq!(err, UploadError::Endpoint, "{endpoint}");
        }
    }

    #[test]
    fn upload_without_bytes_is_refused() {
        assert_eq!(
            upload_image("https://example.com/upload", b"").expect_err("no image"),
            UploadError::NoImage
        );
    }

    #[test]
    fn links_are_parsed_from_the_common_shapes() {
        assert_eq!(
            extract_link(r#"{"link":"https://e.com/1"}"#).expect("link"),
            "https://e.com/1"
        );
        assert_eq!(
            extract_link(r#"{"href":"https://e.com/2"}"#).expect("href"),
            "https://e.com/2"
        );
        assert_eq!(
            extract_link(r#"{"data":{"url":"https://e.com/3"}}"#).expect("data.url"),
            "https://e.com/3"
        );
        assert_eq!(
            extract_link("  https://e.com/4\n").expect("plain"),
            "https://e.com/4"
        );

        assert!(matches!(
            extract_link(r#"{"ok":true}"#),
            Err(UploadError::NoLink(_))
        ));
        assert!(matches!(extract_link(""), Err(UploadError::NoLink(_))));
        assert!(matches!(
            extract_link(r#"{"url":"file:///etc/passwd"}"#),
            Err(UploadError::NoLink(_))
        ));
        assert!(matches!(
            extract_link(r#"{"url":"javascript:alert(1)"}"#),
            Err(UploadError::NoLink(_))
        ));
        assert!(matches!(
            extract_link(r#"{"url":"#),
            Err(UploadError::NoLink(_))
        ));
    }

    #[test]
    fn curl_arguments_are_an_array_ending_in_the_endpoint() {
        let args = curl_args("https://example.com/u");
        assert_eq!(args.first().map(String::as_str), Some("-sS"));
        assert_eq!(
            args.last().map(String::as_str),
            Some("https://example.com/u")
        );
        assert!(args.iter().any(|a| a == "-X"));
        assert!(
            args.iter()
                .any(|a| a == "file=@-;filename=snagcraft.png;type=image/png")
        );
        // No argument is a compound command: nothing is ever handed to a shell.
        assert!(args.iter().all(|a| !a.contains("&&") && !a.contains("|")));
    }

    #[test]
    fn responses_are_size_capped() {
        let over = vec![b'a'; MAX_RESPONSE_BYTES + 1];
        assert!(read_capped(over.as_slice()).is_err());
        let under = vec![b'a'; 64];
        assert_eq!(read_capped(under.as_slice()).expect("small body"), under);
    }
}
