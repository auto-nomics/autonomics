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

fn world_endpoint(e: &Value, start: bool) -> [f64; 2] {
    let points = e["points"].as_array().unwrap();
    let p = if start {
        &points[0]
    } else {
        points.last().unwrap()
    };
    [
        e["x"].as_f64().unwrap() + p[0].as_f64().unwrap(),
        e["y"].as_f64().unwrap() + p[1].as_f64().unwrap(),
    ]
}
#[test]
fn fitted_orthogonal_connectors_follow_one_component_with_far_endpoints_unchanged() {
    let before = scene::apply(&fixture(), &[fit()]).unwrap();
    assert!(
        ["edge-0", "edge-1"]
            .iter()
            .any(|id| element(&before, id)["points"].as_array().unwrap().len() > 2)
    );
    let next = visualization::arrange::translate(&before, &["node-1".into()], 0., -20.).unwrap();
    for id in ["node-0", "node-2"] {
        assert_eq!(element(&next, id), element(&before, id));
    }
    let anchor = element(&before, "node-1");
    let image_id = anchor["customData"]["visualAssetElementId"]
        .as_str()
        .unwrap();
    for id in ["node-1", image_id, "label-1"] {
        assert_eq!(element(&next, id)["x"], element(&before, id)["x"]);
        assert!(
            (element(&next, id)["y"].as_f64().unwrap()
                - element(&before, id)["y"].as_f64().unwrap()
                + 20.)
                .abs()
                < 1e-6
        );
    }
    assert_eq!(next["files"], before["files"]);
    for (id, moving_start) in [("edge-0", false), ("edge-1", true)] {
        let old = element(&before, id);
        let new = element(&next, id);
        assert_eq!(
            world_endpoint(new, !moving_start),
            world_endpoint(old, !moving_start)
        );
        let a = world_endpoint(old, moving_start);
        let b = world_endpoint(new, moving_start);
        assert!((a[0] - b[0]).abs() < 1e-6);
        assert!((b[1] - a[1] + 20.).abs() < 1e-6);
        for key in [
            "startBinding",
            "endBinding",
            "startArrowhead",
            "endArrowhead",
            "customData",
            "strokeColor",
            "strokeStyle",
            "strokeWidth",
            "roundness",
        ] {
            assert_eq!(new[key], old[key]);
        }
        let p = new["points"].as_array().unwrap();
        assert!(p.windows(2).all(
            |p| (p[0][0].as_f64().unwrap() - p[1][0].as_f64().unwrap()).abs() < 1e-6
                || (p[0][1].as_f64().unwrap() - p[1][1].as_f64().unwrap()).abs() < 1e-6
        ));
    }
    for old in before["elements"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["type"] == "text")
    {
        let new = element(&next, old["id"].as_str().unwrap());
        for key in [
            "originalText",
            "text",
            "fontSize",
            "fontFamily",
            "customData",
        ] {
            assert_eq!(new[key], old[key]);
        }
    }
}
#[test]
fn unsupported_geometry_and_locked_connector_members_still_reject_atomically() {
    let original = scene::apply(&fixture(), &[fit()]).unwrap();
    for case in 0..4 {
        let mut doc = original.clone();
        let arrow = doc["elements"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|e| e["id"] == "edge-0")
            .unwrap();
        match case {
            0 => arrow["roundness"] = json!({"type":2}),
            1 => arrow["locked"] = json!(true),
            2 => arrow["endBinding"]["focus"] = json!(0.5),
            _ => {
                let p = arrow["points"].as_array_mut().unwrap();
                p[1][1] = json!(p[1][1].as_f64().unwrap() + 3.);
            }
        }
        let saved = doc.clone();
        assert!(visualization::arrange::translate(&doc, &["node-1".into()], 0., -20.).is_err());
        assert_eq!(doc, saved);
    }
}
#[test]
fn optional_real_saved_component_translation_preserves_science_and_files() {
    let Ok(source) = std::env::var("VISUALIZATION_ORTHOGONAL_SOURCE") else {
        return;
    };
    let input: Value = serde_json::from_slice(&std::fs::read(source).unwrap()).unwrap();
    let doc = &input["document"];
    let next = visualization::arrange::translate(doc, &["node-1".into()], 0., -30.).unwrap();
    assert_eq!(next["files"], doc["files"]);
    assert_eq!(element(&next, "node-0"), element(doc, "node-0"));
    assert_eq!(element(&next, "node-2"), element(doc, "node-2"));
    for old in doc["elements"].as_array().unwrap() {
        let new = element(&next, old["id"].as_str().unwrap());
        if old["id"] != "node-1" {
            assert_eq!(
                new["customData"], old["customData"],
                "metadata for {}",
                old["id"]
            );
        }
        if old["type"] == "text" {
            for key in ["text", "originalText", "fontSize", "fontFamily"] {
                assert_eq!(new[key], old[key]);
            }
        }
        if old["type"] == "arrow" {
            for key in [
                "startBinding",
                "endBinding",
                "startArrowhead",
                "endArrowhead",
                "strokeColor",
                "strokeStyle",
                "strokeWidth",
            ] {
                assert_eq!(new[key], old[key]);
            }
            for (field, start) in [("startBinding", true), ("endBinding", false)] {
                let a = world_endpoint(old, start);
                let b = world_endpoint(new, start);
                let dy = if old[field]["elementId"] == "node-1" {
                    -30.
                } else {
                    0.
                };
                assert!((b[0] - a[0]).abs() < 1e-6);
                assert!((b[1] - a[1] - dy).abs() < 1e-6);
            }
        }
    }
    if let Ok(output) = std::env::var("VISUALIZATION_ORTHOGONAL_OUTPUT") {
        std::fs::write(
            output,
            serde_json::to_vec_pretty(&json!({"document":next})).unwrap(),
        )
        .unwrap();
    }
}
