use serde_json::{Value, json};
use visualization::scene::{self, Operation};
fn fixture() -> Value {
    let plan=serde_json::from_value(json!({"message":"Local routing","audience":"researchers","layout":"vertical","nodes":[{"id":"a","label":"A"},{"id":"b","label":"B"}],"edges":[{"from":"a","to":"b","label":"activates"}]})).unwrap();
    let mut d = visualization::sketch::compile(&plan).unwrap();
    for e in d["elements"].as_array_mut().unwrap() {
        e["frameId"] = json!("panel");
    }
    d["elements"].as_array_mut().unwrap().push(
        json!({"id":"panel","type":"frame","x":-400,"y":-100,"width":1200,"height":1200,"angle":0}),
    );
    d
}
fn endpoint(e: &Value, start: bool) -> [f64; 2] {
    let p = if start {
        &e["points"][0]
    } else {
        e["points"].as_array().unwrap().last().unwrap()
    };
    [
        e["x"].as_f64().unwrap() + p[0].as_f64().unwrap(),
        e["y"].as_f64().unwrap() + p[1].as_f64().unwrap(),
    ]
}
fn check(d: &Value, ids: &[&str]) -> Value {
    let ops = ids
        .iter()
        .map(|id| Operation::Reroute { id: (*id).into() })
        .collect::<Vec<_>>();
    let next = scene::apply(d, &ops).unwrap();
    assert_eq!(next["files"], d["files"]);
    for old in d["elements"].as_array().unwrap() {
        let new = next["elements"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["id"] == old["id"])
            .unwrap();
        if ids.contains(&old["id"].as_str().unwrap()) {
            for start in [true, false] {
                assert_eq!(endpoint(new, start), endpoint(old, start));
            }
            for key in [
                "customData",
                "startBinding",
                "endBinding",
                "startArrowhead",
                "endArrowhead",
                "strokeWidth",
            ] {
                assert_eq!(new[key], old[key]);
            }
        } else {
            assert_eq!(new, old);
        }
    }
    next
}
#[test]
fn local_reroute_preserves_endpoints_caption_and_unrelated_content() {
    check(&fixture(), &["edge-0"]);
}
#[test]
fn unsupported_routes_reject_without_mutation() {
    for case in 0..4 {
        let mut d = fixture();
        let arrow = d["elements"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|e| e["id"] == "edge-0")
            .unwrap();
        match case {
            0 => arrow["locked"] = json!(true),
            1 => arrow["roundness"] = json!({"type":2}),
            2 => arrow["frameId"] = Value::Null,
            _ => arrow["endBinding"]["elementId"] = json!("missing"),
        };
        let saved = d.clone();
        assert!(
            scene::apply(
                &d,
                &[Operation::Reroute {
                    id: "edge-0".into()
                }]
            )
            .is_err()
        );
        assert_eq!(d, saved);
    }
}
#[test]
fn optional_saved_spacing_routes() {
    let Ok(path) = std::env::var("VISUALIZATION_LOCAL_ROUTE_SOURCE") else {
        return;
    };
    let raw: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    let next = check(&raw["document"], &["edge-0", "edge-1"]);
    if let Ok(path) = std::env::var("VISUALIZATION_LOCAL_ROUTE_OUTPUT") {
        std::fs::write(
            path,
            serde_json::to_vec_pretty(&json!({"document":next})).unwrap(),
        )
        .unwrap();
    }
}

#[test]
fn blocked_endpoint_and_locked_caption_reject_atomically() {
    for locked_caption in [false, true] {
        let mut doc = fixture();
        if locked_caption {
            let label = doc["elements"]
                .as_array_mut()
                .unwrap()
                .iter_mut()
                .find(|e| e["type"] == "text" && e["containerId"] == "edge-0")
                .unwrap();
            label["locked"] = json!(true);
        } else {
            let arrow = doc["elements"]
                .as_array()
                .unwrap()
                .iter()
                .find(|e| e["id"] == "edge-0")
                .unwrap();
            let start = endpoint(arrow, true);
            doc["elements"].as_array_mut().unwrap().push(json!({"id":"blocking-image","type":"image","x":start[0]-10.,"y":start[1]-10.,"width":20.,"height":20.,"angle":0,"frameId":"panel"}));
        }
        let before = doc.clone();
        assert!(
            scene::apply(
                &doc,
                &[Operation::Reroute {
                    id: "edge-0".into()
                }]
            )
            .is_err()
        );
        assert_eq!(doc, before);
    }
}
