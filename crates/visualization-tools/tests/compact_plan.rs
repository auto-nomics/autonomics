use serde_json::{Value, json};
#[tokio::test]
async fn compact_save_receipt_keeps_validation_history_and_exact_readback() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("figures.sqlite");
    let mut store = visualization::Store::open(&path).unwrap();
    store.create_project("p", "Study").unwrap();
    let document = json!({"elements":[],"files":{}});
    let figure = store
        .create("p", "Figure", "excalidraw/v2", &document)
        .unwrap();
    let tool = visualization_tools::registrations(path).remove(0);
    let brief = json!({"message":"User-declared scope. ".repeat(200),"audience":"Researchers","nodes":[{"id":"a","label":"A"},{"id":"b","label":"B"}],"edges":[{"from":"a","to":"b","label":"Declared relation","evidence":["user-provided source"]}],"assumptions":["Known scope limitation. ".repeat(80)],"questions":[{"id":"scope","question":"Confirm organism?","answer":"","blocking":true}]});
    let request = json!({"action":"save_plan","project_id":"p","figure_id":figure.id,"expected_revision":1,"planning_version":0,"planning":brief});
    let schema = serde_json::to_value(&tool.definition.input_schema).unwrap();
    assert!(
        schema["properties"]["compact_response"]
            .to_string()
            .contains("boolean")
    );
    let mut malformed = request.clone();
    malformed["compact_response"] = json!("true");
    assert!(tool.implementation.execute(malformed).await.is_err());
    assert!(store.planning("p", &figure.id).unwrap().is_none());
    let full =
        serde_json::to_value(tool.implementation.execute(request.clone()).await.unwrap()).unwrap();
    assert!(full["content"].get("document").is_some());
    let first = store.planning("p", &figure.id).unwrap().unwrap();
    let mut compact = request.clone();
    compact["compact_response"] = json!(true);
    compact["planning_version"] = json!(1);
    let receipt =
        serde_json::to_value(tool.implementation.execute(compact.clone()).await.unwrap()).unwrap();
    let payload = &receipt["content"];
    let saved = store.planning("p", &figure.id).unwrap().unwrap();
    let stored = serde_json::to_value(&saved).unwrap();
    assert_eq!(
        serde_json::to_value(&saved.document).unwrap(),
        serde_json::to_value(&first.document).unwrap()
    );
    assert!(payload.get("document").is_none());
    assert_eq!(payload["document_omitted"], true);
    for key in [
        "version",
        "based_on_revision",
        "status",
        "issues",
        "layout_review",
        "created_at",
        "updated_at",
    ] {
        assert_eq!(payload[key], stored[key], "{key}");
    }
    assert_eq!(payload["status"], "needs_clarification");
    assert_eq!(
        payload["plan_counts"],
        json!({"nodes":2,"edges":1,"panels":0,"questions":1,"assumptions":1})
    );
    let read: Value = serde_json::to_value(
        tool.implementation
            .execute(payload["follow_up_inputs"]["read_plan"].clone())
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(read["content"]["document"], stored["document"]);
    assert!(
        serde_json::to_vec(&receipt).unwrap().len() < serde_json::to_vec(&full).unwrap().len() / 2
    );
    assert!(
        tool.implementation.execute(compact).await.is_err(),
        "stale planning version rejected"
    );
    assert_eq!(store.planning("p", &figure.id).unwrap().unwrap().version, 2);
    assert_eq!(
        serde_json::to_value(
            store
                .planning_revision("p", &figure.id, 1)
                .unwrap()
                .document
        )
        .unwrap(),
        serde_json::to_value(&first.document).unwrap()
    );
    assert_eq!(
        store.revision("p", &figure.id, 1).unwrap().document,
        document
    );
    assert!(store.proposals("p", &figure.id).unwrap().is_empty());
    assert!(tool.implementation.execute(json!({"action":"read_plan","project_id":"p","figure_id":figure.id,"compact_response":true})).await.unwrap_err().to_string().contains("only supported for save_plan"));
}

#[tokio::test]
#[ignore = "requires VISUALIZATION_PLAN_RECEIPT_REPLAY pointing to an explicit saved plan"]
async fn explicit_saved_plan_receipt_size_and_readback() {
    let source = std::env::var("VISUALIZATION_PLAN_RECEIPT_REPLAY")
        .expect("explicit saved plan path required");
    let value: Value = serde_json::from_slice(&std::fs::read(source).unwrap()).unwrap();
    let brief = value.get("document").unwrap_or(&value);
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("replay.sqlite");
    let mut store = visualization::Store::open(&path).unwrap();
    store.create_project("p", "Receipt replay").unwrap();
    let figure = store
        .create("p", "Figure", "excalidraw/v2", &json!({"elements":[]}))
        .unwrap();
    let tool = visualization_tools::registrations(path).remove(0);
    let request = json!({"action":"save_plan","project_id":"p","figure_id":figure.id,"expected_revision":1,"planning_version":0,"planning":brief});
    let full =
        serde_json::to_value(tool.implementation.execute(request.clone()).await.unwrap()).unwrap();
    let saved = full["content"]["document"].clone();
    let mut compact = request;
    compact["planning_version"] = json!(1);
    compact["compact_response"] = json!(true);
    let receipt =
        serde_json::to_value(tool.implementation.execute(compact).await.unwrap()).unwrap();
    let read = serde_json::to_value(
        tool.implementation
            .execute(receipt["content"]["follow_up_inputs"]["read_plan"].clone())
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(read["content"]["document"], saved);
    for key in ["status", "issues", "layout_review"] {
        assert_eq!(receipt["content"][key], full["content"][key]);
    }
    assert_eq!(store.planning("p", &figure.id).unwrap().unwrap().version, 2);
    assert!(store.proposals("p", &figure.id).unwrap().is_empty());
    let full_bytes = serde_json::to_vec(&full).unwrap().len();
    let compact_bytes = serde_json::to_vec(&receipt).unwrap().len();
    assert!(compact_bytes < full_bytes);
    println!(
        "{}",
        json!({"scope":"registered tool response JSON bytes, not token counts or model latency","full_response_bytes":full_bytes,"compact_response_bytes":compact_bytes,"saved_document_and_validation_unchanged":true})
    );
}
