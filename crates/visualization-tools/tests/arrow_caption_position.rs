use serde_json::{Value, json};
#[tokio::test]
async fn caption_position_uses_exact_pending_scope_and_failed_batches_save_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("figures.sqlite");
    let mut store = visualization::Store::open(&path).unwrap();
    store.create_project("p", "Review").unwrap();
    let doc: Value = serde_json::from_str(include_str!(
        "../../visualization/tests/fixtures/arrow-caption-component.json"
    ))
    .unwrap();
    let figure = store.create("p", "Figure", "excalidraw/v2", &doc).unwrap();
    let parent = store.propose("p", &figure.id, 1, "Parent", &doc).unwrap();
    let tool = visualization_tools::registrations(path).remove(0);
    let input = json!({"action":"revise_proposal","project_id":"p","figure_id":figure.id,"expected_revision":1,"proposal_id":parent.id,"document_sha256":parent.document_sha256,"title":"Move caption along path","operations":[{"op":"position_arrow_caption","id":"caption","position":0.5}]});
    let mut failed = input.clone();
    failed["operations"] = json!([{"op":"position_arrow_caption","id":"caption","position":0.5},{"op":"position_arrow_caption","id":"caption","position":1}]);
    assert!(tool.implementation.execute(failed).await.is_err());
    assert_eq!(store.proposals("p", &figure.id).unwrap().len(), 1);
    for (key, value) in [
        ("document_sha256", json!("wrong")),
        ("expected_revision", json!(2)),
    ] {
        let mut stale = input.clone();
        stale[key] = value;
        assert!(tool.implementation.execute(stale).await.is_err());
        assert_eq!(store.proposals("p", &figure.id).unwrap().len(), 1);
    }
    let mut wrong = input.clone();
    wrong["operations"][0]["position"] = json!("0.5");
    assert!(tool.implementation.execute(wrong).await.is_err());
    assert_eq!(store.proposals("p", &figure.id).unwrap().len(), 1);
    let result = serde_json::to_value(tool.implementation.execute(input).await.unwrap()).unwrap();
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
}
