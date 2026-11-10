use serde_json::{Value, json};
use visualization::{
    scene::{self, Operation},
    sketch::{self, Plan},
};
fn source() -> Value {
    let plan:Plan=serde_json::from_value(json!({"message":"Copy fixture","audience":"Testers","layout":"horizontal","nodes":[{"id":"a","label":"A"},{"id":"b","label":"B"}],"edges":[{"from":"a","to":"b","label":"relation","evidence":["fixture-reference"]}]})).unwrap();
    sketch::compile(&plan).unwrap()
}
fn get<'a>(doc: &'a Value, id: &str) -> &'a Value {
    doc["elements"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["id"] == id)
        .unwrap()
}
fn copy(ids: &[&str], prefix: &str) -> Operation {
    Operation::Duplicate {
        element_ids: ids.iter().map(|id| (*id).into()).collect(),
        id_prefix: prefix.into(),
        dx: 300.,
        dy: 200.,
    }
}
#[test]
fn grouped_illustration_copies_bindings_semantic_members_and_files_without_touching_source() {
    let mut original = source();
    for e in original["elements"].as_array_mut().unwrap() {
        e["groupIds"] = json!(["whole-component"]);
        if e["id"] == "node-0" {
            e["customData"]["visualAssetElementId"] = json!("illustration");
            e["customData"]["visualAssetGroupId"] = json!("whole-component");
            e["customData"]["visualLabelIds"] = json!(["label-0"]);
        }
    }
    let mut image = get(&original, "node-0").clone();
    image["id"] = json!("illustration");
    image["type"] = json!("image");
    image["fileId"] = json!("shared-file");
    image["customData"] = json!({"semanticAnchorId":"node-0","asset":{"author":"Fixture artist","license":"CC0"},"privateNote":"preserve only within this copied document"});
    image["boundElements"] = Value::Null;
    original["elements"].as_array_mut().unwrap().push(image);
    original["files"] =
        json!({"shared-file":{"dataURL":"data:image/png;base64,fixture","mimeType":"image/png"}});
    let changed = scene::apply(&original, &[copy(&["illustration"], "repeat")]).unwrap();
    assert_eq!(changed["elements"].as_array().unwrap().len(), 14);
    assert_eq!(changed["files"], original["files"]);
    for e in original["elements"].as_array().unwrap() {
        assert_eq!(get(&changed, e["id"].as_str().unwrap()), e);
        let duplicate = get(&changed, &format!("repeat:e:{}", e["id"].as_str().unwrap()));
        assert_eq!(
            duplicate["x"].as_f64().unwrap(),
            e["x"].as_f64().unwrap() + 300.
        );
        assert_eq!(
            duplicate["y"].as_f64().unwrap(),
            e["y"].as_f64().unwrap() + 200.
        );
        assert_eq!(duplicate["groupIds"], json!(["repeat:g:whole-component"]));
    }
    assert_eq!(
        get(&changed, "repeat:e:edge-0")["startBinding"]["elementId"],
        "repeat:e:node-0"
    );
    assert_eq!(
        get(&changed, "repeat:e:edge-label-0")["containerId"],
        "repeat:e:edge-0"
    );
    assert_eq!(
        get(&changed, "repeat:e:illustration")["customData"]["semanticAnchorId"],
        "repeat:e:node-0"
    );
    assert_eq!(
        get(&changed, "repeat:e:illustration")["fileId"],
        "shared-file"
    );
    assert_eq!(
        get(&changed, "repeat:e:node-0")["customData"]["visualLabelIds"],
        json!(["repeat:e:label-0"])
    );
    assert_eq!(
        get(&changed, "repeat:e:edge-0")["customData"]["evidence"],
        json!(["fixture-reference"])
    );
}
#[test]
fn isolated_components_detach_external_links_and_new_ids_are_editable_in_same_batch() {
    let original = source();
    let changed = scene::apply(
        &original,
        &[
            copy(&["node-0"], "isolated"),
            Operation::Update {
                id: "isolated:e:node-0".into(),
                changes: json!({"backgroundColor":"#ff0000"})
                    .as_object()
                    .unwrap()
                    .clone(),
            },
        ],
    )
    .unwrap();
    assert_eq!(changed["elements"].as_array().unwrap().len(), 8);
    assert_eq!(
        get(&changed, "isolated:e:node-0")["boundElements"],
        json!([{"type":"text","id":"isolated:e:label-0"}])
    );
    assert_eq!(get(&changed, "edge-0"), get(&original, "edge-0"));
    let arrows = scene::apply(&original, &[copy(&["edge-0"], "edgecopy")]).unwrap();
    assert_eq!(
        get(&arrows, "edgecopy:e:edge-0")["startBinding"],
        Value::Null
    );
    assert_eq!(get(&arrows, "edgecopy:e:edge-0")["endBinding"], Value::Null);
    assert_eq!(
        get(&arrows, "edgecopy:e:edge-0")["points"],
        get(&original, "edge-0")["points"]
    );
}
#[test]
fn invalid_copies_reject_the_whole_batch() {
    let original = source();
    let once = scene::apply(&original, &[copy(&["node-0"], "same")]).unwrap();
    assert!(scene::apply(&once, &[copy(&["node-0"], "same")]).is_err());
    for patch in [
        json!({"locked":true}),
        json!({"customData":{"annotationKind":"review"}}),
        json!({"customData":{"inset":{}}}),
        json!({"customData":"opaque"}),
        json!({"x":1000000}),
    ] {
        let mut invalid = original.clone();
        invalid["elements"][0]
            .as_object_mut()
            .unwrap()
            .extend(patch.as_object().unwrap().clone());
        let before = invalid.clone();
        assert!(scene::apply(&invalid, &[copy(&["node-0"], "fail")]).is_err());
        assert_eq!(invalid, before);
    }
    for operation in [
        copy(&[], "x"),
        copy(&["missing"], "x"),
        copy(&["node-0", "node-0"], "x"),
        copy(&["node-0"], ""),
    ] {
        assert!(scene::apply(&original, &[operation]).is_err());
    }
}

#[test]
fn frame_copies_expand_contents_and_partial_copies_detach_frame_membership() {
    let mut original = source();
    let mut frame = get(&original, "node-0").clone();
    frame["id"] = json!("panel");
    frame["type"] = json!("frame");
    frame["boundElements"] = Value::Null;
    frame["width"] = json!(600);
    frame["height"] = json!(300);
    for e in original["elements"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .filter(|e| e["id"] == "node-0" || e["id"] == "label-0")
    {
        e["frameId"] = json!("panel");
    }
    original["elements"].as_array_mut().unwrap().push(frame);
    let changed = scene::apply(&original, &[copy(&["panel"], "panelcopy")]).unwrap();
    assert_eq!(
        get(&changed, "panelcopy:e:node-0")["frameId"],
        "panelcopy:e:panel"
    );
    assert_eq!(
        get(&changed, "panelcopy:e:label-0")["frameId"],
        "panelcopy:e:panel"
    );
    let partial = scene::apply(&original, &[copy(&["node-0"], "partial")]).unwrap();
    assert_eq!(get(&partial, "partial:e:node-0")["frameId"], Value::Null);
    original["elements"][1]["locked"] = json!(true);
    assert!(scene::apply(&original, &[copy(&["node-0"], "lockedchild")]).is_err());
}
