//! Client-side enforcement of KEGG's documented request-rate limit.

use std::time::{Duration, Instant};

use tokio::sync::{Mutex, Semaphore};

use crate::error::{KeggError, Result};

#[derive(Debug)]
pub(crate) struct RateLimiter {
    period: Duration,
    next_at: Mutex<Option<Instant>>,
    in_flight: Semaphore,
}

impl RateLimiter {
    pub(crate) fn new(max_per_second: u32) -> Result<Self> {
        if max_per_second == 0 {
            return Err(KeggError::InvalidParameter(
                "max_per_second must be greater than zero".to_string(),
            ));
        }
        Ok(Self {
            period: Duration::from_nanos(1_000_000_000 / u64::from(max_per_second)),
            next_at: Mutex::new(None),
            in_flight: Semaphore::new(usize::try_from(max_per_second).map_err(|_| {
                KeggError::InvalidParameter("max_per_second is too large".to_string())
            })?),
        })
    }

    pub(crate) async fn acquire(&self) {
        let permit = self
            .in_flight
            .acquire()
            .await
            .expect("rate-limiter semaphore is never closed");
        let wait = {
            let mut next_at = self.next_at.lock().await;
            let now = Instant::now();
            let wait = next_at.map_or(Duration::ZERO, |at| at.saturating_duration_since(now));
            let scheduled = now + wait + self.period;
            *next_at = Some(scheduled);
            wait
        };
        drop(permit);
        if !wait.is_zero() {
            tokio::time::sleep(wait).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::time::Instant;

    #[tokio::test]
    async fn spaces_four_concurrent_requests_at_three_per_second() {
        let limiter = Arc::new(RateLimiter::new(3).unwrap());
        let start = Instant::now();
        let mut tasks = tokio::task::JoinSet::new();
        for _ in 0..4 {
            let limiter = Arc::clone(&limiter);
            tasks.spawn(async move {
                limiter.acquire().await;
            });
        }
        while let Some(task) = tasks.join_next().await {
            task.unwrap();
        }
        assert!(start.elapsed() >= Duration::from_millis(330));
    }
}
