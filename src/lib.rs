//! Library target — re-exports all modules so integration tests in `tests/`
//! can import from `opencode_voice::*`.

pub mod config;
pub mod state;
pub mod app;
pub mod audio;
pub mod bridge;
pub mod input;
pub mod transcribe;
pub mod approval;
pub mod ui;
