use serde_json::{Value, json};
use visualization::{
    image,
    scene::{self, Operation},
};
fn fixture() -> Value {
    fixture_for("horizontal")
}
fn fixture_for(layout: &str) -> Value {
    let plan=serde_json::from_value(json!({"message":"Generic relation","audience":"Researchers","layout":layout,"nodes":[{"id":"a","label":"Ligand"},{"id":"b","label":"Receptor","evidence":["declared reference"]},{"id":"c","label":"Response"}],"edges":[{"from":"a","to":"b","label":"binds","evidence":["binding reference"]},{"from":"b","to":"c","label":"activates","evidence":["activation reference"]}]})).unwrap();
    let scene = visualization::sketch::compile(&plan).unwrap();
    image::illustrate(
        &scene,
        "node-1",
        image::Image {
            file_id: "art",
            data_url: "data:image/png;base64,fixture",
            width: 30,
            height: 100,
            provenance: json!({"asset":{"license":"cc-0","author":"Fixture"},"source":"Fixture"}),
        },
    )
    .unwrap()
    .0
}
fn element<'a>(d: &'a Value, id: &str) -> &'a Value {
    d["elements"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["id"] == id)
        .unwrap()
}
fn fit() -> Operation {
    Operation::FitIllustrationAnchor {
        id: "node-1".into(),
        file_id: "art".into(),
        normalized_bounds: [0.2, 0., 0.6, 1.],
    }
}
#[test]
fn narrows_anchor_and_connects_both_sides_without_changing_art_science_or_other_nodes() {
    let original = fixture();
    let next = scene::apply(&original, &[fit()]).unwrap();
    let old = element(&original, "node-1");
    let fitted = element(&next, "node-1");
    let visual = old["customData"]["visualAssetElementId"].as_str().unwrap();
    assert_eq!(element(&next, visual), element(&original, visual));
    assert_eq!(next["files"], original["files"]);
    assert_eq!(fitted["width"].as_f64(), Some(18.)); // A 30-wide illustration fitted to its middle 60 percent.
    assert_eq!(
        fitted["customData"]["evidence"],
        old["customData"]["evidence"]
    );
    for id in [
        "node-0",
        "label-0",
        "label-1",
        "node-2",
        "label-2",
        "edge-label-0",
        "edge-label-1",
    ] {
        assert_eq!(element(&next, id), element(&original, id));
    }
    for id in ["edge-0", "edge-1"] {
        let before = element(&original, id);
        let after = element(&next, id);
        for key in [
            "startBinding",
            "endBinding",
            "startArrowhead",
            "endArrowhead",
            "customData",
            "strokeColor",
            "strokeWidth",
        ] {
            assert_eq!(after[key], before[key], "{id}/{key}");
        }
        let points = after["points"].as_array().unwrap();
        let point = if id == "edge-0" {
            points.last().unwrap()
        } else {
            &points[0]
        };
        let endpoint = after["x"].as_f64().unwrap() + point[0].as_f64().unwrap();
        let expected = if id == "edge-0" {
            fitted["x"].as_f64().unwrap() - 10.
        } else {
            fitted["x"].as_f64().unwrap() + fitted["width"].as_f64().unwrap() + 10.
        };
        assert!((endpoint - expected).abs() < 1e-6);
    }
}
#[test]
fn invalid_ownership_versions_geometry_and_protected_edges_reject_the_whole_batch() {
    let original = fixture();
    let visual = element(&original, "node-1")["customData"]["visualAssetElementId"]
        .as_str()
        .unwrap()
        .to_owned();
    for (id, patch) in [
        ("node-1", json!({"locked":true})),
        (visual.as_str(), json!({"locked":true})),
        (visual.as_str(), json!({"crop":{"x":0}})),
        (visual.as_str(), json!({"scale":[-1,1]})),
        ("edge-0", json!({"locked":true})),
        ("edge-0", json!({"roundness":{"type":2}})),
        ("edge-label-0", json!({"locked":true})),
        ("edge-1", json!({"angle":0.2})),
    ] {
        let mut broken = original.clone();
        broken["elements"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|e| e["id"] == id)
            .unwrap()
            .as_object_mut()
            .unwrap()
            .extend(patch.as_object().unwrap().clone());
        assert!(scene::apply(&broken, &[fit()]).is_err(), "{id}/{patch}");
        assert_eq!(element(&original, "node-1")["width"].as_f64(), Some(220.));
    }
    for bounds in [
        [-0.1, 0., 1., 1.],
        [0., 0., 0., 1.],
        [0., 0., 2., 1.],
        [0., 0., 0.001, 1.],
    ] {
        assert!(
            scene::apply(
                &original,
                &[Operation::FitIllustrationAnchor {
                    id: "node-1".into(),
                    file_id: "art".into(),
                    normalized_bounds: bounds
                }]
            )
            .is_err()
        );
    }
    assert!(
        scene::apply(
            &original,
            &[Operation::FitIllustrationAnchor {
                id: visual,
                file_id: "wrong-file".into(),
                normalized_bounds: [0., 0., 1., 1.]
            }]
        )
        .is_err()
    );
}

#[test]
fn vertical_ports_keep_existing_label_centers_and_native_bindings() {
    let original = fixture_for("vertical");
    let operation = Operation::FitIllustrationAnchor {
        id: "node-1".into(),
        file_id: "art".into(),
        normalized_bounds: [0.2, 0.1, 0.6, 0.8],
    };
    let next = scene::apply(&original, &[operation]).unwrap();
    let anchor = element(&next, "node-1");
    for id in ["edge-0", "edge-1"] {
        let before = element(&original, id);
        let after = element(&next, id);
        let points = after["points"].as_array().unwrap();
        let point = if id == "edge-0" {
            points.last().unwrap()
        } else {
            &points[0]
        };
        let y = after["y"].as_f64().unwrap() + point[1].as_f64().unwrap();
        let expected = if id == "edge-0" {
            anchor["y"].as_f64().unwrap() - 10.
        } else {
            anchor["y"].as_f64().unwrap() + anchor["height"].as_f64().unwrap() + 10.
        };
        assert!((y - expected).abs() < 1e-6);
        assert_eq!(after["startBinding"], before["startBinding"]);
        assert_eq!(after["endBinding"], before["endBinding"]);
    }
    for id in ["edge-label-0", "edge-label-1"] {
        assert_eq!(element(&next, id), element(&original, id));
    }
}

#[test]
fn replacing_fitted_art_preserves_the_layout_slot_and_allows_explicit_expansion_and_refit() {
    let original = fixture();
    let fitted = scene::apply(&original, &[fit()]).unwrap();
    let replacement = |id, width| image::Image {
        file_id: id,
        data_url: "data:image/png;base64,replacement",
        width,
        height: 100,
        provenance: json!({"source":"fixture"}),
    };
    let (wide, id) = image::illustrate(&fitted, "node-1", replacement("wide", 200)).unwrap();
    assert!(
        (element(&wide, &id)["width"].as_f64().unwrap() - 200.).abs() < 1e-6,
        "new art uses the original layout slot, not the tiny fitted anchor"
    );
    assert!(
        element(&wide, "node-1")["customData"]["visualAnchorFit"].is_null(),
        "new pixels invalidate the old fit"
    );
    for arrow in ["edge-0", "edge-1"] {
        assert_eq!(
            element(&wide, arrow),
            element(&fitted, arrow),
            "replacement alone preserves relation geometry"
        );
    }
    let wide_fit = scene::apply(
        &wide,
        &[Operation::FitIllustrationAnchor {
            id: id.clone(),
            file_id: "wide".into(),
            normalized_bounds: [0., 0., 1., 1.],
        }],
    )
    .unwrap();
    assert!((element(&wide_fit, "node-1")["width"].as_f64().unwrap() - 200.).abs() < 1e-6);
    let (narrow, narrow_id) =
        image::illustrate(&wide_fit, "node-1", replacement("narrow", 30)).unwrap();
    assert_eq!(id, narrow_id);
    assert!(
        (element(&narrow, &id)["width"].as_f64().unwrap() - 30.).abs() < 1e-6,
        "repeated replacement does not accumulate shrinkage"
    );
    let refitted = scene::apply(
        &narrow,
        &[Operation::FitIllustrationAnchor {
            id: id.clone(),
            file_id: "narrow".into(),
            normalized_bounds: [0., 0., 1., 1.],
        }],
    )
    .unwrap();
    assert!((element(&refitted, "node-1")["width"].as_f64().unwrap() - 30.).abs() < 1e-6);
    assert_eq!(
        element(&refitted, "node-1")["customData"]["evidence"],
        element(&original, "node-1")["customData"]["evidence"]
    );
    let moved=scene::apply(&fitted,&[Operation::Update{id:"node-1".into(),changes:serde_json::from_value(json!({"x":element(&fitted,"node-1")["x"].as_f64().unwrap()+50.,"y":element(&fitted,"node-1")["y"].as_f64().unwrap()+30.})).unwrap()}]).unwrap();
    let (moved_replacement, moved_id) =
        image::illustrate(&moved, "node-1", replacement("moved", 200)).unwrap();
    assert!(
        (element(&moved_replacement, &moved_id)["width"]
            .as_f64()
            .unwrap()
            - 200.)
            .abs()
            < 1e-6
    );
    assert!(
        (element(&moved_replacement, &moved_id)["x"]
            .as_f64()
            .unwrap()
            - element(&wide, &id)["x"].as_f64().unwrap()
            - 50.)
            .abs()
            < 1e-6
    );
    assert!(
        (element(&moved_replacement, &moved_id)["y"]
            .as_f64()
            .unwrap()
            - element(&wide, &id)["y"].as_f64().unwrap()
            - 30.)
            .abs()
            < 1e-6
    );
}
