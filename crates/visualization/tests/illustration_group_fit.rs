use serde_json::{Value, json};
use visualization::scene::{self, IllustrationMember, Operation};
fn fixture() -> Value {
    json!({"elements":[{"id":"frame","type":"frame","x":0,"y":0,"width":500,"height":500,"angle":0},{"id":"anchor","type":"rectangle","x":100,"y":100,"width":100,"height":100,"angle":0,"frameId":"frame","customData":{"scientificId":"complex","evidence":["retained"]},"boundElements":[{"id":"label","type":"text"}]},{"id":"primary","type":"image","x":110,"y":110,"width":60,"height":80,"angle":0,"frameId":"frame","fileId":"f","customData":{"source":"Fixture","asset":{"author":"Artist","license":"CC0"}}},{"id":"label","type":"text","x":100,"y":220,"width":200,"height":25,"angle":0,"frameId":"frame","containerId":"anchor","text":"Complete complex","originalText":"Complete complex","fontSize":20},{"id":"part","type":"image","x":180,"y":110,"width":60,"height":80,"angle":0,"frameId":"frame","fileId":"f","customData":{"source":"Fixture"}}],"files":{"f":{"dataURL":"data:image/png;base64,fixture"}}})
}
fn prepared() -> Value {
    let mut d = fixture();
    for e in d["elements"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .filter(|e| e["type"] == "image")
    {
        e["scale"] = json!([1, 1]);
    }
    scene::apply(
        &d,
        &[Operation::BindIllustration {
            anchor_id: "anchor".into(),
            primary_id: "primary".into(),
            label_ids: vec!["label".into()],
            members: vec![IllustrationMember {
                id: "part".into(),
                meaning: "Generic protein component".into(),
            }],
        }],
    )
    .unwrap()
}
fn samples() -> Vec<scene::IllustrationSample> {
    ["primary", "part"]
        .into_iter()
        .map(|id| scene::IllustrationSample {
            id: id.into(),
            file_id: "f".into(),
            normalized_bounds: [0., 0., 1., 1.],
        })
        .collect()
}
#[test]
fn group_fit_encloses_all_members_and_preserves_images_labels_and_files() {
    let d = prepared();
    let next = scene::apply(
        &d,
        &[Operation::FitIllustrationGroup {
            anchor_id: "anchor".into(),
            samples: samples(),
        }],
    )
    .unwrap();
    let a = &next["elements"][1];
    assert_eq!(
        [
            a["x"].as_f64().unwrap(),
            a["y"].as_f64().unwrap(),
            a["width"].as_f64().unwrap(),
            a["height"].as_f64().unwrap()
        ],
        [110., 110., 130., 80.]
    );
    for i in [0, 2, 3, 4] {
        assert_eq!(next["elements"][i], d["elements"][i]);
    }
    assert_eq!(next["files"], d["files"]);
    assert_eq!(
        a["customData"]["visualAnchorFit"]["scope"],
        "illustration_group"
    );
    assert_eq!(
        a["customData"]["visualAnchorFit"]["members"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    if let Ok(path) = std::env::var("VISUALIZATION_GROUP_FIT_OUTPUT") {
        std::fs::write(
            path,
            serde_json::to_vec_pretty(&json!({"document":next})).unwrap(),
        )
        .unwrap();
    }
}
#[test]
fn incomplete_duplicate_stale_and_invalid_samples_reject_atomically() {
    let d = prepared();
    for case in 0..4 {
        let mut values = samples();
        match case {
            0 => {
                values.pop();
            }
            1 => values[1].id = "primary".into(),
            2 => values[1].file_id = "stale".into(),
            _ => values[1].normalized_bounds = [0., 0., 2., 1.],
        };
        let before = d.clone();
        assert!(
            scene::apply(
                &d,
                &[Operation::FitIllustrationGroup {
                    anchor_id: "anchor".into(),
                    samples: values
                }]
            )
            .is_err()
        );
        assert_eq!(d, before);
    }
}

#[test]
fn group_snapshots_follow_rigid_translation_and_duplicate_identity_mapping() {
    let fitted = scene::apply(
        &prepared(),
        &[Operation::FitIllustrationGroup {
            anchor_id: "anchor".into(),
            samples: samples(),
        }],
    )
    .unwrap();
    let moved = visualization::arrange::translate(&fitted, &["anchor".into()], 5., 7.).unwrap();
    let a = &moved["elements"][1];
    assert_eq!(
        a["customData"]["visualAnchorFit"]["anchorBounds"],
        json!([115., 117., 130., 80.])
    );
    let copied = scene::apply(
        &moved,
        &[Operation::Duplicate {
            element_ids: vec!["anchor".into()],
            id_prefix: "copy".into(),
            dx: 20.,
            dy: 20.,
        }],
    )
    .unwrap();
    let a = copied["elements"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["id"] == "copy:e:anchor")
        .unwrap();
    let fit = &a["customData"]["visualAnchorFit"];
    assert_eq!(fit["anchorBounds"], json!([135., 137., 130., 80.]));
    assert_eq!(fit["elementId"], "copy:e:primary");
    assert!(
        fit["members"]
            .as_array()
            .unwrap()
            .iter()
            .all(|m| m["elementId"].as_str().unwrap().starts_with("copy:e:"))
    );
}

#[test]
fn connectors_follow_the_whole_group_boundary_without_changing_directions() {
    let mut doc = prepared();
    doc["elements"].as_array_mut().unwrap().extend([json!({"id":"target","type":"rectangle","x":350,"y":100,"width":100,"height":100,"frameId":"frame","angle":0}),json!({"id":"edge","type":"arrow","x":200,"y":150,"width":150,"height":0,"points":[[0,0],[150,0]],"frameId":"frame","angle":0,"roughness":0,"startBinding":{"elementId":"anchor","focus":0,"gap":0},"endBinding":{"elementId":"target","focus":0,"gap":0},"endArrowhead":"arrow"})]);
    let next = scene::apply(
        &doc,
        &[Operation::FitIllustrationGroup {
            anchor_id: "anchor".into(),
            samples: samples(),
        }],
    )
    .unwrap();
    let edge = next["elements"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["id"] == "edge")
        .unwrap();
    assert_eq!(edge["x"], 240.);
    assert_eq!(edge["y"], 150.);
    let p = edge["points"].as_array().unwrap().last().unwrap();
    assert_eq!(edge["x"].as_f64().unwrap() + p[0].as_f64().unwrap(), 350.);
    assert_eq!(edge["endArrowhead"], "arrow");
    assert_eq!(edge["startBinding"]["elementId"], "anchor");
    assert_eq!(edge["endBinding"]["elementId"], "target");
}

#[test]
fn group_scale_keeps_center_relative_positions_labels_sources_and_files() {
    let fitted = scene::apply(
        &prepared(),
        &[Operation::FitIllustrationGroup {
            anchor_id: "anchor".into(),
            samples: samples(),
        }],
    )
    .unwrap();
    let next = scene::apply(
        &fitted,
        &[Operation::ScaleIllustration {
            id: "primary".into(),
            file_id: "f".into(),
            factor: 2.,
        }],
    )
    .unwrap();
    let a = &next["elements"][1];
    assert_eq!(
        [
            a["x"].as_f64().unwrap(),
            a["y"].as_f64().unwrap(),
            a["width"].as_f64().unwrap(),
            a["height"].as_f64().unwrap()
        ],
        [45., 70., 260., 160.]
    );
    for i in [2, 4] {
        let old = &fitted["elements"][i];
        let new = &next["elements"][i];
        assert_eq!(
            new["x"].as_f64().unwrap(),
            175. + (old["x"].as_f64().unwrap() - 175.) * 2.
        );
        assert_eq!(
            new["y"].as_f64().unwrap(),
            150. + (old["y"].as_f64().unwrap() - 150.) * 2.
        );
        assert_eq!(
            new["width"].as_f64().unwrap(),
            old["width"].as_f64().unwrap() * 2.
        );
        assert_eq!(new["customData"], old["customData"]);
    }
    assert_eq!(next["elements"][3], fitted["elements"][3]);
    assert_eq!(next["files"], fitted["files"]);
    assert_eq!(
        a["customData"]["visualAnchorFit"]["scope"],
        "illustration_group"
    );
}
#[test]
fn stale_or_locked_members_and_oversize_group_scale_reject_atomically() {
    let fitted = scene::apply(
        &prepared(),
        &[Operation::FitIllustrationGroup {
            anchor_id: "anchor".into(),
            samples: samples(),
        }],
    )
    .unwrap();
    for case in 0..4 {
        let mut d = fitted.clone();
        match case {
            0 => d["elements"][4]["fileId"] = json!("changed"),
            1 => d["elements"][4]["x"] = json!(181),
            2 => d["elements"][4]["locked"] = json!(true),
            _ => (),
        };
        let before = d.clone();
        assert!(
            scene::apply(
                &d,
                &[Operation::ScaleIllustration {
                    id: "primary".into(),
                    file_id: "f".into(),
                    factor: if case == 3 { 100. } else { 1.1 }
                }]
            )
            .is_err()
        );
        assert_eq!(d, before);
    }
}
