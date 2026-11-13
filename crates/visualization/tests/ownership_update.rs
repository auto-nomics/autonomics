use serde_json::{Value, json};
use visualization::scene::{self, Operation};
fn doc() -> Value {
    json!({"elements":[{"id":"node","type":"rectangle","x":0,"y":0,"width":100,"height":100,"customData":{"scientificId":"protein","visualAssetElementId":"image","visualLabelIds":["label"],"visualAnchorFit":{"fileId":"f"},"evidence":["source"]}},{"id":"image","type":"image","x":0,"y":0,"width":100,"height":100,"fileId":"f","customData":{"semanticAnchorId":"node","asset":{"author":"Artist","license":"CC0"}}}],"files":{"f":{"dataURL":"data:image/png;base64,fixture"}}})
}
fn update(id: &str, metadata: Value) -> Operation {
    Operation::Update {
        id: id.into(),
        changes: serde_json::from_value(json!({"customData":metadata})).unwrap(),
    }
}
#[test]
fn generic_update_cannot_forge_or_erase_owned_illustration_records() {
    let before = doc();
    for (id, key) in [
        ("node", "visualAssetElementId"),
        ("node", "visualLabelIds"),
        ("node", "visualAnchorFit"),
        ("image", "semanticAnchorId"),
        ("image", "semanticBadgeAnchorId"),
        ("node", "visualBadgeIds"),
        ("node", "visualAssetSlot"),
        ("node", "visualAssetGroupId"),
        ("image", "visualBadgeGroupId"),
        ("image", "badgeMeaning"),
    ] {
        let mut data = before["elements"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["id"] == id)
            .unwrap()["customData"]
            .clone();
        data[key] = json!("forged");
        let saved = before.clone();
        let result = scene::apply(&before, &[update(id, data)]);
        assert!(matches!(result, Err(scene::SceneError::InvalidUpdate(_))));
        assert_eq!(before, saved);
    }
    assert!(scene::apply(&before, &[update("node", Value::Null)]).is_err());
}
#[test]
fn unrelated_metadata_update_preserves_all_ownership_and_files() {
    let before = doc();
    let mut metadata = before["elements"][0]["customData"].clone();
    metadata["reviewNote"] = json!("Keep the complete scientific label");
    let next = scene::apply(&before, &[update("node", metadata.clone())]).unwrap();
    assert_eq!(next["elements"][0]["customData"], metadata);
    assert_eq!(next["elements"][1], before["elements"][1]);
    assert_eq!(next["files"], before["files"]);
}
#[test]
fn invalid_later_ownership_edit_rejects_the_whole_operation_batch() {
    let before = doc();
    let ops = vec![
        Operation::Update {
            id: "node".into(),
            changes: serde_json::from_value(json!({"strokeColor":"#ff0000"})).unwrap(),
        },
        update("image", json!({"semanticAnchorId":"another"})),
    ];
    let saved = before.clone();
    assert!(scene::apply(&before, &ops).is_err());
    assert_eq!(before, saved);
}
