//! Strongly-typed errors for the `rr` library surface.
//!
//! Library code returns [`Result<T>`] which carries [`Error`]; CLI code may
//! convert with `?` into `anyhow::Result` for ergonomic propagation.

use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("not authenticated; run `rr auth`")]
    NotAuthenticated,

    #[error("authentication expired; run `rr auth` to re-pair")]
    AuthExpired,

    #[error("token refresh failed: {0}")]
    RefreshFailed(String),

    #[error("network error: {0}")]
    Network(#[from] reqwest::Error),

    #[error("io error at {path:?}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("io error: {0}")]
    BareIo(#[from] std::io::Error),

    #[error("config error: {0}")]
    Config(String),

    #[error("conversion error: {0}")]
    Convert(String),

    #[error("api error: HTTP {status} – {body}")]
    Api { status: u16, body: String },

    #[error("invalid response: {0}")]
    InvalidResponse(String),

    #[error("rate limited; retry after {retry_after_secs}s")]
    RateLimited { retry_after_secs: u64 },

    #[error("operation cancelled")]
    Cancelled,

    #[error("job error: {0}")]
    Job(String),

    #[error("{0}")]
    Other(String),
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

impl Error {
    pub fn is_retryable(&self) -> bool {
        match self {
            Error::Network(e) => e.is_timeout() || e.is_connect() || e.is_request(),
            Error::Api { status, .. } => matches!(*status, 408 | 429 | 500 | 502 | 503 | 504),
            Error::RateLimited { .. } => true,
            _ => false,
        }
    }
}
