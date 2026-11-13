use serde_json::json;

#[tokio::test]
async fn registered_tool_requires_explicit_alternatives_and_continues_leaf() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("figures.sqlite");
    let mut store = visualization::Store::open(&path).unwrap();
    store.create_project("p", "Study").unwrap();
    let document =
        json!({"elements":[{"id":"a","type":"rectangle","x":0,"y":0,"width":100,"height":100,"angle":0,"groupIds":[],"locked":false}]});
    let figure = store
        .create("p", "Figure", "excalidraw/v2", &document)
        .unwrap();
    let parent = store
        .propose("p", &figure.id, 1, "root", &document)
        .unwrap();
    let tool = visualization_tools::registrations(path.clone()).remove(0);
    let input = |parent: &visualization::proposal::Proposal| json!({"action":"revise_proposal","project_id":"p","figure_id":figure.id,"proposal_id":parent.id,"document_sha256":parent.document_sha256,"expected_revision":1,"title":"Move","operations":[{"op":"update","id":"a","changes":{"x":10}}]});
    tool.implementation.execute(input(&parent)).await.unwrap();
    let error = tool
        .implementation
        .execute(input(&parent))
        .await
        .unwrap_err()
        .to_string();
    assert!(error.contains("pending child"), "{error}");
    assert_eq!(store.proposals("p", &figure.id).unwrap().len(), 2);
    let child = store
        .proposals("p", &figure.id)
        .unwrap()
        .into_iter()
        .find(|p| p.id != parent.id)
        .unwrap();
    tool.implementation.execute(input(&child)).await.unwrap();
    let translate = visualization_tools::registrations(path).remove(2);
    let error = translate.implementation.execute(json!({"project_id":"p","figure_id":figure.id,"proposal_id":parent.id,"document_sha256":parent.document_sha256,"expected_revision":1,"title":"Move component","element_ids":["a"],"dx":5,"dy":5})).await.unwrap_err().to_string();
    assert!(error.contains("pending child"), "{error}");
    assert_eq!(store.proposals("p", &figure.id).unwrap().len(), 3);
    let mut alternative = input(&parent);
    alternative["allow_branch"] = json!(true);
    tool.implementation.execute(alternative).await.unwrap();
    let all = store.proposals("p", &figure.id).unwrap();
    assert_eq!(all.len(), 4);
    assert_eq!(
        all.iter()
            .filter(|p| p.parent_proposal_id.as_deref() == Some(parent.id.as_str()))
            .count(),
        2
    );
    assert_eq!(store.get("p", &figure.id).unwrap().revision, 1);
    assert_eq!(
        store
            .proposal("p", &figure.id, &parent.id)
            .unwrap()
            .document,
        document
    );
}
