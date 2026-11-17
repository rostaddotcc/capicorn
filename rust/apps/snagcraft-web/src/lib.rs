#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]
//! W5d stub — WASM web entry (eframe WebRunner), track fills per contract.
use snagcraft_engine as _;
use snagcraft_ui_egui as _;
use eframe as _;
use egui as _;
use wasm_bindgen as _;
use wasm_bindgen_futures as _;
use web_sys as _;
use js_sys as _;
use console_error_panic_hook as _;
use tracing as _;
