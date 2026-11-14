//! Application operations shared by API and agent adapters.
use crate::{Revision, Store, Visualization, scene};

#[derive(Debug, thiserror::Error)]
pub enum ServiceError {
    #[error(transparent)]
    Store(#[from] crate::Error),
    #[error(transparent)]
    Scene(#[from] scene::SceneError),
    #[error(transparent)]
    Arrangement(#[from] crate::arrange::InvalidArrangement),
}

pub struct VisualizationService {
    store: Store,
}
impl VisualizationService {
    /// Storage location is selected by the host, never implicitly by the service.
    pub fn new(store: Store) -> Self {
        Self { store }
    }
    pub fn store(&self) -> &Store {
        &self.store
    }
    pub fn store_mut(&mut self) -> &mut Store {
        &mut self.store
    }
    pub fn current(&self, project_id: &str, id: &str) -> crate::Result<Revision> {
        let metadata = self.store.get(project_id, id)?;
        self.store.revision(project_id, id, metadata.revision)
    }
    /// Apply a batch against the exact scene revision observed by the caller.
    pub fn apply(
        &mut self,
        project_id: &str,
        id: &str,
        expected_revision: i64,
        operations: &[scene::Operation],
    ) -> Result<Visualization, ServiceError> {
        if operations.len() > 1000 {
            return Err(crate::Error::Invalid("at most 1000 operations per batch").into());
        }
        let metadata = self.store.get(project_id, id)?;
        if metadata.revision != expected_revision {
            return Err(crate::Error::Conflict.into());
        }
        let current = self.store.revision(project_id, id, expected_revision)?;
        let document = scene::apply(&current.document, operations)?;
        Ok(self.store.save(
            project_id,
            id,
            expected_revision,
            &metadata.title,
            &document,
        )?)
    }
}
