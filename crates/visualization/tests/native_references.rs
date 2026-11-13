use serde_json::{Value, json};
use visualization::scene::{self, Operation};
fn doc() -> Value {
    json!({"elements":[{"id":"n","type":"rectangle","x":0,"y":0,"width":100,"height":100,"boundElements":[],"groupIds":[]},{"id":"label","type":"text","x":10,"y":10,"width":60,"height":20,"text":"Scientific label","fontSize":16,"containerId":null,"boundElements":[]},{"id":"edge","type":"arrow","x":100,"y":50,"width":100,"height":0,"points":[[0,0],[100,0]],"startBinding":{"elementId":"n","focus":0,"gap":0}}],"files":{}})
}
fn update(id: &str, changes: Value) -> Operation {
    Operation::Update {
        id: id.into(),
        changes: changes.as_object().unwrap().clone(),
    }
}
#[test]
fn malformed_native_references_cannot_enter_through_generic_updates() {
    let before = doc();
    for changes in [
        json!({"boundElements":""}),
        json!({"groupIds":""}),
        json!({"containerId":""}),
        json!({"frameId":""}),
        json!({"boundElements":[{"id":"edge","type":"wrong"}]}),
        json!({"boundElements":[{"id":"edge","type":"arrow"},{"id":"edge","type":"arrow"}]}),
        json!({"groupIds":[false]}),
    ] {
        assert!(matches!(
            scene::apply(&before, &[update("n", changes)]),
            Err(scene::SceneError::InvalidUpdate(_))
        ));
    }
}
#[test]
fn valid_empty_and_nonempty_native_arrays_and_optional_ids_remain_supported() {
    let before = doc();
    let after=scene::apply(&before,&[update("n",json!({"boundElements":[{"id":"edge","type":"arrow"},{"id":"label","type":"text"}],"groupIds":["component"],"frameId":null})),update("label",json!({"containerId":"n","groupIds":["component"]}))]).unwrap();
    assert_eq!(
        after["elements"][0]["boundElements"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert_eq!(after["elements"][1]["containerId"], "n");
    assert_eq!(after["elements"][1]["text"], before["elements"][1]["text"]);
    assert!(
        scene::apply(
            &after,
            &[update("n", json!({"boundElements":[],"groupIds":[]}))]
        )
        .is_ok()
    );
}
#[test]
fn invalid_reference_update_rejects_a_whole_batch_without_partial_style_changes() {
    let before = doc();
    let saved = before.clone();
    assert!(
        scene::apply(
            &before,
            &[
                update("n", json!({"strokeColor":"#ff0000"})),
                update("label", json!({"boundElements":""}))
            ]
        )
        .is_err()
    );
    assert_eq!(before, saved);
}
