#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]
//! L4 recording — W4a stub (scaffold pre-wired wake #13 to de-risk bindgen).
use snagcraft_raster as _;
use snagcraft_geom as _;
use thiserror as _;
use tracing as _;
#[cfg(target_os = "linux")]
use pipewire as _;
#[cfg(target_os = "linux")]
use ashpd as _;
#[cfg(target_os = "linux")]
use tokio as _;
