use serde_json::{Value, json};
use visualization::image::{self, Image};
use visualization::scene::{self, Operation};
fn scene() -> Value {
    json!({"files":{},"elements":[
    {"id":"panel","type":"frame","x":0,"y":0,"width":340,"height":500,"angle":0},
    {"id":"node","type":"rectangle","x":40,"y":100,"width":220,"height":100,"angle":0,"frameId":"panel","boundElements":[{"id":"label","type":"text"},{"id":"arrow","type":"arrow"}],"customData":{"scientificId":"receptor","evidence":["retained"]}},
    {"id":"label","type":"text","x":50,"y":125,"width":200,"height":50,"angle":0,"fontFamily":9,"fontSize":20,"lineHeight":1.25,"text":"EGFR receptor complex","originalText":"EGFR receptor complex","containerId":"node","frameId":"panel","customData":{"history":"retained"}},
    {"id":"arrow","type":"arrow","x":150,"y":210,"width":0,"height":140,"angle":0,"points":[[0,0],[0,140]],"startBinding":{"elementId":"node","gap":10},"endBinding":null}
    ]})
}
fn replace(doc: &Value) -> (Value, String) {
    image::illustrate(
        doc,
        "node",
        Image {
            file_id: "asset",
            data_url: "data:image/png;base64,abc",
            width: 40,
            height: 100,
            provenance: json!({"license":"test","source":"fixture"}),
        },
    )
    .unwrap()
}
fn get<'a>(doc: &'a Value, id: &str) -> &'a Value {
    doc["elements"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["id"] == id)
        .unwrap()
}
#[test]
fn narrow_artwork_label_avoids_the_bottom_connector_without_changing_scientific_text() {
    let original = scene();
    let (next, id) = replace(&original);
    let label = get(&next, "label");
    let image = get(&next, &id);
    assert!(
        label["x"].as_f64().unwrap()
            > image["x"].as_f64().unwrap() + image["width"].as_f64().unwrap()
    );
    assert!(label["x"].as_f64().unwrap() + label["width"].as_f64().unwrap() <= 324.);
    assert!(
        label["y"].as_f64().unwrap() >= 100.
            && label["y"].as_f64().unwrap() + label["height"].as_f64().unwrap() <= 200.
    );
    for key in [
        "originalText",
        "fontSize",
        "fontFamily",
        "lineHeight",
        "customData",
        "frameId",
    ] {
        assert_eq!(label[key], get(&original, "label")[key]);
    }
    assert_eq!(
        label["text"]
            .as_str()
            .unwrap()
            .split_whitespace()
            .collect::<String>(),
        "EGFRreceptorcomplex"
    );
    assert_eq!(get(&next, "arrow"), get(&original, "arrow"));
    assert_eq!(get(&next, "panel"), get(&original, "panel"));
    let mut positioned = next.clone();
    let index = positioned["elements"]
        .as_array()
        .unwrap()
        .iter()
        .position(|e| e["id"] == "label")
        .unwrap();
    positioned["elements"][index]["x"] = json!(190);
    positioned["elements"][index]["y"] = json!(130);
    let (again, _) = replace(&positioned);
    assert_eq!(
        get(&again, "label"),
        get(&positioned, "label"),
        "later material replacement preserves manually positioned caption"
    );
}
#[test]
fn blocked_right_side_uses_clear_left_side_and_blocked_both_sides_falls_back() {
    let mut original = scene();
    original["elements"].as_array_mut().unwrap().push(json!({"id":"block-right","type":"rectangle","x":180,"y":95,"width":150,"height":110,"angle":0}));
    let (left, id) = replace(&original);
    assert!(
        get(&left, "label")["x"].as_f64().unwrap() + get(&left, "label")["width"].as_f64().unwrap()
            < get(&left, &id)["x"].as_f64().unwrap()
    );
    original["elements"].as_array_mut().unwrap().push(json!({"id":"block-left","type":"rectangle","x":0,"y":95,"width":125,"height":110,"angle":0}));
    let (blocked, _) = replace(&original);
    assert_eq!(get(&blocked, "label")["y"], 212.);
    assert_eq!(get(&blocked, "label")["width"], 200);
}
#[test]
fn visible_unsupported_labels_keep_default_but_unmeasurable_or_clipped_frames_reject() {
    for mode in [
        "font",
        "locked",
        "rotated_image",
        "invalid_angle",
        "outside_frame",
        "hidden_connector",
        "no_connector",
    ] {
        let mut original = scene();
        match mode {
            "font" => original["elements"][2]["fontFamily"] = json!(1),
            "locked" => original["elements"][2]["locked"] = json!(true),
            "rotated_image" => original["elements"][1]["angle"] = json!(0.42),
            "invalid_angle" => original["elements"][2]["angle"] = json!("0"),
            "outside_frame" => original["elements"][0]["height"] = json!(150),
            "hidden_connector" => original["elements"][3]["opacity"] = json!(0),
            _ => original["elements"][3]["startBinding"] = Value::Null,
        };
        let result = image::illustrate(&original, "node", Image {
            file_id: "asset", data_url: "data:image/png;base64,abc", width: 40, height: 100,
            provenance: json!({"license":"test","source":"fixture"}),
        });
        if matches!(mode, "invalid_angle" | "outside_frame") {
            assert!(result.is_err(), "{mode} must not silently hide an unmeasurable/clipped label");
        } else {
            let (next, _) = result.unwrap();
            assert_eq!(get(&next, "label")["y"], 212.);
        }
    }
}

#[test]
fn two_components_keep_separate_artwork_labels_and_local_edit_scope() {
    let mut original = scene();
    original["elements"][3]["endBinding"] = json!({"elementId":"second","gap":10,"focus":0});
    original["elements"][3]["startBinding"]["focus"] = json!(0);
    original["elements"][3]["roughness"] = json!(0);
    original["elements"][3]["points"] = json!([[0, 0], [0, 130]]);
    original["elements"].as_array_mut().unwrap().extend([
        json!({"id":"second","type":"rectangle","x":40,"y":350,"width":220,"height":100,"angle":0,"frameId":"panel","boundElements":[{"id":"second-label","type":"text"},{"id":"arrow","type":"arrow"}],"customData":{"scientificId":"imaging","evidence":["declared conceptual step"]}}),
        json!({"id":"second-label","type":"text","x":50,"y":375,"width":200,"height":25,"angle":0,"fontFamily":9,"fontSize":20,"lineHeight":1.25,"text":"Microscopy","originalText":"Microscopy","containerId":"second","frameId":"panel"})
    ]);
    let (first, first_image) = replace(&original);
    let (both, second_image) = image::illustrate(
        &first,
        "second",
        Image {
            file_id: "microscope",
            data_url: "data:image/png;base64,def",
            width: 100,
            height: 100,
            provenance: json!({"source":"different fixture","license":"test"}),
        },
    )
    .unwrap();
    assert_ne!(first_image, second_image);
    for id in ["node", "label", &first_image, "arrow", "panel"] {
        assert_eq!(
            get(&both, id),
            get(&first, id),
            "second materialization preserves {id}"
        );
    }
    assert_eq!(both["files"]["asset"], first["files"]["asset"]);
    assert_eq!(
        get(&both, "node")["customData"]["visualAssetElementId"],
        first_image
    );
    assert_eq!(
        get(&both, "second")["customData"]["visualAssetElementId"],
        second_image
    );
    assert_eq!(
        get(&both, &first_image)["customData"]["semanticAnchorId"],
        "node"
    );
    assert_eq!(
        get(&both, &second_image)["customData"]["semanticAnchorId"],
        "second"
    );
    let fitted = scene::apply(
        &both,
        &[Operation::FitIllustrationAnchor {
            id: "node".into(),
            file_id: "asset".into(),
            normalized_bounds: [0., 0., 1., 1.],
        }],
    )
    .unwrap();
    let scaled = scene::apply(
        &fitted,
        &[Operation::ScaleIllustration {
            id: "node".into(),
            file_id: "asset".into(),
            factor: 1.1,
        }],
    )
    .unwrap();
    for id in ["second", "second-label", &second_image] {
        assert_eq!(
            get(&scaled, id),
            get(&both, id),
            "local first-component scaling preserves {id}"
        );
    }
    assert_eq!(scaled["files"], both["files"]);
}
