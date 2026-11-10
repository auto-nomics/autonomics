use serde_json::{Value, json};
use visualization::{
    Store, image,
    planning::Brief,
    scene::{self, Operation},
    sketch::{self, Plan},
};
fn plan() -> Plan {
    serde_json::from_value(json!({"message":"Conceptual condition comparison", "audience":"researchers", "layout":"horizontal",
        "panels":[{"id":"control","title":"A · Control","column":0,"row":0},{"id":"treatment","title":"B · Treatment","column":1,"row":0}],
        "nodes":[{"id":"a","label":"Input","panel":"control"},{"id":"b","label":"Output","panel":"control"},{"id":"c","label":"Input","panel":"treatment"},{"id":"d","label":"Output","panel":"treatment"}],
        "edges":[{"from":"a","to":"b","label":"conceptual relation"},{"from":"c","to":"d","label":"explicit comparison"}],
        "assumptions":["Synthetic comparison, no biological claim"]})).unwrap()
}
fn find<'a>(scene: &'a Value, id: &str) -> &'a Value {
    scene["elements"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["id"] == id)
        .unwrap()
}
#[test]
fn panels_preserve_global_ids_relations_and_native_frame_membership() {
    let scene = sketch::compile(&plan()).unwrap();
    let elements = scene["elements"].as_array().unwrap();
    assert_eq!(elements.iter().filter(|e| e["type"] == "frame").count(), 2);
    assert_eq!(elements.iter().filter(|e| e["type"] == "arrow").count(), 2);
    assert_eq!(find(&scene, "node-2")["frameId"], "panel-1");
    assert_eq!(
        find(&scene, "edge-1")["startBinding"]["elementId"],
        "node-2"
    );
    assert_eq!(find(&scene, "edge-label-1")["containerId"], "edge-1");
    assert_eq!(find(&scene, "panel-title-0")["text"], "A · Control");
    for element in elements.iter().filter(|e| e["type"] != "frame") {
        let frame = find(&scene, element["frameId"].as_str().unwrap());
        assert!(element["x"].as_f64().unwrap() >= frame["x"].as_f64().unwrap());
        assert!(element["y"].as_f64().unwrap() >= frame["y"].as_f64().unwrap());
    }
    let left = find(&scene, "panel-0");
    let right = find(&scene, "panel-1");
    assert!(
        left["x"].as_f64().unwrap() + left["width"].as_f64().unwrap()
            < right["x"].as_f64().unwrap()
    );
}
#[test]
fn moving_an_illustrated_panel_moves_all_contents_and_preserves_other_panel() {
    let scene = sketch::compile(&plan()).unwrap();
    let (scene, visual) = image::illustrate(
        &scene,
        "node-0",
        image::Image {
            file_id: "fixture",
            data_url: "data:image/png;base64,abc",
            width: 40,
            height: 100,
            provenance: json!({"source":"fixture"}),
        },
    )
    .unwrap();
    assert_eq!(find(&scene, &visual)["frameId"], "panel-0");
    let moved = scene::apply(
        &scene,
        &[Operation::Update {
            id: "panel-0".into(),
            changes: serde_json::from_value(json!({"x":60,"y":80})).unwrap(),
        }],
    )
    .unwrap();
    for before in scene["elements"].as_array().unwrap() {
        let after = find(&moved, before["id"].as_str().unwrap());
        if before["frameId"] == "panel-0" || before["id"] == "panel-0" {
            assert_eq!(
                after["x"].as_f64().unwrap(),
                before["x"].as_f64().unwrap() + 60.
            );
            assert_eq!(
                after["y"].as_f64().unwrap(),
                before["y"].as_f64().unwrap() + 80.
            );
            assert_eq!(after["points"], before["points"]);
            assert_eq!(after["frameId"], before["frameId"]);
        } else {
            assert_eq!(before, after);
        }
    }
    let unframed = scene::apply(
        &scene,
        &[Operation::Remove {
            id: "panel-0".into(),
        }],
    )
    .unwrap();
    assert_eq!(
        unframed["elements"].as_array().unwrap().len() + 1,
        scene["elements"].as_array().unwrap().len()
    );
    assert!(find(&unframed, "node-0")["frameId"].is_null());
}
#[test]
fn invalid_panel_structures_are_rejected_without_dropping_relations() {
    let mut p = plan();
    p.edges[0].to = "missing".into();
    assert!(
        sketch::compile(&p)
            .unwrap_err()
            .to_string()
            .contains("endpoint")
    );
    let mut p = plan();
    p.nodes[0].panel = None;
    assert!(sketch::compile(&p).is_err());
    let mut p = plan();
    p.panels[1].column = 0;
    assert!(sketch::compile(&p).is_err());
    let mut p = plan();
    p.panels[1].id = "control".into();
    assert!(sketch::compile(&p).is_err());
    let mut p = plan();
    p.nodes[2].id = "a".into();
    assert!(sketch::compile(&p).is_err());
    let mut p = plan();
    p.panels.clear();
    assert!(sketch::compile(&p).is_err());
}
#[test]
fn panel_planning_is_versioned_and_proposed_before_adoption() -> visualization::Result<()> {
    let mut store = Store::in_memory()?;
    store.create_project("p", "Comparison")?;
    let figure = store.create("p", "Panels", "excalidraw/v2", &json!({"elements":[]}))?;
    let brief: Brief = serde_json::from_value(serde_json::to_value(plan()).unwrap()).unwrap();
    let saved = store.save_planning("p", &figure.id, 1, 0, &brief)?;
    assert!(saved.issues.is_empty());
    assert_eq!(saved.document.panels.len(), 2);
    let proposal = store.planned_proposal("p", &figure.id, 1, 1, "Comparison")?;
    assert_eq!(
        proposal.document["elements"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|e| e["type"] == "frame")
            .count(),
        2
    );
    assert_eq!(
        store.revision("p", &figure.id, 1)?.document["elements"],
        json!([])
    );
    Ok(())
}
fn cross_plan() -> Plan {
    let mut p = plan();
    for edge in [
        json!({"from":"b","to":"c","label":"carry forward","evidence":["explicit user relationship"]}),
        json!({"from":"d","to":"a","label":"feedback","inhibitory":true}),
        json!({"from":"b","to":"c","label":"parallel comparison"}),
    ] {
        p.edges.push(serde_json::from_value(edge).unwrap());
    }
    p
}
fn endpoint(e: &Value, last: bool) -> [f64; 2] {
    let points = e["points"].as_array().unwrap();
    let p = if last {
        points.last().unwrap()
    } else {
        &points[0]
    };
    [
        e["x"].as_f64().unwrap() + p[0].as_f64().unwrap(),
        e["y"].as_f64().unwrap() + p[1].as_f64().unwrap(),
    ]
}
#[test]
fn explicit_cross_panel_edges_are_editable_bound_and_avoid_nodes_and_titles() {
    let p = cross_plan();
    let scene = sketch::compile(&p).unwrap();
    assert_eq!(
        scene,
        sketch::compile(&p).unwrap(),
        "routing is deterministic"
    );
    let elements = scene["elements"].as_array().unwrap();
    assert_eq!(
        elements.iter().filter(|e| e["type"] == "arrow").count(),
        p.edges.len()
    );
    for index in 2..5 {
        let arrow = find(&scene, &format!("edge-{index}"));
        assert!(arrow["frameId"].is_null());
        assert_eq!(arrow["customData"]["crossPanel"], true);
        let points = arrow["points"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| {
                [
                    arrow["x"].as_f64().unwrap() + p[0].as_f64().unwrap(),
                    arrow["y"].as_f64().unwrap() + p[1].as_f64().unwrap(),
                ]
            })
            .collect::<Vec<_>>();
        for segment in points.windows(2) {
            let [a, b] = [segment[0], segment[1]];
            assert!(a[0] == b[0] || a[1] == b[1]);
            for obstacle in elements.iter().filter(|e| {
                e["type"] == "rectangle"
                    || e["id"]
                        .as_str()
                        .is_some_and(|id| id.starts_with("panel-title-"))
            }) {
                let (l, t) = (
                    obstacle["x"].as_f64().unwrap(),
                    obstacle["y"].as_f64().unwrap(),
                );
                let r = l + obstacle["width"].as_f64().unwrap();
                let bottom = t + obstacle["height"].as_f64().unwrap();
                assert!(
                    !(a[0] == b[0]
                        && a[0] > l
                        && a[0] < r
                        && a[1].min(b[1]) < bottom
                        && a[1].max(b[1]) > t
                        || a[1] == b[1]
                            && a[1] > t
                            && a[1] < bottom
                            && a[0].min(b[0]) < r
                            && a[0].max(b[0]) > l),
                    "route crosses {}",
                    obstacle["id"]
                );
            }
        }
        let label = find(&scene, &format!("edge-label-{index}"));
        assert!(label["frameId"].is_null());
        let mid = points.len() / 2;
        let center = if points.len() % 2 == 1 {
            points[mid]
        } else {
            [
                (points[mid - 1][0] + points[mid][0]) / 2.,
                (points[mid - 1][1] + points[mid][1]) / 2.,
            ]
        };
        assert!(
            (label["x"].as_f64().unwrap() + label["width"].as_f64().unwrap() / 2. - center[0])
                .abs()
                < 0.001
        );
        assert!(
            (label["y"].as_f64().unwrap() + label["height"].as_f64().unwrap() / 2. - center[1])
                .abs()
                < 0.001
        );
    }
    assert_eq!(find(&scene, "edge-3")["endArrowhead"], "bar");
    assert_eq!(
        find(&scene, "edge-2")["customData"]["evidence"],
        json!(["explicit user relationship"])
    );
}
#[test]
fn frame_moves_update_external_bindings_without_double_moving_internal_edges() {
    let scene = sketch::compile(&cross_plan()).unwrap();
    let moved = scene::apply(
        &scene,
        &[Operation::Update {
            id: "panel-0".into(),
            changes: serde_json::from_value(json!({"x":30,"y":40})).unwrap(),
        }],
    )
    .unwrap();
    for index in 0..5 {
        let id = format!("edge-{index}");
        let before = find(&scene, &id);
        let after = find(&moved, &id);
        for last in [false, true] {
            let binding = if last { "endBinding" } else { "startBinding" };
            let node = find(&scene, before[binding]["elementId"].as_str().unwrap());
            let delta = if node["frameId"] == "panel-0" {
                [30., 40.]
            } else {
                [0., 0.]
            };
            let old = endpoint(before, last);
            let new = endpoint(after, last);
            assert_eq!(
                new,
                [old[0] + delta[0], old[1] + delta[1]],
                "{id}, last={last}"
            );
        }
    }
    assert_eq!(find(&scene, "node-2"), find(&moved, "node-2"));
    assert_eq!(find(&scene, "edge-1"), find(&moved, "edge-1"));
}
#[test]
fn reroute_uses_current_geometry_and_preserves_text_style_and_unrelated_objects() {
    let scene = sketch::compile(&cross_plan()).unwrap();
    let before=scene::apply(&scene,&[
        Operation::Add {element:json!({"id":"obstacle","type":"rectangle","x":720,"y":105,"width":60,"height":100,"angle":0})},
        Operation::Update {id:"edge-label-2".into(),changes:serde_json::from_value(json!({"text":"Edited caption","width":156,"height":36,"fontSize":24,"strokeColor":"#e03131"})).unwrap()},
    ]).unwrap();
    let after = scene::apply(
        &before,
        &[Operation::Reroute {
            id: "edge-2".into(),
        }],
    )
    .unwrap();
    let arrow = find(&after, "edge-2");
    let label = find(&after, "edge-label-2");
    for element in before["elements"].as_array().unwrap() {
        let id = element["id"].as_str().unwrap();
        let next = find(&after, id);
        if !["edge-2", "edge-label-2"].contains(&id) {
            assert_eq!(element, next);
            continue;
        }
        let mut old = element.clone();
        let mut new = next.clone();
        for key in ["x", "y", "version", "versionNonce", "updated"] {
            old.as_object_mut().unwrap().remove(key);
            new.as_object_mut().unwrap().remove(key);
        }
        if id == "edge-2" {
            for key in ["width", "height", "points"] {
                old.as_object_mut().unwrap().remove(key);
                new.as_object_mut().unwrap().remove(key);
            }
        }
        assert_eq!(old, new, "reroute preserves non-geometric fields of {id}");
    }
    assert_ne!(arrow["points"], find(&before, "edge-2")["points"]);
    assert!(
        label["x"].as_f64().unwrap() + 156. <= 720.
            || label["x"].as_f64().unwrap() >= 780.
            || label["y"].as_f64().unwrap() + 36. <= 105.
            || label["y"].as_f64().unwrap() >= 205.
    );
    let points = arrow["points"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| {
            [
                arrow["x"].as_f64().unwrap() + p[0].as_f64().unwrap(),
                arrow["y"].as_f64().unwrap() + p[1].as_f64().unwrap(),
            ]
        })
        .collect::<Vec<_>>();
    for pair in points.windows(2) {
        let a = pair[0];
        let b = pair[1];
        assert!(a[0] == b[0] || a[1] == b[1]);
        assert!(
            !(a[0] == b[0]
                && a[0] > 720.
                && a[0] < 780.
                && a[1].min(b[1]) < 205.
                && a[1].max(b[1]) > 105.
                || a[1] == b[1]
                    && a[1] > 105.
                    && a[1] < 205.
                    && a[0].min(b[0]) < 780.
                    && a[0].max(b[0]) > 720.)
        );
    }
}
#[test]
fn reroute_failures_leave_the_original_scene_unchanged() {
    let original = sketch::compile(&cross_plan()).unwrap();
    let before = original.clone();
    assert!(
        scene::apply(
            &original,
            &[
                Operation::Reroute {
                    id: "edge-2".into()
                },
                Operation::Reroute {
                    id: "edge-0".into()
                }
            ]
        )
        .is_err()
    );
    assert_eq!(original, before);
    let mut detached = original.clone();
    detached["elements"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|e| e["id"] == "edge-2")
        .unwrap()["endBinding"] = Value::Null;
    assert!(
        scene::apply(
            &detached,
            &[Operation::Reroute {
                id: "edge-2".into()
            }]
        )
        .unwrap_err()
        .to_string()
        .contains("detached")
    );
}
