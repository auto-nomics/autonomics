use serde_json::json;

#[tokio::test]
async fn registered_append_proposes_native_graph_without_replacing_reference_or_history() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("figures.sqlite");
    let mut store = visualization::Store::open(&path).unwrap();
    store.create_project("p", "Study").unwrap();
    let original = json!({"elements":[{"id":"reference","type":"image","x":0,"y":0,"width":840,"height":180,"fileId":"r"}],"files":{"r":{"dataURL":"private image bytes"}},"scientificPlan":{"message":"Prior historical plan"},"appState":{"viewBackgroundColor":"#fffaf0"}});
    let figure = store
        .create("p", "Figure", "excalidraw/v2", &original)
        .unwrap();
    let tool = visualization_tools::registrations(path).remove(0);
    let operation = json!({"op":"insert_sketch","id_prefix":"reference_concept","x":0,"y":240,"plan":{"message":"Supplied concept","audience":"Researchers","layout":"horizontal","nodes":[{"id":"a","label":"Collect"},{"id":"b","label":"Image"},{"id":"c","label":"Review"}],"edges":[{"from":"a","to":"b"},{"from":"b","to":"c"}],"assumptions":["No experiment conditions"]}});
    tool.implementation.execute(json!({"action":"propose","project_id":"p","figure_id":figure.id,"expected_revision":1,"title":"Append editable concept","operations":[operation.clone()]})).await.unwrap();
    let proposals = store.proposals("p", &figure.id).unwrap();
    assert_eq!(proposals.len(), 1);
    let parent = &proposals[0];
    assert_eq!(parent.document["elements"][0], original["elements"][0]);
    for key in ["files", "scientificPlan", "appState"] {
        assert_eq!(parent.document[key], original[key]);
    }
    assert_eq!(parent.document["elements"].as_array().unwrap().len(), 9);
    let error = tool.implementation.execute(json!({"action":"revise_proposal","project_id":"p","figure_id":figure.id,"proposal_id":parent.id,"document_sha256":parent.document_sha256,"expected_revision":1,"title":"Colliding insert","operations":[operation]})).await.unwrap_err().to_string();
    assert!(error.contains("collides"), "{error}");
    assert_eq!(store.proposals("p", &figure.id).unwrap().len(), 1);
    assert_eq!(
        store
            .proposal("p", &figure.id, &parent.id)
            .unwrap()
            .document,
        parent.document
    );
    assert_eq!(store.get("p", &figure.id).unwrap().revision, 1);
}
