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

#[derive(Debug, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProposalRevisionInput {
    pub parent_proposal_id: String,
    pub parent_document_sha256: String,
    pub summary: String,
    pub operations: Option<Vec<scene::Operation>>,
    pub arrangement: Option<crate::arrange::Spec>,
    pub markup_ids: Option<Vec<String>>,
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
        self.propose_with_markups(project_id, id, expected_revision, summary, operations, &[])
    }
    pub fn propose_with_markups(
        &self,
        project_id: &str,
        id: &str,
        expected_revision: i64,
        summary: &str,
        operations: &[scene::Operation],
        markup_ids: &[String],
    ) -> Result<crate::proposal::Proposal, ServiceError> {
        let metadata = self.store.get(project_id, id)?;
        if metadata.revision != expected_revision {
            return Err(crate::Error::Conflict.into());
        }
        let current = self.store.revision(project_id, id, expected_revision)?;
        let document = scene::apply(&current.document, operations)?;
        Ok(self.store.propose_with_markups(
            project_id,
            id,
            expected_revision,
            summary,
            &document,
            markup_ids,
        )?)
    }
    pub fn revise_proposal(
        &self,
        project: &str,
        id: &str,
        expected_revision: i64,
        input: &ProposalRevisionInput,
    ) -> Result<crate::proposal::Proposal, ServiceError> {
        if self.store.get(project, id)?.revision != expected_revision {
            return Err(crate::Error::Conflict.into());
        }
        let parent = self
            .store
            .proposal(project, id, &input.parent_proposal_id)?;
        if parent.status != "pending"
            || parent.base_revision != expected_revision
            || parent.document_sha256 != input.parent_document_sha256
        {
            return Err(crate::Error::Conflict.into());
        }
        if input.markup_ids.is_none() && parent.review_markups.iter().any(|m| m.status == "open") {
            return Err(crate::Error::Invalid(
                "explicit markup_ids required to retain or drop the parent's open review links",
            )
            .into());
        }
        let document = match (&input.operations, &input.arrangement) {
            (Some(ops), None) if ops.len() <= 1000 => scene::apply(&parent.document, ops)?,
            (None, Some(spec)) => crate::arrange::compile(&parent.document, spec)?,
            _ => {
                return Err(crate::Error::Invalid(
                    "exactly one operations batch (at most 1000) or arrangement is required",
                )
                .into());
            }
        };
        Ok(self.store.propose_revision(
            project,
            id,
            expected_revision,
            crate::proposal::ProposedRevision {
                parent_id: &parent.id,
                parent_document_sha256: &parent.document_sha256,
                summary: &input.summary,
                document: &document,
                markup_ids: input.markup_ids.as_deref().unwrap_or(&[]),
            },
        )?)
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
