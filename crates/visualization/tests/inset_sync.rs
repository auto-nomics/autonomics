use base64::Engine;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use visualization::{
    inset::{self, Region, Spec},
    scene::{self, Operation},
};
fn fixture() -> Value {
    let bytes=b"<svg xmlns='http://www.w3.org/2000/svg' width='400' height='200'><rect width='400' height='200' fill='red'/></svg>";
    let document = json!({"elements":[{"id":"source","type":"image","x":0.,"y":0.,"width":200.,"height":100.,"angle":0.,"scale":[1,1],"fileId":"file","groupIds":[],"boundElements":[],"customData":{}}],"files":{"file":{"dataURL":format!("data:image/svg+xml;base64,{}",base64::engine::general_purpose::STANDARD.encode(bytes))}}});
    inset::compile(
        &document,
        &Spec {
            element_id: "source".into(),
            region: Region {
                x: 0.25,
                y: 0.25,
                width: 0.5,
                height: 0.5,
            },
            x: 500.,
            y: 200.,
            max_size: 160.,
            title: "Frozen detail".into(),
        },
        400,
        200,
        1,
        &format!("{:x}", Sha256::digest(bytes)),
    )
    .unwrap()
}
fn sync(doc: &Value) -> Result<Value, scene::SceneError> {
    scene::apply(
        doc,
        &[Operation::SyncInsetRegion {
            id: doc["elements"][3]["id"].as_str().unwrap().into(),
        }],
    )
}
fn check_preserved(before: &Value, after: &Value) {
    assert_eq!(before["files"], after["files"]);
    for index in [0, 3, 4, 5] {
        assert_eq!(before["elements"][index], after["elements"][index]);
    }
    for index in [1, 2] {
        assert_eq!(
            before["elements"][index]["customData"],
            after["elements"][index]["customData"]
        );
    }
}
#[test]
fn decorations_follow_translation_without_refreshing_snapshot() {
    let mut doc = fixture();
    doc["elements"][0]["x"] = json!(70.);
    doc["elements"][0]["y"] = json!(-30.);
    let next = sync(&doc).unwrap();
    check_preserved(&doc, &next);
    assert_eq!(next["elements"][1]["x"], 120.);
    assert_eq!(next["elements"][1]["y"], -5.);
    assert_ne!(next["elements"][2]["points"], doc["elements"][2]["points"]);
    assert_eq!(
        next["elements"][2]["startBinding"]["elementId"],
        next["elements"][1]["id"]
    );
}
#[test]
fn frozen_pixel_coordinates_survive_changed_crop_rotation_resize_and_flip() {
    let mut doc = fixture();
    let source = &mut doc["elements"][0];
    source["x"] = json!(30.);
    source["y"] = json!(40.);
    source["width"] = json!(300.);
    source["height"] = json!(160.);
    source["angle"] = json!(std::f64::consts::FRAC_PI_2);
    source["scale"] = json!([-1, -1]);
    source["frameId"] = json!("source-frame");
    source["crop"] =
        json!({"x":50.,"y":0.,"width":300.,"height":200.,"naturalWidth":400.,"naturalHeight":200.});
    let next = sync(&doc).unwrap();
    check_preserved(&doc, &next);
    let roi = &next["elements"][1];
    assert!((roi["x"].as_f64().unwrap() - 80.).abs() < 1e-9);
    assert!((roi["y"].as_f64().unwrap() - 80.).abs() < 1e-9);
    assert_eq!(roi["width"], 200.);
    assert_eq!(roi["height"], 80.);
    assert_eq!(roi["angle"], std::f64::consts::FRAC_PI_2);
    assert_eq!(roi["frameId"], "source-frame");
    // Asymmetric visible crop: old pixels now lie left of the visible center and flip right.
    doc["elements"][0]["crop"]["x"] = json!(100.);
    doc["elements"][0]["angle"] = json!(0.);
    let next = sync(&doc).unwrap();
    assert!((next["elements"][1]["x"].as_f64().unwrap() - 130.).abs() < 1e-9);
}
#[test]
fn missing_changed_or_hidden_sources_and_locked_members_reject_atomically() {
    let original = fixture();
    for case in 0..8 {
        let mut doc = original.clone();
        match case {
            0 => doc["elements"][0]["fileId"] = json!("different"),
            1 => doc["files"]["file"]["dataURL"] = json!("data:image/png;base64,YWJj"),
            2 => doc["elements"][0]["isDeleted"] = json!(true),
            3 => doc["elements"][1]["locked"] = json!(true),
            4 => {
                doc["elements"][0]["crop"] = json!({"x":200.,"y":0.,"width":200.,"height":200.,"naturalWidth":400.,"naturalHeight":200.})
            }
            5 => doc["elements"][3]["angle"] = json!(0.2),
            6 => doc["elements"][0]["scale"] = json!([2, 1]),
            _ => doc["elements"][2]["customData"]["inset"]["anchor_id"] = json!("foreign"),
        }
        let before = doc.clone();
        assert!(sync(&doc).is_err(), "case {case}");
        assert_eq!(doc, before);
    }
}
