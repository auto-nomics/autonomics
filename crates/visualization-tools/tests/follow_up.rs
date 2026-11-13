use serde_json::json;
use visualization::planning::Brief;

#[tokio::test]
async fn returned_read_inputs_work_verbatim_and_never_replace_scope_or_revision_checks() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("figures.sqlite");
    let mut store = visualization::Store::open(&path).unwrap();
    store.create_project("project-real", "Study").unwrap();
    store
        .create_project("other-project", "Another study")
        .unwrap();
    let original = json!({"elements":[]});
    let figure = store
        .create("project-real", "Figure", "excalidraw/v2", &original)
        .unwrap();
    let brief: Brief = serde_json::from_value(json!({"message":"Conceptual cell","audience":"Researchers","nodes":[{"id":"cell","label":"Cell"}]})).unwrap();
    for version in 0..3 {
        store
            .save_planning("project-real", &figure.id, 1, version, &brief)
            .unwrap();
    }
    let tool = visualization_tools::registrations(path).remove(0);
    let created = serde_json::to_value(tool.implementation.execute(json!({"action":"sketch_plan","project_id":"project-real","figure_id":figure.id,"expected_revision":1,"planning_version":3,"title":"Confirmed plan"})).await.unwrap()).unwrap();
    let payload = &created["content"];
    let read_input = payload["follow_up_inputs"]["read_proposal"].clone();
    assert_eq!(
        read_input,
        json!({"action":"read_proposal","project_id":"project-real","figure_id":figure.id,"proposal_id":payload["id"],"expected_revision":1})
    );
    assert!(payload.get("document").is_none());
    assert_eq!(
        payload["candidate_context"]["document_sha256"],
        payload["document_sha256"]
    );
    let read = serde_json::to_value(
        tool.implementation
            .execute(read_input.clone())
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(read["content"]["proposal"]["id"], payload["id"]);
    assert_eq!(
        read["content"]["follow_up_inputs"],
        payload["follow_up_inputs"]
    );
    assert!(
        read["content"]["follow_up_inputs"]
            .as_object()
            .unwrap()
            .values()
            .all(|input| input["action"].as_str().unwrap().starts_with("read"))
    );
    for patch in [
        json!({"project_id":"other-project"}),
        json!({"figure_id":"wrong-figure"}),
        json!({"expected_revision":3}),
        json!({"proposal_id":"wrong-candidate"}),
    ] {
        let mut wrong = read_input.clone();
        wrong
            .as_object_mut()
            .unwrap()
            .extend(patch.as_object().unwrap().clone());
        assert!(tool.implementation.execute(wrong).await.is_err());
    }
    assert_eq!(store.get("project-real", &figure.id).unwrap().revision, 1);
    assert_eq!(
        store.proposals("project-real", &figure.id).unwrap().len(),
        1
    );
    let candidate_id = payload["id"].as_str().unwrap();
    let before = store
        .proposal("project-real", &figure.id, candidate_id)
        .unwrap();
    store
        .save(
            "project-real",
            &figure.id,
            1,
            "Updated outside candidate",
            &original,
        )
        .unwrap();
    assert!(
        tool.implementation
            .execute(read_input.clone())
            .await
            .is_err(),
        "a stale continuation must fail, not silently refresh"
    );
    let current = serde_json::to_value(
        tool.implementation
            .execute(payload["follow_up_inputs"]["read"].clone())
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(current["content"]["metadata"]["revision"], 2);
    let mut explicit_current = read_input;
    explicit_current["expected_revision"] = json!(2);
    let historical =
        serde_json::to_value(tool.implementation.execute(explicit_current).await.unwrap()).unwrap();
    assert_eq!(historical["content"]["base_is_current"], false);
    assert_eq!(
        historical["content"]["candidate_context"]["base_revision"],
        1
    );
    assert_eq!(
        historical["content"]["follow_up_inputs"]["read_proposal_preview"]["expected_revision"],
        2
    );
    assert_eq!(
        store
            .proposal("project-real", &figure.id, candidate_id)
            .unwrap()
            .document_sha256,
        before.document_sha256
    );
    assert_eq!(
        store
            .revision("project-real", &figure.id, 2)
            .unwrap()
            .document,
        original
    );
}
