use serde_json::{Value, json};
#[tokio::test]
async fn registered_badge_operations_preserve_exact_candidate_and_rollback_failed_batches() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("figures.sqlite");
    let mut store = visualization::Store::open(&path).unwrap();
    store.create_project("p", "Review").unwrap();
    let doc: Value = serde_json::from_str(include_str!(
        "../../visualization/tests/fixtures/illustration-badge-component.json"
    ))
    .unwrap();
    let figure = store.create("p", "Figure", "excalidraw/v2", &doc).unwrap();
    let parent = store.propose("p", &figure.id, 1, "Parent", &doc).unwrap();
    let tool = visualization_tools::registrations(path).remove(0);
    let attach = json!({"op":"attach_illustration_badge","id":"badge","anchor_id":"node","meaning":"state specified by user"});
    let base = json!({"action":"revise_proposal","project_id":"p","figure_id":figure.id,"expected_revision":1,"proposal_id":parent.id,"document_sha256":parent.document_sha256,"title":"Attach state badge","operations":[attach.clone()],"markup_ids":[]});
    let mut failed = base.clone();
    failed["operations"] = json!([attach.clone(), attach.clone()]);
    assert!(tool.implementation.execute(failed).await.is_err());
    assert_eq!(store.proposals("p", &figure.id).unwrap().len(), 1);
    let mut stale = base.clone();
    stale["document_sha256"] = json!("wrong");
    assert!(tool.implementation.execute(stale).await.is_err());
    assert_eq!(store.proposals("p", &figure.id).unwrap().len(), 1);
    let result = serde_json::to_value(tool.implementation.execute(base).await.unwrap()).unwrap();
    let child = store
        .proposal("p", &figure.id, result["content"]["id"].as_str().unwrap())
        .unwrap();
    assert_eq!(
        child.parent_proposal_id.as_deref(),
        Some(parent.id.as_str())
    );
    assert_eq!(child.document["files"], doc["files"]);
    assert_eq!(
        store
            .proposal("p", &figure.id, &parent.id)
            .unwrap()
            .document,
        doc
    );
    assert_eq!(store.get("p", &figure.id).unwrap().revision, 1);
    let detached=serde_json::to_value(tool.implementation.execute(json!({"action":"revise_proposal","project_id":"p","figure_id":figure.id,"expected_revision":1,"proposal_id":child.id,"document_sha256":child.document_sha256,"title":"Detach state badge","operations":[{"op":"detach_illustration_badge","id":"badge","anchor_id":"node"}],"markup_ids":[]})).await.unwrap()).unwrap();
    assert!(
        store
            .proposal("p", &figure.id, detached["content"]["id"].as_str().unwrap())
            .unwrap()
            .document["elements"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["id"] == "badge")
            .unwrap()["customData"]["semanticBadgeAnchorId"]
            .is_null()
    );
}
