use serde_json::json;

#[tokio::test]
async fn inconsistent_connector_dimensions_create_no_child_and_points_updates_are_canonical() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("figures.sqlite");
    let mut store = visualization::Store::open(&path).unwrap();
    store.create_project("p", "Study").unwrap();
    let original = json!({"elements":[{"id":"edge","type":"arrow","x":570,"y":265,"width":0,"height":80,"points":[[0,0],[0,80]],"customData":{"evidence":["retained source"]}}],"files":{}});
    let figure = store
        .create("p", "Figure", "excalidraw/v2", &original)
        .unwrap();
    let parent = store
        .propose("p", &figure.id, 1, "Parent", &original)
        .unwrap();
    let tool = visualization_tools::registrations(path).remove(0);
    let input = |changes| json!({"action":"revise_proposal","project_id":"p","figure_id":figure.id,"proposal_id":parent.id,"document_sha256":parent.document_sha256,"expected_revision":1,"title":"Connector edit","operations":[{"op":"update","id":"edge","changes":changes}]});
    let error = tool
        .implementation
        .execute(input(json!({"width":150})))
        .await
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("edge.width") && error.contains("points"),
        "{error}"
    );
    assert_eq!(store.proposals("p", &figure.id).unwrap().len(), 1);
    tool.implementation
        .execute(input(json!({"points":[[0,0],[-30,100],[20,-10]]})))
        .await
        .unwrap();
    let proposals = store.proposals("p", &figure.id).unwrap();
    let child = proposals.iter().find(|p| p.id != parent.id).unwrap();
    assert_eq!(
        child.parent_proposal_id.as_deref(),
        Some(parent.id.as_str())
    );
    assert_eq!(child.document["elements"][0]["width"], 50.0);
    assert_eq!(child.document["elements"][0]["height"], 110.0);
    assert_eq!(
        child.document["elements"][0]["customData"],
        original["elements"][0]["customData"]
    );
    assert_eq!(child.document["files"], original["files"]);
    assert_eq!(
        store
            .proposal("p", &figure.id, &parent.id)
            .unwrap()
            .document,
        original
    );
    assert_eq!(store.get("p", &figure.id).unwrap().revision, 1);
}
