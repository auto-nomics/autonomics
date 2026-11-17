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
        (visual.as_str(), json!({"scale":[0.5,1]})),
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

#[test]
fn transformed_artwork_uses_source_crop_flips_and_rotation_without_changing_the_image() {
    let bounds = [0.2, 0.2, 0.4, 0.5];
    for angle in [0., 0.4, std::f64::consts::FRAC_PI_2] {
        for scale in [[1., 1.], [-1., 1.], [1., -1.], [-1., -1.]] {
            for cropped in [false, true] {
                let mut original = fixture();
                let image_id = element(&original, "node-1")["customData"]["visualAssetElementId"]
                    .as_str()
                    .unwrap()
                    .to_owned();
                let visual = original["elements"]
                    .as_array_mut()
                    .unwrap()
                    .iter_mut()
                    .find(|e| e["id"] == image_id)
                    .unwrap();
                visual["angle"] = json!(angle);
                visual["scale"] = json!(scale);
                if cropped {
                    visual["crop"] = json!({"x":3,"y":10,"width":24,"height":80,"naturalWidth":30,"naturalHeight":100});
                }
                let visual = visual.clone();
                let crop = if cropped {
                    [0.1, 0.1, 0.8, 0.8]
                } else {
                    [0., 0., 1., 1.]
                };
                let iw = visual["width"].as_f64().unwrap();
                let ih = visual["height"].as_f64().unwrap();
                let local_x =
                    ((bounds[0] + bounds[2] / 2. - crop[0]) / crop[2] - 0.5) * iw * scale[0];
                let local_y =
                    ((bounds[1] + bounds[3] / 2. - crop[1]) / crop[3] - 0.5) * ih * scale[1];
                let (sin, cos) = angle.sin_cos();
                let cx = visual["x"].as_f64().unwrap() + iw / 2. + cos * local_x - sin * local_y;
                let cy = visual["y"].as_f64().unwrap() + ih / 2. + sin * local_x + cos * local_y;
                let width =
                    cos.abs() * bounds[2] / crop[2] * iw + sin.abs() * bounds[3] / crop[3] * ih;
                let height =
                    sin.abs() * bounds[2] / crop[2] * iw + cos.abs() * bounds[3] / crop[3] * ih;
                let next = scene::apply(
                    &original,
                    &[Operation::FitIllustrationAnchor {
                        id: image_id.clone(),
                        file_id: "art".into(),
                        normalized_bounds: bounds,
                    }],
                )
                .unwrap();
                let anchor = element(&next, "node-1");
                for (key, expected) in ["x", "y", "width", "height"].into_iter().zip([
                    cx - width / 2.,
                    cy - height / 2.,
                    width,
                    height,
                ]) {
                    assert!(
                        (anchor[key].as_f64().unwrap() - expected).abs() < 1e-8,
                        "{key}: angle={angle} scale={scale:?} crop={cropped}"
                    );
                }
                assert_eq!(element(&next, &image_id), &visual);
                assert_eq!(next["files"], original["files"]);
                assert_eq!(
                    anchor["customData"]["evidence"],
                    element(&original, "node-1")["customData"]["evidence"]
                );
                for id in ["node-0", "node-2", "label-0", "label-1", "label-2"] {
                    assert_eq!(element(&next, id), element(&original, id));
                }
                for id in ["edge-0", "edge-1"] {
                    for key in [
                        "customData",
                        "startBinding",
                        "endBinding",
                        "startArrowhead",
                        "endArrowhead",
                    ] {
                        assert_eq!(element(&next, id)[key], element(&original, id)[key]);
                    }
                }
            }
        }
    }
}

#[test]
fn artwork_outside_saved_crop_is_rejected_without_mutating_the_source() {
    let mut original = fixture();
    let image_id = element(&original, "node-1")["customData"]["visualAssetElementId"]
        .as_str()
        .unwrap()
        .to_owned();
    original["elements"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|e| e["id"] == image_id)
        .unwrap()["crop"] =
        json!({"x":3,"y":10,"width":24,"height":80,"naturalWidth":30,"naturalHeight":100});
    let saved = original.clone();
    let error = scene::apply(&original, &[fit()]).unwrap_err();
    assert!(error.to_string().contains("inside the visible source crop"));
    assert_eq!(original, saved);
}

#[test]
fn resizing_owned_artwork_refits_connectors_without_rewriting_scientific_labels() {
    let original = scene::apply(&fixture(), &[fit()]).unwrap();
    let visual_id = element(&original, "node-1")["customData"]["visualAssetElementId"]
        .as_str()
        .unwrap();
    let before = element(&original, visual_id);
    let operation = Operation::SetIllustrationGeometry {
        id: visual_id.into(),
        file_id: "art".into(),
        x: before["x"].as_f64().unwrap() - 15.,
        y: before["y"].as_f64().unwrap() - 10.,
        width: 60.,
        height: 120.,
        normalized_bounds: [0.2, 0., 0.6, 1.],
    };
    let mut by_anchor = serde_json::to_value(&operation).unwrap();
    by_anchor["id"] = json!("node-1");
    let via_anchor =
        scene::apply(&original, &[serde_json::from_value(by_anchor).unwrap()]).unwrap();
    let next = scene::apply(&original, &[operation]).unwrap();
    for e in next["elements"].as_array().unwrap() {
        let mut expected = e.clone();
        let actual = element(&via_anchor, e["id"].as_str().unwrap());
        for field in ["versionNonce", "updated"] {
            expected[field] = actual[field].clone();
        }
        assert_eq!(
            &expected, actual,
            "anchor and image selection address the same component"
        );
    }
    assert_eq!(next["files"], original["files"]);
    let mut expected = before.clone();
    let after = element(&next, visual_id);
    for field in [
        "x",
        "y",
        "width",
        "height",
        "version",
        "versionNonce",
        "updated",
    ] {
        expected[field] = after[field].clone();
    }
    assert_eq!(
        &expected, after,
        "pixels, source, crop, flips, rotation and identity survive"
    );
    assert!((element(&next, "node-1")["width"].as_f64().unwrap() - 36.).abs() < 1e-8);
    for id in ["label-1", "node-0", "label-0", "node-2", "label-2"] {
        assert_eq!(element(&next, id), element(&original, id));
    }
    for id in ["edge-0", "edge-1"] {
        let old = element(&original, id);
        let arrow = element(&next, id);
        for field in [
            "id",
            "customData",
            "startBinding",
            "endBinding",
            "startArrowhead",
            "endArrowhead",
        ] {
            assert_eq!(arrow[field], old[field]);
        }
        let points = arrow["points"].as_array().unwrap();
        let point = if id == "edge-0" {
            points.last().unwrap()
        } else {
            &points[0]
        };
        let anchor = element(&next, "node-1");
        let expected_x = anchor["x"].as_f64().unwrap()
            + if id == "edge-0" {
                -10.
            } else {
                anchor["width"].as_f64().unwrap() + 10.
            };
        assert!(
            (arrow["x"].as_f64().unwrap() + point[0].as_f64().unwrap() - expected_x).abs() < 1e-8
        );
    }
}

#[test]
fn illustration_resize_rejects_stale_file_invalid_geometry_and_unsupported_connectors_atomically() {
    let original = fixture();
    let before = original.clone();
    let visual_id = element(&original, "node-1")["customData"]["visualAssetElementId"]
        .as_str()
        .unwrap();
    let visual = element(&original, visual_id);
    let operation = serde_json::json!({"op":"set_illustration_geometry","id":visual_id,"file_id":"art","x":visual["x"],"y":visual["y"],"width":60,"height":100,"normalized_bounds":[0.2,0,0.6,1]});
    for (field, value) in [
        ("file_id", json!("stale")),
        ("width", json!(0)),
        ("height", json!(-1)),
        ("x", json!(1000001)),
        ("normalized_bounds", json!([0.9, 0., 0.6, 1.])),
    ] {
        let mut invalid = operation.clone();
        invalid[field] = value;
        assert!(scene::apply(&original, &[serde_json::from_value(invalid).unwrap()]).is_err());
        assert_eq!(original, before);
    }
    let mut framed = original.clone();
    for element in framed["elements"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .filter(|e| e["id"] == "node-1" || e["id"] == visual_id)
    {
        element["frameId"] = json!("panel");
    }
    framed["elements"].as_array_mut().unwrap().push(json!({"id":"panel","type":"frame","x":-1000,"y":-1000,"width":2000,"height":2000,"angle":0}));
    let mut escaped = operation.clone();
    escaped["x"] = json!(5000);
    let frozen_frame = framed.clone();
    assert!(scene::apply(&framed, &[serde_json::from_value(escaped).unwrap()]).is_err());
    assert_eq!(framed, frozen_frame);
    let mut locked = original.clone();
    locked["elements"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|e| e["id"] == "edge-0")
        .unwrap()["locked"] = json!(true);
    let frozen = locked.clone();
    assert!(scene::apply(&locked, &[serde_json::from_value(operation).unwrap()]).is_err());
    assert_eq!(locked, frozen);
}

#[test]
fn centered_scale_reuses_fit_preserves_labels_files_and_connector_bindings() {
    let original = scene::apply(&fixture(), &[fit()]).unwrap();
    let anchor = element(&original, "node-1");
    let image_id = anchor["customData"]["visualAssetElementId"]
        .as_str()
        .unwrap();
    let old = element(&original, image_id);
    let next = scene::apply(
        &original,
        &[Operation::ScaleIllustration {
            id: image_id.into(),
            file_id: "art".into(),
            factor: 1.1,
        }],
    )
    .unwrap();
    let image = element(&next, image_id);
    for (position, size) in [("x", "width"), ("y", "height")] {
        let before = old[position].as_f64().unwrap() + old[size].as_f64().unwrap() / 2.;
        let after = image[position].as_f64().unwrap() + image[size].as_f64().unwrap() / 2.;
        assert!((before - after).abs() < 1e-9);
        assert!((image[size].as_f64().unwrap() - old[size].as_f64().unwrap() * 1.1).abs() < 1e-9);
    }
    assert_eq!(next["files"], original["files"]);
    assert_eq!(
        element(&next, "node-1")["customData"]["visualAnchorFit"]["normalizedBounds"],
        anchor["customData"]["visualAnchorFit"]["normalizedBounds"]
    );
    for old in original["elements"].as_array().unwrap() {
        let actual = element(&next, old["id"].as_str().unwrap());
        if old["type"] == "text"
            || (old["type"] != "arrow" && old["id"] != "node-1" && old["id"] != image_id)
        {
            assert_eq!(actual, old);
        }
        if old["type"] == "arrow" {
            for key in [
                "startBinding",
                "endBinding",
                "startArrowhead",
                "endArrowhead",
                "customData",
            ] {
                assert_eq!(actual[key], old[key]);
            }
        }
    }
    let by_anchor = scene::apply(
        &original,
        &[Operation::ScaleIllustration {
            id: "node-1".into(),
            file_id: "art".into(),
            factor: 1.1,
        }],
    )
    .unwrap();
    assert_eq!(element(&by_anchor, image_id)["width"], image["width"]);
}
#[test]
fn centered_scale_rejects_missing_stale_or_wrong_file_fit_and_invalid_factors() {
    let fitted = scene::apply(&fixture(), &[fit()]).unwrap();
    for factor in [0., -1., f64::INFINITY, f64::NAN] {
        assert!(
            scene::apply(
                &fitted,
                &[Operation::ScaleIllustration {
                    id: "node-1".into(),
                    file_id: "art".into(),
                    factor
                }]
            )
            .is_err()
        );
    }
    let op = Operation::ScaleIllustration {
        id: "node-1".into(),
        file_id: "art".into(),
        factor: 1.1,
    };
    assert!(scene::apply(&fixture(), std::slice::from_ref(&op)).is_err());
    for field in ["x", "width"] {
        let mut stale = fitted.clone();
        let anchor = stale["elements"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|e| e["id"] == "node-1")
            .unwrap();
        anchor[field] = json!(anchor[field].as_f64().unwrap() + 1.);
        assert!(scene::apply(&stale, std::slice::from_ref(&op)).is_err());
    }
    assert!(
        scene::apply(
            &fitted,
            &[Operation::ScaleIllustration {
                id: "node-1".into(),
                file_id: "wrong".into(),
                factor: 1.1
            }]
        )
        .is_err()
    );
}

#[test]
fn refit_accepts_only_sub_hundredth_saved_endpoint_drift_and_keeps_bindings() {
    for (drift, accepted) in [
        (0.0043, true),
        (-0.0043, true),
        (0.02, false),
        (-0.02, false),
    ] {
        let mut doc = fixture();
        let arrow = doc["elements"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|e| e["id"] == "edge-0")
            .unwrap();
        let n = arrow["points"].as_array().unwrap().len();
        arrow["points"][n - 1][0] = json!(arrow["points"][n - 1][0].as_f64().unwrap() + drift);
        arrow["width"] = json!(arrow["width"].as_f64().unwrap() + drift);
        let baseline = doc.clone();
        let result = scene::apply(&doc, &[fit()]);
        assert_eq!(result.is_ok(), accepted, "drift {drift}: {result:?}");
        assert_eq!(doc, baseline);
        if let Ok(next) = result {
            for id in ["edge-0", "edge-1"] {
                for key in [
                    "startBinding",
                    "endBinding",
                    "startArrowhead",
                    "endArrowhead",
                    "customData",
                ] {
                    assert_eq!(element(&next, id)[key], element(&doc, id)[key]);
                }
            }
        }
    }
}
