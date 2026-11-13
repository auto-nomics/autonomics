use serde_json::{Value, json};
use visualization::scene::{self, Operation};
fn document() -> Value {
    json!({"elements":[
        {"id":"note","type":"text","x":20,"y":30,"width":1180,"height":40,"angle":0,"fontFamily":9,"fontSize":16,"lineHeight":1.25,"textAlign":"left","text":"Human IFN-γ scope\n完整保留 Y701 与来源。","originalText":"Human IFN-γ scope\n完整保留 Y701 与来源。","containerId":null,"frameId":null,"groupIds":["existing-group"],"boundElements":[],"autoResize":false,"customData":{"evidence":["reviewed source"],"owner":"retained"}},
        {"id":"other","type":"rectangle","x":0,"y":0,"width":10,"height":10}
    ],"files":{"kept":{"dataURL":"unchanged"}},"scientificPlan":{"message":"retained plan"}})
}
#[test]
fn fitting_existing_text_preserves_visible_alignment_and_every_unrelated_field() {
    for alignment in ["left", "center", "right"] {
        let mut original = document();
        original["elements"][0]["textAlign"] = json!(alignment);
        let changed =
            scene::apply(&original, &[Operation::FitTextWidth { id: "note".into() }]).unwrap();
        let old = &original["elements"][0];
        let note = &changed["elements"][0];
        let width = note["width"].as_f64().unwrap();
        assert!(width < 1180. && width > 100.);
        let anchor = |e: &Value| {
            e["x"].as_f64().unwrap()
                + e["width"].as_f64().unwrap()
                    * match alignment {
                        "center" => 0.5,
                        "right" => 1.,
                        _ => 0.,
                    }
        };
        assert!((anchor(old) - anchor(note)).abs() < 1e-6);
        let mut restored = changed.clone();
        for key in ["x", "width", "version", "versionNonce", "updated"] {
            if let Some(value) = old.get(key) {
                restored["elements"][0][key] = value.clone();
            } else {
                restored["elements"][0].as_object_mut().unwrap().remove(key);
            }
        }
        assert_eq!(restored, original);
        assert_eq!(
            scene::apply(&changed, &[Operation::FitTextWidth { id: "note".into() }]).unwrap(),
            changed,
            "fitting twice is a no-op"
        );
    }
}
#[test]
fn unsupported_or_attached_text_is_rejected_without_changing_the_scene() {
    for patch in [
        json!({"locked":true}),
        json!({"isDeleted":true}),
        json!({"angle":0.2}),
        json!({"fontFamily":2}),
        json!({"containerId":"node"}),
        json!({"boundElements":[{"id":"arrow","type":"arrow"}]}),
        json!({"boundElements":{"opaque":"invalid"}}),
        json!({"textAlign":"top"}),
        json!({"textAlign":7}),
        json!({"text":"🧬"}),
        json!({"width":1}),
        json!({"frameId":"missing"}),
    ] {
        let mut original = document();
        original["elements"][0]
            .as_object_mut()
            .unwrap()
            .extend(patch.as_object().unwrap().clone());
        let saved = original.clone();
        assert!(scene::apply(&original, &[Operation::FitTextWidth { id: "note".into() }]).is_err());
        assert_eq!(original, saved);
    }
    let mut original = document();
    original["elements"]
        .as_array_mut()
        .unwrap()
        .push(json!({"id":"arrow","type":"arrow","startBinding":{"elementId":"note"}}));
    assert!(scene::apply(&original, &[Operation::FitTextWidth { id: "note".into() }]).is_err());
}

#[test]
fn existing_frame_membership_is_preserved_and_locked_frames_reject() {
    let mut original = document();
    original["elements"][0]["frameId"] = json!("frame");
    original["elements"]
        .as_array_mut()
        .unwrap()
        .push(json!({"id":"frame","type":"frame","x":0,"y":0,"width":1200,"height":100,"angle":0}));
    let changed =
        scene::apply(&original, &[Operation::FitTextWidth { id: "note".into() }]).unwrap();
    assert_eq!(changed["elements"][0]["frameId"], "frame");
    assert_eq!(changed["elements"][2], original["elements"][2]);
    for patch in [
        json!({"locked":true}),
        json!({"angle":0.3}),
        json!({"height":10}),
        json!({"isDeleted":true}),
    ] {
        let mut bad = original.clone();
        bad["elements"][2]
            .as_object_mut()
            .unwrap()
            .extend(patch.as_object().unwrap().clone());
        assert!(scene::apply(&bad, &[Operation::FitTextWidth { id: "note".into() }]).is_err());
    }
}
