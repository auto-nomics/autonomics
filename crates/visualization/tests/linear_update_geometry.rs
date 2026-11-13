use serde_json::json;
use visualization::scene::{self, Operation};

fn update(changes: serde_json::Value) -> Operation {
    Operation::Update {
        id: "edge".into(),
        changes: serde_json::from_value(changes).unwrap(),
    }
}

#[test]
fn linear_dimensions_follow_points_and_inconsistent_edits_fail_atomically() {
    for kind in ["arrow", "line", "freedraw"] {
        let original = json!({"elements":[{"id":"edge","type":kind,"x":570,"y":265,"width":0,"height":80,"points":[[0,0],[0,80]],"customData":{"evidence":["source"]}}],"files":{}});
        let error = scene::apply(&original, &[update(json!({"width":150}))])
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("edge.width") && error.contains("points"),
            "{error}"
        );
        for bad in [
            json!({"points":[]}),
            json!({"points":[[0,0],["20",80]]}),
            json!({"points":[[0,0],[20,80,1]]}),
            json!({"points":[[0,0],[20,80]],"height":200}),
        ] {
            assert!(
                scene::apply(
                    &original,
                    &[update(json!({"strokeColor":"red"})), update(bad)]
                )
                .is_err()
            );
            assert!(original["elements"][0].get("strokeColor").is_none());
        }
        let changed = scene::apply(
            &original,
            &[update(json!({"points":[[0,0],[-30,100],[20,-10]]}))],
        )
        .unwrap();
        assert_eq!(changed["elements"][0]["width"], 50.0);
        assert_eq!(changed["elements"][0]["height"], 110.0);
        assert_eq!(
            changed["elements"][0]["customData"],
            original["elements"][0]["customData"]
        );
        assert_eq!(changed["elements"][0]["x"], 570);
        assert_eq!(changed["files"], original["files"]);
        let vertical = scene::apply(&original, &[update(json!({"width":0,"height":80}))]).unwrap();
        assert_eq!(vertical["elements"][0]["width"], 0.0);
    }
}

#[test]
fn a_single_freehand_point_is_valid_but_a_connector_needs_two() {
    let dot = json!({"elements":[{"id":"edge","type":"freedraw","x":0,"y":0,"width":0,"height":0,"points":[[0,0]]}],"files":{}});
    let changed = scene::apply(&dot, &[update(json!({"points":[[5,10]]}))]).unwrap();
    assert_eq!(changed["elements"][0]["width"], 0.0);
    assert_eq!(changed["elements"][0]["height"], 0.0);
    let mut connector = dot;
    connector["elements"][0]["type"] = json!("arrow");
    assert!(scene::apply(&connector, &[update(json!({"points":[[0,0]]}))]).is_err());
}
