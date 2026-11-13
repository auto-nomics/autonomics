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
    /// Copy the reviewed immediate predecessor into a new revision of the current
    /// candidate. Both immutable snapshots remain intact; no opinion links carry.
    pub fn restore_candidate_parent(
        &self,
        project: &str,
        id: &str,
        expected_revision: i64,
        proposal_id: &str,
        document_sha256: &str,
        source_document_sha256: &str,
    ) -> Result<crate::proposal::Proposal, ServiceError> {
        if self.store.get(project, id)?.revision != expected_revision {
            return Err(crate::Error::Conflict.into());
        }
        let current = self.store.proposal(project, id, proposal_id)?;
        if current.status != "pending"
            || current.base_revision != expected_revision
            || current.document_sha256 != document_sha256
        {
            return Err(crate::Error::Conflict.into());
        }
        let source_id = current
            .parent_proposal_id
            .as_deref()
            .ok_or(crate::Error::Invalid(
                "candidate has no previous candidate to restore",
            ))?;
        if current.parent_document_sha256.as_deref() != Some(source_document_sha256) {
            return Err(crate::Error::Conflict.into());
        }
        let source = self.store.proposal(project, id, source_id)?;
        if source.base_revision != expected_revision
            || source.document_sha256 != source_document_sha256
        {
            return Err(crate::Error::Conflict.into());
        }
        Ok(self.store.propose_revision(
            project,
            id,
            expected_revision,
            crate::proposal::ProposedRevision {
                parent_id: &current.id,
                parent_document_sha256: &current.document_sha256,
                summary: "以上一版候选内容创建修订",
                document: &source.document,
                markup_ids: &[],
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
    #[test]
    fn candidate_revision_preserves_parent_and_adopts_combined_changes() -> Result<(), ServiceError>
    {
        let mut store = Store::in_memory()?;
        store.create_project("p", "Study")?;
        let document = json!({"elements":[
            {"id":"original","type":"rectangle","x":10,"y":0,"width":20,"height":20,"angle":0}
        ],"files":{"asset":{"dataURL":"unchanged","mimeType":"image/png"}}});
        let figure = store.create("p", "Figure", "excalidraw/v2", &document)?;
        let mut service = VisualizationService::new(store);
        let parent = service.propose("p", &figure.id, 1, "Add component", &[
            scene::Operation::Add { element: json!({"id":"added","type":"rectangle","x":80,"y":40,"width":20,"height":20,"angle":0}) }
        ])?;
        let input = ProposalRevisionInput {
            parent_proposal_id: parent.id.clone(),
            parent_document_sha256: parent.document_sha256.clone(),
            summary: "Align candidate components".into(),
            operations: None,
            arrangement: Some(crate::arrange::Spec {
                element_ids: vec!["original".into(), "added".into()],
                mode: crate::arrange::Mode::Left,
            }),
            markup_ids: None,
        };
        let child = service.revise_proposal("p", &figure.id, 1, &input)?;
        assert_eq!(
            child.parent_proposal_id.as_deref(),
            Some(parent.id.as_str())
        );
        assert_eq!(
            child.parent_document_sha256.as_deref(),
            Some(parent.document_sha256.as_str())
        );
        assert_eq!(child.document["elements"][1]["x"], 10.0);
        assert_eq!(child.document["files"], document["files"]);
        assert_eq!(service.current("p", &figure.id)?.document, document);
        let reread = service.store().proposal("p", &figure.id, &parent.id)?;
        assert_eq!(reread.document, parent.document);
        assert_eq!(reread.document_sha256, parent.document_sha256);
        let mut wrong = input;
        wrong.parent_document_sha256 = "wrong".into();
        assert!(matches!(
            service.revise_proposal("p", &figure.id, 1, &wrong),
            Err(ServiceError::Store(crate::Error::Conflict))
        ));
        assert_eq!(service.store().proposals("p", &figure.id)?.len(), 2);
        service
            .store_mut()
            .accept_proposal("p", &figure.id, &child.id)?;
        assert_eq!(service.current("p", &figure.id)?.revision, 2);
        assert_eq!(service.current("p", &figure.id)?.document, child.document);
        assert_eq!(
            service
                .store()
                .proposal("p", &figure.id, &parent.id)?
                .status,
            "pending"
        );
        wrong.parent_document_sha256 = parent.document_sha256.clone();
        assert!(matches!(
            service.revise_proposal("p", &figure.id, 2, &wrong),
            Err(ServiceError::Store(crate::Error::Conflict))
        ));
        assert!(
            service
                .store_mut()
                .accept_proposal("p", &figure.id, &parent.id)
                .is_err()
        );
        Ok(())
    }

    #[test]
    fn candidate_revision_requires_explicit_review_link_decision() -> Result<(), ServiceError> {
        let mut store = Store::in_memory()?;
        store.create_project("p", "Study")?;
        let figure = store.create("p", "Figure", "excalidraw/v2", &json!({"elements":[]}))?;
        let ink = [crate::markup::Stroke {
            kind: crate::markup::Kind::Cross,
            points: vec![[0., 0.], [10., 10.]],
        }];
        let png = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVQIHWP4z8DwHwAFgAI/ScLbtAAAAABJRU5ErkJggg==";
        let review = store.create_markup("p", &figure.id, 1, "Revise", &ink, png)?;
        let service = VisualizationService::new(store);
        let parent = service.propose_with_markups(
            "p",
            &figure.id,
            1,
            "Parent",
            &[],
            std::slice::from_ref(&review.id),
        )?;
        let mut input = ProposalRevisionInput {
            parent_proposal_id: parent.id.clone(),
            parent_document_sha256: parent.document_sha256.clone(),
            summary: "Child".into(),
            operations: Some(vec![]),
            arrangement: None,
            markup_ids: None,
        };
        assert!(service.revise_proposal("p", &figure.id, 1, &input).is_err());
        assert_eq!(service.store().proposals("p", &figure.id)?.len(), 1);
        input.markup_ids = Some(vec!["missing-review".into()]);
        assert!(service.revise_proposal("p", &figure.id, 1, &input).is_err());
        assert_eq!(service.store().proposals("p", &figure.id)?.len(), 1);
        input.markup_ids = Some(vec![]);
        assert!(
            service
                .revise_proposal("p", &figure.id, 1, &input)?
                .review_markups
                .is_empty()
        );
        input.markup_ids = Some(vec![review.id.clone()]);
        let child = service.revise_proposal("p", &figure.id, 1, &input)?;
        assert_eq!(child.review_markups[0].id, review.id);
        assert_eq!(child.review_markups[0].status, "open");
        service
            .store()
            .reject_proposal("p", &figure.id, &parent.id)?;
        assert!(matches!(
            service.revise_proposal("p", &figure.id, 1, &input),
            Err(ServiceError::Store(crate::Error::Conflict))
        ));
        assert_eq!(
            service.store().proposal("p", &figure.id, &child.id)?.status,
            "pending"
        );
        Ok(())
    }
}
