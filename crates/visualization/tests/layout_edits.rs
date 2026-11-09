//! Geometry contract for agent edits of automatically routed feedback edges.
use serde_json::{Value, json};
use visualization::{scene, sketch};
fn element<'a>(document: &'a Value, id: &str) -> &'a Value {
    document["elements"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["id"] == id)
        .unwrap()
}
#[test]
fn moving_a_feedback_endpoint_keeps_the_unmoved_middle_label_on_its_route() {
    for direction in ["horizontal", "vertical"] {
        let plan: sketch::Plan = serde_json::from_value(json!({"message":"Explicit feedback","audience":"Research","layout":direction,"nodes":[{"id":"a","label":"Input"},{"id":"b","label":"Output"}],"edges":[{"from":"a","to":"b","label":"activates"},{"from":"b","to":"a","label":"feedback"}]})).unwrap();
        let original = sketch::compile(&plan).unwrap();
        for id in ["node-0", "node-1"] {
            let node = element(&original, id);
            let edit = serde_json::from_value(json!({"op":"update","id":id,"changes":{"x":node["x"].as_f64().unwrap()+40.,"y":node["y"].as_f64().unwrap()+30.}})).unwrap();
            let after = scene::apply(&original, &[edit]).unwrap();
            let before_label = element(&original, "edge-label-1");
            let after_label = element(&after, "edge-label-1");
            assert_eq!(
                (after_label["x"].clone(), after_label["y"].clone()),
                (before_label["x"].clone(), before_label["y"].clone()),
                "{direction}: only an endpoint moved, so the outer middle segment and its label remain fixed"
            );
            let before_arrow = element(&original, "edge-1");
            let after_arrow = element(&after, "edge-1");
            for index in [2, 3] {
                for (axis, origin) in [(0, "x"), (1, "y")] {
                    assert_eq!(
                        before_arrow[origin].as_f64().unwrap()
                            + before_arrow["points"][index][axis].as_f64().unwrap(),
                        after_arrow[origin].as_f64().unwrap()
                            + after_arrow["points"][index][axis].as_f64().unwrap()
                    );
                }
            }
        }
    }
}
