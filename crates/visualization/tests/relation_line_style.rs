use serde_json::{Value, json};
use visualization::{scene, sketch};

fn plan(panels: bool) -> Value {
    let mut value = json!({"message":"Explicit visual legend, no biological claim","audience":"Researchers","layout":"vertical","nodes":[{"id":"a","label":"A"},{"id":"b","label":"B"},{"id":"c","label":"C"}],"edges":[{"from":"a","to":"b","line_style":"dashed","direction":"bidirectional","label":"Declared relation","evidence":["user-reviewed source"]},{"from":"b","to":"c","line_style":"dotted","inhibitory":true}]});
    if panels {
        value["panels"] = json!([{"id":"first","title":"First","column":0,"row":0},{"id":"second","title":"Second","column":1,"row":0}]);
        value["nodes"][0]["panel"] = json!("first");
        value["nodes"][1]["panel"] = json!("first");
        value["nodes"][2]["panel"] = json!("second");
    }
    value
}

fn arrow(document: &Value, id: &str) -> Value {
    document["elements"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["id"] == id)
        .unwrap()
        .clone()
}

#[test]
fn explicit_styles_survive_plain_and_cross_panel_compilation_and_movement() {
    for panels in [false, true] {
        let plan: sketch::Plan = serde_json::from_value(plan(panels)).unwrap();
        let document = sketch::compile(&plan).unwrap();
        let dashed = arrow(&document, "edge-0");
        let dotted = arrow(&document, "edge-1");
        assert_eq!(dashed["strokeStyle"], "dashed");
        assert_eq!(dotted["strokeStyle"], "dotted");
        assert_eq!(dashed["startArrowhead"], "arrow");
        assert_eq!(dashed["endArrowhead"], "arrow");
        assert_eq!(dotted["endArrowhead"], "bar");
        assert_eq!(
            dashed["customData"]["evidence"],
            json!(["user-reviewed source"])
        );
        assert_eq!(dashed["startBinding"]["elementId"], "node-0");
        assert_eq!(dotted["endBinding"]["elementId"], "node-2");
        if panels {
            assert_eq!(dotted["customData"]["crossPanel"], true);
        }
        let moved = scene::apply(
            &document,
            &[serde_json::from_value(
                json!({"op":"translate","element_ids":if panels { vec!["panel-0","panel-1"] } else { vec!["node-0","node-1","node-2"] },"dx":12,"dy":8}),
            )
            .unwrap()],
        )
        .unwrap();
        for id in ["edge-0", "edge-1"] {
            assert_eq!(
                arrow(&moved, id)["strokeStyle"],
                arrow(&document, id)["strokeStyle"]
            );
            assert_eq!(
                arrow(&moved, id)["customData"],
                arrow(&document, id)["customData"]
            );
        }
    }
}

#[test]
fn legacy_defaults_remain_solid_without_rewriting_the_serialized_plan() {
    let mut value = plan(false);
    for edge in value["edges"].as_array_mut().unwrap() {
        edge.as_object_mut().unwrap().remove("line_style");
    }
    let plan: sketch::Plan = serde_json::from_value(value.clone()).unwrap();
    let serialized = serde_json::to_value(&plan).unwrap();
    assert!(serialized["edges"][0].get("line_style").is_none());
    assert_eq!(
        arrow(&sketch::compile(&plan).unwrap(), "edge-0")["strokeStyle"],
        "solid"
    );
    for invalid in [
        json!("optional"),
        json!("DASHED"),
        json!(null),
        json!({"style":"dashed"}),
    ] {
        value["edges"][0]["line_style"] = invalid;
        assert!(serde_json::from_value::<sketch::Plan>(value.clone()).is_err());
    }
}
