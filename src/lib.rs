//! Library crate for the `rr` CLI.
//!
//! Every module here is `#![forbid(unsafe_code)]`. The binary is a thin
//! orchestrator on top of these modules so that integration tests can exercise
//! the same code paths as the CLI does.

#![forbid(unsafe_code)]
#![deny(rust_2018_idioms)]
#![warn(clippy::all, clippy::pedantic)]
#![allow(
    clippy::module_name_repetitions,
    clippy::missing_errors_doc,
    clippy::missing_panics_doc,
    clippy::too_many_lines
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
