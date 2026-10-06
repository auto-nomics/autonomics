//! Memory-guard sampling state and the abort-on-drop task handle.
//!
//! Both are scheduler-internal helpers (see [`super::scheduler`]); they live
//! apart from the scheduler loop only to keep that file focused on ordering.

use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::dag::runtime::{MemoryRunReport, ResourceRunReport};
use crate::resource::{MemoryGuardConfig, MemoryObservation, MemorySample, sample_memory_usage};

/// Cancels a still-live node task if the scheduler returns before the task.
///
/// Normal completed tasks ignore `abort`; a task dropped by the memory guard is
/// explicitly cancelled instead of continuing invisibly in the background.
pub(super) struct AbortOnDropHandle(pub(super) Option<JoinHandle<()>>);

impl AbortOnDropHandle {
    /// Consume the wrapper, returning the inner handle without firing the
    /// Drop-based `abort`. Callers take responsibility for joining (and may
    /// still abort explicitly). Returning `None` means the handle was
    /// already taken — `into_join` should only be called once.
    pub(super) fn into_join(mut self) -> JoinHandle<()> {
        self.0
            .take()
            .expect("AbortOnDropHandle::into_join called twice")
    }
}

impl Drop for AbortOnDropHandle {
    fn drop(&mut self) {
        if let Some(handle) = self.0.take() {
            handle.abort();
        }
    }
}

pub(super) struct MemoryGuardState {
    config: MemoryGuardConfig,
    sample_count: usize,
    peak: Option<MemorySample>,
    source: Option<&'static str>,
    error: Option<String>,
}

impl MemoryGuardState {
    pub(super) fn new(config: MemoryGuardConfig) -> Self {
        Self {
            config,
            sample_count: 0,
            peak: None,
            source: None,
            error: None,
        }
    }

    pub(super) fn record(&mut self, sample: MemorySample) -> Option<MemorySample> {
        self.sample_count += 1;
        self.source = Some(sample.source);
        let triggered = sample.ratio() >= self.config.threshold_ratio;
        if self
            .peak
            .as_ref()
            .is_none_or(|peak| peak.usage_bytes < sample.usage_bytes)
        {
            self.peak = Some(sample);
        }
        triggered.then_some(sample)
    }

    pub(super) fn unavailable(&mut self, error: String) {
        self.error.get_or_insert(error);
    }

    pub(super) fn into_report(self, trigger: Option<MemorySample>) -> ResourceRunReport {
        let memory = MemoryRunReport {
            enabled: true,
            source: self.source,
            threshold_ratio: Some(self.config.threshold_ratio),
            sample_interval_ms: Some(
                self.config
                    .sample_interval
                    .as_millis()
                    .min(u64::MAX as u128) as u64,
            ),
            sample_count: self.sample_count,
            peak: self.peak.as_ref().map(Into::into),
            trigger: trigger.as_ref().map(Into::into),
            error: self.error,
        };
        ResourceRunReport { memory }
    }
}

/// Sample process memory on an interval until the sender is dropped or the
/// threshold trips, then stop after delivering the triggering sample.
pub(super) async fn run_memory_monitor(
    config: MemoryGuardConfig,
    tx: mpsc::Sender<MemoryObservation>,
) {
    loop {
        match sample_memory_usage() {
            Ok(sample) => {
                let triggered = sample.ratio() >= config.threshold_ratio;
                if tx.send(MemoryObservation::Sample(sample)).await.is_err() {
                    break;
                }
                if triggered {
                    break;
                }
            }
            Err(error) => {
                let _ = tx.send(MemoryObservation::Unavailable(error)).await;
                break;
            }
        }
        tokio::time::sleep(config.sample_interval).await;
    }
}
