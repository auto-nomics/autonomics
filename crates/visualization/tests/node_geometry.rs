use serde_json::{Value, json};
use visualization::{
    scene::{self, Operation},
    sketch::{self, Plan},
};
fn fixture(panels: bool) -> Value {
    let mut spec = json!({"message":"Explicit concept","audience":"Researchers","layout":"horizontal","nodes":[{"id":"a","label":"Source","evidence":["retained source"]},{"id":"b","label":"Target"},{"id":"c","label":"Other"}],"edges":[{"from":"a","to":"b","label":""},{"from":"b","to":"c","label":""}]});
    if panels {
        spec["panels"] = json!([{"id":"one","title":"First","column":0,"row":0},{"id":"two","title":"Second","column":1,"row":0}]);
        for (i, node) in spec["nodes"].as_array_mut().unwrap().iter_mut().enumerate() {
            node["panel"] = json!(if i == 0 { "one" } else { "two" });
        }
        spec["edges"][0]["label"] = json!("activates");
    }
    sketch::compile(&serde_json::from_value::<Plan>(spec).unwrap()).unwrap()
}
fn target(document: &Value) -> [f64; 4] {
    let n = &document["elements"][0];
    let n = if n["id"] == "node-0" {
        n
    } else {
        document["elements"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["id"] == "node-0")
            .unwrap()
    };
    [
        n["x"].as_f64().unwrap() + 10.,
        n["y"].as_f64().unwrap() + 5.,
        n["width"].as_f64().unwrap() + 30.,
        n["height"].as_f64().unwrap() + 20.,
    ]
}
fn edit(bounds: [f64; 4]) -> Operation {
    Operation::SetNodeGeometry {
        id: "node-0".into(),
        x: bounds[0],
        y: bounds[1],
        width: bounds[2],
        height: bounds[3],
    }
}
fn by_id<'a>(document: &'a Value, id: &str) -> &'a Value {
    document["elements"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["id"] == id)
        .unwrap()
}

#[test]
fn node_resize_reflows_complete_text_and_keeps_ports_attached_without_touching_other_nodes() {
    for kind in ["rectangle", "ellipse", "diamond"] {
        let mut original = fixture(false);
        original["elements"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|e| e["id"] == "node-0")
            .unwrap()["type"] = json!(kind);
        let bounds = target(&original);
        let changed = scene::apply(&original, &[edit(bounds)]).unwrap();
        let (node, label, arrow) = (
            by_id(&changed, "node-0"),
            by_id(&changed, "label-0"),
            by_id(&changed, "edge-0"),
        );
        for (field, expected) in ["x", "y", "width", "height"].into_iter().zip(bounds) {
            assert_eq!(node[field], expected);
        }
        assert_eq!(
            label["originalText"],
            by_id(&original, "label-0")["originalText"]
        );
        assert_eq!(label["fontFamily"], 9);
        assert_eq!(label["fontSize"].as_f64(), Some(20.));
        assert!(
            (label["x"].as_f64().unwrap() + label["width"].as_f64().unwrap() / 2.
                - (bounds[0] + bounds[2] / 2.))
                .abs()
                < 0.5 // native ellipse inner text dimensions round to scene units
        );
        assert!((arrow["x"].as_f64().unwrap() - (bounds[0] + bounds[2] + 10.)).abs() < 1e-6);
        assert!((arrow["y"].as_f64().unwrap() - (bounds[1] + bounds[3] / 2.)).abs() < 1e-6);
        let old_arrow = by_id(&original, "edge-0");
        let last = arrow["points"].as_array().unwrap().last().unwrap();
        let old_last = old_arrow["points"].as_array().unwrap().last().unwrap();
        for axis in 0..2 {
            let key = if axis == 0 { "x" } else { "y" };
            assert!(
                (arrow[key].as_f64().unwrap() + last[axis].as_f64().unwrap()
                    - old_arrow[key].as_f64().unwrap()
                    - old_last[axis].as_f64().unwrap())
                .abs()
                    < 1e-6
            );
        }
        for e in original["elements"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|e| !["node-0", "label-0", "edge-0"].contains(&e["id"].as_str().unwrap()))
        {
            assert_eq!(by_id(&changed, e["id"].as_str().unwrap()), e);
        }
        for key in ["files", "scientificPlan", "appState"] {
            assert_eq!(changed[key], original[key]);
        }
        assert_eq!(node["customData"], by_id(&original, "node-0")["customData"]);
    }
}

#[test]
fn cross_panel_resize_preserves_frame_and_caption_content_while_rerouting() {
    let original = fixture(true);
    let changed = scene::apply(&original, &[edit(target(&original))]).unwrap();
    for id in ["node-1", "node-2", "label-1", "label-2", "edge-1"] {
        assert_eq!(by_id(&changed, id), by_id(&original, id));
    }
    let node = by_id(&changed, "node-0");
    assert_eq!(node["frameId"], by_id(&original, "node-0")["frameId"]);
    for field in [
        "text",
        "originalText",
        "fontFamily",
        "fontSize",
        "width",
        "height",
    ] {
        assert_eq!(
            by_id(&changed, "edge-label-0")[field],
            by_id(&original, "edge-label-0")[field]
        );
    }
    let arrow = by_id(&changed, "edge-0");
    assert_eq!(
        arrow["customData"],
        by_id(&original, "edge-0")["customData"]
    );
    let (x, y, w, h) = (
        node["x"].as_f64().unwrap(),
        node["y"].as_f64().unwrap(),
        node["width"].as_f64().unwrap(),
        node["height"].as_f64().unwrap(),
    );
    assert!(
        [
            [x - 12., y + h / 2.],
            [x + w + 12., y + h / 2.],
            [x + w / 2., y - 12.],
            [x + w / 2., y + h + 12.]
        ]
        .iter()
        .any(|p| (p[0] - arrow["x"].as_f64().unwrap()).abs() < 1e-6
            && (p[1] - arrow["y"].as_f64().unwrap()).abs() < 1e-6),
        "connector starts at a coherent centered port: {arrow}"
    );
    let points = arrow["points"].as_array().unwrap();
    let length: f64 = points
        .windows(2)
        .map(|p| {
            (p[0][0].as_f64().unwrap() - p[1][0].as_f64().unwrap()).abs()
                + (p[0][1].as_f64().unwrap() - p[1][1].as_f64().unwrap()).abs()
        })
        .sum();
    assert!(
        length < 700.,
        "use a local route, not an outer loop: {length}"
    );
}

#[test]
fn unsupported_or_overflowing_node_edits_reject_the_whole_batch() {
    let original = fixture(false);
    for (id, field, value) in [
        ("node-0", "locked", json!(true)),
        ("label-0", "locked", json!(true)),
        ("node-0", "angle", json!(0.2)),
        ("edge-0", "roundness", json!({"type":2})),
        (
            "edge-0",
            "startBinding",
            json!({"elementId":"node-0","focus":0.5,"gap":10}),
        ),
        (
            "node-0",
            "customData",
            json!({"visualAssetElementId":"owned-image"}),
        ),
        ("node-0", "boundElements", json!([])),
    ] {
        let mut source = original.clone();
        source["elements"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|e| e["id"] == id)
            .unwrap()[field] = value;
        assert!(
            scene::apply(&source, &[edit(target(&source))]).is_err(),
            "{id}.{field}"
        );
    }
    assert!(
        scene::apply(&original, &[edit([40., 40., 10., 10.])]).is_err(),
        "complete text must fit, never truncate"
    );
    let framed = fixture(true);
    assert!(scene::apply(&framed, &[edit([0., 0., 1000., 1000.])]).is_err());
    assert!(scene::apply(&original, &[edit([f64::NAN, 0., 200., 100.])]).is_err());
}
