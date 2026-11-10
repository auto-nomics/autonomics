use serde_json::{Value, json};
use visualization::scene::{self, Operation};
fn fixture() -> Value {
    json!({"elements":[{"id":"image","type":"image","x":0,"y":0,"width":200,"height":100,"angle":0.3,"scale":[-1,1],"fileId":"pixels","crop":{"x":10,"y":5,"width":80,"height":40,"naturalWidth":100,"naturalHeight":50},"customData":{"asset":{"author":"Fixture","license":"cc-0"},"source_sha256":"original"},"boundElements":[],"version":2},{"id":"label","type":"text","text":"Declared stage","x":0,"y":-40}],"files":{"pixels":{"dataURL":"data:image/png;base64,fixture"}},"customData":{"scientificPlan":{"scope":"Supplied concept"}}})
}
fn placement(id: &str) -> Operation {
    Operation::SetImageGeometry {
        id: id.into(),
        x: 32.5,
        y: 170.,
        width: 155.,
        height: 112.31,
    }
}
#[test]
fn standalone_geometry_preserves_pixels_crop_flip_provenance_and_unrelated_content() {
    let doc = fixture();
    let result = scene::apply(&doc, &[placement("image")]).unwrap();
    for key in ["files", "customData"] {
        assert_eq!(result[key], doc[key]);
    }
    assert_eq!(result["elements"][1], doc["elements"][1]);
    for (key, value) in doc["elements"][0].as_object().unwrap() {
        if ![
            "x",
            "y",
            "width",
            "height",
            "version",
            "versionNonce",
            "updated",
        ]
        .contains(&key.as_str())
        {
            assert_eq!(&result["elements"][0][key], value, "preserve {key}");
        }
    }
    assert_eq!(result["elements"][0]["x"], 32.5);
    assert_eq!(result["elements"][0]["width"], 155.);
    assert_eq!(result["elements"][0]["scale"], json!([-1, 1]));
    assert_eq!(doc["elements"][0]["x"], 0);
}
#[test]
fn protected_images_invalid_values_and_quoted_updates_fail_atomically() {
    for (key, value) in [
        ("locked", json!(true)),
        ("isDeleted", json!(true)),
        ("type", json!("rectangle")),
        ("customData", json!({"semanticAnchorId":"owner"})),
        (
            "customData",
            json!({"inset":{"kind":"image_inset_snapshot"}}),
        ),
        ("boundElements", json!([{"id":"arrow","type":"arrow"}])),
    ] {
        let mut doc = fixture();
        doc["elements"][0][key] = value;
        let before = doc.clone();
        assert!(
            scene::apply(
                &doc,
                &[
                    Operation::Update {
                        id: "label".into(),
                        changes: serde_json::from_value(json!({"text":"must not commit"})).unwrap()
                    },
                    placement("image")
                ]
            )
            .is_err()
        );
        assert_eq!(doc, before);
    }
    let doc = fixture();
    assert!(scene::apply(&doc, &[placement("missing")]).is_err());
    for (width, x) in [
        (0., 0.),
        (-10., 0.),
        (f64::NAN, 0.),
        (10., f64::INFINITY),
        (10., 2e6),
    ] {
        assert!(
            scene::apply(
                &doc,
                &[Operation::SetImageGeometry {
                    id: "image".into(),
                    x,
                    y: 0.,
                    width,
                    height: 10.
                }]
            )
            .is_err()
        );
    }
    let error = scene::apply(
        &doc,
        &[Operation::Update {
            id: "image".into(),
            changes: serde_json::from_value(json!({"x":"32.5"})).unwrap(),
        }],
    )
    .unwrap_err()
    .to_string();
    assert!(error.contains("image.x") && error.contains("JSON number"));
    let mut inconsistent_binding = fixture();
    inconsistent_binding["elements"]
        .as_array_mut()
        .unwrap()
        .push(json!({"id":"arrow","type":"arrow","startBinding":{"elementId":"image"}}));
    assert!(
        scene::apply(&inconsistent_binding, &[placement("image")]).is_err(),
        "reverse binding protects the image even if boundElements is missing"
    );
    let mut owned = fixture();
    owned["elements"].as_array_mut().unwrap().push(
        json!({"id":"owner","type":"rectangle","customData":{"visualAssetElementId":"image"}}),
    );
    assert!(scene::apply(&owned, &[placement("image")]).is_err());
}
