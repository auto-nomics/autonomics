use serde_json::{Value, json};
#[tokio::test]
async fn typed_translation_preserves_existing_note_ids_and_rejects_quoted_positions_without_writing()
 {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("figures.sqlite");
    let mut store = visualization::Store::open(&path).unwrap();
    store.create_project("p", "Review").unwrap();
    let notes=(0..3).map(|i|json!({"id":format!("note-{i}"),"type":"text","x":0,"y":200+i*35,"width":400,"height":20,"angle":0,"text":format!("Source note {i}"),"originalText":format!("Source note {i}"),"fontFamily":9,"fontSize":16,"customData":{"evidence":["https://example.org/source"]}})).collect::<Vec<_>>();
    let original =
        json!({"elements":notes,"files":{},"appState":{"viewBackgroundColor":"#ffffff"}});
    let figure = store
        .create("p", "Figure", "excalidraw/v2", &original)
        .unwrap();
    let parent = store
        .propose("p", &figure.id, 1, "Pending notes", &original)
        .unwrap();
    let tool = visualization_tools::registrations(path).remove(0);
    let input = json!({"action":"revise_proposal","project_id":"p","figure_id":figure.id,"proposal_id":parent.id,"document_sha256":parent.document_sha256,"expected_revision":1,"markup_ids":[],"title":"Move source notes without replacing them","operations":[{"op":"translate","element_ids":["note-0","note-1","note-2"],"dx":0,"dy":79}]});
    let mut malformed = input.clone();
    malformed["operations"][0]["dy"] = json!("79");
    let error = tool
        .implementation
        .execute(malformed)
        .await
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("operations[0].dy") && error.contains("JSON number"),
        "{error}"
    );
    let mut legacy = input.clone();
    legacy["operations"] = json!([{"op":"update","id":"note-0","changes":{"y":"294"}}]);
    let error = tool
        .implementation
        .execute(legacy)
        .await
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("operations[0].changes.y") && error.contains("translate"),
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
    assert_eq!(child.document["elements"].as_array().unwrap().len(), 3);
    for (i, actual) in child.document["elements"]
        .as_array()
        .unwrap()
        .iter()
        .enumerate()
    {
        let mut expected = original["elements"][i].clone();
        expected["y"] = json!(279 + i * 35);
        let clean = |v: &Value| {
            v.as_object()
                .unwrap()
                .iter()
                .filter(|(k, _)| !["version", "updated", "versionNonce"].contains(&k.as_str()))
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect::<serde_json::Map<_, _>>()
        };
        let actual = clean(actual);
        let expected = clean(&expected);
        for (k, v) in expected {
            if k == "x" || k == "y" {
                assert_eq!(actual[&k].as_f64(), v.as_f64());
            } else {
                assert_eq!(actual[&k], v);
            }
        }
    }
    assert_eq!(child.document["files"], original["files"]);
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
    assert_eq!(store.get("p", &figure.id).unwrap().revision, 1);
}
