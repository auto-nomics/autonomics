use serde_json::{Value, json};
use visualization::scene::{Operation, apply};
fn shape(id: &str, x: f64, y: f64) -> Value {
    json!({"id":id,"type":"rectangle","x":x,"y":y,"width":100,"height":60,"angle":0,"groupIds":[]})
}
fn element<'a>(doc: &'a Value, id: &str) -> &'a Value {
    doc["elements"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["id"] == id)
        .unwrap()
}
fn translate(ids: &[&str], dx: f64, dy: f64) -> Operation {
    Operation::Translate {
        element_ids: ids.iter().map(|id| id.to_string()).collect(),
        dx,
        dy,
    }
}
#[test]
fn rigid_translation_keeps_owned_members_ids_files_science_and_external_connections() {
    let mut anchor = shape("anchor", 10., 20.);
    anchor["groupIds"] = json!(["component"]);
    anchor["customData"] = json!({"scientificId":"receptor","evidence":["https://example.org/record"],"visualAssetElementId":"art","visualLabelIds":["label"],"visualAssetGroupId":"component","planningSource":{"version":3,"document":{"message":"Historical plan"}}});
    let scene = json!({"elements":[anchor,{"id":"art","type":"image","x":20,"y":20,"width":80,"height":60,"angle":0,"fileId":"f","groupIds":["component"],"customData":{"semanticAnchorId":"anchor","asset":{"author":"Fixture","license":"cc-0"}}},{"id":"label","type":"text","x":20,"y":90,"width":80,"height":20,"angle":0,"text":"Receptor","originalText":"Receptor","groupIds":["component"]},shape("other",310.,20.),{"id":"arrow","type":"arrow","x":110,"y":50,"width":200,"height":0,"angle":0,"points":[[0,0],[200,0]],"roughness":0,"roundness":null,"startBinding":{"elementId":"anchor"},"endBinding":{"elementId":"other"},"customData":{"evidence":["source"],"direction":"forward"}},{"id":"arrow-label","type":"text","containerId":"arrow","x":190,"y":40,"width":40,"height":20,"angle":0,"text":"signal","originalText":"signal"}],"files":{"f":{"dataURL":"data:image/svg+xml;base64,fixture","mimeType":"image/svg+xml"}},"appState":{"zoom":{"value":1}}});
    let moved = apply(&scene, &[translate(&["anchor", "art", "label"], 20., 70.)]).unwrap();
    for id in ["anchor", "art", "label"] {
        assert_eq!(
            element(&moved, id)["x"].as_f64().unwrap(),
            element(&scene, id)["x"].as_f64().unwrap() + 20.
        );
        assert_eq!(
            element(&moved, id)["y"].as_f64().unwrap(),
            element(&scene, id)["y"].as_f64().unwrap() + 70.
        );
        assert_eq!(
            element(&moved, id)["customData"],
            element(&scene, id)["customData"]
        );
    }
    assert_eq!(element(&moved, "arrow")["x"], 130.);
    assert_eq!(element(&moved, "arrow")["y"], 120.);
    assert_eq!(
        element(&moved, "arrow")["points"],
        json!([[0., 0.], [180., -70.]])
    );
    assert_eq!(
        element(&moved, "arrow")["customData"],
        element(&scene, "arrow")["customData"]
    );
    assert_eq!(element(&moved, "arrow-label")["x"], 200.);
    assert_eq!(element(&moved, "arrow-label")["y"], 75.);
    assert_eq!(element(&moved, "other"), element(&scene, "other"));
    assert_eq!(moved["files"], scene["files"]);
    assert_eq!(moved["appState"], scene["appState"]);
    assert_eq!(moved["elements"].as_array().unwrap().len(), 6);
    assert_eq!(element(&scene, "anchor")["x"], 10.);
}
#[test]
fn frames_curves_and_ink_move_once_and_failures_are_atomic() {
    let mut frame = shape("frame", 0., 0.);
    frame["type"] = json!("frame");
    frame["width"] = json!(500);
    frame["height"] = json!(400);
    let mut a = shape("a", 20., 20.);
    a["frameId"] = json!("frame");
    let mut b = shape("b", 300., 20.);
    b["frameId"] = json!("frame");
    let scene = json!({"elements":[frame,a,b,{"id":"curve","type":"arrow","frameId":"frame","x":100,"y":50,"width":200,"height":50,"angle":0.2,"roughness":1,"roundness":{"type":2},"points":[[0,0],[100,50],[200,0]],"startBinding":{"elementId":"a"},"endBinding":{"elementId":"b"}},{"id":"ink","type":"freedraw","frameId":"frame","x":60,"y":100,"width":20,"height":30,"angle":0.3,"points":[[0,0],[20,30]],"pressures":[0.2,0.8]}]});
    let moved = apply(&scene, &[translate(&["frame", "a"], 12., 25.)]).unwrap();
    for e in scene["elements"].as_array().unwrap() {
        let actual = element(&moved, e["id"].as_str().unwrap());
        assert_eq!(
            actual["x"].as_f64().unwrap(),
            e["x"].as_f64().unwrap() + 12.
        );
        assert_eq!(
            actual["y"].as_f64().unwrap(),
            e["y"].as_f64().unwrap() + 25.
        );
        for field in [
            "points",
            "pressures",
            "startBinding",
            "endBinding",
            "frameId",
            "angle",
        ] {
            assert_eq!(actual[field], e[field]);
        }
    }
    let original = scene.clone();
    for op in [
        translate(&["a"], 10., 0.),
        translate(&["curve"], 10., 0.),
        translate(&["missing"], 10., 0.),
        translate(&["a", "a"], 1., 0.),
        translate(&[], 1., 0.),
        translate(&["a"], f64::INFINITY, 0.),
    ] {
        assert!(apply(&scene, &[op]).is_err());
        assert_eq!(scene, original);
    }
    let mut locked = scene.clone();
    locked["elements"][2]["locked"] = json!(true);
    assert!(apply(&locked, &[translate(&["frame"], 10., 0.)]).is_err());
    let mut protected = scene.clone();
    protected["elements"][4]["customData"] = json!({"annotationKind":"inset_region"});
    assert!(apply(&protected, &[translate(&["frame"], 10., 0.)]).is_err());
    let mut edge = shape("edge", 999999., 0.);
    edge["width"] = json!(1);
    let out = json!({"elements":[edge]});
    assert!(apply(&out, &[translate(&["edge"], 2., 0.)]).is_err());
    assert!(
        serde_json::from_value::<Operation>(
            json!({"op":"translate","element_ids":["a"],"dx":0,"dy":"79"})
        )
        .is_err()
    );
}
