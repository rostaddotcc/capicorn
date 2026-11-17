//! The encoder child: argument lists, spawn, and a shutdown that finalises the
//! container.
//!
//! ffmpeg is a subprocess on purpose — libx264 through a pipe is a few dozen
//! lines and gives MP4, GIF and every filter for free, where in-process encoding
//! would mean openh264/rav1e plus a muxer for no gain.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::time::{Duration, Instant};

use crate::{RecordingError, RecordingFormat, STOP_WAIT};

/// GIFs are downscaled to this width: a full-res desktop GIF is enormous.
const GIF_MAX_WIDTH: u32 = 1280;
/// GIFs are re-timed to this rate: 30 fps stutters in most viewers and bloats
/// the file.
const GIF_FPS: u32 = 15;

/// `ffmpeg` from `PATH`, unless the user pinned a binary via `SNAGCRAFT_FFMPEG`.
pub(crate) fn program() -> String {
    std::env::var("SNAGCRAFT_FFMPEG")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "ffmpeg".to_string())
}

/// Is an ffmpeg binary actually runnable?
pub(crate) fn available() -> bool {
    Command::new(program())
        .arg("-version")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

/// Audio input args for one optional source: `-f pulse -i <id>`.
fn audio_input(source: Option<&str>) -> Vec<String> {
    match source {
        Some(id) => ["-f", "pulse", "-i", id].map(String::from).to_vec(),
        None => Vec::new(),
    }
}

/// Complete argument list for the encoder child.
///
/// Layout is always: the rawvideo stdin input, then the audio input (if any),
/// then either `-an` or the map/codec tail, then the video codec args, then the
/// output path. `fps` goes *before* `-i` because that is the input rate of a
/// rawvideo stream.
pub(crate) fn args(
    out: &Path,
    format: RecordingFormat,
    width: u32,
    height: u32,
    fps: u32,
    audio: Option<&str>,
) -> Vec<String> {
    // A GIF has no audio track: never open the input, even when asked.
    let audio = audio.filter(|_| format != RecordingFormat::Gif);

    let mut args: Vec<String> = [
        "-y",
        "-hide_banner",
        "-loglevel",
        "error",
        "-f",
        "rawvideo",
        "-pix_fmt",
        "rgb24",
        "-s",
    ]
    .into_iter()
    .map(String::from)
    .collect();
    args.push(format!("{width}x{height}"));
    args.extend(["-r"].map(String::from));
    args.push(fps.to_string());
    args.extend(["-i", "-"].map(String::from));

    if let Some(id) = audio {
        args.extend(audio_input(Some(id)));
        args.extend(
            [
                "-map",
                "0:v",
                "-map",
                "1:a",
                "-c:a",
                "aac",
                "-b:a",
                "160k",
                "-shortest",
            ]
            .map(String::from),
        );
    } else {
        args.push("-an".to_string());
    }

    match format {
        RecordingFormat::Mp4 => args.extend(
            [
                "-c:v",
                "libx264",
                "-preset",
                "veryfast",
                "-crf",
                "20",
                // yuv420p needs even dimensions. The crop already pads, but a
                // full-frame take on an odd screen width would still break.
                "-vf",
                "pad=ceil(iw/2)*2:ceil(ih/2)*2",
                "-pix_fmt",
                "yuv420p",
                "-movflags",
                "+faststart",
            ]
            .map(String::from),
        ),
        RecordingFormat::Gif => {
            let gif_width = (width.min(GIF_MAX_WIDTH) / 2 * 2).max(2);
            // Single-pass palettegen/paletteuse (verified recipe); two-pass would
            // mean re-reading a temp file for no visible gain.
            let graph = format!(
                "fps={GIF_FPS},scale={gif_width}:-1:flags=lanczos,\
                 split[a][b];[a]palettegen=stats_mode=diff[p];\
                 [b][p]paletteuse=dither=bayer:bayer_scale=3"
            );
            args.extend(["-vf", graph.as_str(), "-loop", "0"].map(String::from));
        }
    }

    args.push(out.to_string_lossy().into_owned());
    args
}

/// A running ffmpeg with its stdin pipe.
#[derive(Debug)]
pub(crate) struct Encoder {
    child: Child,
    stdin: ChildStdin,
    out: PathBuf,
    log: PathBuf,
}

impl Encoder {
    /// Spawn the encoder. Sizes of 0 are rejected: they would make ffmpeg exit
    /// before the first frame.
    pub(crate) fn spawn(job: EncoderJob<'_>) -> Result<Self, RecordingError> {
        let EncoderJob {
            out,
            format,
            width,
            height,
            fps,
            audio,
        } = job;
        if width == 0 || height == 0 {
            return Err(RecordingError::Ffmpeg(
                "the capture stream reported an empty frame size".to_string(),
            ));
        }

        // ffmpeg's own diagnostics go next to the recording, never to the app's
        // stdout (which belongs to tracing).
        let mut log = out.as_os_str().to_os_string();
        log.push(".ffmpeg.log");
        let log = PathBuf::from(log);
        let stderr = std::fs::File::create(&log)
            .or_else(|_| std::fs::File::create("/dev/null"))
            .map_err(|error| {
                RecordingError::Io(format!("cannot open {}: {error}", log.display()))
            })?;

        let mut child = Command::new(program())
            .args(args(out, format, width, height, fps, audio))
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::from(stderr))
            .spawn()
            .map_err(|error| {
                RecordingError::Ffmpeg(format!(
                    "cannot start ffmpeg ({error}); is ffmpeg installed and on PATH?"
                ))
            })?;

        let stdin = child.stdin.take().ok_or_else(|| {
            RecordingError::Ffmpeg("ffmpeg did not provide a stdin pipe".to_string())
        })?;

        Ok(Self {
            child,
            stdin,
            out: out.to_path_buf(),
            log,
        })
    }

    /// True while ffmpeg is still accepting frames (used by the tests; the
    /// capture loop relies on `write_frame` returning `false` instead).
    #[cfg(test)]
    pub(crate) fn is_running(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }

    /// Write one whole frame. `false` means the pipe is gone (ffmpeg died), which
    /// the capture loop treats as a fatal error instead of spinning.
    pub(crate) fn write_frame(&mut self, frame: &[u8]) -> bool {
        self.stdin.write_all(frame).is_ok()
    }

    /// Close stdin (EOF) and wait so the container is finalised before the path
    /// is reported. A hung ffmpeg is killed rather than blocking stop forever.
    pub(crate) fn finish(self) -> Result<(), RecordingError> {
        let Self {
            mut child,
            stdin,
            out,
            log,
        } = self;
        drop(stdin);

        let deadline = Instant::now() + STOP_WAIT;
        loop {
            match child.try_wait() {
                Ok(Some(status)) if status.success() => return Ok(()),
                Ok(Some(status)) => {
                    return Err(RecordingError::Ffmpeg(format!(
                        "ffmpeg exited with {status} writing {} — see {}",
                        out.display(),
                        log.display()
                    )));
                }
                Ok(None) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(50));
                }
                Ok(None) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(RecordingError::Ffmpeg(format!(
                        "ffmpeg did not finish within {}s writing {}",
                        STOP_WAIT.as_secs(),
                        out.display()
                    )));
                }
                Err(error) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(RecordingError::Io(format!(
                        "cannot read ffmpeg's exit status: {error}"
                    )));
                }
            }
        }
    }
}

/// Everything [`Encoder::spawn`] needs (a struct, not seven arguments).
pub(crate) struct EncoderJob<'a> {
    pub out: &'a Path,
    pub format: RecordingFormat,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub audio: Option<&'a str>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn arg_list(format: RecordingFormat, width: u32, fps: u32, audio: Option<&str>) -> Vec<String> {
        args(Path::new("/tmp/take.mp4"), format, width, 480, fps, audio)
    }

    fn has_pair(args: &[String], first: &str, second: &str) -> bool {
        args.windows(2).any(|w| w[0] == first && w[1] == second)
    }

    #[test]
    fn mp4_args_encode_h264_at_the_negotiated_size() {
        let args = arg_list(RecordingFormat::Mp4, 640, 30, None);
        assert!(has_pair(&args, "-f", "rawvideo"));
        assert!(has_pair(&args, "-pix_fmt", "rgb24"));
        assert!(has_pair(&args, "-s", "640x480"));
        assert!(has_pair(&args, "-r", "30"));
        assert!(has_pair(&args, "-c:v", "libx264"));
        assert!(has_pair(&args, "-preset", "veryfast"));
        assert!(has_pair(&args, "-pix_fmt", "yuv420p"));
        assert!(args.iter().any(|a| a == "-an"), "no audio requested");
        assert_eq!(args.last().map(String::as_str), Some("/tmp/take.mp4"));
        // The input option block must precede `-i -`.
        let input = args.iter().position(|a| a == "-i").expect("-i present");
        let size = args.iter().position(|a| a == "-s").expect("-s present");
        let rate = args.iter().position(|a| a == "-r").expect("-r present");
        assert!(size < input && rate < input, "-s/-r are input options");
    }

    #[test]
    fn gif_args_use_the_palette_recipe_and_a_downscale() {
        let args = args(
            Path::new("/tmp/take.gif"),
            RecordingFormat::Gif,
            1920,
            1080,
            30,
            Some("alsa_input.foo"),
        );
        let graph = args
            .iter()
            .position(|a| a.starts_with("fps="))
            .map(|i| args[i].clone())
            .expect("filter graph");
        assert!(graph.contains("split[a][b]"));
        assert!(graph.contains("palettegen"));
        assert!(graph.contains("paletteuse"));
        assert!(graph.contains("scale=1280:-1"), "downscaled to the cap");
        assert!(has_pair(&args, "-loop", "0"));
        // A GIF is silent whatever was asked: no pulse input, no audio mapping.
        assert!(!args.iter().any(|a| a == "pulse"));
        assert!(args.iter().any(|a| a == "-an"));
        assert!(!has_pair(&args, "-c:a", "aac"));
    }

    #[test]
    fn an_audio_source_is_mapped_and_encoded_as_aac() {
        let args = arg_list(RecordingFormat::Mp4, 640, 30, Some("alsa_input.usb-mic"));
        assert!(has_pair(&args, "-f", "pulse"));
        let mic = args
            .iter()
            .position(|a| a == "alsa_input.usb-mic")
            .expect("the source id is an input");
        let input = args.iter().position(|a| a == "-i").expect("-i present");
        assert!(mic > input, "audio input comes after the video input");
        assert!(has_pair(&args, "-map", "0:v"));
        assert!(has_pair(&args, "-map", "1:a"));
        assert!(has_pair(&args, "-c:a", "aac"));
        assert!(has_pair(&args, "-b:a", "160k"));
        assert!(args.iter().any(|a| a == "-shortest"));
        assert!(!args.iter().any(|a| a == "-an"));
    }

    #[test]
    fn the_gif_downscale_never_goes_below_two_pixels() {
        let args = args(
            Path::new("/tmp/t.gif"),
            RecordingFormat::Gif,
            1,
            1,
            30,
            None,
        );
        let graph = args
            .iter()
            .find(|a| a.starts_with("fps="))
            .expect("filter graph");
        assert!(graph.contains("scale=2:-1"), "got {graph}");
    }

    #[test]
    fn an_empty_frame_size_is_rejected_before_spawning() {
        let error = Encoder::spawn(EncoderJob {
            out: Path::new("/tmp/zero.mp4"),
            format: RecordingFormat::Mp4,
            width: 0,
            height: 240,
            fps: 30,
            audio: None,
        })
        .expect_err("0 width");
        assert!(matches!(error, RecordingError::Ffmpeg(_)));
    }

    /// End-to-end with a real ffmpeg child: synthetic rgb24 frames in, a real
    /// h264 MP4 out. This is the same write path the PipeWire callback uses.
    #[test]
    fn the_encoder_child_produces_a_real_mp4_from_piped_frames() {
        if !available() {
            return;
        }
        let dir = std::env::temp_dir().join("snagcraft-recording-encoder-test");
        std::fs::create_dir_all(&dir).expect("scratch dir");
        let out = dir.join("child.mp4");
        let _ = std::fs::remove_file(&out);

        let mut encoder = Encoder::spawn(EncoderJob {
            out: &out,
            format: RecordingFormat::Mp4,
            width: 64,
            height: 48,
            fps: 15,
            audio: None,
        })
        .expect("spawn ffmpeg");

        for frame_index in 0..10u8 {
            let mut frame = vec![0u8; 64 * 48 * 3];
            for y in 0..48 {
                for x in 0..64 {
                    let i = (y * 64 + x) * 3;
                    frame[i] = x as u8 * 4;
                    frame[i + 1] = y as u8 * 5;
                    frame[i + 2] = frame_index * 10;
                }
            }
            assert!(encoder.write_frame(&frame), "ffmpeg must accept the frame");
        }
        assert!(encoder.is_running());
        encoder.finish().expect("ffmpeg must finalise the file");

        let meta = std::fs::metadata(&out).expect("output exists");
        assert!(meta.len() > 1000, "got {} bytes", meta.len());

        let probe = Command::new("ffprobe")
            .args([
                "-v",
                "error",
                "-select_streams",
                "v:0",
                "-show_entries",
                "stream=codec_name,nb_frames,width,height",
                "-of",
                "default=nw=1",
                &out.to_string_lossy(),
            ])
            .output()
            .expect("ffprobe runs");
        let text = String::from_utf8_lossy(&probe.stdout);
        assert!(text.contains("codec_name=h264"), "ffprobe said: {text}");
        assert!(text.contains("width=64"), "ffprobe said: {text}");
        let _ = std::fs::remove_file(&out);
    }
}
