use serde_json::{Value, json};
fn compile(layout: &str) -> Value {
    let mut plan = json!({"message":"Declared conceptual sequence","audience":"Researchers","layout":layout,"nodes":[{"id":"a","label":"Source","evidence":["source reference"]},{"id":"b","label":"Intermediate"},{"id":"c","label":"Endpoint"}],"edges":[{"from":"a","to":"b","label":"受体相关复合物经明确的修饰位点及中间状态形成下游信号转导复合物，具体激酶归属尚未确定","evidence":["reviewed transition reference"]},{"from":"b","to":"c","label":"enters nucleus","evidence":["endpoint reference"]}]});
    if layout == "grid" {
        for (i, node) in plan["nodes"].as_array_mut().unwrap().iter_mut().enumerate() {
            node["column"] = json!(i);
            node["row"] = json!(0);
        }
    }
    visualization::sketch::compile(&serde_json::from_value(plan).unwrap()).unwrap()
}
fn element<'a>(scene: &'a Value, id: &str) -> &'a Value {
    scene["elements"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["id"] == id)
        .unwrap()
}
#[test]
fn long_relation_gets_space_and_short_relations_stay_bounded_without_losing_text() {
    let scene = compile("horizontal");
    let a = element(&scene, "node-0");
    let b = element(&scene, "node-1");
    let c = element(&scene, "node-2");
    assert!(b["x"].as_f64().unwrap() - a["x"].as_f64().unwrap() > 400.);
    // A readable single-line short caption may need slightly more than the old
    // fixed 400-unit step, but must not inherit the long relation's large gap.
    let short_step = c["x"].as_f64().unwrap() - b["x"].as_f64().unwrap();
    assert!((400. ..=460.).contains(&short_step));
    let short = element(&scene, "edge-label-1");
    assert_eq!(short["text"], "enters nucleus");
    assert!(short["x"].as_f64().unwrap() > b["x"].as_f64().unwrap() + b["width"].as_f64().unwrap());
    assert!(
        short["x"].as_f64().unwrap() + short["width"].as_f64().unwrap() < c["x"].as_f64().unwrap()
    );
    let label = element(&scene, "edge-label-0");
    let text = label["text"].as_str().unwrap();
    let original = label["originalText"].as_str().unwrap();
    assert_eq!(text.replace('\n', ""), original);
    assert!(
        text.lines().count() <= 3,
        "long Chinese relation retains readable line count: {text}"
    );
    assert!(label["width"].as_f64().unwrap() > 104.);
    assert!(label["x"].as_f64().unwrap() > a["x"].as_f64().unwrap() + a["width"].as_f64().unwrap());
    assert!(
        label["x"].as_f64().unwrap() + label["width"].as_f64().unwrap() < b["x"].as_f64().unwrap()
    );
    assert_eq!(
        element(&scene, "edge-0")["customData"]["evidence"],
        json!(["reviewed transition reference"])
    );
    assert_eq!(
        element(&scene, "edge-0")["startBinding"]["elementId"],
        "node-0"
    );
    assert_eq!(
        element(&scene, "edge-0")["endBinding"]["elementId"],
        "node-1"
    );
}
#[test]
fn grid_preserves_cell_order_and_expands_spacing_for_long_captions() {
    let scene = compile("grid");
    let step = element(&scene, "node-1")["x"].as_f64().unwrap();
    assert!(step >= 400.);
    for (i, id) in ["node-0", "node-1", "node-2"].iter().enumerate() {
        assert_eq!(element(&scene, id)["x"].as_f64(), Some(i as f64 * step));
        assert_eq!(element(&scene, id)["y"], json!(0.));
    }
    let label = element(&scene, "edge-label-0");
    assert_eq!(
        label["text"].as_str().unwrap().replace('\n', ""),
        label["originalText"].as_str().unwrap()
    );
}
#[test]
fn vertical_labels_keep_text_and_binding_with_matching_font_metrics() {
    let scene = compile("vertical");
    let label = element(&scene, "edge-label-0");
    assert_eq!(
        label["text"].as_str().unwrap().replace('\n', ""),
        label["originalText"].as_str().unwrap()
    );
    assert_eq!(label["fontFamily"], 9);
    assert_eq!(label["fontSize"], 16);
    assert!(label["width"].as_f64().unwrap() > 104.);
    assert_eq!(label["containerId"], "edge-0");
}

#[test]
fn grid_wraps_long_node_labels_and_reserves_vertical_caption_space() {
    let plan: visualization::sketch::Plan = serde_json::from_value(json!({
        "message":"Explicit conceptual geometry", "audience":"Researchers", "layout":"grid",
        "nodes":[
            {"id":"a","label":"受体相关复合物与下游转录调控因子之间的相互作用以及细胞内信号传导机制", "shape":"ellipse", "column":0,"row":0},
            {"id":"b","label":"Nuclear response", "column":0,"row":1}],
        "edges":[{"from":"a","to":"b","label":"受体相关复合物经明确修饰位点形成下游信号转导复合物，激酶归属尚未确定"}]
    })).unwrap();
    let scene = visualization::sketch::compile(&plan).unwrap();
    let node = element(&scene, "node-0");
    let label = element(&scene, "label-0");
    let caption = element(&scene, "edge-label-0");
    let next = element(&scene, "node-1");
    assert!(label["text"].as_str().unwrap().contains('\n'));
    assert_eq!(label["originalText"], plan.nodes[0].label);
    assert_eq!(
        label["text"].as_str().unwrap().replace('\n', ""),
        plan.nodes[0].label
    );
    assert!(node["height"].as_f64().unwrap() > 100.);
    let bottom = node["y"].as_f64().unwrap() + node["height"].as_f64().unwrap();
    assert!(caption["y"].as_f64().unwrap() >= bottom);
    assert!(
        caption["y"].as_f64().unwrap() + caption["height"].as_f64().unwrap()
            <= next["y"].as_f64().unwrap()
    );
    let arrow = element(&scene, "edge-0");
    assert_eq!(arrow["y"].as_f64().unwrap(), bottom + 10.);
    let last = arrow["points"].as_array().unwrap().last().unwrap();
    assert_eq!(
        arrow["y"].as_f64().unwrap() + last[1].as_f64().unwrap(),
        next["y"].as_f64().unwrap() - 10.
    );
    assert!(caption["width"].as_f64().unwrap() >= 104.);
}
