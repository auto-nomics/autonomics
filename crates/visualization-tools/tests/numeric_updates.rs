use serde_json::json;
#[tokio::test]
async fn invalid_numeric_update_identifies_the_field_without_creating_a_child() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("figures.sqlite");
    let mut store = visualization::Store::open(&path).unwrap();
    store.create_project("p", "Study").unwrap();
    let original = json!({"elements":[{"id":"node","type":"rectangle","x":0,"y":0,"width":220,"height":100,"angle":0}],"files":{}});
    let figure = store
        .create("p", "Figure", "excalidraw/v2", &original)
        .unwrap();
    let parent = store
        .propose("p", &figure.id, 1, "Parent", &original)
        .unwrap();
    let tool = visualization_tools::registrations(path).remove(0);
    for field in ["width", "height", "fontSize", "lineHeight", "opacity"] {
        let mut changes = serde_json::Map::new();
        changes.insert(field.into(), json!("240"));
        let input = json!({"action":"revise_proposal","project_id":"p","figure_id":figure.id,"proposal_id":parent.id,"document_sha256":parent.document_sha256,"expected_revision":1,"title":"Invalid numeric edit","operations":[{"op":"update","id":"node","changes":changes}]});
        let error = tool
            .implementation
            .execute(input)
            .await
            .unwrap_err()
            .to_string();
        assert!(
            error.contains(&format!("operations[0].changes.{field}")),
            "{error}"
        );
    }
    for (operation, field) in [
        (json!({"op":"update","id":"node","text":"Changed"}), "text"),
        (
            json!({"op":"update","id":"node","height":240,"changes":{}}),
            "height",
        ),
        (json!({"op":"update","id":"node","changes":null}), "changes"),
        (json!({"op":"update","id":"node","changes":[]}), "changes"),
        (json!({"op":"update","id":"node","changes":"{}"}), "changes"),
    ] {
        let input = json!({"action":"revise_proposal","project_id":"p","figure_id":figure.id,"proposal_id":parent.id,"document_sha256":parent.document_sha256,"expected_revision":1,"title":"Invalid update envelope","operations":[{"op":"update","id":"node","changes":{"backgroundColor":"red"}},operation]});
        let error = tool
            .implementation
            .execute(input)
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains(&format!("operations[1].{field}")), "{error}");
        assert!(
            error.contains("\"changes\":{") && error.contains("nothing was saved"),
            "{error}"
        );
        assert!(!error.contains("For translate"), "{error}");
    }
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
