use std::sync::Arc;

use tokio::sync::{mpsc, oneshot};

use crate::error::Result;
use crate::{Observation, ObservationInput};
use crate::{
    ObservationHub,
    cluster::{ClusterReport, DEFAULT_TRIGGER_THRESHOLD},
    handle::EvolutionHandle,
};

/// Commands the evolution worker consumes from its control channel.
pub enum EvolutionCommand {
    /// Run one incremental clustering pass over the observation pool.
    /// The reply carries the pass outcome; dropping the receiver
    /// turns the trigger into fire-and-forget.
    Clustering {
        reply: oneshot::Sender<Result<ClusterReport>>,
    },
    RecordObservation {
        observation: ObservationInput,
        reply: oneshot::Sender<Result<Observation>>,
    },
}

/// Spawn the worker and return its control handle. Must be called
/// from a tokio runtime context — `tokio::spawn` panics otherwise.
pub fn start(observation_hub: Arc<ObservationHub>) -> EvolutionHandle {
    // Capacity 128: a clustering pass is slow relative to its
    // triggers, and a trigger burst must not push commands out the
    // back door.
    let (tx, rx) = mpsc::channel::<EvolutionCommand>(128);
    let worker = EvolutionWorker::new(tx.clone(), rx, observation_hub);
    tokio::spawn(worker.run());
    EvolutionHandle::new(tx)
}

pub struct EvolutionWorker {
    rx: mpsc::Receiver<EvolutionCommand>,
    /// Self-addressed copy of the control channel sender: recording
    /// an observation schedules a follow-up clustering pass through
    /// the same serialized loop instead of running it inline (which
    /// would hold the record's reply hostage for a full pass).
    tx: mpsc::Sender<EvolutionCommand>,
    observation_hub: Arc<ObservationHub>,
    /// True while a self-scheduled clustering command sits queued in
    /// the channel. The flag coalesces a record burst into one pass:
    /// the first record enqueues the command, the rest see the flag
    /// and skip; the pass itself clears it when it dequeues.
    clustering_scheduled: bool,
}

impl EvolutionWorker {
    pub fn new(
        tx: mpsc::Sender<EvolutionCommand>,
        rx: mpsc::Receiver<EvolutionCommand>,
        observation_hub: Arc<ObservationHub>,
    ) -> Self {
        Self {
            tx,
            rx,
            observation_hub,
            clustering_scheduled: false,
        }
    }

    /// The control loop: one command at a time, so clustering passes
    /// are strictly serialized. Exits when the last handle drops and
    /// the channel closes.
    async fn run(mut self) {
        while let Some(command) = self.rx.recv().await {
            match command {
                EvolutionCommand::Clustering { reply } => {
                    self.clustering_scheduled = false;
                    let outcome = self.run_clustering().await;
                    // The log is the record when nobody awaits the
                    // reply (fire-and-forget triggers drop the
                    // receiver); the reply serves the caller that does.
                    match &outcome {
                        Ok(report) => tracing::info!(
                            assigned = report.assigned,
                            surfaced = report.surfaced.len(),
                            merged = report.merged,
                            changed = report.changes.len(),
                            "clustering pass finished"
                        ),
                        Err(error) => tracing::warn!(%error, "clustering pass failed"),
                    }
                    // A send error only means the receiver was
                    // dropped — fire-and-forget, nothing to report to.
                    let _ = reply.send(outcome);
                }
                EvolutionCommand::RecordObservation { observation, reply } => {
                    let observation_result = self.observation_hub.record(observation).await;
                    // Reply BEFORE scheduling: the caller's record
                    // latency must not include a clustering pass.
                    let _ = reply.send(observation_result);
                    self.schedule_clustering();
                }
            }
        }
        tracing::debug!("evolution worker exiting: control channel closed");
    }

    /// Enqueue a fire-and-forget clustering pass unless one is
    /// already queued. A full channel drops the trigger and resets
    /// the flag — the next record re-arms it, so no pass is lost
    /// forever, just deferred.
    fn schedule_clustering(&mut self) {
        if !self.clustering_scheduled {
            let (tx_drop, _) = oneshot::channel();
            let command = EvolutionCommand::Clustering { reply: tx_drop };
            if self.tx.try_send(command).is_ok() {
                self.clustering_scheduled = true;
            }
        }
    }

    /// One incremental clustering pass under the current embedder
    /// signature. A failure is logged, never fatal — the loop
    /// survives and the next trigger retries.
    async fn run_clustering(&self) -> Result<ClusterReport> {
        let signature = self.observation_hub.embedding_signature();
        self.observation_hub
            .clusterer()
            .clustering(&signature, DEFAULT_TRIGGER_THRESHOLD)
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::observation::{ObservationInput, ObservationKind, ObservationSource};

    async fn observe(hub: &ObservationHub, summary: &str) {
        hub.record(ObservationInput {
            kind: ObservationKind::Failure,
            source: ObservationSource::Agent,
            summary: summary.into(),
            body: "cast to f64".into(),
            node_kind: Some("sql".into()),
            error: Some("bad type".into()),
        })
        .await
        .unwrap();
    }

    /// The trigger survives the buffered channel and one pass
    /// persists clusters: the spawned worker runs while the test's
    /// awaits yield (current-thread runtime — no ordering guarantee
    /// to lean on, only the side effect).
    #[tokio::test]
    async fn clustering_trigger_persists_clusters() {
        let tmp = tempfile::tempdir().unwrap();
        let hub = Arc::new(ObservationHub::new(tmp.path()));
        for i in 0..3 {
            observe(&hub, &format!("fix for error {i}")).await;
        }

        let handle = start(Arc::clone(&hub));
        handle.trigger_clustering();

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while hub.storage().load_clusters().await.unwrap().is_empty() {
            assert!(
                std::time::Instant::now() < deadline,
                "clustering pass never ran"
            );
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    }

    /// Recording through the handle auto-triggers a clustering pass
    /// — no explicit trigger needed — and the record reply arrives
    /// before the pass runs (the caller is never held hostage).
    #[tokio::test]
    async fn recording_auto_triggers_clustering() {
        let tmp = tempfile::tempdir().unwrap();
        let hub = Arc::new(ObservationHub::new(tmp.path()));
        let handle = start(Arc::clone(&hub));
        for i in 0..3 {
            observe(&hub, &format!("fix for error {i}")).await;
            handle
                .record_observation(ObservationInput {
                    kind: ObservationKind::Failure,
                    source: ObservationSource::Agent,
                    summary: format!("recorded {i}"),
                    body: "cast to f64".into(),
                    node_kind: Some("sql".into()),
                    error: Some("bad type".into()),
                })
                .await
                .unwrap();
        }

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while hub.storage().load_clusters().await.unwrap().is_empty() {
            assert!(
                std::time::Instant::now() < deadline,
                "auto-triggered clustering pass never ran"
            );
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    }
}
