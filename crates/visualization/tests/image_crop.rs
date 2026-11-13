use serde_json::{Value, json};
use visualization::scene::{self, Operation};
fn source() -> Value {
    json!({"elements":[{"id":"i","type":"image","x":10.,"y":20.,"width":100.,"height":80.,"angle":0.,"scale":[1.,1.],"crop":null,"fileId":"f","customData":{"source":"Fixture","asset":{"author":"Artist","license":"CC0"}}},{"id":"other","type":"text","x":300.,"y":300.,"text":"Complete scientific wording"}],"files":{"f":{"dataURL":"unchanged"}}})
}
fn crop(v: &Value, region: [f64; 4]) -> Result<Value, scene::SceneError> {
    scene::apply(
        v,
        &[Operation::CropImage {
            id: "i".into(),
            file_id: "f".into(),
            natural_width: 1000.,
            natural_height: 800.,
            normalized_crop: region,
        }],
    )
}
#[test]
fn native_crop_preserves_source_pixel_positions_for_both_flip_axes_and_recropping() {
    for sx in [-1., 1.] {
        for sy in [-1., 1.] {
            let mut d = source();
            d["elements"][0]["scale"] = json!([sx, sy]);
            let once = crop(&d, [0.1, 0.2, 0.6, 0.5]).unwrap();
            let next = crop(&once, [0.2, 0.25, 0.2, 0.2]).unwrap();
            let e = &next["elements"][0];
            assert!((e["x"].as_f64().unwrap() - (if sx == 1. { 30. } else { 70. })).abs() < 1e-8);
            assert!((e["y"].as_f64().unwrap() - (if sy == 1. { 40. } else { 64. })).abs() < 1e-8);
            assert!((e["width"].as_f64().unwrap() - 20.).abs() < 1e-8);
            assert!((e["height"].as_f64().unwrap() - 16.).abs() < 1e-8);
            assert_eq!(
                e["crop"],
                json!({"x":200.,"y":200.,"width":200.,"height":160.,"naturalWidth":1000.,"naturalHeight":800.})
            );
            assert_eq!(e["customData"], d["elements"][0]["customData"]);
            assert_eq!(e["scale"], d["elements"][0]["scale"]);
            assert_eq!(next["elements"][1], d["elements"][1]);
            assert_eq!(next["files"], d["files"]);
            assert_eq!(crop(&next, [0.2, 0.25, 0.2, 0.2]).unwrap(), next);
        }
    }
}
#[test]
fn owned_crop_invalidates_union_snapshot_without_moving_other_members_or_labels() {
    let mut d = source();
    d["elements"][0]["customData"]["semanticBadgeAnchorId"] = json!("a");
    d["elements"][0]["groupIds"] = json!(["g"]);
    d["elements"].as_array_mut().unwrap().push(json!({"id":"a","type":"rectangle","x":0.,"y":0.,"width":200.,"height":200.,"groupIds":["g"],"customData":{"visualBadgeIds":["i"],"visualAssetGroupId":"g","visualAnchorFit":{"scope":"illustration_group"},"scientificId":"retained"}}));
    let next = crop(&d, [0.2, 0.25, 0.2, 0.2]).unwrap();
    assert!(
        next["elements"][2]["customData"]
            .get("visualAnchorFit")
            .is_none()
    );
    assert_eq!(next["elements"][2]["x"], d["elements"][2]["x"]);
    assert_eq!(
        next["elements"][2]["customData"]["scientificId"],
        "retained"
    );
    assert_eq!(next["elements"][1], d["elements"][1]);
    d["elements"][2]["locked"] = json!(true);
    assert!(crop(&d, [0.2, 0.25, 0.2, 0.2]).is_err());
}
#[test]
fn unsafe_crops_reject_without_modifying_input() {
    let original = source();
    for r in [[-0.1, 0., 0.5, 0.5], [0., 0., 0., 1.], [0.9, 0., 0.2, 1.]] {
        assert!(crop(&original, r).is_err());
    }
    for (key, value) in [
        ("angle", json!(0.1)),
        ("locked", json!(true)),
        ("fileId", json!("other")),
        ("scale", json!([2, 1])),
    ] {
        let mut d = original.clone();
        d["elements"][0][key] = value;
        let before = d.clone();
        assert!(crop(&d, [0., 0., 0.5, 0.5]).is_err());
        assert_eq!(d, before);
    }
    let once = crop(&original, [0.2, 0.2, 0.5, 0.5]).unwrap();
    assert!(crop(&once, [0., 0., 1., 1.]).is_err());
}
