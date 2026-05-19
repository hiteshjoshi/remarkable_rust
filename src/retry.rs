//! Exponential-backoff retry helper.
//!
//! Used by network callers in `sync_v3`. Backoff is jittered to avoid
//! thundering-herd retries against the reMarkable cloud.

use std::future::Future;
use std::time::Duration;

use rand::Rng;

use crate::error::Error;

/// Retry policy configuration.
#[derive(Debug, Clone, Copy)]
pub struct Policy {
    pub max_attempts: u32,
    pub base_delay: Duration,
    pub max_delay: Duration,
}

impl Policy {
    pub const fn default_network() -> Self {
        Self {
            max_attempts: 4,
            base_delay: Duration::from_millis(250),
            max_delay: Duration::from_secs(8),
        }
    }

    pub const fn quick() -> Self {
        Self {
            max_attempts: 2,
            base_delay: Duration::from_millis(100),
            max_delay: Duration::from_millis(500),
        }
    }
}

/// Run `op` with retries. Only retries when `is_retryable` returns true *and*
/// attempts remain. The final error is returned unchanged.
pub async fn retry<F, Fut, T, E>(
    policy: Policy,
    is_retryable: impl Fn(&E) -> bool,
    mut op: F,
) -> Result<T, E>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, E>>,
{
    let mut attempt: u32 = 0;
    loop {
        attempt += 1;
        match op().await {
            Ok(value) => return Ok(value),
            Err(err) if attempt < policy.max_attempts && is_retryable(&err) => {
                let delay = backoff_for(policy, attempt);
                tracing::debug!(
                    attempt,
                    delay_ms = delay.as_millis() as u64,
                    "retrying after error"
                );
                tokio::time::sleep(delay).await;
            }
            Err(err) => return Err(err),
        }
    }
}

/// Convenience for [`Error`]: retries on its `is_retryable` predicate.
pub async fn retry_default<F, Fut, T>(policy: Policy, op: F) -> Result<T, Error>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, Error>>,
{
    retry(policy, Error::is_retryable, op).await
}

fn backoff_for(policy: Policy, attempt: u32) -> Duration {
    let exp = policy
        .base_delay
        .saturating_mul(1u32.checked_shl(attempt - 1).unwrap_or(u32::MAX));
    let capped = exp.min(policy.max_delay);
    // Full jitter: pick uniformly in [0, capped]
    let mut rng = rand::thread_rng();
    let jitter = rng.gen_range(0..=capped.as_millis() as u64);
    Duration::from_millis(jitter.max(policy.base_delay.as_millis() as u64 / 2))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    #[tokio::test]
    async fn succeeds_first_try() {
        let result: Result<i32, Error> =
            retry(Policy::quick(), |_| true, || async { Ok::<i32, Error>(42) }).await;
        assert_eq!(result.unwrap(), 42);
    }

    #[tokio::test]
    async fn retries_then_succeeds() {
        let counter = AtomicU32::new(0);
        let result: Result<i32, Error> = retry(
            Policy::quick(),
            |_| true,
            || async {
                let n = counter.fetch_add(1, Ordering::SeqCst);
                if n < 1 {
                    Err(Error::Other("transient".into()))
                } else {
                    Ok(7)
                }
            },
        )
        .await;
        assert_eq!(result.unwrap(), 7);
        assert_eq!(counter.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn does_not_retry_non_retryable() {
        let counter = AtomicU32::new(0);
        let result: Result<i32, Error> = retry(
            Policy::quick(),
            |_| false,
            || async {
                counter.fetch_add(1, Ordering::SeqCst);
                Err(Error::Other("nope".into()))
            },
        )
        .await;
        assert!(result.is_err());
        assert_eq!(counter.load(Ordering::SeqCst), 1);
    }
}
