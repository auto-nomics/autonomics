use std::time::Duration;
use std::time::Instant;

use tokio::sync::Mutex;

/// Small deterministic minimum-interval limiter shared by cloned clients.
#[derive(Debug)]
pub(crate) struct RateLimiter {
    period: Duration,
    next_at: Mutex<Option<Instant>>,
}

impl RateLimiter {
    pub(crate) fn from_period(period: Duration) -> Self {
        Self {
            period,
            next_at: Mutex::new(None),
        }
    }

    pub(crate) async fn acquire(&self) {
        let mut next_at = self.next_at.lock().await;
        let now = Instant::now();
        let wait = next_at.map_or(Duration::ZERO, |at| at.saturating_duration_since(now));
        *next_at = Some(now + wait + self.period);
        drop(next_at);
        if !wait.is_zero() {
            tokio::time::sleep(wait).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn first_call_does_not_wait() {
        let started = std::time::Instant::now();
        RateLimiter::from_period(Duration::from_secs(60))
            .acquire()
            .await;
        assert!(started.elapsed() < Duration::from_millis(100));
    }
}
