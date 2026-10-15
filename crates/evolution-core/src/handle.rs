use tokio::sync::{mpsc, oneshot};

use crate::error::{Error, Result};
use crate::{Observation, ObservationInput, cluster::ClusterReport, worker::EvolutionCommand};

/// Control handle for the evolution worker. Cloning shares the same
/// worker; dropping the last handle closes the channel and the
/// worker exits after its current pass.
#[derive(Clone)]
pub struct EvolutionHandle {
    tx: mpsc::Sender<EvolutionCommand>,
}

impl EvolutionHandle {
    pub(crate) fn new(tx: mpsc::Sender<EvolutionCommand>) -> Self {
        Self { tx }
    }

    pub async fn record_observation(&self, obs_input: ObservationInput) -> Result<Observation> {
        let (tx, rx) = oneshot::channel::<Result<Observation>>();
        if let Err(error) = self
            .tx
            .send(EvolutionCommand::RecordObservation {
                observation: obs_input,
                reply: tx,
            })
            .await
        {
            tracing::error!(%error, "send command to record observation failed");
        }
        rx.await.map_err(|e| Error::WorkerGone(e.to_string()))?
    }

    /// Request one clustering pass. Never blocks: the channel is
    /// buffered, and a full channel drops the trigger (logged) rather
    /// than back-pressuring the caller.
    pub fn trigger_clustering(&self) {
        let (tx, _) = oneshot::channel::<Result<ClusterReport>>();
        if let Err(error) = self.tx.try_send(EvolutionCommand::Clustering { reply: tx }) {
            tracing::warn!(%error, "clustering trigger dropped");
        }
    }
}
