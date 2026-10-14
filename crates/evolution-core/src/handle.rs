use tokio::sync::mpsc;

use crate::worker::EvolutionCommand;

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

    /// Request one clustering pass. Never blocks: the channel is
    /// buffered, and a full channel drops the trigger (logged) rather
    /// than back-pressuring the caller.
    pub fn trigger_clustering(&self) {
        if let Err(error) = self.tx.try_send(EvolutionCommand::Clustering) {
            tracing::warn!(%error, "clustering trigger dropped");
        }
    }
}
