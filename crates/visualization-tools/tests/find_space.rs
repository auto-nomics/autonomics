use serde_json::{Value, json};
#[tokio::test]
async fn registered_space_query_is_scoped_hash_checked_and_read_only() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("figures.sqlite");
    let mut store = visualization::Store::open(&path).unwrap();
    store.create_project("p", "Study").unwrap();
    store.create_project("other", "Other").unwrap();
    let document = json!({"elements":[{"id":"panel","type":"frame","x":0,"y":0,"width":320,"height":480}],"files":{"keep":{"dataURL":"not returned"}}});
    let figure = store
        .create("p", "Figure", "excalidraw/v2", &document)
        .unwrap();
    let parent = store
        .propose("p", &figure.id, 1, "Parent", &document)
        .unwrap();
    let tool = visualization_tools::registrations(path)
        .into_iter()
        .find(|t| t.definition.name == "visualization_find_space")
        .unwrap();
    let input = json!({"project_id":"p","figure_id":figure.id,"expected_revision":1,"proposal_id":parent.id,"document_sha256":parent.document_sha256,"relative_to":"panel","width":280,"height":360,"gap":24});
    let result: Value =
        serde_json::to_value(tool.implementation.execute(input.clone()).await.unwrap()).unwrap();
    assert_eq!(result["content"]["placement_review"]["status"], "available");
    assert_eq!(
        result["content"]["candidate_context"]["document_sha256"],
        parent.document_sha256
    );
    assert!(!result.to_string().contains("not returned"));
    for (key, value) in [
        ("expected_revision", json!(2)),
        ("document_sha256", json!("stale")),
        ("project_id", json!("other")),
        ("width", json!("280")),
        ("gap", json!(0)),
    ] {
        let mut bad = input.clone();
        bad[key] = value;
        assert!(tool.implementation.execute(bad).await.is_err(), "{key}");
    }
    let mut missing = input.clone();
    missing.as_object_mut().unwrap().remove("document_sha256");
    assert!(tool.implementation.execute(missing).await.is_err());
    let mut formal = input.clone();
    formal.as_object_mut().unwrap().remove("proposal_id");
    formal.as_object_mut().unwrap().remove("document_sha256");
    let result: Value =
        serde_json::to_value(tool.implementation.execute(formal).await.unwrap()).unwrap();
    assert!(result["content"]["candidate_context"].is_null());
    assert_eq!(store.proposals("p", &figure.id).unwrap().len(), 1);
    assert_eq!(
        store
            .proposal("p", &figure.id, &parent.id)
            .unwrap()
            .document,
        document
    );
    assert_eq!(
        store.revision("p", &figure.id, 1).unwrap().document,
        document
    );
    let gap_document = json!({"elements":[
        {"id":"center","type":"rectangle","x":0,"y":0,"width":40,"height":40},
        {"id":"below","type":"rectangle","x":0,"y":50,"width":40,"height":40},
        {"id":"above","type":"rectangle","x":0,"y":-50,"width":40,"height":40},
        {"id":"left","type":"rectangle","x":-50,"y":0,"width":40,"height":40},
        {"id":"right","type":"rectangle","x":50,"y":0,"width":40,"height":40}
    ]});
    let gap_figure = store
        .create("p", "Internal gap", "excalidraw/v2", &gap_document)
        .unwrap();
    let result:Value=serde_json::to_value(tool.implementation.execute(json!({"project_id":"p","figure_id":gap_figure.id,"expected_revision":1,"relative_to":"center","width":40,"height":40,"gap":10})).await.unwrap()).unwrap();
    assert_eq!(
        result["content"]["placement_review"]["candidates"][0]["position"],
        "internal_gap"
    );
    assert_eq!(
        result["content"]["placement_review"]["candidates"][0]["added_bounding_area"],
        0.
    );
    assert_eq!(
        result["content"]["placement_review"]["candidates"][0]["resulting_aspect_ratio"],
        1.
    );
    assert_eq!(
        result["content"]["placement_review"]["candidates"][0]["resulting_bounds"]["width"],
        140.
    );
    assert!(
        result["content"]["placement_review"]["candidates"]
            .as_array()
            .unwrap()
            .len()
            <= 8
    );
    assert_eq!(
        store.revision("p", &gap_figure.id, 1).unwrap().document,
        gap_document
    );
    assert!(store.proposals("p", &gap_figure.id).unwrap().is_empty());
}
