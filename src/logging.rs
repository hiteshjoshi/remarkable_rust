//! Logging initialisation.
//!
//! Library code uses the `tracing` crate; user-facing output stays on
//! `println!`/`eprintln!` so logs and command output don't interleave.

use std::sync::Once;

use tracing_subscriber::{fmt, EnvFilter};

static INIT: Once = Once::new();

/// Initialise tracing exactly once. Safe to call multiple times.
///
/// `verbose` raises the default filter from `warn` to `debug` for the `rr`
/// crate. Env-var `RUST_LOG` still wins if set.
pub fn init(verbose: bool) {
    INIT.call_once(|| {
        let default = if verbose {
            "rr=debug,info"
        } else {
            "rr=warn,error"
        };
        let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(default));
        fmt()
            .with_env_filter(filter)
            .with_target(false)
            .with_writer(std::io::stderr)
            .compact()
            .try_init()
            .ok();
    });
}
