use serde_json::json;
#[tokio::test]
async fn registered_node_geometry_is_atomic_and_preserves_candidate_scope() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("figures.sqlite");
    let mut store = visualization::Store::open(&path).unwrap();
    store.create_project("p", "Study").unwrap();
    let plan:visualization::sketch::Plan=serde_json::from_value(json!({"message":"Explicit concept","audience":"Researchers","layout":"horizontal","nodes":[{"id":"a","label":"Source"},{"id":"b","label":"Target"}],"edges":[{"from":"a","to":"b"}]})).unwrap();
    let original = visualization::sketch::compile(&plan).unwrap();
    let figure = store
        .create("p", "Figure", "excalidraw/v2", &original)
        .unwrap();
    let parent = store
        .propose("p", &figure.id, 1, "Parent", &original)
        .unwrap();
    let tool = visualization_tools::registrations(path).remove(0);
    let operation =
        json!({"op":"set_node_geometry","id":"node-0","x":50,"y":45,"width":250,"height":120});
    let input = |op| json!({"action":"revise_proposal","project_id":"p","figure_id":figure.id,"proposal_id":parent.id,"document_sha256":parent.document_sha256,"expected_revision":1,"title":"Resize native node","operations":[op]});
    let mut invalid = operation.clone();
    invalid["width"] = json!("250");
    let error = tool
        .implementation
        .execute(input(invalid))
        .await
        .unwrap_err()
        .to_string();
    assert!(error.contains("operations[0].width"), "{error}");
    let mut overflow = operation.clone();
    overflow["width"] = json!(1);
    overflow["height"] = json!(1);
    assert!(tool.implementation.execute(input(overflow)).await.is_err());
    assert_eq!(store.proposals("p", &figure.id).unwrap().len(), 1);
    tool.implementation.execute(input(operation)).await.unwrap();
    let all = store.proposals("p", &figure.id).unwrap();
    assert_eq!(all.len(), 2);
    let child = all.iter().find(|p| p.id != parent.id).unwrap();
    assert_eq!(
        child.parent_proposal_id.as_deref(),
        Some(parent.id.as_str())
    );
    assert_eq!(
        store
            .proposal("p", &figure.id, &parent.id)
            .unwrap()
            .document,
        original
    );
    assert_eq!(store.get("p", &figure.id).unwrap().revision, 1);
    assert_eq!(child.document["files"], original["files"]);
    assert_eq!(child.document["scientificPlan"], original["scientificPlan"]);
    let element = |doc: &serde_json::Value, id: &str| {
        doc["elements"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["id"] == id)
            .unwrap()
            .clone()
    };
    assert_eq!(
        element(&child.document, "node-1"),
        element(&original, "node-1")
    );
    assert_eq!(
        element(&child.document, "label-1"),
        element(&original, "label-1")
    );
    let (node, arrow) = (
        element(&child.document, "node-0"),
        element(&child.document, "edge-0"),
    );
    assert_eq!(node["width"], 250.);
    assert_eq!(arrow["x"], 310.);
    assert_eq!(arrow["startBinding"]["elementId"], "node-0");
}
