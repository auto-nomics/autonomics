use std::time::{Duration, Instant};

/// Throttles byte-level callbacks to one event per interval (plus the first
/// event), preventing a fast local transfer from flooding progress logs.
#[derive(Debug)]
pub struct ProgressThrottle {
    interval: Duration,
    last: Option<Instant>,
}

impl ProgressThrottle {
    pub fn new(interval: Duration) -> Self {
        Self {
            interval,
            last: None,
        }
    }

    pub fn should_emit(&mut self) -> bool {
        let now = Instant::now();
        if self
            .last
            .is_none_or(|last| now.duration_since(last) >= self.interval)
        {
            self.last = Some(now);
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_event_emits_and_subsequent_events_are_throttled() {
        let mut throttle = ProgressThrottle::new(Duration::from_secs(1));
        assert!(throttle.should_emit());
        assert!(!throttle.should_emit());
    }
}
