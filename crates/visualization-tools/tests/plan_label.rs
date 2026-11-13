use serde_json::{Value, json};
#[tokio::test]
async fn single_label_compare_and_set_preserves_plan_sources_history_and_canvas() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("figures.sqlite");
    let mut store = visualization::Store::open(&path).unwrap();
    store.create_project("p", "Study").unwrap();
    store.create_project("other", "Other").unwrap();
    let scene = json!({"elements":[],"files":{}});
    let figure = store
        .create("p", "Figure", "excalidraw/v2", &scene)
        .unwrap();
    let brief:visualization::planning::Brief=serde_json::from_value(json!({"message":"Reviewed scope","audience":"Researchers","panels":[{"id":"a","title":"Panel","column":0,"row":0}],"nodes":[{"id":"a","label":"Original node","panel":"a","evidence":["node source"]},{"id":"b","label":"Other node","panel":"a"}],"edges":[{"from":"a","to":"b","label":"Original relation","evidence":["reaction source"],"line_style":"dashed"}],"assumptions":["Scope qualifier"],"questions":[{"id":"q","question":"Species?","answer":"Human","blocking":true}]})).unwrap();
    let original = store.save_planning("p", &figure.id, 1, 0, &brief).unwrap();
    let tool = visualization_tools::registrations(path)
        .into_iter()
        .find(|t| t.definition.name == "visualization_update_plan_label")
        .unwrap();
    let base = json!({"project_id":"p","figure_id":figure.id,"expected_revision":1,"planning_version":1,"plan_field":"edges[0].label","expected_text":"Original relation","text":"Reviewed relation"});
    for (key, value) in [
        ("planning_version", json!(0)),
        ("expected_revision", json!(2)),
        ("project_id", json!("other")),
        ("figure_id", json!("missing")),
        ("expected_text", json!("truncated…")),
        ("text", json!("")),
        ("text", json!(" ")),
        ("text", json!("a".repeat(121))),
        ("text", json!("Original relation")),
        ("plan_field", json!("nodes[0].evidence")),
        ("plan_field", json!("edges[01].label")),
        ("plan_field", json!("edges[99].label")),
        ("plan_field", json!("edges[-1].label")),
    ] {
        let mut input = base.clone();
        input[key] = value;
        assert!(tool.implementation.execute(input).await.is_err(), "{key}");
        assert_eq!(
            serde_json::to_value(store.planning("p", &figure.id).unwrap().unwrap()).unwrap(),
            serde_json::to_value(&original).unwrap()
        );
    }
    let result: Value =
        serde_json::to_value(tool.implementation.execute(base.clone()).await.unwrap()).unwrap();
    let payload = &result["content"];
    assert_eq!(payload["version"], 2);
    assert_eq!(payload["scope"], "saved_plan_single_label");
    assert_eq!(
        payload["follow_up_inputs"]["read_plan"]["figure_id"],
        figure.id
    );
    assert!(payload.get("document").is_none());
    let mut expected = serde_json::to_value(&brief).unwrap();
    expected["edges"][0]["label"] = json!("Reviewed relation");
    assert_eq!(
        serde_json::to_value(store.planning("p", &figure.id).unwrap().unwrap().document).unwrap(),
        expected
    );
    assert_eq!(
        serde_json::to_value(
            store
                .planning_revision("p", &figure.id, 1)
                .unwrap()
                .document
        )
        .unwrap(),
        serde_json::to_value(&brief).unwrap()
    );
    assert!(tool.implementation.execute(base.clone()).await.is_err());
    for (version, field, old, new) in [
        (2, "nodes[0].label", "Original node", "New node"),
        (3, "panels[0].title", "Panel", "New panel"),
    ] {
        let mut input = base.clone();
        input["planning_version"] = json!(version);
        input["plan_field"] = json!(field);
        input["expected_text"] = json!(old);
        input["text"] = json!(new);
        tool.implementation.execute(input).await.unwrap();
    }
    assert_eq!(store.get("p", &figure.id).unwrap().revision, 1);
    assert_eq!(store.revision("p", &figure.id, 1).unwrap().document, scene);
    assert!(store.proposals("p", &figure.id).unwrap().is_empty());
    store.save("p", &figure.id, 1, "Figure", &scene).unwrap();
    let mut stale = base;
    stale["expected_revision"] = json!(2);
    stale["planning_version"] = json!(4);
    assert!(tool.implementation.execute(stale).await.is_err());
    assert_eq!(store.planning("p", &figure.id).unwrap().unwrap().version, 4);
}

#[tokio::test]
async fn label_batch_is_all_or_nothing_and_creates_one_plan_version() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("figures.sqlite");
    let mut store = visualization::Store::open(&path).unwrap();
    store.create_project("p", "Study").unwrap();
    let scene = json!({"elements":[],"files":{}});
    let figure = store
        .create("p", "Figure", "excalidraw/v2", &scene)
        .unwrap();
    let brief:visualization::planning::Brief=serde_json::from_value(json!({"message":"Explicit scope","audience":"Researchers","panels":[{"id":"p","title":"Original panel","column":0,"row":0}],"nodes":[{"id":"a","label":"Original node","panel":"p","evidence":["node source"]},{"id":"b","label":"Other node","panel":"p"}],"edges":[{"from":"a","to":"b","label":"Original edge","evidence":["edge source"]}],"assumptions":["Qualifier"],"questions":[{"id":"q","question":"Species?","answer":"Human"}]})).unwrap();
    let initial = store.save_planning("p", &figure.id, 1, 0, &brief).unwrap();
    let tool = visualization_tools::registrations(path)
        .into_iter()
        .find(|t| t.definition.name == "visualization_update_plan_labels")
        .unwrap();
    let changes = json!([{"plan_field":"nodes[0].label","expected_text":"Original node","text":"Reviewed node"},{"plan_field":"edges[0].label","expected_text":"Original edge","text":"Reviewed edge"},{"plan_field":"panels[0].title","expected_text":"Original panel","text":"Reviewed panel"}]);
    let base = json!({"project_id":"p","figure_id":figure.id,"expected_revision":1,"planning_version":1,"changes":changes});
    let mut invalids = vec![];
    for (key, value) in [
        ("expected_text", json!("wrong")),
        ("plan_field", json!("message")),
        ("text", json!("")),
        ("text", json!("Original panel")),
        ("evidence", json!(["injected source"])),
    ] {
        let mut bad = base.clone();
        bad["changes"][2][key] = value;
        invalids.push(bad);
    }
    let mut duplicate = base.clone();
    duplicate["changes"][2] = changes[0].clone();
    invalids.push(duplicate);
    for value in [
        json!([]),
        json!(vec![changes[0].clone(); 21]),
        json!("quoted array"),
    ] {
        let mut bad = base.clone();
        bad["changes"] = value;
        invalids.push(bad);
    }
    for bad in invalids {
        assert!(tool.implementation.execute(bad).await.is_err());
        assert_eq!(
            serde_json::to_value(store.planning("p", &figure.id).unwrap().unwrap()).unwrap(),
            serde_json::to_value(&initial).unwrap()
        );
        assert!(store.planning_revision("p", &figure.id, 2).is_err());
    }
    let result =
        serde_json::to_value(tool.implementation.execute(base.clone()).await.unwrap()).unwrap();
    assert_eq!(result["content"]["version"], 2);
    assert_eq!(result["content"]["changed_labels"], changes);
    assert_eq!(result["content"]["scope"], "saved_plan_label_batch");
    assert!(result["content"].get("document").is_none());
    let mut expected = serde_json::to_value(&brief).unwrap();
    expected["nodes"][0]["label"] = json!("Reviewed node");
    expected["edges"][0]["label"] = json!("Reviewed edge");
    expected["panels"][0]["title"] = json!("Reviewed panel");
    assert_eq!(
        serde_json::to_value(store.planning("p", &figure.id).unwrap().unwrap().document).unwrap(),
        expected
    );
    assert!(tool.implementation.execute(base).await.is_err());
    assert_eq!(store.planning("p", &figure.id).unwrap().unwrap().version, 2);
    assert_eq!(
        serde_json::to_value(
            store
                .planning_revision("p", &figure.id, 1)
                .unwrap()
                .document
        )
        .unwrap(),
        serde_json::to_value(&brief).unwrap()
    );
    assert_eq!(store.get("p", &figure.id).unwrap().revision, 1);
    assert_eq!(store.revision("p", &figure.id, 1).unwrap().document, scene);
    assert!(store.proposals("p", &figure.id).unwrap().is_empty());
}
