use serde_json::{Value, json};
use visualization::scene::{self, Operation};

#[test]
fn a_caption_uses_the_local_gap_instead_of_an_outer_figure_loop() {
    // Reduced geometry from the reviewed EGFR figure: a narrow, viable
    // caption channel between RAS and RAF/MEK is absent from the edge grid.
    let original = json!({"elements":[
        {"id":"grb2","type":"rectangle","x":460,"y":105,"width":260,"height":130,"angle":0},
        {"id":"ras","type":"ellipse","x":460,"y":305,"width":240,"height":130,"angle":0},
        {"id":"raf","type":"rectangle","x":880,"y":130,"width":260,"height":130,"angle":0},
        {"id":"mek","type":"rectangle","x":880,"y":330,"width":260,"height":130,"angle":0},
        {"id":"grb2-text","type":"text","x":465,"y":145,"width":250,"height":50,"angle":0,"containerId":"grb2"},
        {"id":"ras-text","type":"text","x":503.0761184457488,"y":332.5380592228744,"width":174,"height":75,"angle":0,"containerId":"ras"},
        {"id":"sos-caption","type":"text","x":482,"y":235,"width":176,"height":40,"angle":0},
        {"id":"raf-text","type":"text","x":885,"y":170,"width":250,"height":50,"angle":0,"containerId":"raf"},
        {"id":"mek-text","type":"text","x":885,"y":370,"width":250,"height":50,"angle":0,"containerId":"mek"},
        {"id":"edge","type":"arrow","x":692,"y":355,"width":176,"height":175,"angle":0,"points":[[0,0],[20,0],[20,-175],[176,-175]],"startBinding":{"elementId":"ras","focus":0,"gap":12},"endBinding":{"elementId":"raf","focus":0,"gap":12},"boundElements":[{"id":"caption","type":"text"}],"customData":{"crossPanel":true,"evidence":["retained"]}},
        {"id":"caption","type":"text","x":723.77734375,"y":160,"width":132.4453125,"height":40,"angle":0,"text":"RAF recruitment","containerId":"edge","fontSize":16}
    ],"files":{}});
    let changed = scene::apply(&original, &[Operation::Reroute { id: "edge".into() }]).unwrap();
    let arrow = changed["elements"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["id"] == "edge")
        .unwrap();
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
        "a local caption route exists; do not take an outer loop of length {length}: {arrow}"
    );
    assert!(arrow["x"].as_f64().unwrap() >= 700.);
    for old in original["elements"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["id"] != "edge" && e["id"] != "caption")
    {
        assert_eq!(
            changed["elements"]
                .as_array()
                .unwrap()
                .iter()
                .find(|e| e["id"] == old["id"])
                .unwrap(),
            old
        );
    }
    let caption = changed["elements"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["id"] == "caption")
        .unwrap();
    assert_eq!(caption["text"], "RAF recruitment");
    assert_eq!(caption["width"], 132.4453125);
    assert_eq!(caption["height"], 40);
    assert_eq!(arrow["customData"], original["elements"][9]["customData"]);
    assert_eq!(changed["files"], Value::Object(Default::default()));
}

#[test]
fn ifng_cross_panel_arrow_keeps_a_visible_terminal_and_its_scientific_caption() {
    // Actual v61 sketch geometry: edge-3 ended with a 3.68-unit straight leg.
    let original: Value =
        serde_json::from_str(include_str!("fixtures/ifng-short-terminal.json")).unwrap();
    let changed = scene::apply(
        &original,
        &[Operation::Reroute {
            id: "edge-3".into(),
        }],
    )
    .unwrap();
    let find = |document: &Value, id: &str| {
        document["elements"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["id"] == id)
            .unwrap()
            .clone()
    };
    let arrow = find(&changed, "edge-3");
    let points = arrow["points"].as_array().unwrap();
    let last = &points[points.len() - 1];
    let previous = &points[points.len() - 2];
    let span = (last[0].as_f64().unwrap() - previous[0].as_f64().unwrap())
        .hypot(last[1].as_f64().unwrap() - previous[1].as_f64().unwrap());
    assert!(
        span >= 24.,
        "arrowhead must remain visible after label balancing: {span}, {points:?}"
    );
    for old in original["elements"].as_array().unwrap() {
        let id = old["id"].as_str().unwrap();
        if id != "edge-3" && id != "edge-label-3" {
            assert_eq!(find(&changed, id), *old);
        }
    }
    let before = find(&original, "edge-3");
    for key in [
        "startBinding",
        "endBinding",
        "startArrowhead",
        "endArrowhead",
        "boundElements",
    ] {
        assert_eq!(arrow[key], before[key]);
    }
    let caption = find(&changed, "edge-label-3");
    let before_caption = find(&original, "edge-label-3");
    for key in [
        "text",
        "originalText",
        "fontSize",
        "fontFamily",
        "width",
        "height",
        "containerId",
    ] {
        assert_eq!(caption[key], before_caption[key]);
    }
    assert_eq!(changed["files"], original["files"]);
}

#[test]
fn endpoint_clearance_has_local_turning_points_instead_of_a_return_loop() {
    let original: Value =
        serde_json::from_str(include_str!("fixtures/ifng-three-column-routes.json")).unwrap();
    for id in ["edge-1", "edge-3"] {
        let changed = scene::apply(&original, &[Operation::Reroute { id: id.into() }]).unwrap();
        let find = |doc: &Value, id: &str| {
            doc["elements"]
                .as_array()
                .unwrap()
                .iter()
                .find(|e| e["id"] == id)
                .unwrap()
                .clone()
        };
        let arrow = find(&changed, id);
        let old = find(&original, id);
        let points = arrow["points"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| [p[0].as_f64().unwrap(), p[1].as_f64().unwrap()])
            .collect::<Vec<_>>();
        let length: f64 = points
            .windows(2)
            .map(|p| (p[0][0] - p[1][0]).abs() + (p[0][1] - p[1][1]).abs())
            .sum();
        let first = points[0];
        let last = *points.last().unwrap();
        let direct = (first[0] - last[0]).abs() + (first[1] - last[1]).abs();
        assert!(
            length <= direct + 150.,
            "{id} takes a return loop: {length} versus Manhattan {direct}: {points:?}"
        );
        let penultimate = points[points.len() - 2];
        assert!((last[0] - penultimate[0]).hypot(last[1] - penultimate[1]) >= 24.);
        let caption_id = format!("edge-label-{}", id.strip_prefix("edge-").unwrap());
        for e in original["elements"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|e| e["id"] != id && e["id"] != caption_id)
        {
            assert_eq!(find(&changed, e["id"].as_str().unwrap()), *e);
        }
        for key in ["startArrowhead", "endArrowhead", "customData"] {
            assert_eq!(arrow[key], old[key]);
        }
        for key in ["startBinding", "endBinding"] {
            assert_eq!(arrow[key]["elementId"], old[key]["elementId"]);
        }
        let caption = find(&changed, &caption_id);
        let before_caption = find(&original, &caption_id);
        for key in [
            "text",
            "originalText",
            "width",
            "height",
            "fontSize",
            "fontFamily",
            "containerId",
        ] {
            assert_eq!(caption[key], before_caption[key]);
        }
    }
}

#[test]
fn locked_connector_or_caption_rejects_reroute_without_mutation() {
    let original: Value =
        serde_json::from_str(include_str!("fixtures/ifng-three-column-routes.json")).unwrap();
    let arrow = original["elements"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["type"] == "arrow" && e["customData"]["crossPanel"] == true)
        .unwrap();
    let arrow_id = arrow["id"].as_str().unwrap().to_owned();
    let caption_id = original["elements"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["type"] == "text" && e["containerId"] == arrow_id)
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    for id in [&arrow_id, &caption_id] {
        let mut locked = original.clone();
        locked["elements"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|e| e["id"] == *id)
            .unwrap()["locked"] = json!(true);
        let before = locked.clone();
        assert!(
            scene::apply(
                &locked,
                &[Operation::Reroute {
                    id: arrow_id.clone()
                }]
            )
            .is_err()
        );
        assert_eq!(locked, before);
    }
}

#[test]
fn translating_a_frame_rigidly_moves_contents_and_reroutes_cross_panel_connections() {
    let original: Value =
        serde_json::from_str(include_str!("fixtures/ifng-three-column-routes.json")).unwrap();
    let changed =
        visualization::arrange::translate(&original, &["panel-1".into()], 30., 20.).unwrap();
    let cross_ids: Vec<_> = original["elements"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["type"] == "arrow" && e["customData"]["crossPanel"] == true)
        .map(|e| e["id"].clone())
        .collect();

    let find = |doc: &Value, id: &Value| {
        doc["elements"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["id"] == *id)
            .unwrap()
            .clone()
    };
    for old in original["elements"].as_array().unwrap() {
        let next = find(&changed, &old["id"]);
        if old["id"] == "panel-1" || old["frameId"] == "panel-1" {
            assert_eq!(
                next["x"].as_f64().unwrap(),
                old["x"].as_f64().unwrap() + 30.
            );
            assert_eq!(
                next["y"].as_f64().unwrap(),
                old["y"].as_f64().unwrap() + 20.
            );
        }
        if old["id"] != "panel-1"
            && old["frameId"] != "panel-1"
            && !cross_ids.contains(&old["id"])
            && !cross_ids.contains(&old["containerId"])
        {
            assert_eq!(next, *old, "unrelated object moved: {}", old["id"]);
        }
        for key in [
            "type",
            "frameId",
            "text",
            "originalText",
            "fontSize",
            "fontFamily",
            "customData",
            "width",
            "height",
        ] {
            if old["type"] != "arrow" {
                assert_eq!(next[key], old[key], "{} {key}", old["id"]);
            }
        }
        if old["type"] == "arrow" {
            for key in [
                "customData",
                "startArrowhead",
                "endArrowhead",
                "strokeStyle",
            ] {
                assert_eq!(next[key], old[key]);
            }
            for key in ["startBinding", "endBinding"] {
                assert_eq!(next[key]["elementId"], old[key]["elementId"]);
            }
        }
    }
    assert_eq!(changed["files"], original["files"]);
}

#[test]
fn translating_owned_artwork_updates_only_derived_fit_position_metadata() {
    let original = json!({"elements":[
        {"id":"anchor","type":"rectangle","x":20,"y":20,"width":30,"height":40,"angle":0,
         "customData":{"evidence":["retained"],"visualAssetElementId":"art","visualAnchorFit":{"fileId":"f","normalizedBounds":[0.1,0.2,0.3,0.4],"imageBounds":[10,0,100,100]}}},
        {"id":"art","type":"image","fileId":"f","x":10,"y":0,"width":100,"height":100,"angle":0,"customData":{"semanticAnchorId":"anchor"}}
    ],"files":{}});
    let changed = visualization::arrange::translate(&original, &["anchor".into()], 7., 9.).unwrap();
    assert_eq!(
        changed["elements"][0]["customData"]["visualAnchorFit"]["imageBounds"],
        json!([17., 9., 100, 100])
    );
    assert_eq!(
        changed["elements"][0]["customData"]["visualAnchorFit"]["normalizedBounds"],
        original["elements"][0]["customData"]["visualAnchorFit"]["normalizedBounds"]
    );
    assert_eq!(
        changed["elements"][0]["customData"]["evidence"],
        original["elements"][0]["customData"]["evidence"]
    );
    assert_eq!(changed["elements"][1]["x"], 17.);
    assert_eq!(changed["elements"][1]["y"], 9.);
    assert_eq!(
        changed["elements"][1]["customData"],
        original["elements"][1]["customData"]
    );
}
