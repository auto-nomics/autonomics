use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{Notify, watch};

use super::{KnowledgeJobKind, KnowledgePipeline};
use crate::now_ms;

#[derive(Debug, Clone)]
pub struct KnowledgeWorkerConfig {
    pub batch_limit: usize,
    pub poll_interval: Duration,
}

impl Default for KnowledgeWorkerConfig {
    fn default() -> Self {
        Self {
            batch_limit: 16,
            poll_interval: Duration::from_secs(30),
        }
    }
}

pub struct KnowledgeWorker {
    pipeline: Arc<KnowledgePipeline>,
    kind: KnowledgeJobKind,
    label: &'static str,
    config: KnowledgeWorkerConfig,
    wake: Arc<Notify>,
    shutdown_rx: watch::Receiver<bool>,
    status: Arc<std::sync::Mutex<KnowledgeWorkerStatus>>,
}

impl KnowledgeWorker {
    async fn run(mut self) {
        loop {
            if *self.shutdown_rx.borrow() {
                break;
            }

            self.set_running(true);
            let result = self
                .pipeline
                .run_due_for_kind(self.kind, self.config.batch_limit)
                .await;
            self.set_running(false);

            let mut continue_draining = false;
            match result {
                Ok(summary) => {
                    let mut status = self.lock_status();
                    status.total_claimed += u64::try_from(summary.claimed).unwrap_or(u64::MAX);
                    status.total_executed += u64::try_from(summary.executed).unwrap_or(u64::MAX);
                    status.total_released += u64::try_from(summary.released).unwrap_or(u64::MAX);
                    status.last_run_at = Some(now_ms());
                    status.last_error = None;
                    continue_draining = summary.executed > 0;
                }
                Err(error) => {
                    let mut status = self.lock_status();
                    status.last_run_at = Some(now_ms());
                    status.last_error = Some(error.to_string());
                }
            }

            if continue_draining {
                continue;
            }
            if !*self.shutdown_rx.borrow() {
                tokio::select! {
                    _ = self.wake.notified() => {}
                    _ = tokio::time::sleep(self.config.poll_interval) => {}
                    changed = self.shutdown_rx.changed() => {
                        if changed.is_ok() && *self.shutdown_rx.borrow() {
                            break;
                        }
                    }
                }
            }
        }

        self.set_running(false);
        tracing::debug!(worker = self.label, "knowledge worker stopped");
    }

    fn lock_status(&self) -> std::sync::MutexGuard<'_, KnowledgeWorkerStatus> {
        self.status
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
    }

    fn set_running(&self, running: bool) {
        self.lock_status().running = running;
    }
}

#[derive(Clone)]
pub struct KnowledgeWorkerHandle {
    wake: Arc<Notify>,
    shutdown_tx: watch::Sender<bool>,
    status: Arc<std::sync::Mutex<KnowledgeWorkerStatus>>,
}

impl KnowledgeWorkerHandle {
    pub fn wake(&self) {
        self.wake.notify_one();
    }

    pub fn request_shutdown(&self) {
        let _ = self.shutdown_tx.send(true);
        self.wake.notify_one();
    }

    #[must_use]
    pub fn status(&self) -> KnowledgeWorkerStatus {
        self.status
            .lock()
            .map(|status| status.clone())
            .unwrap_or_default()
    }
}

#[derive(Debug, Clone, Default)]
pub struct KnowledgeWorkerStatus {
    pub running: bool,
    pub last_run_at: Option<i64>,
    pub last_error: Option<String>,
    pub total_claimed: u64,
    pub total_executed: u64,
    pub total_released: u64,
}

pub fn start_knowledge_worker(
    pipeline: Arc<KnowledgePipeline>,
    kind: KnowledgeJobKind,
    label: &'static str,
    config: KnowledgeWorkerConfig,
) -> KnowledgeWorkerHandle {
    let wake = Arc::new(Notify::new());
    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let status = Arc::new(std::sync::Mutex::new(KnowledgeWorkerStatus::default()));
    let worker = KnowledgeWorker {
        pipeline,
        kind,
        label,
        config,
        wake: Arc::clone(&wake),
        shutdown_rx,
        status: Arc::clone(&status),
    };
    tokio::spawn(worker.run());
    KnowledgeWorkerHandle {
        wake,
        shutdown_tx,
        status,
    }
}
