use serde_json::json;
#[tokio::test]
async fn registered_frame_fit_preserves_parent_children_and_formal_scene() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("figures.sqlite");
    let mut store = visualization::Store::open(&path).unwrap();
    store.create_project("p", "Study").unwrap();
    let original = json!({"type":"excalidraw","elements":[
        {"id":"panel","type":"frame","x":0,"y":0,"width":500,"height":700,"angle":0},
        {"id":"child","type":"rectangle","x":40,"y":80,"width":200,"height":120,"angle":0,"frameId":"panel"}
    ],"files":{}});
    let figure = store
        .create("p", "Figure", "excalidraw/v2", &original)
        .unwrap();
    let parent = store
        .propose("p", &figure.id, 1, "Review frame", &original)
        .unwrap();
    let tool = visualization_tools::registrations(path).remove(0);
    let input = |padding| {
        json!({"action":"revise_proposal","project_id":"p","figure_id":figure.id,
        "proposal_id":parent.id,"document_sha256":parent.document_sha256,"expected_revision":1,
        "title":"Fit native frame","operations":[{"op":"fit_frame","id":"panel","padding":padding}]})
    };
    for invalid in [json!("24"), json!(0), json!(129)] {
        assert!(tool.implementation.execute(input(invalid)).await.is_err());
        assert_eq!(store.proposals("p", &figure.id).unwrap().len(), 1);
    }
    tool.implementation.execute(input(json!(24))).await.unwrap();
    let all = store.proposals("p", &figure.id).unwrap();
    assert_eq!(all.len(), 2);
    let child = all.iter().find(|p| p.id != parent.id).unwrap();
    assert_eq!(
        child.parent_proposal_id.as_deref(),
        Some(parent.id.as_str())
    );
    assert_eq!(child.document["elements"][0]["width"], 264.);
    assert_eq!(child.document["elements"][0]["height"], 224.);
    assert_eq!(child.document["elements"][1], original["elements"][1]);
    assert_eq!(child.document["files"], original["files"]);
    assert_eq!(
        store
            .proposal("p", &figure.id, &parent.id)
            .unwrap()
            .document,
        original
    );
    assert_eq!(
        store.revision("p", &figure.id, 1).unwrap().document,
        original
    );
}
