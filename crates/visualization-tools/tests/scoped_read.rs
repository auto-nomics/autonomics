use serde_json::{Value, json};

#[tokio::test]
async fn scoped_read_preserves_exact_objects_full_hash_and_diagnostics() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("figures.sqlite");
    let mut store = visualization::Store::open(&path).unwrap();
    store.create_project("p", "Study").unwrap();
    let document = json!({"elements":[
        {"id":"note","type":"text","x":0,"y":0,"width":100,"height":20,"text":"本次已读来源未见","customData":{"evidence":["source"],"unknown":{"keep":true}}},
        {"id":"image","type":"image","fileId":"asset","x":200,"y":0,"width":40,"height":40},
        {"id":"gone","isDeleted":true}
    ],"files":{"asset":{"dataURL":"private-image-data","mimeType":"image/png"}},"scientificPlan":{"large":"must not be repeated"}});
    let figure = store
        .create("p", "Figure", "excalidraw/v2", &document)
        .unwrap();
    let parent = store
        .propose("p", &figure.id, 1, "Candidate", &document)
        .unwrap();
    let tool = visualization_tools::registrations(path).remove(0);
    let input = json!({"action":"read_proposal","project_id":"p","figure_id":figure.id,"proposal_id":parent.id,"expected_revision":1});
    let full: Value =
        serde_json::to_value(tool.implementation.execute(input.clone()).await.unwrap()).unwrap();
    let mut scoped = input.clone();
    scoped["element_ids"] = json!(["image", "note"]);
    let response: Value =
        serde_json::to_value(tool.implementation.execute(scoped.clone()).await.unwrap()).unwrap();
    let result = &response["content"];
    assert_eq!(
        result["selected_elements"],
        json!([document["elements"][1], document["elements"][0]])
    );
    assert!(result["proposal"].get("document").is_none());
    assert_eq!(
        result["proposal"]["document_sha256"],
        parent.document_sha256
    );
    assert_eq!(
        result["scene_geometry_review"],
        full["content"]["scene_geometry_review"]
    );
    assert_eq!(
        full["content"]["proposal"]["document"]["scientificPlan"],
        document["scientificPlan"]
    );
    assert!(result["selected_files"]["asset"].get("dataURL").is_none());
    assert_eq!(result["scope"]["live_element_count"], 2);
    assert_eq!(result["base_is_current"], true);
    for ids in [
        json!([]),
        json!(["note", "note"]),
        json!(["gone"]),
        json!(["missing"]),
        json!([" note"]),
        json!([" "]),
    ] {
        scoped["element_ids"] = ids;
        assert!(tool.implementation.execute(scoped.clone()).await.is_err());
    }
    scoped["action"] = json!("create");
    scoped["element_ids"] = json!(["note"]);
    assert!(tool.implementation.execute(scoped).await.is_err());
    assert_eq!(store.proposals("p", &figure.id).unwrap().len(), 1);
    assert_eq!(
        store
            .proposal("p", &figure.id, &parent.id)
            .unwrap()
            .document,
        document
    );
}

#[tokio::test]
async fn saved_read_requires_current_revision_and_keeps_review_context() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("figures.sqlite");
    let mut store = visualization::Store::open(&path).unwrap();
    store.create_project("p", "Study").unwrap();
    let document = json!({"elements":[{"id":"a","type":"rectangle","x":0,"y":0,"width":20,"height":20,"customData":{"evidence":["kept"]}},{"id":"b","type":"image","fileId":"f"}],"files":{"f":{"dataURL":"omitted","mimeType":"image/png"}},"scientificPlan":{"keep":"full-only"}});
    let figure = store
        .create("p", "Figure", "excalidraw/v2", &document)
        .unwrap();
    let tool = visualization_tools::registrations(path).remove(0);
    let mut input =
        json!({"action":"read","project_id":"p","figure_id":figure.id,"expected_revision":1});
    let full =
        serde_json::to_value(tool.implementation.execute(input.clone()).await.unwrap()).unwrap();
    input["element_ids"] = json!(["b", "a"]);
    let scoped =
        serde_json::to_value(tool.implementation.execute(input.clone()).await.unwrap()).unwrap();
    let output = &scoped["content"];
    assert_eq!(
        output["selected_elements"],
        json!([document["elements"][1], document["elements"][0]])
    );
    assert!(output["scene"].get("document").is_none());
    assert_eq!(
        output["scope"]["revision_scope"],
        "complete_immutable_saved_revision"
    );
    assert!(output["scope"].get("hash_scope").is_none());
    assert!(output["selected_files"]["f"].get("dataURL").is_none());
    for key in [
        "metadata",
        "scene_geometry_review",
        "proposal_summary",
        "annotations",
        "markups",
        "markups_pagination",
    ] {
        assert_eq!(output[key], full["content"][key], "{key}");
    }
    input.as_object_mut().unwrap().remove("expected_revision");
    assert!(tool.implementation.execute(input.clone()).await.is_err());
    input["expected_revision"] = json!(2);
    assert!(tool.implementation.execute(input.clone()).await.is_err());
    input.as_object_mut().unwrap().remove("element_ids");
    assert!(tool.implementation.execute(input.clone()).await.is_err());
    input.as_object_mut().unwrap().remove("expected_revision");
    assert!(
        tool.implementation.execute(input).await.is_ok(),
        "legacy unscoped read can omit revision"
    );
    assert_eq!(
        store.revision("p", &figure.id, 1).unwrap().document,
        document
    );
    assert!(store.proposals("p", &figure.id).unwrap().is_empty());
}
