use serde_json::{Value, json};
use visualization::{
    scene::{self, Operation},
    sketch::Plan,
};

fn plan(panels: bool) -> Plan {
    serde_json::from_value(if panels {
        json!({"message":"Declared concept","audience":"Researchers","layout":"vertical","panels":[{"id":"first","title":"First","column":0,"row":0},{"id":"second","title":"Second","column":1,"row":0}],"nodes":[{"id":"a","label":"Collect","panel":"first","evidence":["supplied reference"]},{"id":"b","label":"Image","panel":"second","evidence":[]}],"edges":[{"from":"a","to":"b","label":"then","evidence":["concept only"]}],"assumptions":["No experiment parameters"]})
    } else {
        json!({"message":"Declared concept","audience":"Researchers","layout":"horizontal","nodes":[{"id":"a","label":"Collect","evidence":["supplied reference"]},{"id":"b","label":"Image"}],"edges":[{"from":"a","to":"b"}],"assumptions":["No experiment parameters"]})
    }).unwrap()
}
fn original() -> Value {
    json!({"elements":[{"id":"reference","type":"image","x":5,"y":10,"width":400,"height":100,"fileId":"ref","customData":{"note":"User supplied"}}],"files":{"ref":{"dataURL":"private image bytes"}},"scientificPlan":{"message":"Previous historical plan"},"appState":{"viewBackgroundColor":"#fffaf0"}})
}
fn insert(panels: bool, prefix: &str, x: f64) -> Operation {
    Operation::InsertSketch {
        plan: plan(panels),
        id_prefix: prefix.into(),
        x,
        y: 240.,
    }
}

#[test]
fn native_graphs_append_with_reciprocal_remapped_bindings_and_preserve_old_scene() {
    for panels in [false, true] {
        let source = original();
        let next = scene::apply(&source, &[insert(panels, "concept", 20.)]).unwrap();
        assert_eq!(next["elements"][0], source["elements"][0]);
        for key in ["files", "scientificPlan", "appState"] {
            assert_eq!(next[key], source[key]);
        }
        let added = &next["elements"].as_array().unwrap()[1..];
        let by_id: std::collections::HashMap<&str, &Value> = added
            .iter()
            .map(|e| (e["id"].as_str().unwrap(), e))
            .collect();
        assert_eq!(by_id.len(), added.len());
        for e in added {
            assert!(e["id"].as_str().unwrap().starts_with("concept:e:"));
            for field in ["containerId", "frameId"] {
                if let Some(id) = e[field].as_str() {
                    assert!(by_id.contains_key(id), "{field} {id}");
                }
            }
            if let Some(bound) = e["boundElements"].as_array() {
                for member in bound {
                    assert!(by_id.contains_key(member["id"].as_str().unwrap()));
                }
            }
            if e["type"] == "arrow" {
                for field in ["startBinding", "endBinding"] {
                    let owner = by_id[e[field]["elementId"].as_str().unwrap()];
                    assert!(
                        owner["boundElements"]
                            .as_array()
                            .unwrap()
                            .iter()
                            .any(|b| b["id"] == e["id"])
                    );
                }
            }
        }
        assert!(
            added
                .iter()
                .any(|e| e["customData"]["scientificPlan"] == json!(plan(panels)))
        );
        let placed = json!({"elements":added});
        let bounds = visualization::geometry_review::review(&placed)
            .bounds
            .unwrap();
        assert!((bounds.x - 20.).abs() < 1e-6 && (bounds.y - 240.).abs() < 1e-6);
        let twice = scene::apply(&next, &[insert(panels, "other", 900.)]).unwrap();
        assert_eq!(
            &twice["elements"].as_array().unwrap()[..next["elements"].as_array().unwrap().len()],
            next["elements"].as_array().unwrap()
        );
    }
}

#[test]
fn insertion_rejects_collisions_invalid_plans_and_bounds_atomically() {
    let source = original();
    assert!(
        scene::apply(
            &source,
            &[insert(false, "same", 0.), insert(false, "same", 500.)]
        )
        .is_err()
    );
    for prefix in ["", "bad:prefix", "prefix with space"] {
        assert!(scene::apply(&source, &[insert(false, prefix, 0.)]).is_err());
    }
    for x in [f64::NAN, f64::INFINITY, 1e7] {
        assert!(scene::apply(&source, &[insert(false, "ok", x)]).is_err());
    }
    let mut invalid = plan(false);
    invalid.edges[0].to = "missing".into();
    assert!(
        scene::apply(
            &source,
            &[Operation::InsertSketch {
                plan: invalid,
                id_prefix: "bad".into(),
                x: 0.,
                y: 0.
            }]
        )
        .is_err()
    );
    assert_eq!(source, original());
}
