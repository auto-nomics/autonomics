use std::sync::Arc;

use tokio::sync::{mpsc, oneshot};

use crate::error::Result;
use crate::{Observation, ObservationInput};
use crate::{ObservationHub, cluster::ClusterReport, handle::EvolutionHandle};

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
    let worker = EvolutionWorker::new(rx, observation_hub);
    tokio::spawn(worker.run());
    EvolutionHandle::new(tx)
}

pub struct EvolutionWorker {
    rx: mpsc::Receiver<EvolutionCommand>,
    observation_hub: Arc<ObservationHub>,
}

impl EvolutionWorker {
    pub fn new(rx: mpsc::Receiver<EvolutionCommand>, observation_hub: Arc<ObservationHub>) -> Self {
        Self {
            rx,
            observation_hub,
        }
    }

    /// The control loop: one command at a time, so clustering passes
    /// are strictly serialized. Exits when the last handle drops and
    /// the channel closes.
    async fn run(mut self) {
        while let Some(command) = self.rx.recv().await {
            match command {
                EvolutionCommand::Clustering { reply } => {
                    let outcome = self.run_clustering().await;
                    // The log is the record when nobody awaits the
                    // reply (fire-and-forget triggers drop the
                    // receiver); the reply serves the caller that does.
                    match &outcome {
                        Ok(report) => tracing::info!(
                            assigned = report.assigned,
                            surfaced = report.surfaced.len(),
                            merged = report.merged,
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
                    let _ = reply.send(observation_result);
                }
            }
        }
        tracing::debug!("evolution worker exiting: control channel closed");
    }

    /// One incremental clustering pass under the current embedder
    /// signature. A failure is logged, never fatal — the loop
    /// survives and the next trigger retries.
    async fn run_clustering(&self) -> Result<ClusterReport> {
        let signature = self.observation_hub.embedding_signature();
        self.observation_hub
            .clusterer()
            .execute_cluster(&signature)
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
}
