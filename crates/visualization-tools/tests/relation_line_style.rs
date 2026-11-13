use serde_json::{Value, json};

#[tokio::test]
async fn registered_plan_and_candidate_read_preserve_native_relation_styles() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("figures.sqlite");
    let mut store = visualization::Store::open(&path).unwrap();
    store.create_project("p", "Study").unwrap();
    let original = json!({"elements":[]});
    let figure = store
        .create("p", "Figure", "excalidraw/v2", &original)
        .unwrap();
    let tool = visualization_tools::registrations(path).remove(0);
    let input = json!({"action":"sketch","project_id":"p","figure_id":figure.id,"expected_revision":1,"title":"Declared style","document":{"message":"User-declared diagram and line legend","audience":"Researchers","nodes":[{"id":"a","label":"A"},{"id":"b","label":"B"}],"edges":[{"from":"a","to":"b","line_style":"dashed","label":"Declared relationship","evidence":["user-reviewed source"]}]}});
    let mut bad = input.clone();
    bad["document"]["edges"][0]["line_style"] = json!("optional");
    assert!(tool.implementation.execute(bad).await.is_err());
    assert!(store.proposals("p", &figure.id).unwrap().is_empty());
    let result: Value = serde_json::to_value(tool.implementation.execute(input).await.unwrap()).unwrap();
    let id = result["content"]["id"].as_str().unwrap();
    let candidate = store.proposal("p", &figure.id, id).unwrap();
    let response: Value = serde_json::to_value(tool.implementation.execute(json!({"action":"read_proposal","project_id":"p","figure_id":figure.id,"expected_revision":1,"proposal_id":id,"element_ids":["edge-0"]})).await.unwrap()).unwrap();
    let selected = &response["content"]["selected_elements"][0];
    assert_eq!(selected["strokeStyle"], "dashed");
    assert_eq!(
        selected["customData"]["evidence"],
        json!(["user-reviewed source"])
    );
    assert_eq!(
        response["content"]["proposal"]["document_sha256"],
        candidate.document_sha256
    );
    assert_eq!(
        store.revision("p", &figure.id, 1).unwrap().document,
        original
    );
}
