//! Library crate for the `rr` CLI.
//!
//! Every module here is `#![forbid(unsafe_code)]`. The binary is a thin
//! orchestrator on top of these modules so that integration tests can exercise
//! the same code paths as the CLI does.

#![forbid(unsafe_code)]
#![deny(rust_2018_idioms)]
// Keep clippy::all (correctness, suspicious, style, complexity, perf) as the
// CI gate. clippy::pedantic is intentionally NOT enabled — it's opinion-heavy
// (must-use-candidate on every public fn, uninlined format args everywhere,
// debug-vs-display on PathBuf, etc.) and would produce hundreds of stylistic
// nits without catching real bugs.
#![warn(clippy::all)]
#![allow(
    clippy::module_name_repetitions,
    clippy::missing_errors_doc,
    clippy::missing_panics_doc,
    clippy::too_many_lines,
    // Cosmetic: `"{}", name` is just as clear as `"{name}"`. Not worth the churn.
    clippy::uninlined_format_args
)]

pub mod cli;
pub mod cloud_api;
pub mod config;
pub mod device_auth;
pub mod epub;
pub mod error;
pub mod jobs;
pub mod logging;
pub mod markdown;
pub mod raster;
pub mod retry;
pub mod skills;
pub mod source;
pub mod token_store;

pub use error::{Error, Result};
