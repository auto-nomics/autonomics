use serde_json::{Value, json};
use visualization::scene::{self, IllustrationMember, Operation};
fn fixture() -> Value {
    json!({"elements":[{"id":"frame","type":"frame","x":0,"y":0,"width":500,"height":500,"angle":0},{"id":"anchor","type":"rectangle","x":100,"y":100,"width":100,"height":100,"angle":0,"frameId":"frame","customData":{"scientificId":"complex","evidence":["retained"]},"boundElements":[{"id":"label","type":"text"}]},{"id":"primary","type":"image","x":110,"y":110,"width":60,"height":80,"angle":0,"frameId":"frame","fileId":"f","customData":{"source":"Fixture","asset":{"author":"Artist","license":"CC0"}}},{"id":"label","type":"text","x":100,"y":220,"width":200,"height":25,"angle":0,"frameId":"frame","containerId":"anchor","text":"Complete complex","originalText":"Complete complex","fontSize":20},{"id":"part","type":"image","x":180,"y":110,"width":60,"height":80,"angle":0,"frameId":"frame","fileId":"f","customData":{"source":"Fixture"}}],"files":{"f":{"dataURL":"data:image/png;base64,fixture"}}})
}
fn prepared() -> Value {
    let mut d = scene::apply(
        &fixture(),
        &[Operation::BindIllustration {
            anchor_id: "anchor".into(),
            primary_id: "primary".into(),
            label_ids: vec!["label".into()],
            members: vec![IllustrationMember {
                id: "part".into(),
                meaning: "Generic component".into(),
            }],
        }],
    )
    .unwrap();
    let mut label = d["elements"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["id"] == "label")
        .unwrap()
        .clone();
    label["id"] = json!("extra");
    label["containerId"] = json!("");
    label["groupIds"] = json!([]);
    label["boundElements"] = json!("");
    d["elements"].as_array_mut().unwrap().push(label);
    d
}
fn get<'a>(d: &'a Value, id: &str) -> &'a Value {
    d["elements"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["id"] == id)
        .unwrap()
}
#[test]
fn explicit_label_attachment_preserves_metadata_and_follows_component_moves() {
    let before = prepared();
    let after = scene::apply(
        &before,
        &[Operation::AttachIllustrationLabel {
            id: "extra".into(),
            anchor_id: "anchor".into(),
        }],
    )
    .unwrap();
    let l = get(&after, "extra");
    assert_eq!(l["containerId"], Value::Null);
    assert_eq!(l["boundElements"], json!([]));
    for k in [
        "text",
        "originalText",
        "fontSize",
        "x",
        "y",
        "width",
        "height",
    ] {
        assert_eq!(l[k], get(&before, "extra")[k]);
    }
    assert_eq!(l.get("customData"), get(&before, "extra").get("customData"));
    let moved = visualization::arrange::translate(&after, &["anchor".into()], 0., 10.).unwrap();
    assert_eq!(
        get(&moved, "extra")["y"].as_f64().unwrap(),
        l["y"].as_f64().unwrap() + 10.
    );
    assert_eq!(after["files"], before["files"]);
}
#[test]
fn attachment_rejects_locked_foreign_group_frame_and_live_containers() {
    for case in 0..4 {
        let mut d = prepared();
        let label = d["elements"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|e| e["id"] == "extra")
            .unwrap();
        match case {
            0 => label["locked"] = json!(true),
            1 => label["frameId"] = Value::Null,
            2 => label["groupIds"] = json!(["foreign"]),
            _ => label["containerId"] = json!("primary"),
        };
        let before = d.clone();
        assert!(
            scene::apply(
                &d,
                &[Operation::AttachIllustrationLabel {
                    id: "extra".into(),
                    anchor_id: "anchor".into()
                }]
            )
            .is_err()
        );
        assert_eq!(d, before);
    }
}
#[test]
fn binding_does_not_add_empty_metadata_to_original_labels() {
    let before = fixture();
    let after = scene::apply(
        &before,
        &[Operation::BindIllustration {
            anchor_id: "anchor".into(),
            primary_id: "primary".into(),
            label_ids: vec!["label".into()],
            members: vec![],
        }],
    )
    .unwrap();
    assert_eq!(
        get(&after, "label").get("customData"),
        get(&before, "label").get("customData")
    );
}
