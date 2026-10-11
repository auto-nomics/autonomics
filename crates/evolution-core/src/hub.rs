//! The observation hub: the handle every consumer shares for
//! observation evidence.
//!
//! Observations are shared evidence — skills and plugins are both
//! downstream consumers. Neither owns the ledger; this hub does. It
//! is constructed once per process by the host assembly and passed by
//! reference (the skill manager and plugin RSI each keep their own
//! `Arc`); the state directory remains the cross-process truth, so a
//! CLI and a daemon pointed at the same directory observe the same
//! ledger without any in-process singleton.
//!
//! Since the Turso migration the hub is backed by
//! `<state_dir>/evolution.db` (records plus cached vectors); the file
//! backend remains available for tests via [`ObservationHub::with_storage`].

use std::path::PathBuf;
use std::sync::Arc;

use crate::error::Result;
use crate::observation::{Observation, ObservationInput};
use crate::routing::ObservationRouteStore;
use crate::storage::{FileObservationStorage, ObservationStorage, TursoObservationStorage};

/// The shared handle for observation evidence and its storage.
#[derive(Clone)]
pub struct ObservationHub {
    state_dir: PathBuf,
    storage: Arc<dyn ObservationStorage>,
}

impl ObservationHub {
    /// Production hub over `<state_dir>/evolution.db`. No I/O happens
    /// here — the database is built lazily on first use.
    #[must_use]
    pub fn new(state_dir: impl Into<PathBuf>) -> Self {
        let state_dir = state_dir.into();
        let storage = Arc::new(TursoObservationStorage::new(state_dir.clone()));
        Self { state_dir, storage }
    }

    /// Hub over a caller-supplied storage backend (tests inject the
    /// file backend here; the database backend is the default).
    #[must_use]
    pub fn with_storage(
        state_dir: impl Into<PathBuf>,
        storage: Arc<dyn ObservationStorage>,
    ) -> Self {
        Self {
            state_dir: state_dir.into(),
            storage,
        }
    }

    /// Hub over the legacy file backend, files only — no database is
    /// created or consulted.
    #[must_use]
    pub fn file_backed(state_dir: impl Into<PathBuf>) -> Self {
        let state_dir = state_dir.into();
        Self::with_storage(
            state_dir.clone(),
            Arc::new(FileObservationStorage::new(&state_dir)),
        )
    }

    /// The state directory every store below is rooted at.
    pub fn state_dir(&self) -> &std::path::Path {
        &self.state_dir
    }

    /// The durable storage behind this hub.
    pub fn storage(&self) -> &Arc<dyn ObservationStorage> {
        &self.storage
    }

    /// The route ledger under this hub's state dir. Routing decisions
    /// are still file-backed; see the storage module for the database
    /// migration of observation records.
    pub fn routes(&self) -> ObservationRouteStore {
        ObservationRouteStore::open(&self.state_dir)
    }

    /// Record one observation (idempotent on content). This is the
    /// canonical entry every producer lands on — agent tools,
    /// automatic failure capture, eval capture, and plugin feedback.
    ///
    /// Downstream notification is the caller's concern: the hub is a
    /// pure ledger and knows nothing about its consumers.
    pub async fn record(&self, input: ObservationInput) -> Result<Observation> {
        self.storage.record(input).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::observation::{ObservationKind, ObservationSource};

    #[tokio::test]
    async fn record_is_visible_through_the_shared_storage() {
        let tmp = tempfile::tempdir().unwrap();
        let hub = ObservationHub::new(tmp.path());
        let observation = hub
            .record(ObservationInput {
                kind: ObservationKind::Failure,
                source: ObservationSource::Agent,
                summary: "summary".into(),
                body: "body".into(),
                node_kind: None,
                error: None,
            })
            .await
            .unwrap();
        let listed = hub.storage().list().await;
        assert!(listed.iter().any(|o| o.id == observation.id));
    }

    /// The DB-backed hub materializes evolution.db under the state
    /// dir on first use — the state directory remains the portable
    /// unit of deployment.
    #[tokio::test]
    async fn default_backend_is_the_database() {
        let tmp = tempfile::tempdir().unwrap();
        let hub = ObservationHub::new(tmp.path());
        hub.storage()
            .list()
            .await
            .iter()
            .for_each(drop);
        assert!(tmp.path().join(crate::EVOLUTION_DB).is_file());
    }
}
