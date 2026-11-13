use serde_json::json;
use visualization::{Store, proposal::ProposedRevision, service::VisualizationService};
fn fixture() -> (
    VisualizationService,
    String,
    visualization::proposal::Proposal,
    visualization::proposal::Proposal,
) {
    let mut store = Store::in_memory()
        .unwrap()
        .with_linear_candidate_revisions(true);
    store.create_project("p", "Study").unwrap();
    let original = json!({"elements":[{"id":"note","type":"text","text":"Reviewed original","customData":{"evidence":[{"doi":"fixture","unshown":"preserve"}]}}],"files":{"source":{"dataURL":"private-source-bytes"}},"historical":{"version":1}});
    let figure = store
        .create("p", "Figure", "excalidraw/v2", &json!({"elements":[]}))
        .unwrap();
    let parent = store
        .propose("p", &figure.id, 1, "Reviewed candidate", &original)
        .unwrap();
    let mut edited = original.clone();
    edited["elements"][0]["text"] = json!("Changed text");
    edited["files"]["extra"] = json!({"dataURL":"new-artwork"});
    let child = store
        .propose_revision(
            "p",
            &figure.id,
            1,
            ProposedRevision {
                parent_id: &parent.id,
                parent_document_sha256: &parent.document_sha256,
                summary: "Edited candidate",
                document: &edited,
                markup_ids: &[],
            },
        )
        .unwrap();
    (VisualizationService::new(store), figure.id, parent, child)
}
#[test]
fn restore_creates_cumulative_child_with_exact_previous_content_and_keeps_history() {
    let (service, id, parent, child) = fixture();
    let restored = service
        .restore_candidate_parent(
            "p",
            &id,
            1,
            &child.id,
            &child.document_sha256,
            &parent.document_sha256,
        )
        .unwrap();
    assert_eq!(restored.document, parent.document);
    assert_eq!(restored.document_sha256, parent.document_sha256);
    assert_eq!(
        restored.parent_proposal_id.as_deref(),
        Some(child.id.as_str())
    );
    assert_eq!(
        restored.parent_document_sha256.as_deref(),
        Some(child.document_sha256.as_str())
    );
    assert!(restored.review_markups.is_empty());
    assert_eq!(
        service
            .store()
            .proposal("p", &id, &parent.id)
            .unwrap()
            .document,
        parent.document
    );
    assert_eq!(
        service
            .store()
            .proposal("p", &id, &child.id)
            .unwrap()
            .document,
        child.document
    );
    assert_eq!(
        service.current("p", &id).unwrap().document,
        json!({"elements":[]})
    );
    assert!(
        service
            .restore_candidate_parent(
                "p",
                &id,
                1,
                &child.id,
                &child.document_sha256,
                &parent.document_sha256
            )
            .unwrap_err()
            .to_string()
            .contains("pending child")
    );
    assert_eq!(service.store().proposals("p", &id).unwrap().len(), 3);
}
#[test]
fn restore_rejects_root_wrong_snapshot_scope_and_stale_head_without_partial_writes() {
    let (mut service, id, parent, child) = fixture();
    for (candidate, hash, source) in [
        (&parent.id, &parent.document_sha256, &parent.document_sha256),
        (&child.id, &child.document_sha256, &child.document_sha256),
        (&child.id, &parent.document_sha256, &parent.document_sha256),
    ] {
        assert!(
            service
                .restore_candidate_parent("p", &id, 1, candidate, hash, source)
                .is_err()
        );
    }
    let other = service
        .store_mut()
        .create("p", "Other", "excalidraw/v2", &json!({"elements":[]}))
        .unwrap();
    assert!(
        service
            .restore_candidate_parent(
                "p",
                &other.id,
                1,
                &child.id,
                &child.document_sha256,
                &parent.document_sha256
            )
            .is_err()
    );
    assert_eq!(service.store().proposals("p", &id).unwrap().len(), 2);
    service
        .store_mut()
        .save("p", &id, 1, "External save", &json!({"elements":[]}))
        .unwrap();
    assert!(
        service
            .restore_candidate_parent(
                "p",
                &id,
                1,
                &child.id,
                &child.document_sha256,
                &parent.document_sha256
            )
            .is_err()
    );
    assert_eq!(service.store().proposals("p", &id).unwrap().len(), 2);
}
