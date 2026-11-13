use serde_json::json;
#[tokio::test]
async fn scalar_scale_preserves_reviewed_fit_and_candidate_lineage() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("figures.sqlite");
    let original: serde_json::Value = serde_json::from_str(include_str!(
        "../../../apps/gui/tests/fixtures/scientific-ifng-art-before-clearance.json"
    ))
    .unwrap();
    let file = original["elements"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["type"] == "image")
        .unwrap()["fileId"]
        .as_str()
        .unwrap()
        .to_owned();
    let fitted = visualization::scene::apply(
        &original,
        &[visualization::scene::Operation::FitIllustrationAnchor {
            id: "node-0".into(),
            file_id: file.clone(),
            normalized_bounds: [0.347441, 0.016803, 0.268522, 0.960235],
        }],
    )
    .unwrap();
    let mut store = visualization::Store::open(&path).unwrap();
    store.create_project("p", "Review").unwrap();
    let figure = store
        .create("p", "Figure", "excalidraw/v2", &original)
        .unwrap();
    let parent = store
        .propose("p", &figure.id, 1, "Reviewed fit", &fitted)
        .unwrap();
    let parent = store.proposal("p", &figure.id, &parent.id).unwrap();
    let tool = visualization_tools::registrations(path)
        .into_iter()
        .find(|t| t.definition.name == "visualization_scale_illustration")
        .unwrap();
    assert_eq!(
        serde_json::to_value(&tool.definition.input_schema).unwrap()["properties"]["factor"]["type"],
        "number"
    );
    let input = json!({"project_id":"p","figure_id":figure.id,"proposal_id":parent.id,"document_sha256":parent.document_sha256,"expected_revision":1,"title":"Enlarge 10%","element_id":"node-0","file_id":file,"factor":1.1,"markup_ids":[]});
    for value in [json!("1.1"), json!(0), json!(-1)] {
        let mut invalid = input.clone();
        invalid["factor"] = value;
        assert!(tool.implementation.execute(invalid).await.is_err());
    }
    let result =
        serde_json::to_value(tool.implementation.execute(input.clone()).await.unwrap()).unwrap();
    let child = store
        .proposal("p", &figure.id, result["content"]["id"].as_str().unwrap())
        .unwrap();
    assert_eq!(
        child.parent_proposal_id.as_deref(),
        Some(parent.id.as_str())
    );
    assert_eq!(child.document["files"], parent.document["files"]);
    assert_eq!(store.get("p", &figure.id).unwrap().revision, 1);
    assert_eq!(
        store
            .proposal("p", &figure.id, &parent.id)
            .unwrap()
            .document,
        parent.document
    );
    let mut stale = input;
    stale["document_sha256"] = json!("wrong");
    assert!(tool.implementation.execute(stale).await.is_err());
    assert_eq!(store.proposals("p", &figure.id).unwrap().len(), 2);
}
