use serde_json::json;
#[tokio::test]
async fn object_annotation_ids_cannot_be_linked_as_sketch_markups_and_rejection_is_atomic() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("figures.sqlite");
    let mut store = visualization::Store::open(&path).unwrap();
    store.create_project("p", "Study").unwrap();
    let document = json!({"elements":[{"id":"target","type":"rectangle","x":0,"y":0,"width":100,"height":80}],"files":{}});
    let figure = store
        .create("p", "Figure", "excalidraw/v2", &document)
        .unwrap();
    let annotation = store
        .annotate("p", &figure.id, 1, &["target".into()], "Move this object")
        .unwrap();
    let tool = visualization_tools::registrations(path).remove(0);
    let request = json!({"action":"propose","project_id":"p","figure_id":figure.id,"expected_revision":1,"title":"Move target","operations":[{"op":"update","id":"target","changes":{"x":8}}]});
    for bad_id in [&annotation.id, &"missing-markup".to_string()] {
        let mut bad = request.clone();
        bad["markup_ids"] = json!([bad_id]);
        let error = tool
            .implementation
            .execute(bad)
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains("markup_ids"), "{error}");
        assert!(
            error.contains("object annotation IDs are a separate channel"),
            "{error}"
        );
        assert!(error.contains("nothing was saved"), "{error}");
        assert!(
            !error.contains("project or visualization not found"),
            "{error}"
        );
        assert!(store.proposals("p", &figure.id).unwrap().is_empty());
        assert_eq!(
            store.revision("p", &figure.id, 1).unwrap().document,
            document
        );
    }
    let result = serde_json::to_value(tool.implementation.execute(request).await.unwrap()).unwrap();
    let child = store
        .proposal("p", &figure.id, result["content"]["id"].as_str().unwrap())
        .unwrap();
    assert_eq!(child.document["elements"][0]["x"], 8);
    assert!(child.review_markups.is_empty());
    assert_eq!(
        store.annotations("p", &figure.id).unwrap()[0].status,
        "open"
    );
}
