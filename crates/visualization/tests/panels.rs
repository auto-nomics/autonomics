use serde_json::{Value, json};
use visualization::{
    Store, image,
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
    for index in 0..2 {
        assert_eq!(find(&scene, &format!("panel-{index}"))["name"], "");
        let heading = find(&scene, &format!("panel-title-{index}"));
        assert_eq!(heading["originalText"], plan().panels[index].title);
        assert_eq!(heading["frameId"], format!("panel-{index}"));
    }
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
    let mut original = sketch::compile(&cross_plan()).unwrap();
    // Force a real invalid second operation; legal caption route discovery
    // must not determine whether this atomicity test fails.
    original["elements"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|e| e["id"] == "edge-0")
        .unwrap()["locked"] = json!(true);
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

#[test]
fn reroute_after_fitting_an_illustration_uses_the_anchor_instead_of_its_transparent_viewport() {
    let scene = sketch::compile(&cross_plan()).unwrap();
    let (illustrated, image_id) = image::illustrate(
        &scene,
        "node-1",
        image::Image {
            file_id: "transparent-art",
            data_url: "data:image/png;base64,fixture",
            width: 100,
            height: 100,
            provenance: json!({"source":"fixture"}),
        },
    )
    .unwrap();
    let fitted = scene::apply(
        &illustrated,
        &[Operation::FitIllustrationAnchor {
            id: image_id.clone(),
            file_id: "transparent-art".into(),
            normalized_bounds: [0.3, 0.3, 0.4, 0.4],
        }],
    )
    .unwrap();
    let mut unrelated_obstacle = fitted.clone();
    let mut foreign = find(&fitted, &image_id).clone();
    foreign["id"] = json!("foreign-image");
    foreign["customData"]["semanticAnchorId"] = json!("foreign-owner");
    unrelated_obstacle["elements"]
        .as_array_mut()
        .unwrap()
        .push(foreign);
    assert!(
        scene::apply(
            &unrelated_obstacle,
            &[Operation::Reroute {
                id: "edge-2".into()
            }]
        )
        .is_err(),
        "unrelated images remain obstacles"
    );
    let mut stale = fitted.clone();
    stale["elements"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|e| e["id"] == image_id)
        .unwrap()["fileId"] = json!("changed-file");
    assert!(
        scene::apply(
            &stale,
            &[Operation::Reroute {
                id: "edge-2".into()
            }]
        )
        .is_err(),
        "stale file fits cannot ignore the image viewport"
    );
    let routed = scene::apply(
        &fitted,
        &[Operation::Reroute {
            id: "edge-2".into(),
        }],
    )
    .expect("an endpoint's own viewport must not block its explicitly fitted port");
    assert_eq!(find(&routed, &image_id), find(&fitted, &image_id));
    assert_eq!(find(&routed, "node-1"), find(&fitted, "node-1"));
    assert_eq!(routed["files"], fitted["files"]);
    for key in [
        "startBinding",
        "endBinding",
        "startArrowhead",
        "endArrowhead",
        "customData",
    ] {
        assert_eq!(find(&routed, "edge-2")[key], find(&fitted, "edge-2")[key]);
    }
}

#[test]
fn generated_and_rerouted_arrows_enter_and_leave_ports_normally() {
    let initial = sketch::compile(&cross_plan()).unwrap();
    let rerouted = scene::apply(
        &initial,
        &[Operation::Reroute {
            id: "edge-2".into(),
        }],
    )
    .unwrap();
    for scene in [&initial, &rerouted] {
        for arrow in scene["elements"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|e| e["customData"]["crossPanel"] == true)
        {
            let points = arrow["points"].as_array().unwrap();
            for (key, start) in [("startBinding", true), ("endBinding", false)] {
                let node = find(scene, arrow[key]["elementId"].as_str().unwrap());
                let gap = arrow[key]["gap"].as_f64().unwrap();
                let x = node["x"].as_f64().unwrap();
                let y = node["y"].as_f64().unwrap();
                let w = node["width"].as_f64().unwrap();
                let h = node["height"].as_f64().unwrap();
                let port = endpoint(arrow, !start);
                let n = if start {
                    &points[1]
                } else {
                    &points[points.len() - 2]
                };
                let outside = [
                    arrow["x"].as_f64().unwrap() + n[0].as_f64().unwrap(),
                    arrow["y"].as_f64().unwrap() + n[1].as_f64().unwrap(),
                ];
                let normal = ((port[0] - (x - gap)).abs() < 1e-6
                    && outside[0] < port[0]
                    && (outside[1] - port[1]).abs() < 1e-6)
                    || ((port[0] - (x + w + gap)).abs() < 1e-6
                        && outside[0] > port[0]
                        && (outside[1] - port[1]).abs() < 1e-6)
                    || ((port[1] - (y - gap)).abs() < 1e-6
                        && outside[1] < port[1]
                        && (outside[0] - port[0]).abs() < 1e-6)
                    || ((port[1] - (y + h + gap)).abs() < 1e-6
                        && outside[1] > port[1]
                        && (outside[0] - port[0]).abs() < 1e-6);
                assert!(
                    normal,
                    "{} / {key} must point into the component rather than along its edge",
                    arrow["id"]
                );
            }
        }
    }
}

#[test]
fn available_ports_do_not_reverse_over_an_existing_incoming_connection() {
    let plan:Plan=serde_json::from_value(json!({"message":"Explicit reduced relay","audience":"Researchers","layout":"vertical","panels":[{"id":"a","title":"A","column":0,"row":0},{"id":"b","title":"B","column":1,"row":0}],"nodes":[{"id":"ligand","label":"Ligand","panel":"a"},{"id":"receptor","label":"Receptor","panel":"a"},{"id":"relay","label":"Relay","panel":"b"}],"edges":[{"from":"ligand","to":"receptor","label":"binds"},{"from":"receptor","to":"relay","label":"recruits"}]})).unwrap();
    let initial = sketch::compile(&plan).unwrap();
    let (art, image_id) = image::illustrate(
        &initial,
        "node-1",
        image::Image {
            file_id: "art",
            data_url: "data:image/png;base64,fixture",
            width: 100,
            height: 100,
            provenance: json!({"source":"fixture"}),
        },
    )
    .unwrap();
    let fitted = scene::apply(
        &art,
        &[Operation::FitIllustrationAnchor {
            id: image_id,
            file_id: "art".into(),
            normalized_bounds: [0.3, 0.3, 0.4, 0.4],
        }],
    )
    .unwrap();
    let routed = scene::apply(
        &fitted,
        &[Operation::Reroute {
            id: "edge-1".into(),
        }],
    )
    .unwrap();
    for scene in [&initial, &routed] {
        let node = find(scene, "node-1");
        let incoming = endpoint(find(scene, "edge-0"), true);
        let outgoing = endpoint(find(scene, "edge-1"), false);
        assert!(
            (incoming[0] - node["x"].as_f64().unwrap() - node["width"].as_f64().unwrap() / 2.)
                .abs()
                < 1e-6
        );
        assert!((incoming[1] - node["y"].as_f64().unwrap() + 10.).abs() < 1e-6);
        assert!(
            (outgoing[0] - node["x"].as_f64().unwrap() - node["width"].as_f64().unwrap() / 2.)
                .abs()
                > 1e-6
                || (outgoing[1] - node["y"].as_f64().unwrap() + 12.).abs() > 1e-6,
            "do not overlap the existing incoming arrow in reverse when other ports are available"
        );
    }
}

#[test]
fn cross_panel_captions_receive_space_without_expanding_unrelated_tracks() {
    let mut p = plan();
    p.panels.push(
        serde_json::from_value(
            json!({"id":"unrelated","title":"C · Independent","column":2,"row":0}),
        )
        .unwrap(),
    );
    p.nodes.push(
        serde_json::from_value(
            json!({"id":"independent","label":"Unrelated component","panel":"unrelated"}),
        )
        .unwrap(),
    );
    let without = sketch::compile(&p).unwrap();
    p.edges.push(serde_json::from_value(json!({"from":"b","to":"c","label":"受体相关复合物经明确修饰位点形成下游信号转导复合物，激酶归属尚未确定","evidence":["explicit conceptual relationship"]})).unwrap());
    let scene = sketch::compile(&p).unwrap();
    let gap = |document: &Value, a: &str, b: &str| {
        let left = find(document, a);
        find(document, b)["x"].as_f64().unwrap()
            - left["x"].as_f64().unwrap()
            - left["width"].as_f64().unwrap()
    };
    let caption = find(&scene, "edge-label-2");
    assert!(gap(&scene, "panel-0", "panel-1") >= caption["width"].as_f64().unwrap() + 40. - 1e-6);
    assert!(gap(&scene, "panel-0", "panel-1") > gap(&without, "panel-0", "panel-1"));
    assert_eq!(
        gap(&scene, "panel-1", "panel-2"),
        gap(&without, "panel-1", "panel-2")
    );
    assert_eq!(caption["originalText"], p.edges[2].label);
    assert_eq!(
        caption["text"].as_str().unwrap().replace('\n', ""),
        p.edges[2].label
    );
    assert_eq!(
        find(&scene, "edge-2")["customData"]["evidence"],
        json!(p.edges[2].evidence)
    );
}

#[test]
fn ifng_captions_avoid_panel_borders_and_retain_native_label_centers() {
    let plan: Plan = serde_json::from_str(include_str!("fixtures/ifng-panel-plan.json")).unwrap();
    let scene = sketch::compile(&plan).unwrap();
    for id in ["edge-label-1", "edge-label-3"] {
        let caption = find(&scene, id);
        let (l, t) = (
            caption["x"].as_f64().unwrap(),
            caption["y"].as_f64().unwrap(),
        );
        let (r, b) = (
            l + caption["width"].as_f64().unwrap(),
            t + caption["height"].as_f64().unwrap(),
        );
        for frame in scene["elements"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|e| e["type"] == "frame")
        {
            let (fl, ft) = (frame["x"].as_f64().unwrap(), frame["y"].as_f64().unwrap());
            let (fr, fb) = (
                fl + frame["width"].as_f64().unwrap(),
                ft + frame["height"].as_f64().unwrap(),
            );
            assert!(
                !(l < fr
                    && r > fl
                    && ((t < ft + 2. && b > ft - 2.) || (t < fb + 2. && b > fb - 2.))
                    || t < fb
                        && b > ft
                        && ((l < fl + 2. && r > fl - 2.) || (l < fr + 2. && r > fr - 2.))),
                "{id} overlaps {}",
                frame["id"]
            );
        }
        let arrow = find(&scene, caption["containerId"].as_str().unwrap());
        let points = arrow["points"].as_array().unwrap();
        let mid = points.len() / 2;
        let center = if points.len() % 2 == 1 {
            [
                points[mid][0].as_f64().unwrap(),
                points[mid][1].as_f64().unwrap(),
            ]
        } else {
            [
                (points[mid - 1][0].as_f64().unwrap() + points[mid][0].as_f64().unwrap()) / 2.,
                (points[mid - 1][1].as_f64().unwrap() + points[mid][1].as_f64().unwrap()) / 2.,
            ]
        };
        assert!((arrow["x"].as_f64().unwrap() + center[0] - (l + r) / 2.).abs() < 1e-6);
        assert!((arrow["y"].as_f64().unwrap() + center[1] - (t + b) / 2.).abs() < 1e-6);
    }
}

#[test]
fn frames_share_row_height_without_changing_independent_panel_content() {
    let plan:visualization::sketch::Plan=serde_json::from_value(json!({"message":"Compare independently declared concepts","audience":"Researchers","layout":"vertical","panels":[{"id":"short","title":"Short","column":0,"row":0},{"id":"tall","title":"Tall","column":1,"row":0}],"nodes":[{"id":"a","label":"A","panel":"short","evidence":["source A"]},{"id":"b","label":"B","panel":"tall"},{"id":"c","label":"C","panel":"tall"}],"edges":[{"from":"b","to":"c","label":"Declared relation","evidence":["relation source"]}]})).unwrap();
    let scene = visualization::sketch::compile(&plan).unwrap();
    let short = find(&scene, "panel-0");
    let tall = find(&scene, "panel-1");
    assert_eq!(short["height"], tall["height"]);
    assert_eq!(short["y"], tall["y"]);
    let mut alone = plan.clone();
    alone.panels.truncate(1);
    alone.nodes.truncate(1);
    alone.edges.clear();
    let single = visualization::sketch::compile(&alone).unwrap();
    assert!(
        short["height"].as_f64().unwrap() > find(&single, "panel-0")["height"].as_f64().unwrap()
    );
    for id in ["node-0", "label-0", "panel-title-0"] {
        let mut actual = find(&scene, id).clone();
        let mut expected = find(&single, id).clone();
        if let Some(data) = actual
            .get_mut("customData")
            .and_then(serde_json::Value::as_object_mut)
        {
            data.remove("scientificPlan");
        }
        if let Some(data) = expected
            .get_mut("customData")
            .and_then(serde_json::Value::as_object_mut)
        {
            data.remove("scientificPlan");
        }
        assert_eq!(
            actual, expected,
            "{id} content and position remain independent of the taller neighbor"
        );
    }
    assert_eq!(
        find(&scene, "edge-0")["customData"]["evidence"],
        json!(["relation source"])
    );
}

#[test]
fn automatic_heading_balance_keeps_source_identifiers_and_line_budget() {
    let title = "胞质：凋亡小体上 procaspase-9 二聚化与加工";
    let plan:visualization::sketch::Plan=serde_json::from_value(json!({"message":"Reviewed heading","audience":"Researchers","layout":"vertical","panels":[{"id":"p","title":title,"column":0,"row":0}],"nodes":[{"id":"n","label":"N","panel":"p"}]})).unwrap();
    let scene = visualization::sketch::compile(&plan).unwrap();
    let heading = find(&scene, "panel-title-0");
    let text = heading["text"].as_str().unwrap();
    assert_eq!(heading["originalText"], title);
    assert_eq!(text.lines().count(), 3);
    assert!(text.contains("procaspase-9"));
    assert!(text.contains("二聚化与加工"));
    assert!(
        text.lines().last().unwrap().chars().count() >= 4,
        "orphan remains: {text}"
    );
    assert_eq!(
        text.chars()
            .filter(|c| !c.is_whitespace())
            .collect::<String>(),
        title
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect::<String>()
    );
    assert_eq!(heading["width"], 240.);
    assert_eq!(heading["height"], 75.);
    let mut explicit = plan;
    explicit.panels[0].title = "Explicit title\nPreserved break".into();
    let scene = visualization::sketch::compile(&explicit).unwrap();
    assert_eq!(
        find(&scene, "panel-title-0")["text"],
        "Explicit title\nPreserved break"
    );
}

#[test]
fn reroute_keeps_its_caption_clear_of_an_unrelated_straight_connector() {
    let original = sketch::compile(&cross_plan()).unwrap();
    let label = find(&original, "edge-label-2");
    let x = label["x"].as_f64().unwrap() - 40.;
    let y = label["y"].as_f64().unwrap() + label["height"].as_f64().unwrap() / 2.;
    let width = label["width"].as_f64().unwrap() + 80.;
    let before = scene::apply(
        &original,
        &[Operation::Add {
            element: json!({
                "id":"unrelated-connector", "type":"arrow", "x":x, "y":y,
                "width":width, "height":0., "angle":0., "roundness":null,
                "points":[[0.,0.],[width,0.]], "elbowed":false
            }),
        }],
    )
    .unwrap();
    let after = scene::apply(
        &before,
        &[Operation::Reroute {
            id: "edge-2".into(),
        }],
    )
    .unwrap();
    let next = find(&after, "edge-label-2");
    let left = next["x"].as_f64().unwrap();
    let top = next["y"].as_f64().unwrap();
    let right = left + next["width"].as_f64().unwrap();
    let bottom = top + next["height"].as_f64().unwrap();
    assert!(
        right <= x - 2. || left >= x + width + 2. || bottom <= y - 2. || top >= y + 2.,
        "the caption must not cover the unrelated connector"
    );
    for element in before["elements"].as_array().unwrap() {
        let id = element["id"].as_str().unwrap();
        if id != "edge-2" && id != "edge-label-2" {
            assert_eq!(element, find(&after, id));
        }
    }
    for field in [
        "text",
        "originalText",
        "width",
        "height",
        "fontSize",
        "fontFamily",
        "containerId",
    ] {
        assert_eq!(find(&before, "edge-label-2")[field], next[field]);
    }
    for field in [
        "startBinding",
        "endBinding",
        "startArrowhead",
        "endArrowhead",
        "customData",
    ] {
        assert_eq!(
            find(&before, "edge-2")[field],
            find(&after, "edge-2")[field]
        );
    }
}

#[test]
fn swap_frames_moves_complete_contents_and_preserves_scientific_fields() {
    let mut p = cross_plan();
    p.edges.truncate(3);
    let original = sketch::compile(&p).unwrap();
    let swapped = scene::apply(
        &original,
        &[Operation::SwapFrames {
            first_id: "panel-0".into(),
            second_id: "panel-1".into(),
        }],
    )
    .unwrap();
    for (a, b) in [("panel-0", "panel-1"), ("panel-1", "panel-0")] {
        assert_eq!(find(&swapped, a)["x"], find(&original, b)["x"]);
        assert_eq!(find(&swapped, a)["y"], find(&original, b)["y"]);
    }
    for old in original["elements"].as_array().unwrap() {
        let next = find(&swapped, old["id"].as_str().unwrap());
        for field in [
            "id",
            "text",
            "originalText",
            "fontFamily",
            "fontSize",
            "lineHeight",
            "frameId",
            "groupIds",
            "customData",
            "startArrowhead",
            "endArrowhead",
        ] {
            assert_eq!(next[field], old[field]);
        }
        if let Some(frame) = old["frameId"].as_str() {
            for axis in ["x", "y"] {
                let delta = find(&swapped, frame)[axis].as_f64().unwrap()
                    - find(&original, frame)[axis].as_f64().unwrap();
                assert!(
                    (next[axis].as_f64().unwrap() - old[axis].as_f64().unwrap() - delta).abs()
                        < 1e-6
                );
            }
        }
    }
    for field in ["files", "scientificPlan"] {
        assert_eq!(swapped[field], original[field]);
    }
    for field in ["startBinding", "endBinding"] {
        assert_eq!(
            find(&swapped, "edge-2")[field]["elementId"],
            find(&original, "edge-2")[field]["elementId"]
        );
    }
}
#[test]
fn frame_swap_rejects_locked_frames_labels_and_unsupported_connectors_without_mutation() {
    let mut p = cross_plan();
    p.edges.truncate(3);
    let original = sketch::compile(&p).unwrap();
    let op = Operation::SwapFrames {
        first_id: "panel-0".into(),
        second_id: "panel-1".into(),
    };
    for (id, key, value) in [
        ("panel-0", "locked", json!(true)),
        ("edge-label-2", "locked", json!(true)),
        ("edge-2", "roundness", json!({"type":2})),
        ("panel-0", "frameId", json!("panel-1")),
    ] {
        let mut document = original.clone();
        let element = document["elements"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|e| e["id"] == id)
            .unwrap();
        element[key] = value;
        let before = document.clone();
        assert!(scene::apply(&document, std::slice::from_ref(&op)).is_err());
        assert_eq!(document, before);
    }
    assert!(
        scene::apply(
            &original,
            &[Operation::SwapFrames {
                first_id: "panel-0".into(),
                second_id: "panel-0".into()
            }]
        )
        .is_err()
    );
}
