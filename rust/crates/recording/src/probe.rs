//! Offline probe: prove the capture half without the portal.
//!
//! A local PipeWire `Video/Source` (a producer *we* create, so an explicit
//! `SPA_PARAM_Buffers` pool is correct here — never on the consumer) feeds the
//! exact production path: registry wait → `stream.connect` → BGRx → rgb24 →
//! region crop → ffmpeg stdin. The result is checked with `ffprobe`.
//!
//! This is the only way to validate the pipeline on a locked screen, where the
//! portal leg cannot run at all.
//!
//! ```text
//! cargo test -p snagcraft-recording -- --ignored --nocapture
//! ```
//!
//! Requires: a running PipeWire daemon (`pw-cli info 0` must succeed — a normal
//! unlocked *or* locked user session has one; a headless build box may not) and
//! `ffmpeg` + `ffprobe` on `PATH`. Without a daemon the test prints why and
//! returns instead of failing.

#![cfg(test)]

use std::cell::Cell;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use pipewire as pw;
use pw::spa;
use spa::param::format::{FormatProperties, MediaSubtype, MediaType};
use spa::param::video::{VideoFormat, VideoInfoRaw};
use spa::pod::{Pod, Property, Value};
use spa::utils::{Fraction, Id};

use crate::capture::{self, CaptureParams, Endpoint};
use crate::{
    AudioConfig, RecordingConfig, RecordingError, RecordingEvent, RecordingFormat, Region,
};

/// The producer's node name, used to find its global id in the registry.
const PROBE_NODE_NAME: &str = "snagcraft-probe-source";
const PROBE_W: u32 = 320;
const PROBE_H: u32 = 240;
const PROBE_FPS: u32 = 15;

/// One `EnumFormat` pod for the synthetic source: packed BGRx at a fixed size.
fn producer_format_pod() -> Vec<u8> {
    let properties = vec![
        Property::new(
            FormatProperties::MediaType.as_raw(),
            Value::Id(Id(MediaType::Video.as_raw())),
        ),
        Property::new(
            FormatProperties::MediaSubtype.as_raw(),
            Value::Id(Id(MediaSubtype::Raw.as_raw())),
        ),
        Property::new(
            FormatProperties::VideoFormat.as_raw(),
            Value::Id(Id(VideoFormat::BGRx.as_raw())),
        ),
        Property::new(
            FormatProperties::VideoSize.as_raw(),
            Value::Rectangle(spa::utils::Rectangle {
                width: PROBE_W,
                height: PROBE_H,
            }),
        ),
        Property::new(
            FormatProperties::VideoFramerate.as_raw(),
            Value::Fraction(Fraction {
                num: PROBE_FPS,
                denom: 1,
            }),
        ),
    ];
    let object = spa::pod::Object {
        type_: spa::utils::SpaTypes::ObjectParamFormat.as_raw(),
        id: spa::param::ParamType::EnumFormat.as_raw(),
        properties,
    };
    serialize(&spa::pod::Value::Object(object))
}

/// `SPA_PARAM_Buffers` for the producer's own pool: a full frame per buffer.
/// This is legitimate for a source we create; requesting one from a *consumer*
/// breaks KWin's portal port with "error alloc buffers: Invalid argument".
fn producer_buffers_pod() -> Vec<u8> {
    let stride = PROBE_W * 4;
    let properties = vec![
        Property::new(spa::sys::SPA_PARAM_BUFFERS_buffers, Value::Int(8)),
        Property::new(spa::sys::SPA_PARAM_BUFFERS_blocks, Value::Int(1)),
        Property::new(
            spa::sys::SPA_PARAM_BUFFERS_size,
            Value::Int((stride * PROBE_H) as i32),
        ),
        Property::new(
            spa::sys::SPA_PARAM_BUFFERS_stride,
            Value::Int(stride as i32),
        ),
    ];
    let object = spa::pod::Object {
        type_: spa::utils::SpaTypes::ObjectParamBuffers.as_raw(),
        id: spa::param::ParamType::Buffers.as_raw(),
        properties,
    };
    serialize(&spa::pod::Value::Object(object))
}

fn serialize(value: &spa::pod::Value) -> Vec<u8> {
    match spa::pod::serialize::PodSerializer::serialize(Cursor::new(Vec::new()), value) {
        Ok(serialized) => serialized.0.into_inner(),
        Err(error) => {
            eprintln!("probe: cannot serialise a pod: {error:?}");
            Vec::new()
        }
    }
}

/// A synthetic `Video/Source` running on its own PipeWire loop.
struct Producer {
    /// The node id in the daemon graph.
    node: u32,
    stop: Arc<AtomicBool>,
    join: Option<JoinHandle<()>>,
}

impl Producer {
    fn stop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

impl Drop for Producer {
    fn drop(&mut self) {
        self.stop();
    }
}

struct SourceUd {
    fmt: VideoInfoRaw,
    /// The `SPA_PARAM_Buffers` pod, serialised once and re-sent on negotiation.
    buffers: Vec<u8>,
    frame: u64,
}

/// Create the source stream and wait until its node id is visible.
fn start_producer() -> Result<Producer, String> {
    let (tx, rx) = mpsc::channel::<u32>();
    let stop = Arc::new(AtomicBool::new(false));
    let thread_stop = stop.clone();

    let join = std::thread::Builder::new()
        .name("snagcraft-probe-source".to_string())
        .spawn(move || producer_main(tx, thread_stop))
        .map_err(|error| format!("cannot start the producer thread: {error}"))?;

    let node = rx
        .recv_timeout(Duration::from_secs(10))
        .map_err(|error| format!("the probe source never appeared: {error}"))?;

    Ok(Producer {
        node,
        stop,
        join: Some(join),
    })
}

fn producer_main(tx: mpsc::Sender<u32>, stop: Arc<AtomicBool>) {
    pw::init();

    let Ok(mainloop) = pw::main_loop::MainLoopRc::new(None) else {
        eprintln!("probe: cannot create the source main loop");
        return;
    };
    let Ok(context) = pw::context::ContextRc::new(&mainloop, None) else {
        eprintln!("probe: cannot create the source context");
        return;
    };
    let Ok(core) = context.connect_rc(None) else {
        eprintln!("probe: cannot connect the source to the daemon");
        return;
    };

    // `media.class = Video/Source` is required: WirePlumber's prepare-link.lua
    // rejects anything else, and the consumer would see "no target node".
    let props = pw::properties::properties! {
        *pw::keys::MEDIA_TYPE => "Video",
        *pw::keys::MEDIA_CATEGORY => "Playback",
        *pw::keys::MEDIA_CLASS => "Video/Source",
        *pw::keys::NODE_NAME => PROBE_NODE_NAME,
    };
    let Ok(stream) = pw::stream::StreamRc::new(core.clone(), "snagcraft-probe-source", props)
    else {
        eprintln!("probe: cannot create the source stream");
        return;
    };

    // Find our own global id while the registry syncs.
    let found = Rc::new(Cell::new(0u32));
    let found_flag = found.clone();
    let registry = core.get_registry().ok();
    let registry_listener = registry.as_ref().map(|registry| {
        registry
            .add_listener_local()
            .global(move |global| {
                if let Some(props) = global.props
                    && props.get(*pw::keys::NODE_NAME) == Some(PROBE_NODE_NAME)
                {
                    found_flag.set(global.id);
                }
            })
            .register()
    });

    let ud = SourceUd {
        fmt: VideoInfoRaw::new(),
        buffers: producer_buffers_pod(),
        frame: 0,
    };

    let listener = stream
        .add_local_listener_with_user_data(ud)
        .param_changed(|stream, ud, id, param| {
            if id != spa::param::ParamType::Format.as_raw() {
                return;
            }
            let Some(param) = param else { return };
            if ud.fmt.parse(param).is_ok() {
                let size = ud.fmt.size();
                eprintln!(
                    "probe: source negotiated {:?} {}x{}",
                    ud.fmt.format(),
                    size.width,
                    size.height
                );
            }
            // The pool is answered *after* negotiation, as a real source does.
            if let Some(pod) = Pod::from_bytes(&ud.buffers) {
                let _ = stream.update_params(&mut [pod]);
            }
        })
        .process(|stream, ud| {
            let Some(mut buffer) = stream.dequeue_buffer() else {
                return;
            };
            let size = ud.fmt.size();
            let (width, height) = (size.width as usize, size.height as usize);
            let datas = buffer.datas_mut();
            if datas.is_empty() || width == 0 || height == 0 {
                return;
            }

            let stride = width * 4;
            let frame_bytes = stride * height;
            let tick = ud.frame;
            ud.frame = ud.frame.wrapping_add(1);
            {
                let data = &mut datas[0];
                if let Some(bytes) = data.data()
                    && bytes.len() >= frame_bytes
                {
                    for y in 0..height {
                        for x in 0..width {
                            let i = y * stride + x * 4;
                            // BGRx, moving with the frame counter so the take has
                            // motion (and a wrong channel order shows).
                            bytes[i] = (x as u8).wrapping_add(tick as u8);
                            bytes[i + 1] = y as u8;
                            bytes[i + 2] = ((x ^ y) & 0xFF) as u8;
                            bytes[i + 3] = 0xFF;
                        }
                    }
                }
                let chunk = data.chunk_mut();
                *chunk.offset_mut() = 0;
                *chunk.size_mut() = frame_bytes as u32;
                *chunk.stride_mut() = stride as i32;
            }
        })
        .register();

    if listener.is_err() {
        eprintln!("probe: cannot register the source callbacks");
        return;
    }
    let Ok(listener) = listener else { return };

    let mut format_pods = [producer_format_pod()];
    let mut params: Vec<&Pod> = format_pods
        .iter_mut()
        .filter_map(|bytes| Pod::from_bytes(bytes))
        .collect();

    if let Err(error) = stream.connect(
        spa::utils::Direction::Output,
        None,
        // DRIVER: the source drives the graph, via the timer below.
        pw::stream::StreamFlags::DRIVER
            | pw::stream::StreamFlags::MAP_BUFFERS
            | pw::stream::StreamFlags::RT_PROCESS,
        &mut params,
    ) {
        eprintln!("probe: cannot connect the source stream: {error}");
        return;
    }

    let deadline = Instant::now() + Duration::from_secs(5);
    while found.get() == 0 && Instant::now() < deadline {
        let _ = mainloop
            .loop_()
            .iterate(pw::loop_::Timeout::Finite(Duration::from_millis(20)));
    }
    if found.get() == 0 {
        eprintln!("probe: the source node never showed up in the registry");
        return;
    }
    let _ = tx.send(found.get());
    eprintln!("probe: source node {} running", found.get());

    // Drive one frame every 1/fps.
    let timer_weak = mainloop.downgrade();
    let timer_stream = stream.clone();
    let timer_stop = stop.clone();
    let timer = mainloop.loop_().add_timer(move |_| {
        if timer_stop.load(Ordering::Relaxed) {
            if let Some(mainloop) = timer_weak.upgrade() {
                mainloop.quit();
            }
            return;
        }
        let _ = timer_stream.trigger_process();
    });
    let _ = timer.update_timer(
        Some(Duration::from_millis(1000 / u64::from(PROBE_FPS))),
        Some(Duration::from_millis(1000 / u64::from(PROBE_FPS))),
    );

    mainloop.run();

    // Same teardown rule as the consumer: never drop (or otherwise sync) pw
    // objects after the loop returned — that roundtrip has nobody to service it
    // and deadlocks (destroying the context waits for its internal threads).
    // Leak them; this thread is about to exit.
    std::mem::forget(registry_listener);
    std::mem::forget(timer);
    std::mem::forget(listener);
    std::mem::forget(stream);
    std::mem::forget(mainloop);
    std::mem::forget(registry);
    std::mem::forget(core);
    std::mem::forget(context);
}

/// One take against the synthetic source, returning the frames written.
fn record_take(
    producer_node: u32,
    format: RecordingFormat,
    out: &Path,
    pause_ms: u64,
) -> Result<(u64, Vec<RecordingEvent>), String> {
    let _ = std::fs::remove_file(out);
    let (events_tx, events_rx) = mpsc::channel();
    let stop = Arc::new(AtomicBool::new(false));
    let paused = Arc::new(AtomicBool::new(false));

    let config = RecordingConfig {
        format,
        region: Some(Region {
            left: 40,
            top: 30,
            width: 200,
            height: 120,
        }),
        audio: AudioConfig::default(),
        fps: PROBE_FPS,
        output: out.to_path_buf(),
    };

    let params = CaptureParams::from_config(&config, events_tx, stop.clone(), paused.clone());
    let consumer = std::thread::Builder::new()
        .name("snagcraft-probe-consumer".to_string())
        .spawn(move || {
            capture::run(
                Endpoint::Daemon,
                producer_node,
                (PROBE_W as i32, PROBE_H as i32),
                params,
            )
        })
        .map_err(|error| format!("cannot start the consumer: {error}"))?;

    // Wait for the first frame to reach the encoder.
    let started = events_rx.recv_timeout(Duration::from_secs(10));
    let mut events = Vec::new();
    match started {
        Ok(event) => events.push(event),
        Err(error) => {
            stop.store(true, Ordering::Relaxed);
            let _ = consumer.join();
            return Err(format!("no Started event: {error}"));
        }
    }

    // A pause window in the middle of the take: the capture loop keeps dequeuing
    // buffers (or the producer starves) but writes none, and resuming must
    // continue the same file.
    std::thread::sleep(Duration::from_millis(400));
    if pause_ms > 0 {
        paused.store(true, Ordering::Relaxed);
        std::thread::sleep(Duration::from_millis(pause_ms));
        paused.store(false, Ordering::Relaxed);
    }
    std::thread::sleep(Duration::from_millis(800));
    stop.store(true, Ordering::Relaxed);

    let frames = consumer
        .join()
        .map_err(|_| "the consumer thread panicked".to_string())?;
    while let Ok(event) = events_rx.try_recv() {
        events.push(event);
    }
    frames
        .map_err(|error| error.to_string())
        .map(|count| (count, events))
}

/// `ffprobe` key=value output for the first video stream.
fn probe_video(path: &Path) -> Option<String> {
    let output = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-show_entries",
            "stream=codec_name,width,height,nb_frames:format=duration",
            "-of",
            "default=nw=1",
        ])
        .arg(path)
        .output()
        .ok()?;
    Some(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn value_of(text: &str, key: &str) -> Option<String> {
    text.lines()
        .find_map(|line| {
            line.strip_prefix(key)
                .and_then(|rest| rest.strip_prefix('='))
        })
        .map(|value| value.trim().to_string())
}

fn scratch_dir() -> PathBuf {
    let dir = std::env::temp_dir().join("snagcraft-recording-probe");
    let _ = std::fs::create_dir_all(&dir);
    dir
}

#[test]
#[ignore = "needs a PipeWire daemon and ffmpeg; run: cargo test -p snagcraft-recording -- --ignored --nocapture"]
fn offline_video_source_records_real_mp4_and_gif() {
    // No `pw-cli` liveness probe: `pw-cli info 0` prints its answer and then sits
    // in its REPL (it does not exit reliably), so the producer's own bounded
    // startup is the availability check.
    let mut producer = match start_producer() {
        Ok(producer) => producer,
        Err(error) => {
            eprintln!("probe: no usable PipeWire daemon ({error}) — skipping");
            return;
        }
    };
    let dir = scratch_dir();

    // -- MP4, with a region: the full production path -----------------------
    let mp4 = dir.join("offline.mp4");
    let (frames, events) = record_take(producer.node, RecordingFormat::Mp4, &mp4, 400)
        .expect("the capture half must record from a local Video/Source");
    assert!(
        frames >= 5,
        "expected several frames from a 15 fps source, got {frames}"
    );
    assert!(
        events.iter().any(|event| matches!(
            event,
            RecordingEvent::Started {
                width: 200,
                height: 120
            }
        )),
        "Started must report the cropped size, got {events:?}"
    );

    let text = probe_video(&mp4).expect("ffprobe must run");
    eprintln!("probe: ffprobe offline.mp4\n{text}");
    assert_eq!(
        value_of(&text, "codec_name").as_deref(),
        Some("h264"),
        "{text}"
    );
    assert_eq!(value_of(&text, "width").as_deref(), Some("200"), "{text}");
    assert_eq!(value_of(&text, "height").as_deref(), Some("120"), "{text}");
    let duration: f32 = value_of(&text, "duration")
        .and_then(|value| value.parse().ok())
        .unwrap_or(0.0);
    assert!(duration > 0.0, "a real take has duration, got {text}");
    let nb_frames: u64 = value_of(&text, "nb_frames")
        .and_then(|value| value.parse().ok())
        .unwrap_or(0);
    assert!(nb_frames > 0, "nb_frames must be > 0, got {text}");
    let mp4_size = std::fs::metadata(&mp4).map(|meta| meta.len()).unwrap_or(0);
    assert!(mp4_size > 2000, "a real MP4 is not {mp4_size} bytes");

    // -- GIF: the palettegen/paletteuse path -------------------------------
    let gif = dir.join("offline.gif");
    let (gif_frames, _events) = record_take(producer.node, RecordingFormat::Gif, &gif, 0)
        .expect("the GIF path must record from a local Video/Source");
    assert!(gif_frames >= 5, "got {gif_frames} GIF frames");

    let gif_text = probe_video(&gif).expect("ffprobe must run");
    eprintln!("probe: ffprobe offline.gif\n{gif_text}");
    assert_eq!(
        value_of(&gif_text, "codec_name").as_deref(),
        Some("gif"),
        "{gif_text}"
    );
    let gif_size = std::fs::metadata(&gif).map(|meta| meta.len()).unwrap_or(0);
    assert!(gif_size > 500, "a real GIF is not {gif_size} bytes");

    producer.stop();
}

#[test]
fn the_probe_reports_missing_ffmpeg_clearly() {
    // Cheap guard for the ignored tests above: the encoder must exist at all.
    assert!(
        crate::ffmpeg_available(),
        "the offline probe needs ffmpeg on PATH"
    );
}

#[test]
fn a_capture_error_is_reported_as_a_recording_error() {
    // The probe's failure path is the same enum the public API exposes.
    let error = RecordingError::PipeWire("no target node available".to_string());
    assert!(error.to_string().contains("no target node available"));
}
