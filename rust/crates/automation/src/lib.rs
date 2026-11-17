#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]
//! L6 automation — W5a stub (TCP JSON control + MCP), track fills per contract.
use snagcraft_engine as _;
use snagcraft_io as _;
use serde as _;
use serde_json as _;
use thiserror as _;
use tracing as _;
use base64 as _;
use rmcp as _;
use tokio as _;
use getrandom as _;
use schemars as _;
