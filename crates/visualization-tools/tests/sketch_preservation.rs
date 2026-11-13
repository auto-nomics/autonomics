use serde_json::json;

#[tokio::test]
async fn whole_scene_sketch_requires_explicit_replacement_and_keeps_saved_reference() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("figures.sqlite");
    let mut store = visualization::Store::open(&path).unwrap();
    store.create_project("p", "Study").unwrap();
    let original = json!({"elements":[{"id":"reference","type":"image","fileId":"source","x":0,"y":0,"width":100,"height":100}],"files":{"source":{"dataURL":"data:image/png;base64,c291cmNl"}}});
    let figure = store
        .create("p", "Figure", "excalidraw/v2", &original)
        .unwrap();
    let tool = visualization_tools::registrations(path).remove(0);
    let plan = json!({"message":"Explicit conceptual replacement","audience":"Researchers","layout":"horizontal","nodes":[{"id":"a","label":"Collect"}]});
    let input = json!({"action":"sketch","project_id":"p","figure_id":figure.id,"expected_revision":1,"title":"New graph","document":plan});
    for replace in [None, Some(false)] {
        let mut request = input.clone();
        if let Some(value) = replace {
            request["replace_existing"] = json!(value);
        }
        let error = tool
            .implementation
            .execute(request)
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains("insert_sketch"), "{error}");
        assert!(store.proposals("p", &figure.id).unwrap().is_empty());
    }
    let mut replace = input.clone();
    replace["replace_existing"] = json!(true);
    tool.implementation.execute(replace).await.unwrap();
    assert_eq!(store.proposals("p", &figure.id).unwrap().len(), 1);
    assert_eq!(
        store.revision("p", &figure.id, 1).unwrap().document,
        original
    );
    assert_eq!(store.get("p", &figure.id).unwrap().revision, 1);

    let brief = serde_json::from_value::<visualization::planning::Brief>(plan).unwrap();
    store.save_planning("p", &figure.id, 1, 0, &brief).unwrap();
    let planning = store.planning("p", &figure.id).unwrap().unwrap();
    let mut planned = json!({"action":"sketch_plan","project_id":"p","figure_id":figure.id,"expected_revision":1,"planning_version":planning.version});
    let error = tool
        .implementation
        .execute(planned.clone())
        .await
        .unwrap_err()
        .to_string();
    assert!(error.contains("insert_sketch"), "{error}");
    assert_eq!(store.proposals("p", &figure.id).unwrap().len(), 1);
    planned["replace_existing"] = json!(true);
    tool.implementation.execute(planned).await.unwrap();
    assert_eq!(store.proposals("p", &figure.id).unwrap().len(), 2);
    assert_eq!(
        store.revision("p", &figure.id, 1).unwrap().document,
        original
    );
}
