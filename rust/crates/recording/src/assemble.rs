//! Frame folder → one H.264 MP4 (`timelapse.assemble`).
//!
//! The capture half leaves one PNG per tick on disk; this half turns the folder
//! back into a video. It is a **separate** step on purpose: a failed encode
//! never costs the shots, and the user can re-assemble after deleting a bad
//! frame.
//!
//! ffmpeg reads the frames through its own `glob` demuxer, so the frame count
//! never has to fit on a command line. The pattern is the only caller-controlled
//! text that reaches the child, and it is validated first: relative, short, and
//! limited to glob syntax plus `[A-Za-z0-9._-]` — no `/`, no `..`, no shell
//! metacharacters (the child is spawned without a shell, but a pattern is still
//! never allowed to walk out of its folder).
//!
//! Everything is bounded: fps is clamped to 1..=60, the pattern to
//! [`MAX_PATTERN_LEN`], the wait to [`ASSEMBLE_WAIT`] with a kill on expiry.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Longest accepted frame pattern (a glob, not a path).
pub const MAX_PATTERN_LEN: usize = 128;
/// Longest file name we will try to match (the fs limit, so nothing is skipped
/// that could exist).
const MAX_NAME_LEN: usize = 255;
/// Upper bound on one assemble: an hour of 10k frames on a slow box still fits,
/// and a wedged child is killed instead of blocking the caller forever.
pub const ASSEMBLE_WAIT: Duration = Duration::from_secs(300);
/// Bytes of ffmpeg's stderr kept for the error message.
const STDERR_TAIL_BYTES: u64 = 2048;
/// Default output name inside the frame folder.
pub const DEFAULT_OUTPUT_NAME: &str = "timelapse.mp4";

/// Why an assemble refused or failed.
#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum AssembleError {
    /// The pattern matched nothing on disk — a caller error, not an I/O fault.
    #[error("no frames matched {0}")]
    NoFrames(String),
    /// The pattern is absolute, escapes the folder, or is not glob syntax.
    #[error("invalid frame pattern: {0}")]
    Pattern(String),
    /// Missing binary, non-zero exit, empty output, or a timeout kill.
    #[error("ffmpeg: {0}")]
    Ffmpeg(String),
    /// Filesystem plumbing.
    #[error("I/O: {0}")]
    Io(String),
}

/// What one successful assemble produced.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AssembleReport {
    /// The MP4 that was written (finalised before this is returned).
    pub video: PathBuf,
    /// Frames matched on disk (what ffmpeg was told to read).
    pub frames: u32,
    /// Frame rate actually used (after the 1..=60 clamp).
    pub fps: u32,
}

/// Clamp a requested frame rate into the range the encoder accepts.
pub fn clamp_fps(fps: u32) -> u32 {
    fps.clamp(1, 60)
}

/// Reject anything that is not a short, relative glob over a single folder.
pub fn validate_pattern(pattern: &str) -> Result<(), AssembleError> {
    if pattern.is_empty() {
        return Err(AssembleError::Pattern("empty pattern".to_string()));
    }
    if pattern.len() > MAX_PATTERN_LEN {
        return Err(AssembleError::Pattern(format!(
            "pattern longer than {MAX_PATTERN_LEN} bytes"
        )));
    }
    if pattern.contains("..") {
        return Err(AssembleError::Pattern(
            "pattern must not contain '..'".to_string(),
        ));
    }
    if pattern.starts_with('/') || pattern.starts_with('\\') {
        return Err(AssembleError::Pattern(
            "pattern must be relative to the frame folder".to_string(),
        ));
    }
    for ch in pattern.chars() {
        let allowed = ch.is_ascii_alphanumeric()
            || matches!(ch, '.' | '-' | '_' | '*' | '?' | '[' | ']' | '!' | '^');
        if !allowed {
            return Err(AssembleError::Pattern(format!(
                "character {ch:?} is not allowed in a frame pattern"
            )));
        }
    }
    Ok(())
}

/// One token of a compiled glob.
enum Token {
    Lit(char),
    Any,
    Star,
    Class {
        negated: bool,
        items: Vec<(char, char)>,
    },
}

/// Compile a validated pattern into tokens. An unterminated `[` is treated as a
/// literal `[` (what a shell does), so a stray bracket never matches everything.
fn compile(pattern: &str) -> Vec<Token> {
    let chars: Vec<char> = pattern.chars().collect();
    let mut tokens = Vec::with_capacity(chars.len());
    let mut i = 0;
    while i < chars.len() {
        match chars[i] {
            '*' => tokens.push(Token::Star),
            '?' => tokens.push(Token::Any),
            '[' => {
                // Find the closing bracket; `[]]` is an empty-then-literal case
                // we simply do not support — no closing bracket means literal.
                let mut end = None;
                let mut k = i + 1;
                while k < chars.len() {
                    if chars[k] == ']' && k > i + 1 {
                        end = Some(k);
                        break;
                    }
                    k += 1;
                }
                match end {
                    Some(end) => {
                        let mut negated = false;
                        let mut start = i + 1;
                        if matches!(chars.get(start), Some('!' | '^')) {
                            negated = true;
                            start += 1;
                        }
                        let mut items = Vec::new();
                        let mut k = start;
                        while k < end {
                            if k + 2 < end && chars[k + 1] == '-' {
                                items.push((chars[k], chars[k + 2]));
                                k += 3;
                            } else {
                                items.push((chars[k], chars[k]));
                                k += 1;
                            }
                        }
                        if items.is_empty() {
                            tokens.push(Token::Lit('['));
                            tokens.push(Token::Lit(']'));
                        } else {
                            tokens.push(Token::Class { negated, items });
                        }
                        i = end;
                    }
                    None => tokens.push(Token::Lit('[')),
                }
            }
            other => tokens.push(Token::Lit(other)),
        }
        i += 1;
    }
    tokens
}

/// Glob match over one path component: `*`, `?`, `[abc]`, `[a-z]`, `[!abc]`.
pub fn glob_match(pattern: &str, name: &str) -> bool {
    let tokens = compile(pattern);
    let chars: Vec<char> = name.chars().collect();
    let (m, n) = (tokens.len(), chars.len());
    // dp[i][j]: the first i tokens match the first j name characters.
    let mut dp = vec![vec![false; n + 1]; m + 1];
    dp[0][0] = true;
    for i in 0..m {
        for j in 0..=n {
            if !dp[i][j] {
                continue;
            }
            match &tokens[i] {
                Token::Star => {
                    dp[i + 1][j] = true; // matches nothing
                    if j < n {
                        dp[i][j + 1] = true; // consumes one more character
                    }
                }
                Token::Any => {
                    if j < n {
                        dp[i + 1][j + 1] = true;
                    }
                }
                Token::Lit(lit) => {
                    if j < n && chars[j] == *lit {
                        dp[i + 1][j + 1] = true;
                    }
                }
                Token::Class { negated, items } => {
                    if j < n {
                        let hit = items
                            .iter()
                            .any(|(lo, hi)| chars[j] >= *lo && chars[j] <= *hi);
                        if hit != *negated {
                            dp[i + 1][j + 1] = true;
                        }
                    }
                }
            }
        }
    }
    dp[m][n]
}

/// Number of regular files in `dir` whose name matches `pattern`.
pub fn count_frames(dir: &Path, pattern: &str) -> Result<u32, AssembleError> {
    validate_pattern(pattern)?;
    let meta = std::fs::metadata(dir)
        .map_err(|err| AssembleError::Io(format!("{}: {err}", dir.display())))?;
    if !meta.is_dir() {
        return Err(AssembleError::Io(format!(
            "{} is not a directory",
            dir.display()
        )));
    }
    let entries = std::fs::read_dir(dir)
        .map_err(|err| AssembleError::Io(format!("{}: {err}", dir.display())))?;
    let mut frames = 0u32;
    for entry in entries {
        let Ok(entry) = entry else { continue };
        let path = entry.path();
        if !entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
            continue;
        }
        let Some(name) = path.file_name().and_then(OsStr::to_str) else {
            continue;
        };
        if name.len() > MAX_NAME_LEN {
            continue;
        }
        if glob_match(pattern, name) {
            frames = frames.saturating_add(1);
        }
    }
    Ok(frames)
}

/// ffmpeg argument list: glob input → H.264 in a yuv420p MP4.
///
/// `-framerate` is an *input* option and must precede `-i`; the pad filter makes
/// odd dimensions even so yuv420p never refuses the frame.
pub fn args(input_glob: &Path, fps: u32, out: &Path) -> Vec<String> {
    let mut args: Vec<String> = ["-y", "-hide_banner", "-loglevel", "error", "-framerate"]
        .map(String::from)
        .to_vec();
    args.push(clamp_fps(fps).to_string());
    args.extend(["-pattern_type", "glob", "-i"].map(String::from));
    args.push(input_glob.to_string_lossy().into_owned());
    args.extend(
        [
            "-c:v",
            "libx264",
            "-pix_fmt",
            "yuv420p",
            "-vf",
            "pad=ceil(iw/2)*2:ceil(ih/2)*2",
        ]
        .map(String::from),
    );
    args.push(out.to_string_lossy().into_owned());
    args
}

/// `<out>.ffmpeg.log` — ffmpeg's own diagnostics, never the app's stdout.
fn log_path(out: &Path) -> PathBuf {
    let mut log = out.as_os_str().to_os_string();
    log.push(".ffmpeg.log");
    PathBuf::from(log)
}

/// Last [`STDERR_TAIL_BYTES`] of the log, flattened onto one line.
fn stderr_tail(log: &Path) -> String {
    let Ok(meta) = std::fs::metadata(log) else {
        return "no ffmpeg stderr".to_string();
    };
    let len = meta.len();
    let skip = len.saturating_sub(STDERR_TAIL_BYTES);
    let Ok(mut file) = std::fs::File::open(log) else {
        return "no ffmpeg stderr".to_string();
    };
    use std::io::{Read, Seek, SeekFrom};
    if file.seek(SeekFrom::Start(skip)).is_err() {
        return "no ffmpeg stderr".to_string();
    }
    let mut buf = String::new();
    if file.read_to_string(&mut buf).is_err() {
        return "no ffmpeg stderr".to_string();
    }
    let flat = buf.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.is_empty() {
        "no ffmpeg stderr".to_string()
    } else {
        flat
    }
}

/// Assemble `dir`/`pattern` into `out` at `fps`.
///
/// Fails before spawning when the pattern is hostile, the folder is missing, or
/// nothing matches; a real ffmpeg child is bounded by [`ASSEMBLE_WAIT`] and
/// killed on expiry, so a hang can never leak a process.
pub fn assemble_timelapse(
    dir: &Path,
    pattern: &str,
    fps: u32,
    out: &Path,
) -> Result<AssembleReport, AssembleError> {
    validate_pattern(pattern)?;
    let fps = clamp_fps(fps);
    let frames = count_frames(dir, pattern)?;
    if frames == 0 {
        return Err(AssembleError::NoFrames(format!(
            "{}/{}",
            dir.display(),
            pattern
        )));
    }
    if let Some(parent) = out.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)
            .map_err(|err| AssembleError::Io(format!("{}: {err}", parent.display())))?;
    }

    let log = log_path(out);
    let stderr = std::fs::File::create(&log)
        .or_else(|_| std::fs::File::create("/dev/null"))
        .map_err(|err| AssembleError::Io(format!("cannot open {}: {err}", log.display())))?;

    let input = dir.join(pattern);
    let mut child = Command::new(crate::ffmpeg::program())
        .args(args(&input, fps, out))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::from(stderr))
        .spawn()
        .map_err(|err| {
            AssembleError::Ffmpeg(format!(
                "cannot start ffmpeg ({err}) — install it: sudo apt install ffmpeg \
                 (or point SNAGCRAFT_FFMPEG at a binary)"
            ))
        })?;

    let deadline = Instant::now() + ASSEMBLE_WAIT;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(50)),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(AssembleError::Ffmpeg(format!(
                    "ffmpeg did not finish within {}s: {}",
                    ASSEMBLE_WAIT.as_secs(),
                    stderr_tail(&log)
                )));
            }
            Err(err) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(AssembleError::Io(format!(
                    "cannot read ffmpeg's exit status: {err}"
                )));
            }
        }
    };

    if !status.success() {
        return Err(AssembleError::Ffmpeg(format!(
            "ffmpeg exited with {status} writing {}: {}",
            out.display(),
            stderr_tail(&log)
        )));
    }
    let written = std::fs::metadata(out)
        .map_err(|err| AssembleError::Io(format!("{}: {err}", out.display())))?;
    if written.len() == 0 {
        return Err(AssembleError::Ffmpeg(format!(
            "ffmpeg wrote an empty file: {}",
            out.display()
        )));
    }
    let _ = std::fs::remove_file(&log);

    Ok(AssembleReport {
        video: out.to_path_buf(),
        frames,
        fps,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn patterns_reject_escapes_and_shell_metacharacters() {
        for good in [
            "snagcraft-timelapse-*.png",
            "frame-*.png",
            "shot-????.jpg",
            "img[a-c].png",
            "a.b_c-d*.png",
        ] {
            assert!(validate_pattern(good).is_ok(), "{good} must be accepted");
        }
        for bad in [
            "",
            "../*.png",
            "..",
            "/etc/*.png",
            "\\x*.png",
            "a/b*.png",
            "*.png; rm -rf /",
            "$(whoami)*.png",
            "`id`*.png",
            "*.png --x",
            "'*.png",
            "*\n.png",
        ] {
            assert!(
                matches!(validate_pattern(bad), Err(AssembleError::Pattern(_))),
                "{bad:?} must be rejected"
            );
        }
        let long = "a".repeat(MAX_PATTERN_LEN + 1);
        assert!(matches!(
            validate_pattern(&long),
            Err(AssembleError::Pattern(_))
        ));
    }

    #[test]
    fn fps_is_clamped_to_the_encoder_range() {
        assert_eq!(clamp_fps(0), 1);
        assert_eq!(clamp_fps(1), 1);
        assert_eq!(clamp_fps(10), 10);
        assert_eq!(clamp_fps(60), 60);
        assert_eq!(clamp_fps(u32::MAX), 60);
    }

    #[test]
    fn glob_matching_covers_stars_questions_and_classes() {
        assert!(glob_match(
            "snagcraft-timelapse-*.png",
            "snagcraft-timelapse-00007.png"
        ));
        assert!(!glob_match(
            "snagcraft-timelapse-*.png",
            "snagcraft-timelapse-00007.jpg"
        ));
        assert!(glob_match("frame-????.png", "frame-0001.png"));
        assert!(!glob_match("frame-????.png", "frame-001.png"));
        assert!(glob_match("img[0-9].png", "img7.png"));
        assert!(!glob_match("img[0-9].png", "imgx.png"));
        assert!(glob_match("img[!0-9].png", "imgx.png"));
        assert!(!glob_match("img[!0-9].png", "img7.png"));
        assert!(glob_match("*", "anything.png"));
        assert!(glob_match("*.png", ".png"));
        // A stray opening bracket is literal, never a wildcard.
        assert!(glob_match("a[b.png", "a[b.png"));
        assert!(!glob_match("a[b.png", "axb.png"));
    }

    #[test]
    fn counting_skips_subdirectories_and_non_matches() {
        let dir =
            std::env::temp_dir().join(format!("snagcraft-assemble-count-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch dir");
        std::fs::write(dir.join("snagcraft-timelapse-00001.png"), b"x").expect("frame");
        std::fs::write(dir.join("snagcraft-timelapse-00002.png"), b"x").expect("frame");
        std::fs::write(dir.join("other.png"), b"x").expect("decoy");
        std::fs::create_dir_all(dir.join("snagcraft-timelapse-00003.png")).expect("dir decoy");

        assert_eq!(
            count_frames(&dir, "snagcraft-timelapse-*.png").expect("count"),
            2
        );
        assert_eq!(count_frames(&dir, "*.png").expect("count"), 3);
        assert_eq!(count_frames(&dir, "nothing-*.png").expect("count"), 0);
        assert!(matches!(
            count_frames(&dir, "../*.png"),
            Err(AssembleError::Pattern(_))
        ));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_missing_folder_is_an_io_error_not_a_panic() {
        let missing = std::env::temp_dir().join("snagcraft-assemble-no-such-dir");
        let _ = std::fs::remove_dir_all(&missing);
        assert!(matches!(
            count_frames(&missing, "*.png"),
            Err(AssembleError::Io(_))
        ));
    }

    #[test]
    fn args_put_the_input_rate_before_the_glob_input() {
        let argv = super::args(
            Path::new("/tmp/shots/frame-*.png"),
            12,
            Path::new("/tmp/tl.mp4"),
        );
        let rate = argv
            .iter()
            .position(|a| a == "-framerate")
            .expect("-framerate");
        let input = argv.iter().position(|a| a == "-i").expect("-i");
        assert!(rate < input, "-framerate is an input option");
        assert_eq!(argv.get(rate + 1).map(String::as_str), Some("12"));
        assert!(argv.windows(2).any(|w| w == ["-pattern_type", "glob"]));
        assert!(argv.windows(2).any(|w| w == ["-c:v", "libx264"]));
        assert!(argv.windows(2).any(|w| w == ["-pix_fmt", "yuv420p"]));
        assert!(argv.iter().any(|a| a == "pad=ceil(iw/2)*2:ceil(ih/2)*2"));
        assert_eq!(argv.last().map(String::as_str), Some("/tmp/tl.mp4"));
        // Clamped: a hostile rate never reaches the child.
        let clamped = args(Path::new("/tmp/f.png"), 0, Path::new("/tmp/o.mp4"));
        let rate = clamped
            .iter()
            .position(|a| a == "-framerate")
            .expect("-framerate");
        assert_eq!(clamped.get(rate + 1).map(String::as_str), Some("1"));
    }
}
