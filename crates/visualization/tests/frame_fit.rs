use serde_json::{Value, json};
use visualization::scene::{self, Operation};
fn source() -> Value {
    json!({"type":"excalidraw","elements":[
        {"id":"f","type":"frame","x":10.,"y":20.,"width":500.,"height":600.,"angle":0.,"name":"","customData":{"panelId":"panel"}},
        {"id":"heading","type":"text","x":30.,"y":40.,"width":200.,"height":20.,"angle":0.,"frameId":"f","text":"Panel title"},
        {"id":"node","type":"rectangle","x":40.,"y":100.,"width":150.,"height":80.,"angle":0.,"frameId":"f","customData":{"scientificId":"x"}},
        {"id":"outside","type":"image","x":800.,"y":900.,"width":50.,"height":50.,"fileId":"asset"}
    ],"files":{"asset":{"dataURL":"preserved"}},"scientificPlan":{"message":"history"}})
}
fn fit(v: &Value, padding: f64) -> Result<Value, scene::SceneError> {
    scene::apply(
        v,
        &[Operation::FitFrame {
            id: "f".into(),
            padding,
        }],
    )
}
#[test]
fn shrinks_frame_only_and_is_idempotent() {
    let old = source();
    let new = fit(&old, 24.).unwrap();
    assert_eq!(new["elements"][0]["width"], 244.);
    assert_eq!(new["elements"][0]["height"], 184.);
    for i in 1..4 {
        assert_eq!(old["elements"][i], new["elements"][i]);
    }
    for key in ["x", "y", "name", "customData"] {
        assert_eq!(old["elements"][0][key], new["elements"][0][key]);
    }
    assert_eq!(new["files"], old["files"]);
    assert_eq!(new["scientificPlan"], old["scientificPlan"]);
    assert_eq!(fit(&new, 24.).unwrap(), new);
}
#[test]
fn never_grows_or_uses_deleted_children() {
    let mut old = source();
    old["elements"][0]["width"] = json!(220.);
    old["elements"][0]["height"] = json!(160.);
    assert_eq!(fit(&old, 128.).unwrap(), old);
    old["elements"]
        .as_array_mut()
        .unwrap()
        .push(json!({"id":"deleted","type":"unknown","frameId":"f","isDeleted":true}));
    assert!(fit(&old, 24.).is_ok());
}
#[test]
fn rejects_unsafe_bounds_and_targets_atomically() {
    for field in ["locked", "isDeleted"] {
        let mut v = source();
        v["elements"][0][field] = json!(true);
        assert!(fit(&v, 24.).is_err());
    }
    for (field, value) in [
        ("angle", json!(0.2)),
        ("frameId", json!("parent")),
        ("type", json!("rectangle")),
    ] {
        let mut v = source();
        v["elements"][0][field] = value;
        assert!(fit(&v, 24.).is_err());
    }
    for (field, value) in [
        ("x", json!(-10.)),
        ("type", json!("frame")),
        ("type", json!("unsupported")),
    ] {
        let mut v = source();
        v["elements"][1][field] = value;
        let original = v.clone();
        assert!(fit(&v, 24.).is_err());
        assert_eq!(v, original);
    }
    for padding in [0., 7., 129., f64::NAN, f64::INFINITY] {
        assert!(fit(&source(), padding).is_err());
    }
    let mut empty = source();
    empty["elements"]
        .as_array_mut()
        .unwrap()
        .retain(|e| e["frameId"] != "f");
    assert!(fit(&empty, 24.).is_err());
}
#[test]
fn rotated_children_use_complete_extents_and_paths_use_actual_points() {
    let mut v = source();
    v["elements"][2] = json!({"id":"path","type":"line","x":120.,"y":100.,"width":0.,"height":0.,"angle":0.,"frameId":"f","points":[[-80.,0.],[200.,150.]],"roughness":0.,"roundness":null});
    let new = fit(&v, 24.).unwrap();
    assert_eq!(new["elements"][0]["width"], 334.);
    assert_eq!(new["elements"][0]["height"], 254.);
    v["elements"][2] = json!({"id":"r","type":"rectangle","x":80.,"y":90.,"width":100.,"height":40.,"angle":std::f64::consts::FRAC_PI_2,"frameId":"f"});
    let new = fit(&v, 24.).unwrap();
    assert_eq!(new["elements"][0]["height"], 164.);
}
