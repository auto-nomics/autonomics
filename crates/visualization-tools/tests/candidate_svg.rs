use serde_json::{Value, json};
use visualization::Store;
fn metadata(result: Value) -> Value {
    assert_eq!(result["content"][1]["type"], "image");
    assert_eq!(result["content"][1]["source"]["media_type"], "image/png");
    let text = result["content"][0]["text"].as_str().unwrap();
    assert!(
        !text.contains("data:image"),
        "PNG bytes travel only as vision blocks"
    );
    serde_json::from_str(text).unwrap()
}
#[tokio::test]
async fn registered_tool_refines_pending_svg_and_never_reads_the_empty_formal_scene_instead() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("figures.sqlite");
    let mut store = Store::open(&path).unwrap();
    store.create_project("p", "Study").unwrap();
    let original = json!({"elements":[],"files":{}});
    let figure = store
        .create("p", "Figure", "excalidraw/v2", &original)
        .unwrap();
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../apps/gui/tests/fixtures/scientific-egfr-mapk-illustrated.json"
    ))
    .unwrap();
    let parent = store
        .propose(
            "p",
            &figure.id,
            1,
            "Pending materialized scene",
            &fixture["document"],
        )
        .unwrap();
    let tool = visualization_tools::bioicons_registrations(path).remove(0);
    let input = |action: &str, proposal: &visualization::proposal::Proposal| json!({"action":action,"project_id":"p","figure_id":figure.id,"expected_revision":1,"element_id":"node-1","proposal_id":proposal.id,"document_sha256":proposal.document_sha256});
    assert!(tool.implementation.execute(json!({"action":"palette","project_id":"p","figure_id":figure.id,"expected_revision":1,"element_id":"node-1"})).await.is_err());
    let palette = metadata(
        serde_json::to_value(
            tool.implementation
                .execute(input("palette", &parent))
                .await
                .unwrap(),
        )
        .unwrap(),
    );
    assert_eq!(palette["scope"], "proposal_candidate");
    assert_eq!(palette["proposal_id"], parent.id);
    assert_eq!(palette["document_sha256"], parent.document_sha256);
    let mut review_on_read = input("parts", &parent);
    review_on_read["markup_ids"] = json!([]);
    assert!(
        tool.implementation
            .execute(review_on_read)
            .await
            .unwrap_err()
            .to_string()
            .contains("read-only")
    );
    let mut bad = input("palette", &parent);
    bad["document_sha256"] = json!("wrong");
    assert!(tool.implementation.execute(bad).await.is_err());
    assert_eq!(store.proposals("p", &figure.id).unwrap().len(), 1);
    let old = palette["colors"][0].as_str().unwrap();
    let mut recolor = input("recolor", &parent);
    recolor["colors"] = json!({old:"#123456"});
    recolor["markup_ids"] = json!([]);
    let result = serde_json::to_value(tool.implementation.execute(recolor).await.unwrap()).unwrap();
    let child = store
        .proposal(
            "p",
            &figure.id,
            result["content"]["proposal_id"].as_str().unwrap(),
        )
        .unwrap();
    assert_eq!(result["content"]["parent_proposal_id"], parent.id);
    assert_eq!(result["content"]["document_sha256"], child.document_sha256);
    let child_palette = metadata(
        serde_json::to_value(
            tool.implementation
                .execute(input("palette", &child))
                .await
                .unwrap(),
        )
        .unwrap(),
    );
    assert!(
        child_palette["colors"]
            .as_array()
            .unwrap()
            .contains(&json!("#123456"))
    );
    assert_ne!(child_palette["file_id"], palette["file_id"]);
    let parts_result = serde_json::to_value(
        tool.implementation
            .execute(input("parts", &child))
            .await
            .unwrap(),
    )
    .unwrap();
    let original_png = parts_result["content"][1]["source"]["data"].clone();
    let parts = metadata(parts_result);
    assert_eq!(parts["file_id"], child_palette["file_id"]);
    let mut geometry = input("geometry", &child);
    geometry["sha256"] = parts["sha256"].clone();
    geometry["part_id"] = parts["parts"][0]["part_id"].clone();
    let handles = metadata(
        serde_json::to_value(tool.implementation.execute(geometry).await.unwrap()).unwrap(),
    );
    assert_eq!(handles["file_id"], parts["file_id"]);
    assert!(!handles["points"].as_array().unwrap().is_empty());
    let mut edit = input("edit_parts", &child);
    edit["sha256"] = parts["sha256"].clone();
    edit["edits"] = json!([{"part_id":parts["parts"][0]["part_id"],"opacity":0.8}]);
    edit["markup_ids"] = json!([]);
    let result = serde_json::to_value(tool.implementation.execute(edit).await.unwrap()).unwrap();
    let refined = store
        .proposal(
            "p",
            &figure.id,
            result["content"]["proposal_id"].as_str().unwrap(),
        )
        .unwrap();
    assert_eq!(
        refined.parent_proposal_id.as_deref(),
        Some(child.id.as_str())
    );
    let refined_result = serde_json::to_value(
        tool.implementation
            .execute(input("parts", &refined))
            .await
            .unwrap(),
    )
    .unwrap();
    assert_ne!(
        refined_result["content"][1]["source"]["data"], original_png,
        "opacity change affects the actual preview"
    );
    let refined_parts = metadata(refined_result);
    assert_ne!(refined_parts["sha256"], parts["sha256"]);
    let visual = visualization::image::element(&refined.document, "node-1").unwrap();
    assert_eq!(
        visual["customData"]["modifications"]
            .as_array()
            .unwrap()
            .last()
            .unwrap()["edits"][0]["opacity"],
        0.8
    );
    assert_eq!(
        store
            .proposal("p", &figure.id, &parent.id)
            .unwrap()
            .document,
        parent.document
    );
    assert_eq!(
        store.proposal("p", &figure.id, &child.id).unwrap().document,
        child.document
    );
    assert_eq!(
        store.revision("p", &figure.id, 1).unwrap().document,
        original
    );
    assert_eq!(store.get("p", &figure.id).unwrap().revision, 1);
    store.reject_proposal("p", &figure.id, &refined.id).unwrap();
    assert!(
        tool.implementation
            .execute(input("parts", &refined))
            .await
            .is_err()
    );
}
