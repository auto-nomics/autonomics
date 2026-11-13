use serde_json::{Value, json};
use visualization::scene::{self, Operation};
fn source() -> Value {
    json!({"elements":[{"id":"frame","type":"frame","x":0,"y":0,"width":500,"height":500,"angle":0},{"id":"label","type":"text","x":20,"y":20,"width":120,"height":50,"angle":0,"containerId":null,"frameId":"frame","groupIds":["art-group"],"text":"Complete\nlabel","originalText":"Complete label","fontSize":20,"fontFamily":9,"lineHeight":1.25,"customData":{"evidence":["fixture"]}},{"id":"image","type":"image","groupIds":["art-group"],"x":150,"y":20,"width":80,"height":100,"customData":{"semanticAnchorId":"anchor"}}],"files":{"asset":{"dataURL":"private-source-bytes"}},"historicalPlan":{"version":1}})
}
fn edit() -> Operation {
    Operation::SetTextPosition {
        id: "label".into(),
        x: 40.,
        y: 80.,
    }
}
#[test]
fn label_only_position_preserves_group_artwork_and_all_nonposition_fields() {
    let before = source();
    let after = scene::apply(&before, &[edit()]).unwrap();
    let mut restored = after.clone();
    restored["elements"][1]["x"] = before["elements"][1]["x"].clone();
    restored["elements"][1]["y"] = before["elements"][1]["y"].clone();
    for key in ["version", "versionNonce", "updated"] {
        restored["elements"][1].as_object_mut().unwrap().remove(key);
    }
    assert_eq!(restored, before);
    assert_eq!(after["elements"][1]["x"], 40.);
    assert_eq!(after["elements"][1]["y"], 80.);
}
#[test]
fn bound_locked_rotated_connected_and_out_of_frame_moves_fail_atomically() {
    for (key, value) in [
        ("locked", json!(true)),
        ("containerId", json!("node")),
        ("angle", json!(0.2)),
        ("width", json!("120")),
        ("frameId", json!(4)),
        ("boundElements", json!([{"id":"arrow","type":"arrow"}])),
    ] {
        let mut doc = source();
        doc["elements"][1][key] = value;
        let before = doc.clone();
        assert!(scene::apply(&doc, &[edit()]).is_err(), "{key}");
        assert_eq!(doc, before);
    }
    let mut doc = source();
    doc["elements"]
        .as_array_mut()
        .unwrap()
        .push(json!({"id":"arrow","type":"arrow","startBinding":{"elementId":"label"}}));
    assert!(scene::apply(&doc, &[edit()]).is_err());
    let doc = source();
    assert!(
        scene::apply(
            &doc,
            &[Operation::SetTextPosition {
                id: "label".into(),
                x: 490.,
                y: 20.
            }]
        )
        .is_err()
    );
    let mut locked = source();
    locked["elements"][0]["locked"] = json!(true);
    assert!(scene::apply(&locked, &[edit()]).is_err());
    assert!(
        scene::apply(
            &doc,
            &[
                edit(),
                Operation::SetTextPosition {
                    id: "label".into(),
                    x: 490.,
                    y: 20.
                }
            ]
        )
        .is_err()
    );
    assert_eq!(doc, source());
}
