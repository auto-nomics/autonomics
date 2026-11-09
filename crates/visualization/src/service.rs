//! Application operations shared by API and agent adapters.
use crate::{Revision, Store, Visualization, scene};

#[derive(Debug, thiserror::Error)]
pub enum ServiceError {
    #[error(transparent)]
    Store(#[from] crate::Error),
    #[error(transparent)]
    Scene(#[from] scene::SceneError),
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
    pub fn propose(
        &self,
        project_id: &str,
        id: &str,
        expected_revision: i64,
        summary: &str,
        operations: &[scene::Operation],
    ) -> Result<crate::proposal::Proposal, ServiceError> {
        let metadata = self.store.get(project_id, id)?;
        if metadata.revision != expected_revision {
            return Err(crate::Error::Conflict.into());
        }
        let current = self.store.revision(project_id, id, expected_revision)?;
        let document = scene::apply(&current.document, operations)?;
        Ok(self
            .store
            .propose(project_id, id, expected_revision, summary, &document)?)
    }
    /// Apply a batch against the exact scene revision observed by the caller.
    pub fn apply(
        &mut self,
        project_id: &str,
        id: &str,
        expected_revision: i64,
        operations: &[scene::Operation],
    ) -> Result<Visualization, ServiceError> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn rejected_batches_do_not_create_revisions() -> Result<(), ServiceError> {
        let mut store = Store::in_memory()?;
        store.create_project("p", "Project")?;
        let figure = store.create("p", "Figure", "excalidraw/v2", &json!({"elements":[]}))?;
        let mut service = VisualizationService::new(store);
        service.apply(
            "p",
            &figure.id,
            1,
            &[scene::Operation::Add {
                element: json!({"id":"cell","type":"ellipse"}),
            }],
        )?;
        assert!(matches!(
            service.apply("p", &figure.id, 1, &[]),
            Err(ServiceError::Store(crate::Error::Conflict))
        ));
        assert!(
            service
                .apply(
                    "p",
                    &figure.id,
                    2,
                    &[scene::Operation::Remove {
                        id: "missing".into()
                    }]
                )
                .is_err()
        );
        assert_eq!(service.current("p", &figure.id)?.revision, 2);
        assert_eq!(
            service.current("p", &figure.id)?.document["elements"][0]["id"],
            "cell"
        );
        Ok(())
    }
}
