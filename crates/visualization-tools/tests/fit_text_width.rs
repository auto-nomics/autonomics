use serde_json::json;
#[tokio::test]
async fn registered_fit_preserves_parent_metadata_and_rejects_a_bad_hash() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("figures.sqlite");
    let mut store = visualization::Store::open(&path).unwrap();
    store.create_project("p", "Study").unwrap();
    let original = json!({"elements":[{"id":"note","type":"text","x":0,"y":20,"width":1180,"height":20,"angle":0,"fontFamily":9,"fontSize":16,"lineHeight":1.25,"textAlign":"left","text":"Complete source-backed note","originalText":"Complete source-backed note","containerId":null,"frameId":null,"boundElements":[],"groupIds":["retained"],"customData":{"evidence":["source"]}}],"files":{"kept":{"dataURL":"untouched"}}});
    let figure = store
        .create("p", "Figure", "excalidraw/v2", &original)
        .unwrap();
    let parent = store
        .propose("p", &figure.id, 1, "Parent", &original)
        .unwrap();
    let tool = visualization_tools::registrations(path).remove(0);
    let input = json!({"action":"revise_proposal","project_id":"p","figure_id":figure.id,"proposal_id":parent.id,"document_sha256":parent.document_sha256,"expected_revision":1,"title":"Fit note","operations":[{"op":"fit_text_width","id":"note"}]});
    let mut bad = input.clone();
    bad["document_sha256"] = json!("stale");
    assert!(tool.implementation.execute(bad).await.is_err());
    assert_eq!(store.proposals("p", &figure.id).unwrap().len(), 1);
    let output = serde_json::to_value(tool.implementation.execute(input).await.unwrap()).unwrap();
    let child = store
        .proposal("p", &figure.id, output["content"]["id"].as_str().unwrap())
        .unwrap();
    assert_eq!(
        child.parent_proposal_id.as_deref(),
        Some(parent.id.as_str())
    );
    assert!(child.document["elements"][0]["width"].as_f64().unwrap() < 1180.);
    for key in [
        "text",
        "originalText",
        "x",
        "y",
        "height",
        "fontFamily",
        "fontSize",
        "lineHeight",
        "groupIds",
        "customData",
    ] {
        assert_eq!(
            child.document["elements"][0][key],
            original["elements"][0][key]
        );
    }
    assert_eq!(child.document["files"], original["files"]);
    assert_eq!(
        store
            .proposal("p", &figure.id, &parent.id)
            .unwrap()
            .document,
        parent.document
    );
    assert_eq!(
        store.revision("p", &figure.id, 1).unwrap().document,
        original
    );
}
