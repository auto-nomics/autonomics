use serde_json::{Value, json};
#[tokio::test]
async fn scalar_text_entry_requires_numbers_and_preserves_the_reviewed_scene() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("figures.sqlite");
    let tool = visualization_tools::registrations(path.clone())
        .into_iter()
        .find(|t| t.definition.name == "visualization_add_text")
        .unwrap();
    let schema = serde_json::to_value(&tool.definition.input_schema).unwrap();
    for field in ["x", "y", "max_width", "font_size"] {
        assert_eq!(schema["properties"][field]["type"], "number");
        assert!(
            schema["required"]
                .as_array()
                .unwrap()
                .contains(&json!(field))
        );
    }
    let invalid = json!({"project_id":"p","figure_id":"f","proposal_id":"pending","document_sha256":"hash","expected_revision":1,"title":"Note","element_id":"note","text":"Human IFN-γ; JAK attribution remains uncertain.","x":0,"y":1000,"max_width":"1180","font_size":16});
    assert!(tool.implementation.execute(invalid).await.is_err());
    assert!(!path.exists());
    let input: Value = serde_json::from_str(include_str!(
        "../../../apps/gui/tests/fixtures/scientific-ifng-reviewed.json"
    ))
    .unwrap();
    let original = input
        .pointer("/scene/document")
        .or_else(|| input.get("document"))
        .unwrap_or(&input)
        .clone();
    let mut store = visualization::Store::open(&path).unwrap();
    store.create_project("p", "Study").unwrap();
    let figure = store
        .create("p", "Figure", "excalidraw/v2", &original)
        .unwrap();
    let parent = store
        .propose("p", &figure.id, 1, "Reviewed sketch", &original)
        .unwrap();
    let request = json!({"project_id":"p","figure_id":figure.id,"proposal_id":parent.id,"document_sha256":parent.document_sha256,"expected_revision":1,"title":"Visible scope","element_id":"new-scope-note","text":"Human IFN-γ; JAK attribution remains uncertain.","x":0,"y":1000,"max_width":1180,"fit_width":true,"font_size":16,"markup_ids":[]});
    let result =
        serde_json::to_value(tool.implementation.execute(request.clone()).await.unwrap()).unwrap();
    let payload = &result["content"];
    let child = store
        .proposal("p", &figure.id, payload["id"].as_str().unwrap())
        .unwrap();
    assert_eq!(
        child.parent_proposal_id.as_deref(),
        Some(parent.id.as_str())
    );
    assert_eq!(child.document["files"], original["files"]);
    let elements = child.document["elements"].as_array().unwrap();
    assert_eq!(
        elements.len(),
        original["elements"].as_array().unwrap().len() + 1
    );
    for old in original["elements"].as_array().unwrap() {
        assert_eq!(elements.iter().find(|e| e["id"] == old["id"]).unwrap(), old);
    }
    let note = elements
        .iter()
        .find(|e| e["id"] == "new-scope-note")
        .unwrap();
    assert_eq!(note["type"], "text");
    assert_eq!(note["originalText"], request["text"]);
    assert_eq!(note["fontFamily"], 9);
    assert_eq!(note["fontSize"].as_f64(), Some(16.));
    assert!(note["width"].as_f64().unwrap() < 1180.);
    assert!(note["height"].as_f64().unwrap() > 0.);
    assert_eq!(
        payload["follow_up_inputs"]["read_proposal"]["proposal_id"],
        child.id
    );
    assert_eq!(
        store
            .proposal("p", &figure.id, &parent.id)
            .unwrap()
            .document,
        parent.document
    );
    assert_eq!(store.get("p", &figure.id).unwrap().revision, 1);
    for patch in [
        json!({"document_sha256":"wrong"}),
        json!({"font_size":0}),
        json!({"frame_id":"missing-frame"}),
        json!({"proposal_id":child.id,"document_sha256":child.document_sha256}),
    ] {
        let mut bad = request.clone();
        bad.as_object_mut()
            .unwrap()
            .extend(patch.as_object().unwrap().clone());
        assert!(tool.implementation.execute(bad).await.is_err());
        assert_eq!(store.proposals("p", &figure.id).unwrap().len(), 2);
    }
}
