use serde_json::json;
#[tokio::test]
async fn registered_geometry_is_typed_and_refines_the_exact_candidate_without_saving() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("figure.sqlite");
    let mut store = visualization::Store::open(&path).unwrap();
    store.create_project("p", "Study").unwrap();
    let original =
        json!({"elements":[{"id":"label","type":"text","text":"Retain scope"}],"files":{}});
    let figure = store
        .create("p", "Figure", "excalidraw/v2", &original)
        .unwrap();
    let (candidate, image) = visualization::image::insert(
        &original,
        visualization::image::Image {
            file_id: "pixels",
            data_url: "data:image/png;base64,fixture",
            width: 100,
            height: 50,
            provenance: json!({"source":"Fixture","asset":{"license":"cc-0","author":"Fixture"}}),
        },
        0.,
        100.,
    )
    .unwrap();
    let parent = store
        .propose("p", &figure.id, 1, "Pending art", &candidate)
        .unwrap();
    let tool = visualization_tools::registrations(path).remove(0);
    let input = json!({"action":"revise_proposal","project_id":"p","figure_id":figure.id,"expected_revision":1,"proposal_id":parent.id,"document_sha256":parent.document_sha256,"markup_ids":[],"title":"Place illustration","operations":[{"op":"set_image_geometry","id":image,"x":32.5,"y":170,"width":155,"height":112.31}]});
    let mut wrong = input.clone();
    wrong["operations"][0]["x"] = json!("32.5");
    let error = tool
        .implementation
        .execute(wrong)
        .await
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("operations[0].x") && error.contains("number"),
        "{error}"
    );
    assert_eq!(store.proposals("p", &figure.id).unwrap().len(), 1);
    let result = serde_json::to_value(tool.implementation.execute(input).await.unwrap()).unwrap();
    let child = store
        .proposal("p", &figure.id, result["content"]["id"].as_str().unwrap())
        .unwrap();
    assert_eq!(
        child.parent_proposal_id.as_deref(),
        Some(parent.id.as_str())
    );
    assert_eq!(child.document["elements"][1]["x"], 32.5);
    assert_eq!(child.document["elements"][0], original["elements"][0]);
    assert_eq!(child.document["files"], parent.document["files"]);
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
