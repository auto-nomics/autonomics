use serde_json::{Value, json};
use visualization::{
    scene::{self, Operation},
    sketch::{self, Plan},
};
fn plan(panels: bool) -> Plan {
    let mut input = json!({"message":"Explicit relation styles","audience":"Researchers","layout":"horizontal","nodes":[{"id":"a","label":"A"},{"id":"b","label":"B"},{"id":"c","label":"C"},{"id":"d","label":"D"}],"edges":[{"from":"a","to":"b","label":"legacy","evidence":["fixture-source"]},{"from":"b","to":"c","direction":"bidirectional","label":"both","evidence":["fixture-source"]},{"from":"a","to":"d","direction":"undirected","label":"association","evidence":["fixture-source"]},{"from":"d","to":"a","inhibitory":true,"label":"inhibition","evidence":["fixture-source"]}]});
    if panels {
        input["panels"] = json!([{"id":"p1","title":"Left","column":0,"row":0},{"id":"p2","title":"Right","column":1,"row":0}]);
        for (i, node) in input["nodes"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .enumerate()
        {
            node["panel"] = json!(if i < 2 { "p1" } else { "p2" });
        }
    }
    serde_json::from_value(input).unwrap()
}
fn edge(scene: &Value, index: usize) -> &Value {
    scene["elements"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["id"] == format!("edge-{index}"))
        .unwrap()
}
#[test]
fn explicit_directions_survive_panels_rerouting_and_native_scene_edits() {
    for panels in [false, true] {
        let scene = sketch::compile(&plan(panels)).unwrap();
        for (index, start, end, direction) in [
            (0, Value::Null, json!("arrow"), "forward"),
            (1, json!("arrow"), json!("arrow"), "bidirectional"),
            (2, Value::Null, Value::Null, "undirected"),
            (3, Value::Null, json!("bar"), "forward"),
        ] {
            let arrow = edge(&scene, index);
            assert_eq!(arrow["startArrowhead"], start);
            assert_eq!(arrow["endArrowhead"], end);
            assert_eq!(arrow["customData"]["direction"], direction);
            assert_eq!(arrow["customData"]["evidence"], json!(["fixture-source"]));
        }
        if panels {
            for index in [1, 2, 3] {
                let rerouted = scene::apply(
                    &scene,
                    &[Operation::Reroute {
                        id: format!("edge-{index}"),
                    }],
                )
                .unwrap();
                for key in ["startArrowhead", "endArrowhead", "customData"] {
                    assert_eq!(edge(&rerouted, index)[key], edge(&scene, index)[key]);
                }
            }
        }
    }
}
#[test]
fn contradictory_inhibition_is_rejected_without_silent_direction_changes() {
    for panels in [false, true] {
        for direction in [
            sketch::Direction::Undirected,
            sketch::Direction::Bidirectional,
        ] {
            let mut input = plan(panels);
            input.edges[3].direction = direction;
            assert!(
                sketch::compile(&input)
                    .unwrap_err()
                    .to_string()
                    .contains("inhibitory is supported only for forward")
            );
            assert_eq!(input.edges[3].direction, direction);
            assert!(input.edges[3].inhibitory);
        }
    }
}

#[test]
fn local_direction_edits_preserve_geometry_evidence_files_and_other_objects() {
    let mut source = sketch::compile(&plan(true)).unwrap();
    source["files"] = json!({"reference":{"dataURL":"unchanged-image-bytes"}});
    for (direction, inhibitory, start, end) in [
        (
            sketch::Direction::Undirected,
            false,
            Value::Null,
            Value::Null,
        ),
        (
            sketch::Direction::Bidirectional,
            false,
            json!("arrow"),
            json!("arrow"),
        ),
        (sketch::Direction::Forward, true, Value::Null, json!("bar")),
    ] {
        let result = scene::apply(
            &source,
            &[Operation::SetRelationDirection {
                id: "edge-1".into(),
                direction,
                inhibitory,
            }],
        )
        .unwrap();
        assert_eq!(result["files"], source["files"]);
        assert_eq!(result["scientificPlan"], source["scientificPlan"]);
        for original in source["elements"].as_array().unwrap() {
            let next = result["elements"]
                .as_array()
                .unwrap()
                .iter()
                .find(|e| e["id"] == original["id"])
                .unwrap();
            if original["id"] != "edge-1" {
                assert_eq!(next, original);
                continue;
            }
            assert_eq!(next["startArrowhead"], start);
            assert_eq!(next["endArrowhead"], end);
            assert_eq!(next["customData"]["direction"], json!(direction));
            assert_eq!(next["customData"]["inhibitory"], inhibitory);
            let mut expected = original.clone();
            let mut actual = next.clone();
            for key in [
                "startArrowhead",
                "endArrowhead",
                "version",
                "versionNonce",
                "updated",
            ] {
                expected.as_object_mut().unwrap().remove(key);
                actual.as_object_mut().unwrap().remove(key);
            }
            for key in ["direction", "inhibitory"] {
                expected["customData"].as_object_mut().unwrap().remove(key);
                actual["customData"].as_object_mut().unwrap().remove(key);
            }
            assert_eq!(actual, expected, "all unrelated target fields survive");
        }
    }
}
#[test]
fn local_direction_edits_reject_annotations_locks_opaque_metadata_and_conflicts() {
    let source = sketch::compile(&plan(false)).unwrap();
    assert!(
        serde_json::from_value::<Operation>(
            json!({"op":"set_relation_direction","id":"edge-0","direction":"forward"})
        )
        .is_err(),
        "inhibitory must be an explicit decision"
    );
    for changes in [
        json!({"locked":true}),
        json!({"isDeleted":true}),
        json!({"customData":"opaque"}),
        json!({"customData":{"annotationKind":"inset_callout"}}),
        json!({"customData":{"inset":{"kind":"image_inset_snapshot"}}}),
    ] {
        let mut invalid = source.clone();
        let arrow = invalid["elements"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|e| e["id"] == "edge-0")
            .unwrap();
        arrow
            .as_object_mut()
            .unwrap()
            .extend(changes.as_object().unwrap().clone());
        let before = invalid.clone();
        let result = scene::apply(
            &invalid,
            &[
                Operation::Update {
                    id: "node-0".into(),
                    changes: json!({"x":999}).as_object().unwrap().clone(),
                },
                Operation::SetRelationDirection {
                    id: "edge-0".into(),
                    direction: sketch::Direction::Undirected,
                    inhibitory: false,
                },
            ],
        );
        assert!(result.is_err());
        assert_eq!(invalid, before);
    }
    assert!(
        scene::apply(
            &source,
            &[Operation::SetRelationDirection {
                id: "edge-0".into(),
                direction: sketch::Direction::Bidirectional,
                inhibitory: true
            }]
        )
        .is_err()
    );
    assert!(
        scene::apply(
            &source,
            &[Operation::SetRelationDirection {
                id: "node-0".into(),
                direction: sketch::Direction::Forward,
                inhibitory: false
            }]
        )
        .is_err()
    );
}

#[test]
fn evidence_edit_checks_exact_prior_value_and_preserves_other_fields() {
    let mut source = sketch::compile(&plan(false)).unwrap();
    let arrow = source["elements"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|e| e["id"] == "edge-0")
        .unwrap();
    arrow["customData"]["evidence"] = json!(["prior",{"doi":"structured-reference"}]);
    arrow["customData"]["privateNote"] = json!("preserve");
    let prior = arrow["customData"]["evidence"].clone();
    let operation = |expected, evidence| Operation::SetRelationEvidence {
        id: "edge-0".into(),
        expected_evidence: expected,
        evidence,
    };
    let changed = scene::apply(
        &source,
        &[operation(
            prior.clone(),
            vec!["User supplied DOI or reference".into()],
        )],
    )
    .unwrap();
    let mut actual = edge(&changed, 0).clone();
    let mut expected = edge(&source, 0).clone();
    actual.as_object_mut().unwrap().remove("version");
    actual.as_object_mut().unwrap().remove("versionNonce");
    actual.as_object_mut().unwrap().remove("updated");
    expected.as_object_mut().unwrap().remove("version");
    expected.as_object_mut().unwrap().remove("versionNonce");
    expected.as_object_mut().unwrap().remove("updated");
    expected["customData"]["evidence"] = json!(["User supplied DOI or reference"]);
    assert_eq!(actual, expected);
    assert_eq!(changed["scientificPlan"], source["scientificPlan"]);
    for entry in source["elements"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["id"] != "edge-0")
    {
        assert_eq!(
            changed["elements"]
                .as_array()
                .unwrap()
                .iter()
                .find(|e| e["id"] == entry["id"])
                .unwrap(),
            entry
        );
    }
    assert!(scene::apply(&source, &[operation(json!(["prior"]), vec![])]).is_err());
    for invalid in [
        vec![" ".into()],
        vec!["x".repeat(2001)],
        vec!["x".into(); 101],
    ] {
        assert!(scene::apply(&source, &[operation(prior.clone(), invalid)]).is_err());
    }
    let cleared = scene::apply(&source, &[operation(prior.clone(), vec![])]).unwrap();
    assert_eq!(edge(&cleared, 0)["customData"]["evidence"], json!([]));
    for patch in [
        json!({"locked":true}),
        json!({"isDeleted":true}),
        json!({"customData":"opaque"}),
        json!({"customData":{"annotationKind":"review"}}),
        json!({"customData":{"inset":{}}}),
    ] {
        let mut invalid = source.clone();
        invalid["elements"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|e| e["id"] == "edge-0")
            .unwrap()
            .as_object_mut()
            .unwrap()
            .extend(patch.as_object().unwrap().clone());
        assert!(scene::apply(&invalid, &[operation(prior.clone(), vec![])]).is_err());
    }
}

#[test]
fn evidence_replacement_requires_explicit_prior_and_new_values() {
    assert!(
        serde_json::from_value::<Operation>(
            json!({"op":"set_relation_evidence","id":"edge-0","evidence":[]})
        )
        .is_err()
    );
    assert!(
        serde_json::from_value::<Operation>(
            json!({"op":"set_relation_evidence","id":"edge-0","expected_evidence":null})
        )
        .is_err()
    );
}
