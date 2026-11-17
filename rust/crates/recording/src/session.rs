//! Owns one take: the portal session, the capture thread, and the command loop
//! that turns [`Command`]s into pause/stop and events.
//!
//! This runs on the thread [`crate::spawn_recording`] starts. The tokio runtime
//! here is a **current-thread** runtime that stays inside `block_on` for the
//! whole take: that keeps zbus servicing its connection (so the portal session
//! stays healthy) without needing tokio's `rt-multi-thread` feature. The session
//! object lives in the same future, so it cannot be dropped early by accident.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

use crate::capture::{CaptureParams, Endpoint};
use crate::portal::{self, Cast};
use crate::{
    Command, FIRST_FRAME_WAIT, RecordingConfig, RecordingError, RecordingEvent, STOP_WAIT,
};

/// Command poll interval — fast enough that a stop feels instant.
const TICK: Duration = Duration::from_millis(100);

/// Body of the recording thread: always reports exactly one terminal event.
pub(crate) fn run(
    config: RecordingConfig,
    events: mpsc::Sender<RecordingEvent>,
    commands: mpsc::Receiver<Command>,
    stop: Arc<AtomicBool>,
    paused: Arc<AtomicBool>,
    active: Arc<AtomicBool>,
) {
    let path = config.output.clone();
    let result = drive(&config, &events, &commands, &stop, &paused);
    active.store(false, Ordering::Relaxed);

    let event = match result {
        Ok(()) => RecordingEvent::Stopped { path },
        Err(error) => {
            tracing::error!(%error, "recording failed");
            RecordingEvent::Failed(error.user_message())
        }
    };
    let _ = events.send(event);
}

fn drive(
    config: &RecordingConfig,
    events: &mpsc::Sender<RecordingEvent>,
    commands: &mpsc::Receiver<Command>,
    stop: &Arc<AtomicBool>,
    paused: &Arc<AtomicBool>,
) -> Result<(), RecordingError> {
    // Deliberately not multi-thread: zbus only needs *some* runtime to spawn its
    // socket task on, and `drive_async` stays inside `block_on` until the take
    // ends, so that task keeps running.
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| RecordingError::Io(format!("cannot start the tokio runtime: {error}")))?;

    runtime.block_on(drive_async(config, events, commands, stop, paused))
}

async fn drive_async(
    config: &RecordingConfig,
    events: &mpsc::Sender<RecordingEvent>,
    commands: &mpsc::Receiver<Command>,
    stop: &Arc<AtomicBool>,
    paused: &Arc<AtomicBool>,
) -> Result<(), RecordingError> {
    let Cast {
        fd,
        node,
        hint,
        session,
    } = portal::open().await?;

    let params = CaptureParams::from_config(config, events.clone(), stop.clone(), paused.clone());
    let (done_tx, done_rx) = mpsc::channel();
    let capture_thread = std::thread::Builder::new()
        .name("snagcraft-capture".to_string())
        .spawn(move || {
            let result = crate::capture::run(Endpoint::PortalFd(fd), node, hint, params);
            let _ = done_tx.send(result);
        })
        .map_err(|error| RecordingError::Io(format!("cannot start the capture thread: {error}")))?;

    let mut commands_open = true;
    // Only consulted after the handle is dropped without a stop command.
    let orphan_deadline = Instant::now() + STOP_WAIT + FIRST_FRAME_WAIT;
    let outcome = loop {
        match done_rx.try_recv() {
            Ok(result) => break Some(result),
            Err(mpsc::TryRecvError::Disconnected) => break None,
            Err(mpsc::TryRecvError::Empty) => {}
        }

        if commands_open {
            match commands.try_recv() {
                Ok(Command::Stop) => stop.store(true, Ordering::Relaxed),
                Ok(Command::Pause) => {
                    if !paused.swap(true, Ordering::Relaxed) {
                        let _ = events.send(RecordingEvent::Paused);
                    }
                }
                Ok(Command::Resume) => {
                    if paused.swap(false, Ordering::Relaxed) {
                        let _ = events.send(RecordingEvent::Resumed);
                    }
                }
                Err(mpsc::TryRecvError::Empty) => {}
                Err(mpsc::TryRecvError::Disconnected) => {
                    // The handle was dropped: treat it as a stop, then wait for
                    // the container to be finalised.
                    commands_open = false;
                    stop.store(true, Ordering::Relaxed);
                }
            }
        } else if Instant::now() > orphan_deadline {
            break None;
        }

        // Keeps this runtime (and therefore zbus) polled while we wait.
        tokio::time::sleep(TICK).await;
    };

    stop.store(true, Ordering::Relaxed);
    let joined = capture_thread.join();
    // The capture half is finished with the fd; closing the session is the last
    // portal interaction of a take.
    drop(session);

    if joined.is_err() {
        return Err(RecordingError::PipeWire(
            "the capture thread panicked".to_string(),
        ));
    }
    match outcome {
        Some(Ok(_frames)) => Ok(()),
        Some(Err(error)) => Err(error),
        None => Err(RecordingError::PipeWire(
            "the capture thread exited without reporting a result".to_string(),
        )),
    }
}
