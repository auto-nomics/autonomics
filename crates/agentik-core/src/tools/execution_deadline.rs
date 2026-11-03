//! Execution limits are independent of a parent's observation timeout.
use std::time::Duration;

use super::ProgressBuffer;

/// Tools must explicitly select anything other than their existing wall limit.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ExecutionDeadline {
    #[default]
    WallTime,
    /// Expire only after this long without new structured work progress.
    ProgressInactivity,
    /// A separate owner supplies completion and execution admission/cancellation.
    /// Dropping this waiter does not establish that the owned work stopped.
    ExternallyManaged,
}

pub(crate) async fn execution_deadline(
    policy: ExecutionDeadline,
    seconds: u64,
    progress: &ProgressBuffer,
) {
    if policy == ExecutionDeadline::ExternallyManaged {
        std::future::pending::<()>().await;
        return;
    }
    let duration = Duration::from_secs(seconds);
    let started = tokio::time::Instant::now();
    let mut deadline = started + duration;
    loop {
        tokio::time::sleep_until(deadline).await;
        if policy == ExecutionDeadline::WallTime {
            return;
        }
        let last_progress = progress
            .lock()
            .ok()
            .and_then(|log| log.last_work_progress());
        match last_progress {
            Some(last) if last > started && last + duration > tokio::time::Instant::now() => {
                deadline = last + duration;
            }
            _ => return,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::{ProgressLog, ProgressRecord};
    use std::sync::{Arc, Mutex};
    use tokio_util::sync::CancellationToken;

    fn progress() -> ProgressBuffer {
        Arc::new(Mutex::new(ProgressLog::new()))
    }

    async fn tick(seconds: u64) {
        tokio::time::advance(Duration::from_secs(seconds)).await;
        tokio::task::yield_now().await;
    }

    #[tokio::test(start_paused = true)]
    async fn advancing_work_outlives_multiple_wall_budgets_then_expires_when_idle() {
        let progress = progress();
        let observed = progress.clone();
        let deadline = tokio::spawn(async move {
            execution_deadline(ExecutionDeadline::ProgressInactivity, 10, &observed).await;
        });
        tokio::task::yield_now().await;
        for current in 1..=6 {
            tick(9).await;
            progress.lock().unwrap().push(
                ProgressRecord::new("progress")
                    .label("fit")
                    .current(current)
                    .total(6),
            );
            tokio::task::yield_now().await;
            assert!(!deadline.is_finished());
        }
        tick(9).await;
        assert!(!deadline.is_finished());
        tick(1).await;
        assert!(deadline.is_finished());
        deadline.await.unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn repeated_logs_states_counters_and_resource_telemetry_do_not_extend_work() {
        let progress = progress();
        progress
            .lock()
            .unwrap()
            .push(ProgressRecord::new("status").label("fit").status("running"));
        progress
            .lock()
            .unwrap()
            .push(ProgressRecord::new("progress").label("fit").current(3));
        let observed = progress.clone();
        let deadline = tokio::spawn(async move {
            execution_deadline(ExecutionDeadline::ProgressInactivity, 10, &observed).await;
        });
        tokio::task::yield_now().await;
        for _ in 0..4 {
            tick(2).await;
            let mut log = progress.lock().unwrap();
            log.push(ProgressRecord::new("log").message("still working"));
            log.push(ProgressRecord::new("status").label("fit").status("running"));
            log.push(ProgressRecord::new("progress").label("fit").current(3));
            log.push(ProgressRecord::new("progress").label("fit").current(1));
            log.push(ProgressRecord::new("resource").current(12345));
        }
        tick(2).await;
        assert!(deadline.is_finished());
        deadline.await.unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn default_wall_limit_is_preserved_and_external_waiter_needs_explicit_cancel() {
        let progress = progress();
        let external_progress = progress.clone();
        let cancel = CancellationToken::new();
        let task_cancel = cancel.clone();
        let waiter = tokio::spawn(async move {
            tokio::select! {
                _ = execution_deadline(ExecutionDeadline::ExternallyManaged, 10, &external_progress) => panic!("child execution cannot expire from parent's observation"),
                _ = task_cancel.cancelled() => {}
            }
        });
        let wall_progress = progress.clone();
        let wall = tokio::spawn(async move {
            execution_deadline(ExecutionDeadline::WallTime, 10, &wall_progress).await;
        });
        tokio::task::yield_now().await;
        tick(9).await;
        progress
            .lock()
            .unwrap()
            .push(ProgressRecord::new("progress").current(1));
        tick(1).await;
        assert!(wall.is_finished());
        wall.await.unwrap();
        tick(100_000).await;
        assert!(!waiter.is_finished());
        cancel.cancel();
        tokio::task::yield_now().await;
        waiter.await.unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn explicit_cancel_wins_while_progress_is_valid() {
        let progress = progress();
        let observed = progress.clone();
        let cancel = CancellationToken::new();
        let task_cancel = cancel.clone();
        let waiter = tokio::spawn(async move {
            tokio::select! {
                _ = execution_deadline(ExecutionDeadline::ProgressInactivity, 10, &observed) => false,
                _ = task_cancel.cancelled() => true,
            }
        });
        tokio::task::yield_now().await;
        tick(9).await;
        progress
            .lock()
            .unwrap()
            .push(ProgressRecord::new("progress").current(1));
        cancel.cancel();
        assert!(waiter.await.unwrap());
    }
}
